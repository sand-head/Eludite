use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, RwLock};

use serde::{Deserialize, Serialize};
use serde_json::Value;

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
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
        let mut commands = self.write();
        if commands.contains_key(&spec.id) {
            return Err(CommandError::AlreadyRegistered(spec.id));
        }
        commands.insert(
            spec.id.clone(),
            Arc::new(Entry {
                spec,
                handler: Box::new(handler),
            }),
        );
        self.generation.fetch_add(1, Ordering::Release);
        Ok(())
    }

    /// Register `spec`, replacing a command already registered under its id (a placeholder). Returns the
    /// replaced spec.
    pub fn replace<F>(&self, spec: CommandSpec, handler: F) -> Option<CommandSpec>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        let old = self.write().insert(
            spec.id.clone(),
            Arc::new(Entry {
                spec,
                handler: Box::new(handler),
            }),
        );
        self.generation.fetch_add(1, Ordering::Release);
        old.map(|e| e.spec.clone())
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
    /// in the entry.
    pub fn invoke_audited(&self, id: &str, input: Value) -> (u64, Result<Value, CommandError>) {
        let caller = crate::current_caller();
        let arguments = caller.is_agent().then(|| input.clone());
        let entry = CommandId::new(id)
            .ok()
            .and_then(|cid| self.read().get(&cid).cloned());
        let Some(entry) = entry else {
            let err = CommandError::UnknownCommand(id.to_owned());
            let seq =
                self.audit
                    .record_call(id, None, Outcome::Err(err.to_string()), caller, arguments);
            return (seq, Err(err));
        };
        let result = (entry.handler)(input);
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(e) => Outcome::Err(e.to_string()),
        };
        let seq =
            self.audit
                .record_call(id, Some(entry.spec.permission), outcome, caller, arguments);
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
    fn spec_serializes_permission_snake_case() {
        let v = serde_json::to_value(spec("test.a", PermissionClass::EditBuffer)).unwrap();
        assert_eq!(v["permission"], json!("edit_buffer"));
        assert_eq!(v["agent_visible"], json!(true));
        assert_eq!(PermissionClass::EditBuffer.as_str(), "edit_buffer");
        assert_eq!(v["id"], json!("test.a"));
    }
}
