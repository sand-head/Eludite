use std::collections::BTreeMap;

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

/// The public description of a command, shared by the UI and the MCP server.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CommandSpec {
    pub id: CommandId,
    pub title: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub permission: PermissionClass,
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
#[derive(Default)]
pub struct CommandRegistry {
    commands: BTreeMap<CommandId, Entry>,
    audit: AuditLog,
}

impl std::fmt::Debug for CommandRegistry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandRegistry")
            .field("commands", &self.commands.keys().collect::<Vec<_>>())
            .field("audit_entries", &self.audit.len())
            .finish()
    }
}

impl CommandRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register<F>(&mut self, spec: CommandSpec, handler: F) -> Result<(), CommandError>
    where
        F: Fn(Value) -> Result<Value, CommandError> + Send + Sync + 'static,
    {
        if self.commands.contains_key(&spec.id) {
            return Err(CommandError::AlreadyRegistered(spec.id));
        }
        self.commands.insert(
            spec.id.clone(),
            Entry {
                spec,
                handler: Box::new(handler),
            },
        );
        Ok(())
    }

    pub fn lookup(&self, id: &str) -> Option<&CommandSpec> {
        let id = CommandId::new(id).ok()?;
        self.commands.get(&id).map(|e| &e.spec)
    }

    /// All specs, sorted by id.
    pub fn list(&self) -> impl Iterator<Item = &CommandSpec> {
        self.commands.values().map(|e| &e.spec)
    }

    /// Invoke a command and record the outcome in the audit log.
    ///
    /// Permission policy is not enforced yet; the class is recorded so policy can
    /// be layered on without changing callers.
    pub fn invoke(&self, id: &str, input: Value) -> Result<Value, CommandError> {
        let entry = CommandId::new(id)
            .ok()
            .and_then(|cid| self.commands.get(&cid));
        let Some(entry) = entry else {
            let err = CommandError::UnknownCommand(id.to_owned());
            self.audit.record(id, None, Outcome::Err(err.to_string()));
            return Err(err);
        };
        let result = (entry.handler)(input);
        let outcome = match &result {
            Ok(_) => Outcome::Ok,
            Err(e) => Outcome::Err(e.to_string()),
        };
        self.audit.record(id, Some(entry.spec.permission), outcome);
        result
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
        }
    }

    #[test]
    fn register_lookup_list() {
        let mut r = CommandRegistry::new();
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
        let ids: Vec<_> = r.list().map(|s| s.id.as_str()).collect();
        assert_eq!(ids, ["test.a", "test.b"]);
    }

    #[test]
    fn duplicate_registration_fails() {
        let mut r = CommandRegistry::new();
        r.register(spec("test.a", PermissionClass::Read), |_| Ok(Value::Null))
            .unwrap();
        let err = r
            .register(spec("test.a", PermissionClass::Read), |_| Ok(Value::Null))
            .unwrap_err();
        assert!(matches!(err, CommandError::AlreadyRegistered(_)));
    }

    #[test]
    fn invoke_and_audit() {
        let mut r = CommandRegistry::new();
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
    fn spec_serializes_permission_snake_case() {
        let v = serde_json::to_value(spec("test.a", PermissionClass::EditBuffer)).unwrap();
        assert_eq!(v["permission"], json!("edit_buffer"));
        assert_eq!(v["id"], json!("test.a"));
    }
}
