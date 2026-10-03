use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{AlwaysAllow, PolicySource, PolicyView};
use crate::{AuditLog, CommandId, Outcome};

/// Permission classes from PLAN.md 5.3.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionClass {
    /// Always allowed.
    Read,
    /// Shown as a pending diff until accepted (or auto-accepted by policy).
    EditBuffer,
    /// Build, test, run; per-workspace policy.
    Execute,
    /// Push, delete, external network; prompt unless whitelisted.
    Dangerous,
}

impl PermissionClass {
    /// The schema name (`edit_buffer`).
    pub fn as_str(self) -> &'static str {
        match self {
            PermissionClass::Read => "read",
            PermissionClass::EditBuffer => "edit_buffer",
            PermissionClass::Execute => "execute",
            PermissionClass::Dangerous => "dangerous",
        }
    }
}

/// The public description of a command, shared by the UI and the MCP server (`protocol/schemas/command-spec.json`).
/// Its `escalates` ([`CommandSpec::escalates`]) is read from the input schema's root `x-eludite-escalates`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct CommandSpec {
    pub id: CommandId,
    pub title: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub permission: PermissionClass,
    /// Advertised to hosted agents as an MCP tool. UI-only commands (window layout, view filters) are not.
    #[serde(default)]
    pub agent_visible: bool,
}

/// The input schema key documenting when a command's calls escalate (ADR-0009).
pub const ESCALATES_KEY: &str = "x-eludite-escalates";

impl CommandSpec {
    /// When a call is raised above [`CommandSpec::permission`] (ADR-0009): the input schema's `x-eludite-escalates`.
    pub fn escalates(&self) -> Option<&str> {
        self.input_schema
            .get(ESCALATES_KEY)
            .and_then(Value::as_str)
            .filter(|s| !s.is_empty())
    }
}

impl Serialize for CommandSpec {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeStruct;
        let escalates = self.escalates();
        let mut st = s.serialize_struct("CommandSpec", 6 + usize::from(escalates.is_some()))?;
        st.serialize_field("id", &self.id)?;
        st.serialize_field("title", &self.title)?;
        st.serialize_field("input_schema", &self.input_schema)?;
        st.serialize_field("output_schema", &self.output_schema)?;
        st.serialize_field("permission", &self.permission)?;
        st.serialize_field("agent_visible", &self.agent_visible)?;
        if let Some(e) = escalates {
            st.serialize_field("escalates", e)?;
        }
        st.end()
    }
}

/// What a command's escalation hook decides for one call (ADR-0009).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Escalation {
    /// This call is of `class` for `reason`; Always Allow remembers `always_allow`. A class at or below the spec's
    /// is ignored: a hook never lowers a class.
    Raise {
        class: PermissionClass,
        reason: String,
        always_allow: AlwaysAllow,
    },
    /// The solution's policy refuses this call outright, for an agent; the reason names the policy.
    Refuse(String),
}

impl Escalation {
    /// Raise to `class` for `reason`, Always Allow writing a tool rule.
    pub fn raise(class: PermissionClass, reason: impl Into<String>) -> Self {
        Escalation::Raise {
            class,
            reason: reason.into(),
            always_allow: AlwaysAllow::Rule,
        }
    }
}

/// An escalation hook: the call's input and what it may read of the policy, to an escalation or `None` (the spec's
/// class). Runs on the invoking thread, for every call of its command, so it stays cheap.
pub type EscalationHook = Arc<dyn Fn(&Value, &PolicyView) -> Option<Escalation> + Send + Sync>;

/// The class one call runs under: its spec's, or what the command's escalation hook raised it to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallClass {
    pub class: PermissionClass,
    /// Why the hook raised it; `None` for the spec's class.
    pub reason: Option<String>,
    /// What Always Allow remembers for this call.
    pub always_allow: AlwaysAllow,
    /// The policy refuses the call for an agent.
    pub refused: Option<String>,
}

impl CallClass {
    /// A call of the declared class.
    pub fn declared(class: PermissionClass) -> Self {
        Self {
            class,
            reason: None,
            always_allow: AlwaysAllow::Rule,
            refused: None,
        }
    }

