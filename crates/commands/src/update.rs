//! Self-update (brief 0055, ADR-0011): `eludite.update.status`, `eludite.update.check`, `eludite.update.download`
//! and `eludite.update.apply`, registered by the shell through an [`UpdateTarget`] over `eludite-update`. Help >
//! Check for Updates runs `check`; the status bar's update slot runs `download` and `apply`; the settings
//! `updates.channel` and `updates.mode` are `eludite.settings.*`'s. The status output is `eludite-update`'s
//! `Status` as JSON (`update-status.output.json`).

use std::sync::Arc;

use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const STATUS: &str = "eludite.update.status";
pub const CHECK: &str = "eludite.update.check";
pub const DOWNLOAD: &str = "eludite.update.download";
pub const APPLY: &str = "eludite.update.apply";

pub const ALL: [&str; 4] = [STATUS, CHECK, DOWNLOAD, APPLY];

/// The longest a check or download command may wait for its result.
pub const MAX_WAIT_MS: u64 = 600_000;

/// A parsed, validated update command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateRequest {
    Status,
    /// Check the channel; `download` stages what it finds; `wait_ms` waits that long for the result (0: answer at
    /// once with the state so far).
    Check {
        wait_ms: u64,
        download: bool,
    },
    Download {
        wait_ms: u64,
    },
    /// Restart into the staged build.
    Apply,
}

impl UpdateRequest {
    pub fn command(&self) -> &'static str {
        match self {
            UpdateRequest::Status => STATUS,
            UpdateRequest::Check { .. } => CHECK,
            UpdateRequest::Download { .. } => DOWNLOAD,
            UpdateRequest::Apply => APPLY,
        }
    }
}

/// What applies update commands (the shell).
pub trait UpdateTarget: Send + Sync {
    fn apply(&self, request: UpdateRequest) -> Result<Value, CommandError>;
}

fn no_extra(value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let Some(obj) = value.as_object() else {
        return Err(CommandError::InvalidInput("input must be an object".into()));
    };
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(CommandError::InvalidInput(format!("unknown field `{k}`")));
    }
    Ok(())
}

fn wait_ms(value: &Value) -> Result<u64, CommandError> {
    match value.get("wait_ms") {
        None | Some(Value::Null) => Ok(0),
        Some(v) => match v.as_u64() {
            Some(n) if n <= MAX_WAIT_MS => Ok(n),
            _ => Err(CommandError::InvalidInput(format!(
                "`wait_ms` must be an integer from 0 to {MAX_WAIT_MS}"
            ))),
        },
    }
}

fn flag(value: &Value, name: &str) -> Result<bool, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(false),
        Some(Value::Bool(b)) => Ok(*b),
        Some(_) => Err(CommandError::InvalidInput(format!(
            "`{name}` must be a boolean"
        ))),
    }
}

/// Parse and validate a command's input.
pub fn parse(id: &str, value: Value) -> Result<UpdateRequest, CommandError> {
    match id {
        STATUS => {
            no_extra(&value, &[])?;
            Ok(UpdateRequest::Status)
        }
        CHECK => {
            no_extra(&value, &["wait_ms", "download"])?;
            Ok(UpdateRequest::Check {
                wait_ms: wait_ms(&value)?,
                download: flag(&value, "download")?,
            })
        }
        DOWNLOAD => {
            no_extra(&value, &["wait_ms"])?;
            Ok(UpdateRequest::Download {
                wait_ms: wait_ms(&value)?,
            })
        }
        APPLY => {
            no_extra(&value, &[])?;
            Ok(UpdateRequest::Apply)
        }
        other => Err(CommandError::InvalidInput(format!(
            "not an update command: {other}"
        ))),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = match id {
        STATUS => (
            "Help: Update Status",
            include_str!("../../../protocol/schemas/update-status.input.json"),
            include_str!("../../../protocol/schemas/update-status.output.json"),
            PermissionClass::Read,
        ),
        // A network call to the release list: the execute class, as a build is.
        CHECK => (
            "Help: Check for Updates",
            include_str!("../../../protocol/schemas/update-check.input.json"),
            include_str!("../../../protocol/schemas/update-check.output.json"),
            PermissionClass::Execute,
        ),
        DOWNLOAD => (
            "Help: Download Update",
            include_str!("../../../protocol/schemas/update-download.input.json"),
            include_str!("../../../protocol/schemas/update-download.output.json"),
            PermissionClass::Execute,
        ),
        // Quits the IDE and every agent session in it, then replaces the installed files.
        APPLY => (
            "Help: Restart to Update",
            include_str!("../../../protocol/schemas/update-apply.input.json"),
            include_str!("../../../protocol/schemas/update-apply.output.json"),
            PermissionClass::Dangerous,
        ),
        other => unreachable!("not an update command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register the update commands on `registry`, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn UpdateTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |value| target.apply(parse(id, value)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
        }
        let files = std::fs::read_dir(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../protocol/schemas"
        ))
        .unwrap()
        .filter_map(|e| e.ok().map(|e| e.file_name().to_string_lossy().into_owned()))
        .filter(|n| n.starts_with("update-"))
        .count();
        assert_eq!(
            files,
            ALL.len() * 2,
            "two schemas per command, no stray one"
        );
    }

    #[test]
    fn classes() {
        use PermissionClass::*;
        assert_eq!(spec(STATUS).permission, Read);
        assert_eq!(spec(CHECK).permission, Execute);
        assert_eq!(spec(DOWNLOAD).permission, Execute);
        assert_eq!(spec(APPLY).permission, Dangerous);
    }

    #[test]
    fn input_is_checked() {
        assert_eq!(parse(STATUS, json!({})).unwrap(), UpdateRequest::Status);
        assert!(parse(STATUS, json!({"x": 1})).is_err());
        assert_eq!(
            parse(CHECK, json!({"wait_ms": 5000, "download": true})).unwrap(),
            UpdateRequest::Check {
                wait_ms: 5000,
                download: true
            }
        );
        assert_eq!(
            parse(CHECK, json!({})).unwrap(),
            UpdateRequest::Check {
                wait_ms: 0,
                download: false
            }
        );
        assert!(parse(CHECK, json!({"wait_ms": -1})).is_err());
        assert!(parse(CHECK, json!({"wait_ms": 600_001})).is_err());
        assert!(parse(CHECK, json!({"download": "yes"})).is_err());
        assert_eq!(
            parse(DOWNLOAD, json!({"wait_ms": 10})).unwrap(),
            UpdateRequest::Download { wait_ms: 10 }
        );
        assert!(parse(DOWNLOAD, json!({"download": true})).is_err());
        assert_eq!(parse(APPLY, json!({})).unwrap(), UpdateRequest::Apply);
        assert!(parse("eludite.update.other", json!({})).is_err());
    }

    #[test]
    fn registered_commands_reach_the_target() {
        struct Echo;
        impl UpdateTarget for Echo {
            fn apply(&self, request: UpdateRequest) -> Result<Value, CommandError> {
                Ok(json!({"command": request.command()}))
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        assert_eq!(
            registry.invoke(CHECK, json!({"wait_ms": 1})).unwrap()["command"],
            CHECK
        );
        assert!(registry.invoke(APPLY, json!({"force": true})).is_err());
    }
}
