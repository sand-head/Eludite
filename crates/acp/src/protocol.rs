//! The subset of ACP (protocol version 1) that Eludite's client speaks.
//!
//! Hand-written from the schema at agentclientprotocol.com rather than taken
//! from the `agent-client-protocol` crate (see the crate docs). Unknown fields
//! are ignored and unknown `sessionUpdate` kinds decode to
//! [`SessionUpdate::Other`], so adapter extensions do not break decoding.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The ACP protocol version this client implements.
pub const PROTOCOL_VERSION: u16 = 1;

/// JSON-RPC error code ACP uses for "authentication required".
pub const AUTH_REQUIRED: i64 = -32000;

pub mod methods {
    pub const INITIALIZE: &str = "initialize";
    pub const SESSION_NEW: &str = "session/new";
    pub const SESSION_PROMPT: &str = "session/prompt";
    pub const SESSION_CANCEL: &str = "session/cancel";
    pub const SESSION_UPDATE: &str = "session/update";
    pub const SESSION_REQUEST_PERMISSION: &str = "session/request_permission";
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Implementation {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default)]
    pub version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileSystemCapability {
    pub read_text_file: bool,
    pub write_text_file: bool,
}

/// `auth.terminal`: the client can show terminal login methods. Claude's
/// adapter lists its login methods (with the command to run) only when this is
/// set. Eludite shows them; it does not run the login flow itself.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthCapabilities {
    #[serde(default)]
    pub terminal: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ClientCapabilities {
    pub fs: FileSystemCapability,
    pub terminal: bool,
    #[serde(default)]
    pub auth: AuthCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeRequest {
    pub protocol_version: u16,
    pub client_capabilities: ClientCapabilities,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub client_info: Option<Implementation>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpCapabilities {
    #[serde(default)]
    pub http: bool,
    #[serde(default)]
    pub sse: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentCapabilities {
    #[serde(default)]
    pub load_session: bool,
    #[serde(default)]
    pub mcp_capabilities: McpCapabilities,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthMethod {
    pub id: String,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    /// `terminal` for methods the user runs in a terminal.
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    /// For terminal methods: arguments to append to the agent's own command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub args: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResponse {
    pub protocol_version: u16,
    #[serde(default)]
    pub agent_capabilities: AgentCapabilities,
    #[serde(default)]
    pub auth_methods: Vec<AuthMethod>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub agent_info: Option<Implementation>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EnvVariable {
    pub name: String,
    pub value: String,
}

/// An MCP server the agent should connect to. Stdio is mandatory for every
/// ACP agent; HTTP only if the agent advertises `mcpCapabilities.http`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum McpServer {
    Http {
        #[serde(rename = "type")]
        kind: HttpTag,
        name: String,
        url: String,
        headers: Vec<EnvVariable>,
    },
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<EnvVariable>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HttpTag {
    Http,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionRequest {
    pub cwd: String,
    pub mcp_servers: Vec<McpServer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NewSessionResponse {
    pub session_id: String,
}

/// A content block. Only text is interpreted; anything else is kept raw.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ContentBlock {
    Text(TextContent),
    Other(Value),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextContent {
    #[serde(rename = "type")]
    pub kind: TextTag,
    pub text: String,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextTag {
    Text,
}

impl ContentBlock {
    pub fn text(text: impl Into<String>) -> Self {
        ContentBlock::Text(TextContent {
            kind: TextTag::Text,
            text: text.into(),
            meta: None,
        })
    }

    /// The block's `_meta`, if it is a text block that has one.
    pub fn meta(&self) -> Option<&Value> {
        match self {
            ContentBlock::Text(t) => t.meta.as_ref(),
            ContentBlock::Other(v) => v.get("_meta"),
        }
    }

    pub fn as_text(&self) -> Option<&str> {
        match self {
            ContentBlock::Text(t) => Some(&t.text),
            ContentBlock::Other(_) => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptRequest {
    pub session_id: String,
    pub prompt: Vec<ContentBlock>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromptResponse {
    pub stop_reason: StopReason,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CancelNotification {
    pub session_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolCallStatus {
    Pending,
    InProgress,
    Completed,
    Failed,
}

/// `tool_call` and `tool_call_update` share this shape; on an update only the
/// fields that changed are present.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolCall {
    pub tool_call_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub kind: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<ToolCallStatus>,
    /// `ToolCallContent` items (`content`, `diff`, `terminal`), kept raw.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub content: Option<Vec<Value>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_input: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub raw_output: Option<Value>,
    #[serde(rename = "_meta", default, skip_serializing_if = "Option::is_none")]
    pub meta: Option<Value>,
}

impl ToolCall {
    /// The agent's own tool name, if it reports one. Claude's adapter puts it
    /// in `_meta.claudeCode.toolName` (e.g. `mcp__eludite__diagnostics-list`).
    pub fn agent_tool_name(&self) -> Option<&str> {
        self.meta
            .as_ref()?
            .get("claudeCode")?
            .get("toolName")?
            .as_str()
    }

    /// The text of every `content` item, joined with newlines.
    pub fn content_text(&self) -> String {
        let mut out = Vec::new();
        for item in self.content.iter().flatten() {
            match item.get("type").and_then(Value::as_str) {
                Some("content") => {
                    if let Some(t) = item.pointer("/content/text").and_then(Value::as_str) {
                        out.push(t.to_owned());
                    }
                }
                Some("diff") => out.push(format!(
                    "diff {}",
                    item.get("path").and_then(Value::as_str).unwrap_or("?")
                )),
                Some(other) => out.push(format!("[{other}]")),
                None => {}
            }
        }
        out.join("\n")
    }

    /// Overlay the fields present in `update`.
    pub fn apply(&mut self, update: ToolCall) {
        macro_rules! take {
            ($($f:ident),*) => { $( if update.$f.is_some() { self.$f = update.$f; } )* };
        }
        take!(title, kind, status, content, raw_input, raw_output, meta);
    }
}

/// A file change an agent's tool call carries (`ToolCallContent` of type `diff`): the agent's own file tools show
/// what they will write this way.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolDiff {
    pub path: String,
    /// `None` for a new file.
    pub old_text: Option<String>,
    pub new_text: String,
}

impl ToolCall {
    /// The `diff` items of `content`.
    pub fn diffs(&self) -> Vec<ToolDiff> {
        self.content
            .iter()
            .flatten()
            .filter(|c| c.get("type").and_then(Value::as_str) == Some("diff"))
            .filter_map(|c| {
                Some(ToolDiff {
                    path: c.get("path")?.as_str()?.to_owned(),
                    old_text: c.get("oldText").and_then(Value::as_str).map(str::to_owned),
                    new_text: c.get("newText")?.as_str()?.to_owned(),
                })
            })
            .collect()
    }
}

/// One entry of a `plan` update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlanEntry {
    pub content: String,
    #[serde(default)]
    pub priority: String,
    /// `pending`, `in_progress` or `completed`.
    #[serde(default)]
    pub status: String,
}

/// One command of an `available_commands_update` (ACP schema 1.9's `AvailableCommand`): a slash command the person can
/// type as `/name`. Unknown fields are ignored, so an agent's extensions never break decoding.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommand {
    /// Without the leading `/`.
    pub name: String,
    #[serde(default)]
    pub description: String,
    /// What the command takes after its name, when it takes input (ACP's unstructured `input.hint`). An input of
    /// another shape reads as none, as ACP's own `DefaultOnError` does.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "input_or_none"
    )]
    pub input: Option<AvailableCommandInput>,
}

fn input_or_none<'de, D: serde::Deserializer<'de>>(
    d: D,
) -> Result<Option<AvailableCommandInput>, D::Error> {
    Ok(serde_json::from_value(Value::deserialize(d)?).ok())
}

/// An [`AvailableCommand`]'s input: all text typed after the name, with a hint shown until it is typed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AvailableCommandInput {
    #[serde(default)]
    pub hint: String,
}

impl AvailableCommand {
    /// The input hint, when there is a non-empty one.
    pub fn hint(&self) -> Option<&str> {
        self.input
            .as_ref()
            .map(|i| i.hint.as_str())
            .filter(|h| !h.is_empty())
    }
}

/// One `session/update` payload.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionUpdate {
    UserMessageChunk(ContentBlock),
    AgentMessageChunk(ContentBlock),
    AgentThoughtChunk(ContentBlock),
    ToolCall(ToolCall),
    ToolCallUpdate(ToolCall),
    /// `plan`, `available_commands_update`, `current_mode_update`,
    /// `usage_update` and adapter extensions: kind plus the raw object.
    Other {
        kind: String,
        raw: Value,
    },
}

/// A `usage_update` (ACP's context window and cost update; brief 0034), with the turn's token counts an adapter adds
/// in `_meta.claudeCode.usage` (eludite-claude-acp, under the names of ACP's `Usage`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Usage {
    /// Tokens in context.
    pub used: u64,
    /// The context window; 0 when the agent does not know it.
    pub size: u64,
    /// The session's cost so far, as the agent reports it: (amount, currency).
    pub cost: Option<(f64, String)>,
    /// The turn's tokens, when the agent gives them.
    pub turn: Option<TurnTokens>,
}

/// One turn's tokens (`_meta.claudeCode.usage`).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TurnTokens {
    /// Uncached input.
    #[serde(default)]
    pub input_tokens: u64,
    #[serde(default)]
    pub cached_read_tokens: u64,
    #[serde(default)]
    pub cached_write_tokens: u64,
    #[serde(default)]
    pub output_tokens: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thought_tokens: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl TurnTokens {
    /// Every input token: uncached, read from the cache and written to it.
    pub fn input_total(&self) -> u64 {
        self.input_tokens + self.cached_read_tokens + self.cached_write_tokens
    }
}

impl SessionUpdate {
    /// A `usage_update`'s counts (kept as [`SessionUpdate::Other`], like `plan`).
    pub fn usage(&self) -> Option<Usage> {
        let SessionUpdate::Other { kind, raw } = self else {
            return None;
        };
        if kind != "usage_update" {
            return None;
        }
        let n = |k: &str| raw.get(k).and_then(Value::as_u64).unwrap_or(0);
        let cost = raw.get("cost").and_then(|c| {
            Some((
                c.get("amount")?.as_f64()?,
                c.get("currency")
                    .and_then(Value::as_str)
                    .unwrap_or("USD")
                    .to_owned(),
            ))
        });
        let turn = raw
            .pointer("/_meta/claudeCode/usage")
            .and_then(|u| serde_json::from_value(u.clone()).ok());
        Some(Usage {
            used: n("used"),
            size: n("size"),
            cost,
            turn,
        })
    }

    /// The commands of an `available_commands_update` (kept as [`SessionUpdate::Other`], like `plan`; brief 0057). A
    /// command that does not decode (no name) is skipped, not the whole list.
    pub fn available_commands(&self) -> Option<Vec<AvailableCommand>> {
        match self {
            SessionUpdate::Other { kind, raw } if kind == "available_commands_update" => Some(
                raw.get("availableCommands")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(|c| serde_json::from_value(c.clone()).ok())
                    .collect(),
            ),
            _ => None,
        }
    }

    /// The entries of a `plan` update (kept as [`SessionUpdate::Other`] so new fields never break decoding).
    pub fn plan_entries(&self) -> Option<Vec<PlanEntry>> {
        match self {
            SessionUpdate::Other { kind, raw } if kind == "plan" => {
                serde_json::from_value(raw.get("entries")?.clone()).ok()
            }
            _ => None,
        }
    }
}

impl<'de> Deserialize<'de> for SessionUpdate {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        use serde::de::Error;
        let raw = Value::deserialize(d)?;
        let kind = raw
            .get("sessionUpdate")
            .and_then(Value::as_str)
            .ok_or_else(|| D::Error::custom("missing sessionUpdate"))?
            .to_owned();
        let content = || {
            raw.get("content")
                .cloned()
                .ok_or_else(|| D::Error::custom("missing content"))
                .and_then(|c| serde_json::from_value::<ContentBlock>(c).map_err(D::Error::custom))
        };
        let tool = || serde_json::from_value::<ToolCall>(raw.clone()).map_err(D::Error::custom);
        Ok(match kind.as_str() {
            "user_message_chunk" => SessionUpdate::UserMessageChunk(content()?),
            "agent_message_chunk" => SessionUpdate::AgentMessageChunk(content()?),
            "agent_thought_chunk" => SessionUpdate::AgentThoughtChunk(content()?),
            "tool_call" => SessionUpdate::ToolCall(tool()?),
            "tool_call_update" => SessionUpdate::ToolCallUpdate(tool()?),
            _ => SessionUpdate::Other { kind, raw },
        })
    }
}

impl Serialize for SessionUpdate {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::Error;
        let with_kind = |kind: &str, mut v: Value| {
            if let Some(o) = v.as_object_mut() {
                o.insert("sessionUpdate".into(), Value::String(kind.into()));
            }
            v
        };
        let chunk = |kind: &str, c: &ContentBlock| {
            serde_json::to_value(c)
                .map(|c| serde_json::json!({"sessionUpdate": kind, "content": c}))
        };
        let v = match self {
            SessionUpdate::UserMessageChunk(c) => chunk("user_message_chunk", c),
            SessionUpdate::AgentMessageChunk(c) => chunk("agent_message_chunk", c),
            SessionUpdate::AgentThoughtChunk(c) => chunk("agent_thought_chunk", c),
            SessionUpdate::ToolCall(t) => {
                serde_json::to_value(t).map(|v| with_kind("tool_call", v))
            }
            SessionUpdate::ToolCallUpdate(t) => {
                serde_json::to_value(t).map(|v| with_kind("tool_call_update", v))
            }
            SessionUpdate::Other { raw, .. } => Ok(raw.clone()),
        }
        .map_err(S::Error::custom)?;
        v.serialize(s)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionNotification {
    pub session_id: String,
    pub update: SessionUpdate,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PermissionOptionKind {
    AllowOnce,
    AllowAlways,
    RejectOnce,
    RejectAlways,
}

impl PermissionOptionKind {
    pub fn is_allow(self) -> bool {
        matches!(self, Self::AllowOnce | Self::AllowAlways)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionOption {
    pub option_id: String,
    pub name: String,
    pub kind: PermissionOptionKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestPermissionRequest {
    pub session_id: String,
    pub tool_call: ToolCall,
    pub options: Vec<PermissionOption>,
}

impl RequestPermissionRequest {
    /// The first option of `kind`, falling back to any allow (or reject) option.
    pub fn option(&self, allow: bool) -> Option<&PermissionOption> {
        let (first, second) = if allow {
            (
                PermissionOptionKind::AllowOnce,
                PermissionOptionKind::AllowAlways,
            )
        } else {
            (
                PermissionOptionKind::RejectOnce,
                PermissionOptionKind::RejectAlways,
            )
        };
        self.options
            .iter()
            .find(|o| o.kind == first)
            .or_else(|| self.options.iter().find(|o| o.kind == second))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum RequestPermissionOutcome {
    Cancelled,
    #[serde(rename_all = "camelCase")]
    Selected {
        option_id: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RequestPermissionResponse {
    pub outcome: RequestPermissionOutcome,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn available_commands_decode_and_a_bad_entry_is_skipped() {
        // As the Node adapter sends them (crates/acp/tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl), plus
        // an entry with no name and one whose input is not ACP's unstructured input.
        let line = r#"{"sessionUpdate":"available_commands_update","availableCommands":[{"name":"compact","description":"Free up context by summarizing the conversation so far","input":{"hint":"<optional custom summarization instructions>"}},{"name":"init","description":"Initialize a new CLAUDE.md file with codebase documentation","input":null},{"description":"no name"},{"name":"model","description":"Set the model","input":"oops","_meta":{"x":1}}]}"#;
        let update: SessionUpdate = serde_json::from_str(line).unwrap();
        let commands = update.available_commands().expect("a command list");
        let names: Vec<&str> = commands.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["compact", "init", "model"]);
        assert_eq!(
            commands[0].hint(),
            Some("<optional custom summarization instructions>")
        );
        assert_eq!(commands[1].hint(), None);
        assert_eq!(commands[2].hint(), None);
        // It round-trips unchanged, and is neither a plan nor usage.
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            serde_json::from_str::<Value>(line).unwrap()
        );
        assert!(update.plan_entries().is_none() && update.usage().is_none());
        // An empty list is a list (the agent has no commands now).
        let empty: SessionUpdate = serde_json::from_str(
            r#"{"sessionUpdate":"available_commands_update","availableCommands":[]}"#,
        )
        .unwrap();
        assert_eq!(empty.available_commands(), Some(Vec::new()));
        let plan: SessionUpdate =
            serde_json::from_str(r#"{"sessionUpdate":"plan","entries":[]}"#).unwrap();
        assert!(plan.available_commands().is_none());
    }

    #[test]
    fn a_usage_update_decodes_with_the_turns_tokens() {
        // As eludite-claude-acp sends it (its golden file, brief 0034).
        let line = r#"{"sessionUpdate":"usage_update","used":22662,"size":1000000,"cost":{"amount":0.26172724999999997,"currency":"USD"},"_meta":{"claudeCode":{"usage":{"inputTokens":66,"cachedReadTokens":55789,"cachedWriteTokens":10821,"outputTokens":614,"thoughtTokens":57,"totalTokens":67290,"model":"claude-fable-5-1"}}}}"#;
        let update: SessionUpdate = serde_json::from_str(line).unwrap();
        let u = update.usage().expect("a usage update");
        assert_eq!((u.used, u.size), (22_662, 1_000_000));
        assert_eq!(u.cost, Some((0.26172724999999997, "USD".to_owned())));
        let t = u.turn.unwrap();
        assert_eq!(
            (t.input_tokens, t.cached_read_tokens, t.cached_write_tokens),
            (66, 55_789, 10_821)
        );
        assert_eq!((t.output_tokens, t.thought_tokens), (614, Some(57)));
        assert_eq!(t.input_total(), 66 + 55_789 + 10_821);
        assert_eq!(t.model.as_deref(), Some("claude-fable-5-1"));
        // It round-trips unchanged.
        assert_eq!(
            serde_json::to_value(&update).unwrap(),
            serde_json::from_str::<Value>(line).unwrap()
        );
        // ACP's own fields only (another agent): no turn tokens, no cost.
        let plain: SessionUpdate =
            serde_json::from_str(r#"{"sessionUpdate":"usage_update","used":53000,"size":200000}"#)
                .unwrap();
        let u = plain.usage().unwrap();
        assert_eq!(
            (u.used, u.size, u.cost, u.turn),
            (53_000, 200_000, None, None)
        );
        // Other updates are not usage.
        let plan: SessionUpdate =
            serde_json::from_str(r#"{"sessionUpdate":"plan","entries":[]}"#).unwrap();
        assert!(plan.usage().is_none());
    }
}
