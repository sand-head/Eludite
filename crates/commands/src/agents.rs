//! The Agents window's commands (brief 0016): `eludite.agents.start`, `prompt`, `cancel`, `permission` (answer a
//! pending request) and `review` (accept or reject a pending change); brief 0058 adds `configure` (the session's mode
//! or a config option such as the model or the effort, as the pickers under the prompt box set them). The window's
//! buttons, the prompt box, its keys and the pickers run them, and they are agent-visible too, so an outer agent could
//! drive an inner one.
//!
//! The schemas are the files in `protocol/schemas/` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`AgentsRequest`] and serializes the typed [`AgentsOutput`]; the shell implements
//! [`AgentsTarget`].

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const START: &str = "eludite.agents.start";
pub const PROMPT: &str = "eludite.agents.prompt";
pub const CANCEL: &str = "eludite.agents.cancel";
pub const PERMISSION: &str = "eludite.agents.permission";
pub const REVIEW: &str = "eludite.agents.review";
pub const CONFIGURE: &str = "eludite.agents.configure";

pub const ALL: [&str; 6] = [START, PROMPT, CANCEL, PERMISSION, REVIEW, CONFIGURE];

/// `eludite.agents.configure`'s `option` for the session mode (any other value names a config option).
pub const MODE_OPTION: &str = "mode";

const STATE_OUTPUT: &str = include_str!("../../../protocol/schemas/agents-state.output.json");

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    match id {
        // Starting an agent runs a process.
        START => (
            "Agents: Start Agent",
            include_str!("../../../protocol/schemas/agents-start.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        PROMPT => (
            "Agents: Send Prompt",
            include_str!("../../../protocol/schemas/agents-prompt.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        CANCEL => (
            "Agents: Cancel Turn",
            include_str!("../../../protocol/schemas/agents-cancel.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        // An agent answering permission prompts or accepting edits approves itself: always asked.
        PERMISSION => (
            "Agents: Answer Permission Request",
            include_str!("../../../protocol/schemas/agents-permission.input.json"),
            include_str!("../../../protocol/schemas/agents-permission.output.json"),
            Dangerous,
        ),
        REVIEW => (
            "Agents: Review Pending Change",
            include_str!("../../../protocol/schemas/agents-review.input.json"),
            include_str!("../../../protocol/schemas/agents-review.output.json"),
            Dangerous,
        ),
        // An agent changing another agent's model or mode is something to prompt about (brief 0058).
        CONFIGURE => (
            "Agents: Configure Session",
            include_str!("../../../protocol/schemas/agents-configure.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        other => unreachable!("not an agents command: {other}"),
    }
}

/// An answer to a permission request.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionDecision {
    Allow,
    AlwaysAllow,
    Deny,
}

/// Which pending changes a review names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReviewTarget {
    Change(u64),
    Path(String),
    All,
}

/// A parsed, validated agents command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsRequest {
    Start {
        agent: Option<String>,
        restart: bool,
    },
    Prompt {
        text: String,
    },
    Cancel,
    Permission {
        /// `None`: the oldest pending request.
        request: Option<u64>,
        decision: PermissionDecision,
    },
    Review {
        target: ReviewTarget,
        accept: bool,
    },
    /// The session's mode ([`MODE_OPTION`]) or a config option, set to `value`.
    Configure {
        option: String,
        value: String,
    },
}

impl AgentsRequest {
    pub fn command(&self) -> &'static str {
        match self {
            AgentsRequest::Start { .. } => START,
            AgentsRequest::Prompt { .. } => PROMPT,
            AgentsRequest::Cancel => CANCEL,
            AgentsRequest::Permission { .. } => PERMISSION,
            AgentsRequest::Review { .. } => REVIEW,
            AgentsRequest::Configure { .. } => CONFIGURE,
        }
    }
}

/// `agents-state.output.json`'s login methods.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoginRow {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub command: String,
}

/// `agents-state.output.json`'s registry rows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentRow {
    pub name: String,
    pub command: String,
    /// `native`, `npx` or `settings`.
    pub source: String,
}

