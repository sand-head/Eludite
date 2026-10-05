//! The Agents window's commands (brief 0016): `eludite.agents.start`, `prompt`, `cancel`, `permission` (answer a
//! pending request) and `review` (accept or reject a pending change); brief 0057 adds `configure` (the session's mode
//! or a config option such as the model or the effort, as the pickers under the prompt box set them). The window's
//! buttons, the prompt box, its keys and the pickers run them, and they are agent-visible too, so an outer agent could
//! drive an inner one.
//!
//! The schemas are the files in `protocol/schemas/` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`AgentsRequest`] and serializes the typed [`AgentsOutput`]; the shell implements
//! [`AgentsTarget`].
//!
//! Brief 0059 adds the OpenAI-compatible servers' commands, [`PROVIDER_COMMANDS`]: `eludite.agents.provider_set`
//! (`dangerous`: it stores a credential; its `apiKey` is `<redacted>` in the audit and in `Debug`),
//! `provider_remove` and `provider_models` (`execute`: a network call). They parse into a [`ProviderRequest`]
//! answered by a [`ProviderTarget`] and are registered by [`register_providers`], apart from [`register`].

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

pub const PROVIDER_SET: &str = "eludite.agents.provider_set";
pub const PROVIDER_REMOVE: &str = "eludite.agents.provider_remove";
pub const PROVIDER_MODELS: &str = "eludite.agents.provider_models";

/// The OpenAI-compatible servers' commands (brief 0059), registered by [`register_providers`].
pub const PROVIDER_COMMANDS: [&str; 3] = [PROVIDER_SET, PROVIDER_REMOVE, PROVIDER_MODELS];

/// What the audit and logs show in place of a key.
pub const REDACTED: &str = "<redacted>";

const STATE_OUTPUT: &str = include_str!("../../../protocol/schemas/agents-state.output.json");
const PROVIDER_OUTPUT: &str = include_str!("../../../protocol/schemas/agents-provider.output.json");

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
        // An agent changing another agent's model or mode is something to prompt about (brief 0057).
        CONFIGURE => (
            "Agents: Configure Session",
            include_str!("../../../protocol/schemas/agents-configure.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        // It stores a credential.
        PROVIDER_SET => (
            "Agents: Add or Change Server",
            include_str!("../../../protocol/schemas/agents-provider-set.input.json"),
            PROVIDER_OUTPUT,
            Dangerous,
        ),
        PROVIDER_REMOVE => (
            "Agents: Remove Server",
            include_str!("../../../protocol/schemas/agents-provider-remove.input.json"),
            PROVIDER_OUTPUT,
            Execute,
        ),
        // It makes a network call.
        PROVIDER_MODELS => (
            "Agents: List Server Models",
            include_str!("../../../protocol/schemas/agents-provider-models.input.json"),
            include_str!("../../../protocol/schemas/agents-provider-models.output.json"),
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
    /// `native`, `npx`, `provider` or `settings`.
    pub source: String,
}

/// `agents-state.output.json`. Not `Eq`: the usage's cost is an `f64` (brief 0058).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    /// The agent's slash commands (its latest ACP `available_commands_update`; brief 0056).
    #[serde(default)]
    pub commands: Vec<CommandRow>,
    /// The session's mode and the modes offered (brief 0057); absent when the agent offers none.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ModeOutput>,
    /// The session's select config options (brief 0057); absent when the agent offers none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub options: Vec<OptionRow>,
    /// The session's context and cost as the usage strip shows them (brief 0058): the agent's last `usage_update`;
    /// absent before the first one, cleared by a restart.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageOutput>,
}

/// `agents-state.output.json`'s `usage` (brief 0058).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UsageOutput {
    /// Tokens in context.
    pub used: u64,
    /// The context window; 0 when the agent does not know it.
    pub size: u64,
    /// The session's cost so far, the agent's running total.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cost: Option<CostOutput>,
}

/// An amount of money in `currency` (ISO 4217).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CostOutput {
    pub amount: f64,
    pub currency: String,
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

/// One per command, serialized at once: the state's size (it grew the mode and the options in brief 0057) costs
/// nothing worth a box.
#[allow(clippy::large_enum_variant)]
#[derive(Debug, Clone, PartialEq)]
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

