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

impl SessionUpdate {
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