    /// The hook raised the class.
    pub fn is_escalated(&self) -> bool {
        self.reason.is_some()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    #[error("unknown command `{0}`")]
    UnknownCommand(String),
    #[error("command `{0}` is already registered")]
    AlreadyRegistered(CommandId),
    #[error("invalid input: {0}")]
    InvalidInput(String),
    #[error("command failed: {0}")]
    Failed(String),
}

pub type Handler = Box<dyn Fn(Value) -> Result<Value, CommandError> + Send + Sync>;

struct Entry {
    spec: CommandSpec,
    handler: Handler,
    escalation: Option<EscalationHook>,
}

/// All registered commands plus the audit log of their invocations.
///
/// Commands can be registered at any time, from any thread (`&self`): the MCP server lists them afresh on every
/// `tools/list`, and [`CommandRegistry::generation`] grows with each registration so it can tell agents the list
/// changed. A handler runs without any lock held, so it may invoke or register other commands.
#[derive(Default)]
pub struct CommandRegistry {
    commands: RwLock<BTreeMap<CommandId, Arc<Entry>>>,
    generation: AtomicU64,
    audit: AuditLog,
    /// What escalation hooks read ([`CommandRegistry::set_policy_source`]).
    policy: RwLock<Option<PolicySource>>,
}

impl std::fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandRegistry")
            .field("commands", &self.read().keys().cloned().collect::<Vec<_>>())
            .field("audit_entries", &self.audit.len())
            .finish()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    fn read(&self) -> std::sync::RwLockReadGuard<'_, BTreeMap<CommandId, Arc<Entry>>> {
        self.commands.read().unwrap_or_else(|e| e.into_inner())
    }

    fn write(&self) -> std::sync::RwLockWriteGuard<'_, BTreeMap<CommandId, Arc<Entry>>> {
        self.commands.write().unwrap_or_else(|e| e.into_inner())
    }

    pub fn register<F>(&self, spec: CommandSpec, handler: F) -> Result<(), CommandError>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        self.insert(spec, None, Box::new(handler), false).map(drop)
    }

    /// Register `spec` with an escalation hook that may raise one call's class from its input (ADR-0009). The spec's
    /// input schema says when, in `x-eludite-escalates`.
    pub fn register_with_escalation<F>(
        &self,
        spec: CommandSpec,
        escalation: EscalationHook,
        handler: F,
    ) -> Result<(), CommandError>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        self.insert(spec, Some(escalation), Box::new(handler), false)
            .map(drop)
    }

    /// Register `spec`, replacing a command already registered under its id (a placeholder). Returns the
    /// replaced spec.
    pub fn replace<F>(&self, spec: CommandSpec, handler: F) -> Option<CommandSpec>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        self.insert(spec, None, Box::new(handler), true)
            .ok()
            .flatten()
    }

    /// [`CommandRegistry::replace`] with an escalation hook ([`CommandRegistry::register_with_escalation`]).
    pub fn replace_with_escalation<F>(
        &self,
        spec: CommandSpec,
        escalation: Option<EscalationHook>,
        handler: F,
    ) -> Option<CommandSpec>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        self.insert(spec, escalation, Box::new(handler), true)
            .ok()
            .flatten()
    }

    fn insert(
        &self,
        spec: CommandSpec,
        escalation: Option<EscalationHook>,
        handler: Handler,
        replace: bool,
    ) -> Result<Option<CommandSpec>, CommandError> {
        let mut commands = self.write();
        if !replace && commands.contains_key(&spec.id) {
            return Err(CommandError::AlreadyRegistered(spec.id));
        }
        let old = commands.insert(
            spec.id.clone(),
            Arc::new(Entry {
                spec,
                handler,
                escalation,
            }),
        );
        self.generation.fetch_add(1, Ordering::Release);
        Ok(old.map(|e| e.spec.clone()))
    }

    /// Where escalation hooks read the policy from (the shell: the open solution's policy file). Without one they
    /// see the defaults and no workspace.
    pub fn set_policy_source(&self, source: PolicySource) {
        *self.policy.write().unwrap_or_else(|e| e.into_inner()) = Some(source);
    }

    /// A view of the policy for one call, read on first use.
    pub fn policy_view(&self) -> PolicyView {
        PolicyView::from_source(
            self.policy
                .read()
                .unwrap_or_else(|e| e.into_inner())
                .clone(),
        )
    }

    /// Whether `id` registered an escalation hook.
    pub fn has_escalation(&self, id: &str) -> bool {
        CommandId::new(id)
            .ok()
            .and_then(|cid| self.read().get(&cid).map(|e| e.escalation.is_some()))
            .unwrap_or(false)
    }

    /// The class a call of `id` with `input` runs under: the spec's, raised (never lowered) by its escalation hook.
    /// `None` for an unknown command.
    pub fn classify(&self, id: &str, input: &Value) -> Option<CallClass> {
        let entry = self.entry(id)?;
        Some(self.classify_entry(&entry, input))
    }

    fn classify_entry(&self, entry: &Entry, input: &Value) -> CallClass {
        let mut c = CallClass::declared(entry.spec.permission);
        let Some(hook) = &entry.escalation else {
            return c;
        };
        match hook(input, &self.policy_view()) {
            Some(Escalation::Raise {
                class,
                reason,
                always_allow,
            }) if class > c.class => {
                c.class = class;
                c.reason = Some(reason);
                c.always_allow = always_allow;
            }
            Some(Escalation::Refuse(why)) => c.refused = Some(why),
            _ => {}
        }
        c
    }

    fn entry(&self, id: &str) -> Option<Arc<Entry>> {
        CommandId::new(id)
            .ok()
            .and_then(|cid| self.read().get(&cid).cloned())
    }

    /// Grows by one with every registration or replacement.
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    pub fn lookup(&self, id: &str) -> Option<CommandSpec> {
        let id = CommandId::new(id).ok()?;
        self.read().get(&id).map(|e| e.spec.clone())
    }

    /// All specs, sorted by id.
    pub fn list(&self) -> Vec<CommandSpec> {
        self.read().values().map(|e| e.spec.clone()).collect()
    }

    /// The specs advertised to agents, sorted by id.
    pub fn agent_visible(&self) -> Vec<CommandSpec> {
        self.read()
            .values()
            .filter(|e| e.spec.agent_visible)
            .map(|e| e.spec.clone())
            .collect()
    }

    /// Invoke a command and record the outcome in the audit log, with the caller ([`crate::current_caller`]).
    ///
    /// Permission policy is enforced at the MCP boundary (`eludite-mcp`), where agents' calls arrive; the class is
    /// recorded here for every caller.
    pub fn invoke(&self, id: &str, input: Value) -> Result<Value, CommandError> {
        self.invoke_audited(id, input).1
    }

    /// [`CommandRegistry::invoke`], also returning the audit entry's `seq`. An agent caller's arguments are kept
    /// in the entry, and the entry records the call's effective class ([`CommandRegistry::classify`]); a call the
    /// policy refuses (an escalation hook's [`Escalation::Refuse`]) fails for an agent without running.
    pub fn invoke_audited(&self, id: &str, input: Value) -> (u64, Result<Value, CommandError>) {
        self.invoke_classified(id, input, None)
    }

    /// Invoke with the class the MCP boundary decided on (`class`, from [`CommandRegistry::classify`] before its
    /// gate), so the audit entry records what the gate and the prompt used. A class below the spec's is raised to it.
    pub fn invoke_as(
        &self,
        id: &str,
        input: Value,
        class: &CallClass,
    ) -> (u64, Result<Value, CommandError>) {
        self.invoke_classified(id, input, Some(class))
    }

    fn invoke_classified(
        &self,
        id: &str,
        input: Value,
        class: Option<&CallClass>,
    ) -> (u64, Result<Value, CommandError>) {
        let caller = crate::current_caller();
        let arguments = caller.keeps_arguments().then(|| input.clone());
        let Some(entry) = self.entry(id) else {
            let err = CommandError::UnknownCommand(id.to_owned());
            let seq =
                self.audit
                    .record_call(id, None, Outcome::Err(err.to_string()), caller, arguments);
            return (seq, Err(err));
        };
        let class = match class {
            Some(c) => {
                let mut c = c.clone();
                if c.class < entry.spec.permission {
                    c = CallClass::declared(entry.spec.permission);
                }
                c
            }
            None => self.classify_entry(&entry, &input),
        };
        let result = match &class.refused {
            Some(why) if caller.is_agent() => Err(CommandError::Failed(format!(
                "permission denied: `{id}` is refused: {why}"
            ))),
            _ => (entry.handler)(input),
        };
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(e) => Outcome::Err(e.to_string()),
        };
        let seq = self
            .audit
            .record_call_class(id, &class, outcome, caller, arguments);
        (seq, result)
    }

    pub fn audit_log(&self) -> &AuditLog {
        &self.audit
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn spec(id: &str, permission: PermissionClass) -> CommandSpec {
        CommandSpec {
            id: CommandId::new(id).unwrap(),
            title: id.to_owned(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({}),
            permission,
            agent_visible: true,
        }
    }

    #[test]
    fn register_lookup_list() {
        let r = CommandRegistry::new();
        r.register(spec("test.b", PermissionClass::Read), |_| Ok(Value::Null))
            .unwrap();
        r.register(spec("test.a", PermissionClass::Execute), |_| {
            Ok(Value::Null)
        })
        .unwrap();
        assert_eq!(
            r.lookup("test.a").unwrap().permission,
            PermissionClass::Execute
        );
        assert!(r.lookup("test.c").is_none());
        assert!(r.lookup("Not An Id").is_none());
        let ids: Vec<_> = r.list().iter().map(|s| s.id.as_str().to_owned()).collect();
        assert_eq!(ids, ["test.a", "test.b"]);
    }

    #[test]
    fn replace_swaps_the_handler() {
        let r = CommandRegistry::new();
        r.register(spec("test.a", PermissionClass::Read), |_| Ok(json!(1)))
            .unwrap();
        let old = r.replace(spec("test.a", PermissionClass::Execute), |_| Ok(json!(2)));
        assert_eq!(old.unwrap().permission, PermissionClass::Read);
        assert_eq!(r.invoke("test.a", Value::Null).unwrap(), json!(2));
        assert!(
            r.replace(spec("test.b", PermissionClass::Read), Ok)
                .is_none()
        );
        assert_eq!(r.list().len(), 2);
    }

    #[test]
    fn duplicate_registration_fails() {
        let r = CommandRegistry::new();
        r.register(spec("test.a", PermissionClass::Read), |_| Ok(Value::Null))
            .unwrap();
        let err = r
            .register(spec("test.a", PermissionClass::Read), |_| Ok(Value::Null))
            .unwrap_err();
        assert!(matches!(err, CommandError::AlreadyRegistered(_)));
    }

    #[test]
    fn invoke_and_audit() {
        let r = CommandRegistry::new();
        r.register(spec("test.echo", PermissionClass::Read), Ok)
            .unwrap();
        r.register(spec("test.fail", PermissionClass::Dangerous), |_| {
            Err(CommandError::Failed("boom".into()))
        })
        .unwrap();

        assert_eq!(
            r.invoke("test.echo", json!({"x": 1})).unwrap(),
            json!({"x": 1})
        );
        assert!(matches!(
            r.invoke("test.fail", Value::Null),
            Err(CommandError::Failed(_))
        ));
        assert_eq!(
            r.invoke("test.missing", Value::Null),
            Err(CommandError::UnknownCommand("test.missing".into()))
        );

        let log = r.audit_log().entries();
        assert_eq!(log.len(), 3);
        assert_eq!(log[0].command, "test.echo");
        assert!(log[0].is_ok());
        assert_eq!(log[0].permission, Some(PermissionClass::Read));
        assert_eq!(log[1].permission, Some(PermissionClass::Dangerous));
        assert!(matches!(&log[1].outcome, Outcome::Err(m) if m.contains("boom")));
        assert_eq!(log[2].permission, None);
        assert!(!log[2].is_ok());
        assert!(log[0].timestamp <= log[2].timestamp);
        assert_eq!(
            r.audit_log()
                .for_command(&CommandId::new("test.echo").unwrap())
                .len(),
            1
        );
    }

    #[test]
    fn registers_at_runtime_from_another_thread() {
        let r = std::sync::Arc::new(CommandRegistry::new());
        let g0 = r.generation();
        let mut hidden = spec("test.hidden", PermissionClass::Read);
        hidden.agent_visible = false;
        r.register(hidden, Ok).unwrap();
        let r2 = r.clone();
        std::thread::spawn(move || {
            r2.register(spec("test.late", PermissionClass::Execute), Ok)
                .unwrap()
        })
        .join()
        .unwrap();
        assert_eq!(r.generation(), g0 + 2);
        let visible: Vec<_> = r
            .agent_visible()
            .into_iter()
            .map(|s| s.id.as_str().to_owned())
            .collect();
        assert_eq!(visible, ["test.late"]);
        assert_eq!(r.list().len(), 2);
        // A handler may register another command: no lock is held while it runs.
        let r3 = r.clone();
        r.register(spec("test.reg", PermissionClass::Read), move |_| {
            r3.register(spec("test.inner", PermissionClass::Read), Ok)?;
            Ok(Value::Null)
        })
        .unwrap();
        r.invoke("test.reg", Value::Null).unwrap();
        assert!(r.lookup("test.inner").is_some());
    }

    #[test]
    fn agent_calls_are_audited_with_caller_and_arguments() {
        let r = CommandRegistry::new();
        r.register(spec("test.echo", PermissionClass::Read), Ok)
            .unwrap();
        let caller = crate::Caller::Agent {
            agent: "Fake".into(),
            call: 42,
            tool_call: None,
        };
        let (seq, out) = crate::with_caller(caller.clone(), || {
            r.invoke_audited("test.echo", json!({"a": 1}))
        });
        assert_eq!(out.unwrap(), json!({"a": 1}));
        let e = r.audit_log().get(seq).unwrap();
        assert_eq!(e.caller, caller);
        assert_eq!(e.arguments, Some(json!({"a": 1})));
        let (seq, _) = r.invoke_audited("test.echo", json!({"b": 2}));
        let e = r.audit_log().get(seq).unwrap();
        assert_eq!(e.caller, crate::Caller::User);
        assert_eq!(e.arguments, None, "the user's own arguments are not kept");
    }

    #[test]
    fn escalation_raises_never_lowers_and_is_audited() {
        use crate::policy::{AlwaysAllow, PolicySnapshot};
        let r = CommandRegistry::new();
        let mut s = spec("test.go", PermissionClass::Execute);
        s.input_schema["x-eludite-escalates"] = json!("Dangerous when `to` is far.");
        let hook: EscalationHook = Arc::new(|input: &Value, view: &PolicyView| {
            match input["to"].as_str()? {
                "far" => Some(Escalation::Raise {
                    class: PermissionClass::Dangerous,
                    reason: "going far".into(),
                    always_allow: AlwaysAllow::Origin("https://far".into()),
                }),
                // A hook cannot lower a class.
                "low" => Some(Escalation::raise(PermissionClass::Read, "lower")),
                "no" => Some(Escalation::Refuse("the policy says no".into())),
                "ws" => view
                    .workspace()
                    .is_none()
                    .then(|| Escalation::raise(PermissionClass::Dangerous, "no workspace")),
                _ => None,
            }
        });
        r.register_with_escalation(s, hook, Ok).unwrap();
        r.register(spec("test.plain", PermissionClass::Read), Ok)
            .unwrap();
        assert!(r.has_escalation("test.go"));
        assert!(!r.has_escalation("test.plain"));
        assert_eq!(
            r.lookup("test.go").unwrap().escalates(),
            Some("Dangerous when `to` is far.")
        );
        let v = serde_json::to_value(r.lookup("test.go").unwrap()).unwrap();
        assert_eq!(v["escalates"], "Dangerous when `to` is far.");
        assert!(
            serde_json::to_value(r.lookup("test.plain").unwrap())
                .unwrap()
                .get("escalates")
                .is_none()
        );

        let far = r.classify("test.go", &json!({"to": "far"})).unwrap();
        assert_eq!(far.class, PermissionClass::Dangerous);
        assert_eq!(far.reason.as_deref(), Some("going far"));
        assert_eq!(far.always_allow, AlwaysAllow::Origin("https://far".into()));
        assert!(far.is_escalated());
        let near = r.classify("test.go", &json!({"to": "near"})).unwrap();
        assert_eq!(near, CallClass::declared(PermissionClass::Execute));
        let low = r.classify("test.go", &json!({"to": "low"})).unwrap();
        assert_eq!(
            low,
            CallClass::declared(PermissionClass::Execute),
            "never lowered"
        );
        assert!(r.classify("test.missing", &json!({})).is_none());
        // The hook reads the policy source on use: no workspace without one, a workspace with one.
        assert!(
            r.classify("test.go", &json!({"to": "ws"}))
                .unwrap()
                .is_escalated()
        );
        r.set_policy_source(Arc::new(|| PolicySnapshot {
            workspace: Some("/w".into()),
            ..Default::default()
        }));
        assert!(
            !r.classify("test.go", &json!({"to": "ws"}))
                .unwrap()
                .is_escalated()
        );

        // The audit entry records the effective class and the reason, for every caller.
        let agent = crate::Caller::Agent {
            agent: "Fake".into(),
            call: 7,
            tool_call: None,
        };
        let (seq, out) = crate::with_caller(agent.clone(), || {
            r.invoke_audited("test.go", json!({"to": "far"}))
        });
        assert!(out.is_ok());
        let e = r.audit_log().get(seq).unwrap();
        assert_eq!(e.permission, Some(PermissionClass::Dangerous));
        assert_eq!(e.escalation.as_deref(), Some("going far"));
        let (seq, _) = r.invoke_audited("test.go", json!({"to": "near"}));
        let e = r.audit_log().get(seq).unwrap();
        assert_eq!(e.permission, Some(PermissionClass::Execute));
        assert_eq!(e.escalation, None);
        // A refusal stops an agent's call, not the user's.
        let (seq, out) = crate::with_caller(agent.clone(), || {
            r.invoke_audited("test.go", json!({"to": "no"}))
        });
        let err = out.unwrap_err().to_string();
        assert!(err.contains("the policy says no"), "{err}");
        assert_eq!(
            r.audit_log().get(seq).unwrap().escalation.as_deref(),
            Some("the policy says no")
        );
        assert!(r.invoke("test.go", json!({"to": "no"})).is_ok());
        // invoke_as records the class the boundary decided on, raised to the spec's when lower.
        let (seq, _) = crate::with_caller(agent.clone(), || {
            r.invoke_as("test.go", json!({"to": "near"}), &far)
        });
        assert_eq!(
            r.audit_log().get(seq).unwrap().permission,
            Some(PermissionClass::Dangerous)
        );
        let (seq, _) = r.invoke_as(
            "test.go",
            json!({}),
            &CallClass::declared(PermissionClass::Read),
        );
        assert_eq!(
            r.audit_log().get(seq).unwrap().permission,
            Some(PermissionClass::Execute)
        );
        let v = serde_json::to_value(r.audit_log().get(1).unwrap()).unwrap();
        assert_eq!(v["permission"], "dangerous");
        assert_eq!(v["escalation"], "going far");
    }

    #[test]
    fn the_policy_is_read_only_when_a_hook_needs_it() {
        use crate::policy::PolicySnapshot;
        use std::sync::atomic::AtomicUsize;
        let r = CommandRegistry::new();
        let reads = Arc::new(AtomicUsize::new(0));
        let counted = reads.clone();
        r.set_policy_source(Arc::new(move || {
            counted.fetch_add(1, Ordering::SeqCst);
            PolicySnapshot::default()
        }));
        r.register_with_escalation(
            spec("test.cheap", PermissionClass::Read),
            Arc::new(|input: &Value, _: &PolicyView| {
                (input["clear"] == true)
                    .then(|| Escalation::raise(PermissionClass::Execute, "clear"))
            }),
            Ok,
        )
        .unwrap();
        r.register_with_escalation(
            spec("test.reads", PermissionClass::Read),
            Arc::new(|_: &Value, view: &PolicyView| {
                let _ = view.policy();
                let _ = view.workspace();
                None
            }),
            Ok,
        )
        .unwrap();
        r.invoke("test.cheap", json!({"clear": true})).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 0);
        r.invoke("test.reads", json!({})).unwrap();
        assert_eq!(reads.load(Ordering::SeqCst), 1, "once per call");
    }

    #[test]
    fn spec_serializes_permission_snake_case() {
        let v = serde_json::to_value(spec("test.a", PermissionClass::EditBuffer)).unwrap();
        assert_eq!(v["permission"], json!("edit_buffer"));
        assert_eq!(v["agent_visible"], json!(true));
        assert_eq!(PermissionClass::EditBuffer.as_str(), "edit_buffer");
        assert_eq!(v["id"], json!("test.a"));
    }
}