/// A key that never prints: `Debug` shows [`REDACTED`].
#[derive(Clone, PartialEq, Eq)]
pub struct ApiKey(String);

impl ApiKey {
    pub fn new(key: impl Into<String>) -> Self {
        Self(key.into())
    }

    /// The key itself, for the credential store and the agent's environment only.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Debug for ApiKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(REDACTED)
    }
}

/// What `provider_set` does with the stored key.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyChange {
    /// `apiKey` absent: keep the stored key.
    Keep,
    /// `apiKey: ""`: delete it.
    Delete,
    /// Store this key.
    Set(ApiKey),
}

/// One model of a provider's catalog or listing (`{id, name?, contextWindow?}`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderModel {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_window: Option<u64>,
}

/// `provider_set`'s input, validated.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProviderSet {
    pub name: String,
    pub base_url: String,
    pub api_key: KeyChange,
    pub default_model: Option<String>,
    pub headers: std::collections::BTreeMap<String, String>,
    pub models: Vec<ProviderModel>,
    /// `core` or `all`.
    pub tools: Option<String>,
    /// `allowFileStore`: the person agreed to the 0600 file when the credential store is unavailable (brief 0046).
    pub allow_file_store: bool,
}

/// A parsed, validated provider command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderRequest {
    Set(ProviderSet),
    Remove {
        name: String,
    },
    /// List models: a saved server by `name`, or `base_url` (and `api_key`) for one being added.
    Models {
        name: Option<String>,
        base_url: Option<String>,
        api_key: Option<ApiKey>,
    },
}

/// A saved server in `agents-provider.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderRow {
    pub name: String,
    pub base_url: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_model: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tools: Option<String>,
    pub has_key: bool,
}

/// `agents-provider.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProviderOutput {
    pub name: String,
    /// `saved` or `removed`.
    pub action: String,
    pub has_key: bool,
    pub providers: Vec<ProviderRow>,
}

/// `agents-provider-models.output.json`, also what `eludite-openai-acp models` prints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProviderModelsOutput {
    pub models: Vec<ProviderModel>,
    /// `server`, `catalog` or `none`.
    pub listing: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProviderCommandOutput {
    Provider(ProviderOutput),
    Models(ProviderModelsOutput),
}

impl ProviderCommandOutput {
    pub fn to_json(&self) -> Value {
        match self {
            ProviderCommandOutput::Provider(o) => serde_json::to_value(o),
            ProviderCommandOutput::Models(o) => serde_json::to_value(o),
        }
        .expect("provider outputs serialize")
    }
}

