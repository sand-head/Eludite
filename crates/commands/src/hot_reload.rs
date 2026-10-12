//! Hot Reload commands (brief 0074): `eludite.debug.run_watch` (Debug > Start Without Debugging with Hot Reload) and
//! `eludite.debug.apply_code_changes` (Debug > Apply Code Changes, Alt+F10).
//!
//! The schemas are `protocol/schemas/debug-run-watch.*.json` and `debug-apply-code-changes.input.json` (checked in
//! first, CLAUDE.md invariant 4). They live beside the `eludite.debug.*` session commands but outside their request
//! type: a watch is not a debugging session (no adapter, no stops), so its commands have their own small target,
//! [`HotReloadTarget`], which the shell implements.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const RUN_WATCH: &str = "eludite.debug.run_watch";
pub const APPLY_CODE_CHANGES: &str = "eludite.debug.apply_code_changes";

pub const ALL: [&str; 2] = [RUN_WATCH, APPLY_CODE_CHANGES];

/// Hot Reload's state (`debug-run-watch.output.json`'s `state`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchState {
    /// `dotnet watch` started; it builds before it watches.
    Starting,
    /// The program runs and saved changes will be applied.
    Watching,
    /// A file changed: the change is being applied.
    Applying,
    /// The last change was applied to the running program.
    Applied,
    /// The last change does not compile.
    Errors,
    /// The last change cannot be applied without a restart (a rude edit).
    RestartNeeded,
    /// The program is being restarted.
    Restarting,
    /// `dotnet watch` ended.
    Exited,
}

impl WatchState {
    pub fn as_str(self) -> &'static str {
        match self {
            WatchState::Starting => "starting",
            WatchState::Watching => "watching",
            WatchState::Applying => "applying",
            WatchState::Applied => "applied",
            WatchState::Errors => "errors",
            WatchState::RestartNeeded => "restart_needed",
            WatchState::Restarting => "restarting",
            WatchState::Exited => "exited",
        }
    }

    /// The status bar's words (`Hot Reload: changes applied`).
    pub fn words(self) -> &'static str {
        match self {
            WatchState::Starting => "starting",
            WatchState::Watching => "watching for changes",
            WatchState::Applying => "applying changes",
            WatchState::Applied => "changes applied",
            WatchState::Errors => "compilation errors",
            WatchState::RestartNeeded => "restart needed",
            WatchState::Restarting => "restarting",
            WatchState::Exited => "stopped",
        }
    }
}

/// A parsed Hot Reload command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HotReloadRequest {
    RunWatch {
        project: Option<String>,
        profile: Option<String>,
    },
    ApplyCodeChanges,
}

impl HotReloadRequest {
    pub fn command(&self) -> &'static str {
        match self {
            HotReloadRequest::RunWatch { .. } => RUN_WATCH,
            HotReloadRequest::ApplyCodeChanges => APPLY_CODE_CHANGES,
        }
    }
}

/// `debug-run-watch.output.json` (both commands answer it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchOutput {
    pub state: WatchState,
    pub project: String,
    pub terminal: String,
    pub command: String,
}

/// Whatever runs the watch (the shell). Called on the invoking thread.
pub trait HotReloadTarget: Send + Sync {
    fn apply(&self, request: HotReloadRequest) -> Result<WatchOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunWatchIn {
    project: Option<String>,
    profile: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(CommandError::InvalidInput(format!(
            "`{name}` must not be empty"
        ))),
        other => Ok(other),
    }
}

/// Parse and validate the input of command `id`.
pub fn parse(id: &str, value: Value) -> Result<HotReloadRequest, CommandError> {
    Ok(match id {
        RUN_WATCH => {
            let i: RunWatchIn = input(value)?;
            HotReloadRequest::RunWatch {
                project: non_empty("project", i.project)?,
                profile: non_empty("profile", i.profile)?,
            }
        }
        APPLY_CODE_CHANGES => {
            let _: Empty = input(value)?;
            HotReloadRequest::ApplyCodeChanges
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let output = include_str!("../../../protocol/schemas/debug-run-watch.output.json");
    let (title, input) = match id {
        RUN_WATCH => (
            "Debug: Start Without Debugging with Hot Reload",
            include_str!("../../../protocol/schemas/debug-run-watch.input.json"),
        ),
        APPLY_CODE_CHANGES => (
            "Debug: Apply Code Changes",
            include_str!("../../../protocol/schemas/debug-apply-code-changes.input.json"),
        ),
        other => unreachable!("not a Hot Reload command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        // Both run or feed a process the IDE started: execute, like Start Without Debugging.
        permission: PermissionClass::Execute,
        agent_visible: true,
    }
}

/// Register the Hot Reload commands, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn HotReloadTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target
                .apply(request)
                .map(|out| serde_json::to_value(out).expect("watch output serializes"))
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_specs_follow_the_schemas() {
        assert_eq!(
            parse(RUN_WATCH, json!({"project": "App", "profile": "http"})).unwrap(),
            HotReloadRequest::RunWatch {
                project: Some("App".into()),
                profile: Some("http".into())
            }
        );
        assert_eq!(parse(RUN_WATCH, Value::Null).unwrap().command(), RUN_WATCH);
        assert!(parse(RUN_WATCH, json!({"project": ""})).is_err());
        assert!(parse(RUN_WATCH, json!({"debug": true})).is_err());
        assert_eq!(
            parse(APPLY_CODE_CHANGES, json!({})).unwrap(),
            HotReloadRequest::ApplyCodeChanges
        );
        assert!(parse(APPLY_CODE_CHANGES, json!({"x": 1})).is_err());
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.permission, PermissionClass::Execute);
        }
        // Every state the output schema names, and only those.
        let schema: Value = serde_json::from_str(include_str!(
            "../../../protocol/schemas/debug-run-watch.output.json"
        ))
        .unwrap();
        let states: Vec<&str> = schema["properties"]["state"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v.as_str().unwrap())
            .collect();
        let ours: Vec<&str> = [
            WatchState::Starting,
            WatchState::Watching,
            WatchState::Applying,
            WatchState::Applied,
            WatchState::Errors,
            WatchState::RestartNeeded,
            WatchState::Restarting,
            WatchState::Exited,
        ]
        .iter()
        .map(|s| s.as_str())
        .collect();
        assert_eq!(states, ours);
        let out = WatchOutput {
            state: WatchState::RestartNeeded,
            project: "/s/App/App.csproj".into(),
            terminal: "term1".into(),
            command: "dotnet watch".into(),
        };
        assert_eq!(
            serde_json::to_value(&out).unwrap()["state"],
            "restart_needed"
        );
    }
}