/// `agents-state.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AgentsStateOutput {
    pub agent: String,
    /// `stopped`, `starting`, `ready`, `needs_login`, `running` or `error`.
    pub state: String,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub protocol_version: Option<u16>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub login: Vec<LoginRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_stop_reason: Option<String>,
    pub agents: Vec<AgentRow>,
    pub pending_permissions: u64,
    pub pending_changes: u64,
    /// The agent's slash commands (its latest ACP `available_commands_update`; brief 0057).
    #[serde(default)]
    pub commands: Vec<CommandRow>,
    /// The session's mode and the modes offered (brief 0058); absent when the agent offers none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeOutput>,
    /// The session's select config options (brief 0058); absent when the agent offers none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionRow>,
}

/// `agents-state.output.json`'s `mode`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeOutput {
    pub current: String,
    pub available: Vec<ModeRow>,
}

/// One mode of [`ModeOutput::available`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ModeRow {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One config option of `agents-state.output.json`'s `options`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OptionRow {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub category: Option<String>,
    pub current: String,
    pub choices: Vec<ChoiceRow>,
}

/// One value of [`OptionRow::choices`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChoiceRow {
    pub value: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

/// One slash command of `agents-state.output.json`'s `commands`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CommandRow {
    /// Without the leading `/`.
    pub name: String,
    pub description: String,
    /// What the command takes after its name (ACP's `input.hint`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

/// `agents-permission.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PermissionOutput {
    pub request: u64,
    pub decision: PermissionDecision,
    pub tool: String,
    pub class: PermissionClass,
    pub persisted: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub policy_path: Option<String>,
}