/// Whatever keeps the servers (the shell: `agents.json` and the credential store). Called on the invoking thread;
/// `provider_models` makes a network call, so the shell runs it off the UI thread.
pub trait ProviderTarget: Send + Sync {
    fn apply(&self, request: ProviderRequest) -> Result<ProviderCommandOutput, CommandError>;
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderSetIn {
    name: String,
    base_url: String,
    api_key: Option<String>,
    default_model: Option<String>,
    #[serde(default)]
    headers: std::collections::BTreeMap<String, String>,
    #[serde(default)]
    models: Vec<ProviderModel>,
    tools: Option<String>,
    #[serde(default)]
    allow_file_store: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProviderRemoveIn {
    name: String,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderModelsIn {
    name: Option<String>,
    base_url: Option<String>,
    api_key: Option<String>,
}

fn provider_name(name: &str) -> Result<(), CommandError> {
    if name.trim().is_empty() || name.chars().count() > 100 || name.chars().any(char::is_control) {
        return Err(CommandError::InvalidInput(
            "`name` is 1 to 100 characters with no control characters".into(),
        ));
    }
    Ok(())
}

fn base_url(url: &str) -> Result<(), CommandError> {
    let rest = url
        .strip_prefix("http://")
        .or_else(|| url.strip_prefix("https://"));
    match rest {
        Some(host)
            if !host.is_empty() && !host.starts_with('/') && !url.contains(char::is_whitespace) =>
        {
            Ok(())
        }
        _ => Err(CommandError::InvalidInput(
            "`baseUrl` is an http:// or https:// url such as http://localhost:8080/v1".into(),
        )),
    }
}

/// Parse and validate the input of provider command `id` (one of [`PROVIDER_COMMANDS`]).
pub fn parse_provider(id: &str, value: Value) -> Result<ProviderRequest, CommandError> {
    Ok(match id {
        PROVIDER_SET => {
            let i: ProviderSetIn = required(value)?;
            provider_name(&i.name)?;
            base_url(&i.base_url)?;
            if i.headers.len() > 20 {
                return Err(CommandError::InvalidInput("at most 20 `headers`".into()));
            }
            for k in i.headers.keys() {
                if k.is_empty()
                    || !k
                        .chars()
                        .all(|c| c.is_ascii_alphanumeric() || "-_".contains(c))
                {
                    return Err(CommandError::InvalidInput(format!(
                        "{k:?} is not a header name"
                    )));
                }
                if k.eq_ignore_ascii_case("authorization") {
                    return Err(CommandError::InvalidInput(
                        "the key goes in `apiKey`, never in `headers`".into(),
                    ));
                }
            }
            if i.models.iter().any(|m| m.id.trim().is_empty()) {
                return Err(CommandError::InvalidInput(
                    "a model's `id` must not be empty".into(),
                ));
            }
            if let Some(t) = &i.tools
                && t != "core"
                && t != "all"
            {
                return Err(CommandError::InvalidInput(
                    "`tools` is `core` or `all`".into(),
                ));
            }
            if i.default_model.as_deref() == Some("") {
                return Err(CommandError::InvalidInput(
                    "`defaultModel` must not be empty".into(),
                ));
            }
            ProviderRequest::Set(ProviderSet {
                name: i.name,
                base_url: i.base_url,
                api_key: match i.api_key {
                    None => KeyChange::Keep,
                    Some(k) if k.is_empty() => KeyChange::Delete,
                    Some(k) => KeyChange::Set(ApiKey(k)),
                },
                default_model: i.default_model,
                headers: i.headers,
                models: i.models,
                tools: i.tools,
                allow_file_store: i.allow_file_store,
            })
        }
        PROVIDER_REMOVE => {
            let i: ProviderRemoveIn = required(value)?;
            provider_name(&i.name)?;
            ProviderRequest::Remove { name: i.name }
        }
        PROVIDER_MODELS => {
            let i: ProviderModelsIn = input(value)?;
            if i.name.is_none() && i.base_url.is_none() {
                return Err(CommandError::InvalidInput(
                    "give `name` or `baseUrl`".into(),
                ));
            }
            if let Some(n) = &i.name {
                provider_name(n)?;
            }
            if let Some(u) = &i.base_url {
                base_url(u)?;
            }
            ProviderRequest::Models {
                name: i.name,
                base_url: i.base_url,
                api_key: i.api_key.filter(|k| !k.is_empty()).map(ApiKey),
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// What the audit keeps of a provider command's arguments: `apiKey` as [`REDACTED`].
pub fn redact_provider_arguments(input: &Value) -> Value {
    let mut v = input.clone();
    if let Some(o) = v.as_object_mut()
        && o.contains_key("apiKey")
    {
        o.insert("apiKey".into(), Value::String(REDACTED.into()));
    }
    v
}

/// Register the provider commands, applying them to `target`, with the key redacted from the audit.
pub fn register_providers(registry: &CommandRegistry, target: Arc<dyn ProviderTarget>) {
    for id in PROVIDER_COMMANDS {
        let target = target.clone();
        registry.replace_with_redaction(
            spec(id),
            None,
            Arc::new(redact_provider_arguments),
            move |input| {
                let request = parse_provider(id, input)?;
                target.apply(request).map(|out| out.to_json())
            },
        );
    }
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The public description of agents command `id` (one of [`ALL`] or [`PROVIDER_COMMANDS`]). All are agent-visible: an outer agent could drive
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
            usage: Some(UsageOutput {
                used: 61_204,
                size: 1_000_000,
                cost: Some(CostOutput {
                    amount: 0.9512,
                    currency: "USD".into(),
                }),
            }),
        })
        .to_json();
        let schema: Value = serde_json::from_str(STATE_OUTPUT).unwrap();
        for r in schema["required"].as_array().unwrap() {
            assert!(state.get(r.as_str().unwrap()).is_some(), "{r}");
        }
        for k in state.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        // The usage (brief 0058): the schema's members, the cost an object.
        assert_eq!(
            state["usage"],
            json!({"used": 61_204, "size": 1_000_000, "cost": {"amount": 0.9512, "currency": "USD"}})
        );
        let usage = &schema["properties"]["usage"];
        for k in state["usage"].as_object().unwrap().keys() {
            assert!(usage["properties"].get(k).is_some(), "{k}");
        }
        for k in state["usage"]["cost"].as_object().unwrap().keys() {
            assert!(
                usage["properties"]["cost"]["properties"].get(k).is_some(),
                "{k}"
            );
        }
        // The slash commands (brief 0056): each row's members are the schema's, the hint only when there is one.
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
        // The mode and the options (brief 0057): each member is the schema's, the optional ones only when set.
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
    struct Servers(std::sync::Mutex<Vec<ProviderRequest>>);

    impl ProviderTarget for Servers {
        fn apply(&self, request: ProviderRequest) -> Result<ProviderCommandOutput, CommandError> {
            self.0.lock().unwrap().push(request.clone());
            Ok(match request {
                ProviderRequest::Models { .. } => {
                    ProviderCommandOutput::Models(ProviderModelsOutput {
                        models: vec![ProviderModel {
                            id: "qwen3".into(),
                            name: None,
                            context_window: Some(40_960),
                        }],
                        listing: "server".into(),
                        message: None,
                    })
                }
                ProviderRequest::Set(s) => ProviderCommandOutput::Provider(ProviderOutput {
                    name: s.name.clone(),
                    action: "saved".into(),
                    has_key: matches!(s.api_key, KeyChange::Set(_)),
                    providers: vec![ProviderRow {
                        name: s.name,
                        base_url: s.base_url,
                        default_model: s.default_model,
                        tools: s.tools,
                        has_key: true,
                    }],
                }),
                ProviderRequest::Remove { name } => {
                    ProviderCommandOutput::Provider(ProviderOutput {
                        name,
                        action: "removed".into(),
                        has_key: false,
                        providers: Vec::new(),
                    })
                }
            })
        }
    }

    fn conforms(schema: &Value, value: &Value) {
        for r in schema["required"].as_array().unwrap() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "{r}");
        }
        for k in value.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
    }

    #[test]
    fn provider_commands_parse_validate_and_follow_the_schemas() {
        let set = parse_provider(
            PROVIDER_SET,
            json!({"name": "llama", "baseUrl": "http://localhost:8080/v1", "apiKey": "sk-1",
                "headers": {"X-Title": "Eludite"}, "models": [{"id": "m", "contextWindow": 16384}], "tools": "all"}),
        )
        .unwrap();
        let ProviderRequest::Set(s) = &set else {
            panic!()
        };
        assert_eq!(s.api_key, KeyChange::Set(ApiKey::new("sk-1")));
        assert_eq!(s.models[0].context_window, Some(16_384));
        assert!(
            !format!("{set:?}").contains("sk-1"),
            "Debug never shows the key"
        );
        let keep = |v: Value| match parse_provider(PROVIDER_SET, v).unwrap() {
            ProviderRequest::Set(s) => s.api_key,
            _ => unreachable!(),
        };
        assert_eq!(
            keep(json!({"name": "a", "baseUrl": "https://api.openai.com/v1"})),
            KeyChange::Keep
        );
        assert_eq!(
            keep(json!({"name": "a", "baseUrl": "https://api.openai.com/v1", "apiKey": ""})),
            KeyChange::Delete
        );
        // The consent to the 0600 file (brief 0046's rule) is only ever given explicitly.
        let consent = |v: Value| match parse_provider(PROVIDER_SET, v).unwrap() {
            ProviderRequest::Set(s) => s.allow_file_store,
            _ => unreachable!(),
        };
        assert!(!consent(
            json!({"name": "a", "baseUrl": "http://h/v1", "apiKey": "k"})
        ));
        assert!(consent(
            json!({"name": "a", "baseUrl": "http://h/v1", "apiKey": "k", "allowFileStore": true})
        ));
        for bad in [
            json!({"name": "", "baseUrl": "http://h/v1"}),
            json!({"name": "a", "baseUrl": "ftp://h/v1"}),
            json!({"name": "a", "baseUrl": "http://"}),
            json!({"name": "a", "baseUrl": "http://h/v1", "headers": {"Authorization": "Bearer x"}}),
            json!({"name": "a", "baseUrl": "http://h/v1", "tools": "some"}),
            json!({"name": "a", "baseUrl": "http://h/v1", "models": [{"id": ""}]}),
            json!({"name": "a", "baseUrl": "http://h/v1", "extra": 1}),
        ] {
            assert!(parse_provider(PROVIDER_SET, bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(
            parse_provider(PROVIDER_REMOVE, json!({"name": "llama"})).unwrap(),
            ProviderRequest::Remove {
                name: "llama".into()
            }
        );
        assert!(parse_provider(PROVIDER_MODELS, json!({})).is_err());
        assert!(parse_provider(PROVIDER_MODELS, Value::Null).is_err());
        assert_eq!(
            parse_provider(
                PROVIDER_MODELS,
                json!({"baseUrl": "http://localhost:11434/v1", "apiKey": ""})
            )
            .unwrap(),
            ProviderRequest::Models {
                name: None,
                base_url: Some("http://localhost:11434/v1".into()),
                api_key: None
            }
        );
        assert_eq!(spec(PROVIDER_SET).permission, PermissionClass::Dangerous);
        assert_eq!(spec(PROVIDER_REMOVE).permission, PermissionClass::Execute);
        assert_eq!(spec(PROVIDER_MODELS).permission, PermissionClass::Execute);
        for id in PROVIDER_COMMANDS {
            let s = spec(id);
            assert!(s.agent_visible);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["additionalProperties"], false);
        }
        assert!(
            spec(PROVIDER_SET).output_schema["title"]
                .as_str()
                .unwrap()
                .contains("provider_set")
        );
        assert!(
            parse(PROVIDER_SET, json!({})).is_err(),
            "not an AgentsRequest"
        );
    }

    #[test]
    fn provider_commands_answer_their_schemas_and_never_audit_the_key() {
        use crate::{Caller, with_caller};
        let reg = CommandRegistry::new();
        let target = Arc::new(Servers(std::sync::Mutex::default()));
        register_providers(&reg, target.clone());
        let agent = Caller::Agent {
            agent: "test".into(),
            call: 1,
            tool_call: None,
        };
        let out = with_caller(agent.clone(), || {
            reg.invoke(
                PROVIDER_SET,
                json!({"name": "or", "baseUrl": "https://openrouter.ai/api/v1", "apiKey": "sk-or-secret"}),
            )
        })
        .unwrap();
        conforms(&spec(PROVIDER_SET).output_schema, &out);
        assert_eq!(out["action"], "saved");
        let models = with_caller(agent.clone(), || {
            reg.invoke(
                PROVIDER_MODELS,
                json!({"baseUrl": "http://localhost:8080/v1", "apiKey": "sk-or-secret"}),
            )
        })
        .unwrap();
        conforms(&spec(PROVIDER_MODELS).output_schema, &models);
        assert_eq!(
            models["models"][0],
            json!({"id": "qwen3", "contextWindow": 40960})
        );
        let removed =
            with_caller(agent, || reg.invoke(PROVIDER_REMOVE, json!({"name": "or"}))).unwrap();
        conforms(&spec(PROVIDER_REMOVE).output_schema, &removed);
        // The handler got the key; the audit never did.
        assert!(
            matches!(&target.0.lock().unwrap()[0], ProviderRequest::Set(s) if s.api_key == KeyChange::Set(ApiKey::new("sk-or-secret")))
        );
        let audit = serde_json::to_string(&reg.audit_log().entries()).unwrap();
        assert!(!audit.contains("sk-or-secret"), "{audit}");
        assert!(audit.contains(REDACTED), "{audit}");
        assert_eq!(
            redact_provider_arguments(&json!({"name": "x"})),
            json!({"name": "x"})
        );
    }
}