/// One change of `agents-review.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeRow {
    pub id: u64,
    pub path: String,
    /// `pending`, `applying`, `accepted`, `rejected` or `failed`.
    pub state: String,
    pub edits: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_call: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `agents-review.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReviewOutput {
    /// `accept` or `reject`.
    pub decision: String,
    pub changes: Vec<ChangeRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// One per command, serialized at once: the state's size (it grew the mode and the options in brief 0058) costs
/// nothing worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentsOutput {
    State(AgentsStateOutput),
    Permission(PermissionOutput),
    Review(ReviewOutput),
}

impl AgentsOutput {
    pub fn to_json(&self) -> Value {
        match self {
            AgentsOutput::State(o) => serde_json::to_value(o),
            AgentsOutput::Permission(o) => serde_json::to_value(o),
            AgentsOutput::Review(o) => serde_json::to_value(o),
        }
        .expect("agents outputs serialize")
    }
}

/// Whatever hosts the agents (the shell). Called on the invoking thread.
pub trait AgentsTarget: Send + Sync {
    fn apply(&self, request: AgentsRequest) -> Result<AgentsOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StartIn {
    agent: Option<String>,
    #[serde(default)]
    restart: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PromptIn {
    text: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PermissionIn {
    request: Option<u64>,
    decision: PermissionDecision,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ConfigureIn {
    option: String,
    value: String,
}

#[derive(Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum ReviewDecision {
    Accept,
    Reject,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReviewIn {
    change: Option<u64>,
    path: Option<String>,
    all: Option<bool>,
    decision: ReviewDecision,
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn required<T: for<'de> Deserialize<'de>>(value: Value) -> Result<T, CommandError> {
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

/// Parse and validate the input of agents command `id`.
pub fn parse(id: &str, value: Value) -> Result<AgentsRequest, CommandError> {
    Ok(match id {
        START => {
            let i: StartIn = input(value)?;
            if i.agent.as_deref() == Some("") {
                return Err(CommandError::InvalidInput(
                    "`agent` must not be empty".into(),
                ));
            }
            AgentsRequest::Start {
                agent: i.agent,
                restart: i.restart,
            }
        }
        PROMPT => {
            let i: PromptIn = required(value)?;
            if i.text.trim().is_empty() {
                return Err(CommandError::InvalidInput(
                    "`text` must not be empty".into(),
                ));
            }
            AgentsRequest::Prompt { text: i.text }
        }
        CANCEL => {
            let _: Empty = input(value)?;
            AgentsRequest::Cancel
        }
        PERMISSION => {
            let i: PermissionIn = required(value)?;
            if i.request == Some(0) {
                return Err(CommandError::InvalidInput("`request` is at least 1".into()));
            }
            AgentsRequest::Permission {
                request: i.request,
                decision: i.decision,
            }
        }
        REVIEW => {
            let i: ReviewIn = required(value)?;
            let target = match (i.change, i.path, i.all) {
                (Some(0), None, None) => {
                    return Err(CommandError::InvalidInput("`change` is at least 1".into()));
                }
                (Some(c), None, None) => ReviewTarget::Change(c),
                (None, Some(p), None) if !p.is_empty() => ReviewTarget::Path(p),
                (None, None, Some(true)) => ReviewTarget::All,
                _ => {
                    return Err(CommandError::InvalidInput(
                        "give exactly one of `change`, `path` and `all: true`".into(),
                    ));
                }
            };
            AgentsRequest::Review {
                target,
                accept: i.decision == ReviewDecision::Accept,
            }
        }
        CONFIGURE => {
            let i: ConfigureIn = required(value)?;
            if i.option.is_empty() || i.value.is_empty() {
                return Err(CommandError::InvalidInput(
                    "`option` and `value` must not be empty".into(),
                ));
            }
            AgentsRequest::Configure {
                option: i.option,
                value: i.value,
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The public description of agents command `id` (one of [`ALL`]). All are agent-visible: an outer agent could drive
/// an inner one (brief 0016), always through the same permission classes.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: parse_schema(input),
        output_schema: parse_schema(output),
        permission,
        agent_visible: true,
    }
}

/// Register every agents command, applying them to `target`.
pub fn register(registry: &mut CommandRegistry, target: Arc<dyn AgentsTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_validates_and_specs_follow_the_schemas() {
        assert_eq!(
            parse(START, json!({})).unwrap(),
            AgentsRequest::Start {
                agent: None,
                restart: false
            }
        );
        assert_eq!(
            parse(START, json!({"agent": "Claude Code", "restart": true})).unwrap(),
            AgentsRequest::Start {
                agent: Some("Claude Code".into()),
                restart: true
            }
        );
        assert!(parse(START, json!({"agent": ""})).is_err());
        assert_eq!(
            parse(PROMPT, json!({"text": "hi"})).unwrap(),
            AgentsRequest::Prompt { text: "hi".into() }
        );
        assert!(parse(PROMPT, json!({"text": "  "})).is_err());
        assert!(parse(PROMPT, json!({})).is_err());
        assert_eq!(parse(CANCEL, Value::Null).unwrap(), AgentsRequest::Cancel);
        assert!(parse(CANCEL, json!({"x": 1})).is_err());
        assert_eq!(
            parse(PERMISSION, json!({"decision": "always_allow"})).unwrap(),
            AgentsRequest::Permission {
                request: None,
                decision: PermissionDecision::AlwaysAllow
            }
        );
        assert!(parse(PERMISSION, json!({"decision": "maybe"})).is_err());
        assert!(parse(PERMISSION, json!({"request": 0, "decision": "deny"})).is_err());
        assert_eq!(
            parse(REVIEW, json!({"path": "/s/A.cs", "decision": "reject"})).unwrap(),
            AgentsRequest::Review {
                target: ReviewTarget::Path("/s/A.cs".into()),
                accept: false
            }
        );
        assert_eq!(
            parse(REVIEW, json!({"all": true, "decision": "accept"})).unwrap(),
            AgentsRequest::Review {
                target: ReviewTarget::All,
                accept: true
            }
        );
        assert!(
            parse(
                REVIEW,
                json!({"change": 1, "all": true, "decision": "accept"})
            )
            .is_err()
        );
        assert!(parse(REVIEW, json!({"decision": "accept"})).is_err());
        assert_eq!(
            parse(CONFIGURE, json!({"option": "model", "value": "opus"})).unwrap(),
            AgentsRequest::Configure {
                option: "model".into(),
                value: "opus".into()
            }
        );
        assert!(parse(CONFIGURE, json!({"option": "mode"})).is_err());
        assert!(parse(CONFIGURE, json!({"option": "", "value": "plan"})).is_err());
        assert!(
            parse(
                CONFIGURE,
                json!({"option": "mode", "value": "plan", "x": 1})
            )
            .is_err()
        );

        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["type"], "object");
        }
        assert_eq!(spec(PERMISSION).permission, PermissionClass::Dangerous);
        assert_eq!(spec(CANCEL).permission, PermissionClass::Read);
        assert_eq!(spec(CONFIGURE).permission, PermissionClass::Execute);
        // Outputs serialize to their schemas' required members.
        let state = AgentsOutput::State(AgentsStateOutput {
            agent: "A".into(),
            state: "ready".into(),
            generation: 1,
            session_id: None,
            agent_info: None,
            protocol_version: Some(1),
            message: None,
            login: Vec::new(),
            last_stop_reason: None,
            agents: vec![AgentRow {
                name: "A".into(),
                command: "a".into(),
                source: "settings".into(),
            }],
            pending_permissions: 0,
            pending_changes: 0,
            commands: vec![
                CommandRow {
                    name: "compact".into(),
                    description: "Summarize the conversation".into(),
                    hint: Some("<instructions>".into()),
                },
                CommandRow {
                    name: "context".into(),
                    description: "Show context usage".into(),
                    hint: None,
                },
            ],
            mode: Some(ModeOutput {
                current: "default".into(),
                available: vec![
                    ModeRow {
                        id: "default".into(),
                        name: "Manual".into(),
                        description: Some("Prompts as the policy says".into()),
                    },
                    ModeRow {
                        id: "plan".into(),
                        name: "Plan".into(),
                        description: None,
                    },
                ],
            }),
            options: vec![OptionRow {
                id: "model".into(),
                name: "Model".into(),
                description: None,
                category: Some("model".into()),
                current: "default".into(),
                choices: vec![ChoiceRow {
                    value: "default".into(),
                    name: "Default".into(),
                    description: Some("The recommended model".into()),
                }],
            }],
        })
        .to_json();
        let schema: Value = serde_json::from_str(STATE_OUTPUT).unwrap();
        for r in schema["required"].as_array().unwrap() {
            assert!(state.get(r.as_str().unwrap()).is_some(), "{r}");
        }
        for k in state.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        // The slash commands (brief 0057): each row's members are the schema's, the hint only when there is one.
        let row = &schema["properties"]["commands"]["items"];
        assert_eq!(
            state["commands"],
            json!([
                {"name": "compact", "description": "Summarize the conversation", "hint": "<instructions>"},
                {"name": "context", "description": "Show context usage"}
            ])
        );
        for c in state["commands"].as_array().unwrap() {
            for k in c.as_object().unwrap().keys() {
                assert!(row["properties"].get(k).is_some(), "{k}");
            }
        }
        // The mode and the options (brief 0058): each member is the schema's, the optional ones only when set.
        let mode = &schema["properties"]["mode"];
        assert_eq!(
            state["mode"],
            json!({"current": "default", "available": [
                {"id": "default", "name": "Manual", "description": "Prompts as the policy says"},
                {"id": "plan", "name": "Plan"}
            ]})
        );
        for k in state["mode"]["available"][0].as_object().unwrap().keys() {
            assert!(
                mode["properties"]["available"]["items"]["properties"]
                    .get(k)
                    .is_some(),
                "{k}"
            );
        }
        let option = &schema["properties"]["options"]["items"];
        assert_eq!(
            state["options"],
            json!([{"id": "model", "name": "Model", "category": "model", "current": "default",
                "choices": [{"value": "default", "name": "Default", "description": "The recommended model"}]}])
        );
        for k in state["options"][0].as_object().unwrap().keys() {
            assert!(option["properties"].get(k).is_some(), "{k}");
        }
        for r in option["required"].as_array().unwrap() {
            assert!(
                state["options"][0].get(r.as_str().unwrap()).is_some(),
                "{r}"
            );
        }
    }
}
