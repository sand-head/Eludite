//! One turn's translation from `claude` stream-json messages to ACP
//! `session/update` payloads. Pure: no I/O, so the recorded sessions can be
//! replayed through it in tests.
//!
//! - `stream_event` `content_block_delta` `text_delta` -> `agent_message_chunk`;
//!   `thinking_delta` with text -> `agent_thought_chunk`.
//! - `stream_event` `content_block_start` `tool_use` -> `tool_call` (pending,
//!   title from the tool name, `rawInput: {}`).
//! - `assistant` `tool_use` (complete input) -> `tool_call_update` with title,
//!   kind, locations, diff content and `rawInput` (or a full `tool_call` if the
//!   start was not streamed). Text and thinking in `assistant` messages are
//!   emitted only when that message streamed none (synthetic messages).
//! - `user` `tool_result` -> `tool_call_update` completed or failed, with the
//!   result text and `rawOutput`.
//! - `result` ends the turn, after a `usage_update` (brief 0034) with the
//!   turn's token counts: ACP's `used` (the tokens in context at the turn's
//!   last model call), `size` (the model's context window from `modelUsage`, 0
//!   when absent) and `cost` (`total_cost_usd`, the session's running total in
//!   USD), and in `_meta.claudeCode.usage` the turn's `usage` under the names of
//!   ACP's `Usage`: `inputTokens` (uncached), `cachedReadTokens`,
//!   `cachedWriteTokens`, `outputTokens`, `thoughtTokens` when given and
//!   `totalTokens` (their sum), plus `model`.
//!
//! Only top-level messages (`parent_tool_use_id: null`) produce text; tool
//! calls from subagents are shown too.
//!
//! Outside a turn, [`available_commands_update`] turns the slash commands of
//! `claude`'s `initialize` reply into the `available_commands_update` sent
//! right after `session/new` answers (brief 0056).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, ContentBlock, ContentChunk,
    Cost, Meta, SessionUpdate, StopReason, ToolCall, ToolCallStatus, ToolCallUpdate,
    ToolCallUpdateFields, ToolKind, UnstructuredCommandInput, UsageUpdate,
};
use serde_json::{Map, Value, json};

use crate::mapping::{tool_info, tool_meta, tool_result_fields};

/// ACP's commands from `claude`'s (`[{name, description, argumentHint, aliases?, builtin?}]`): `argumentHint` becomes
/// `input.hint` when it is not empty, a name starting with `__` (Claude Code's internal commands) is dropped, an entry
/// without a name is skipped, and aliases are not sent (ACP has no field for them).
pub fn available_commands(commands: &[Value]) -> Vec<AvailableCommand> {
    commands
        .iter()
        .filter_map(|c| {
            let name = c.get("name").and_then(Value::as_str)?;
            if name.is_empty() || name.starts_with("__") {
                return None;
            }
            let description = c.get("description").and_then(Value::as_str).unwrap_or("");
            let hint = c
                .get("argumentHint")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|h| !h.is_empty())
                .map(|h| {
                    AvailableCommandInput::Unstructured(UnstructuredCommandInput::new(h.to_owned()))
                });
            Some(AvailableCommand::new(name, description).input(hint))
        })
        .collect()
}

/// The `available_commands_update` for `claude`'s commands (see [`available_commands`]).
pub fn available_commands_update(commands: &[Value]) -> SessionUpdate {
    SessionUpdate::AvailableCommandsUpdate(AvailableCommandsUpdate::new(available_commands(
        commands,
    )))
}

/// How a turn ended.
#[derive(Debug, Clone, PartialEq)]
pub enum TurnEnd {
    Stop(StopReason),
    /// `claude` is not logged in (`error: authentication_failed`).
    AuthRequired,
    /// The turn failed; the message is for the client.
    Error(String),
}

/// Per-session translation state.
#[derive(Debug)]
pub struct Translator {
    cwd: PathBuf,
    /// Tool uses seen: id -> (name, kind).
    tools: HashMap<String, (String, ToolKind)>,
    /// Message ids whose text or thinking was streamed as deltas.
    streamed: HashSet<String>,
    current_message: Option<String>,
    auth_failed: bool,
    /// The tokens in context at the turn's last top-level model call
    /// (uncached input, cache reads and cache writes), and its model.
    context: Option<u64>,
    model: Option<String>,
}

fn is_top_level(msg: &Value) -> bool {
    msg.get("parent_tool_use_id").is_none_or(Value::is_null)
}

impl Translator {
    pub fn new(cwd: PathBuf) -> Self {
        Self {
            cwd,
            tools: HashMap::new(),
            streamed: HashSet::new(),
            current_message: None,
            auth_failed: false,
            context: None,
            model: None,
        }
    }

    /// Reset per-turn state at the start of a prompt.
    pub fn begin_turn(&mut self) {
        self.streamed.clear();
        self.current_message = None;
        self.auth_failed = false;
        self.context = None;
    }

    /// The name of a tool use seen in this session.
    pub fn tool_name(&self, id: &str) -> Option<&str> {
        self.tools.get(id).map(|(n, _)| n.as_str())
    }

    /// Translate one message. Updates are appended to `out`; returns the end of
    /// the turn on `result`. `cancelled` says whether the client cancelled.
    pub fn on_message(
        &mut self,
        msg: &Value,
        cancelled: bool,
        out: &mut Vec<SessionUpdate>,
    ) -> Option<TurnEnd> {
        match msg.get("type").and_then(Value::as_str)? {
            "stream_event" => self.on_stream_event(msg, out),
            "assistant" => self.on_assistant(msg, out),
            "user" => self.on_user(msg, out),
            "result" => {
                if !self.auth_failed
                    && let Some(u) = self.usage(msg)
                {
                    out.push(SessionUpdate::UsageUpdate(u));
                }
                return Some(self.on_result(msg, cancelled));
            }
            // system (init, status, thinking_tokens, ...), rate_limit_event and
            // anything newer: nothing for the client.
            _ => {}
        }
        None
    }

    fn on_stream_event(&mut self, msg: &Value, out: &mut Vec<SessionUpdate>) {
        let Some(event) = msg.get("event") else {
            return;
        };
        let top = is_top_level(msg);
        match event.get("type").and_then(Value::as_str) {
            Some("message_start") if top => {
                self.current_message = event
                    .pointer("/message/id")
                    .and_then(Value::as_str)
                    .map(str::to_owned);
                self.note_context(&event["message"]);
            }
            Some("content_block_start") => {
                let block = &event["content_block"];
                if block["type"] == "tool_use"
                    && let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                {
                    self.start_tool(id, name, &json!({}), out);
                }
            }
            Some("content_block_delta") if top => {
                let delta = &event["delta"];
                let (text, thought) = match delta["type"].as_str() {
                    Some("text_delta") => (delta["text"].as_str(), false),
                    Some("thinking_delta") => (delta["thinking"].as_str(), true),
                    _ => (None, false),
                };
                let Some(text) = text.filter(|t| !t.is_empty()) else {
                    return;
                };
                if let Some(id) = &self.current_message
                    && !self.streamed.contains(id)
                {
                    self.streamed.insert(id.clone());
                }
                let chunk = ContentChunk::new(ContentBlock::from(text.to_owned()));
                out.push(if thought {
                    SessionUpdate::AgentThoughtChunk(chunk)
                } else {
                    SessionUpdate::AgentMessageChunk(chunk)
                });
            }
            _ => {}
        }
    }

    fn start_tool(&mut self, id: &str, name: &str, input: &Value, out: &mut Vec<SessionUpdate>) {
        let info = tool_info(name, input, &self.cwd);
        self.tools
            .insert(id.to_owned(), (name.to_owned(), info.kind));
        let mut call = ToolCall::new(id.to_owned(), info.title)
            .kind(info.kind)
            .status(ToolCallStatus::Pending)
            .content(info.content)
            .locations(info.locations)
            .raw_input(input.clone())
            .meta(tool_meta(name, None));
        call.name = Some(name.to_owned());
        out.push(SessionUpdate::ToolCall(call));
    }

    fn on_assistant(&mut self, msg: &Value, out: &mut Vec<SessionUpdate>) {
        if msg.get("error").and_then(Value::as_str) == Some("authentication_failed") {
            self.auth_failed = true;
            return;
        }
        let top = is_top_level(msg);
        if top && let Some(m) = msg.get("message") {
            self.note_context(m);
        }
        let message_id = msg.pointer("/message/id").and_then(Value::as_str);
        let streamed = message_id.is_some_and(|id| self.streamed.contains(id));
        let Some(blocks) = msg.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            match block.get("type").and_then(Value::as_str) {
                Some("text") if top && !streamed => {
                    if let Some(t) = block["text"].as_str().filter(|t| !t.is_empty()) {
                        out.push(SessionUpdate::AgentMessageChunk(ContentChunk::new(
                            ContentBlock::from(t.to_owned()),
                        )));
                    }
                }
                Some("thinking") if top && !streamed => {
                    if let Some(t) = block["thinking"].as_str().filter(|t| !t.is_empty()) {
                        out.push(SessionUpdate::AgentThoughtChunk(ContentChunk::new(
                            ContentBlock::from(t.to_owned()),
                        )));
                    }
                }
                Some("tool_use") => {
                    let (Some(id), Some(name)) = (block["id"].as_str(), block["name"].as_str())
                    else {
                        continue;
                    };
                    let input = block.get("input").cloned().unwrap_or_else(|| json!({}));
                    if self.tools.contains_key(id) {
                        let info = tool_info(name, &input, &self.cwd);
                        self.tools
                            .insert(id.to_owned(), (name.to_owned(), info.kind));
                        let fields = ToolCallUpdateFields::new()
                            .title(info.title)
                            .kind(info.kind)
                            .locations((!info.locations.is_empty()).then_some(info.locations))
                            .content(if info.content.is_empty() {
                                None
                            } else {
                                Some(info.content)
                            })
                            .raw_input(input);
                        out.push(SessionUpdate::ToolCallUpdate(
                            ToolCallUpdate::new(id.to_owned(), fields).meta(tool_meta(name, None)),
                        ));
                    } else {
                        self.start_tool(id, name, &input, out);
                    }
                }
                _ => {}
            }
        }
    }

    fn on_user(&mut self, msg: &Value, out: &mut Vec<SessionUpdate>) {
        if msg.get("isReplay").and_then(Value::as_bool) == Some(true) {
            return;
        }
        let Some(blocks) = msg.pointer("/message/content").and_then(Value::as_array) else {
            return;
        };
        for block in blocks {
            if block.get("type").and_then(Value::as_str) != Some("tool_result") {
                continue;
            }
            let Some(id) = block["tool_use_id"].as_str() else {
                continue;
            };
            let kind = self.tools.get(id).map(|(_, k)| *k);
            let fields = tool_result_fields(block, kind);
            let mut update = ToolCallUpdate::new(id.to_owned(), fields);
            if let Some((name, _)) = self.tools.get(id) {
                update = update.meta(tool_meta(name, None));
            }
            out.push(SessionUpdate::ToolCallUpdate(update));
        }
    }

    /// A top-level model call's message: its context size (from `usage`) and
    /// its model.
    fn note_context(&mut self, message: &Value) {
        if let Some(u) = message.get("usage").filter(|u| u.is_object()) {
            let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
            self.context = Some(
                n("input_tokens") + n("cache_read_input_tokens") + n("cache_creation_input_tokens"),
            );
        }
        if let Some(m) = message.get("model").and_then(Value::as_str) {
            self.model = Some(m.to_owned());
        }
    }

    /// The turn's `usage_update` from a `result` message: `None` when it has
    /// no `usage`.
    pub fn usage(&self, msg: &Value) -> Option<UsageUpdate> {
        let u = msg.get("usage").filter(|u| u.is_object())?;
        let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
        let (input, read, write, output) = (
            n("input_tokens"),
            n("cache_read_input_tokens"),
            n("cache_creation_input_tokens"),
            n("output_tokens"),
        );
        let thought = u
            .pointer("/output_tokens_details/thinking_tokens")
            .and_then(Value::as_u64);
        // `modelUsage` is per model and for the whole session: the context
        // window of the turn's model, else the largest one listed.
        let models = msg.get("modelUsage").and_then(Value::as_object);
        let window = |m: &Value| m.get("contextWindow").and_then(Value::as_u64);
        let size = models
            .and_then(|all| {
                self.model
                    .as_deref()
                    .and_then(|name| all.get(name))
                    .and_then(window)
                    .or_else(|| all.values().filter_map(window).max())
            })
            .unwrap_or(0);
        let model = self.model.clone().or_else(|| {
            models
                .filter(|all| all.len() == 1)
                .and_then(|all| all.keys().next().cloned())
        });
        let mut tokens = Map::new();
        tokens.insert("inputTokens".into(), json!(input));
        tokens.insert("cachedReadTokens".into(), json!(read));
        tokens.insert("cachedWriteTokens".into(), json!(write));
        tokens.insert("outputTokens".into(), json!(output));
        if let Some(t) = thought {
            tokens.insert("thoughtTokens".into(), json!(t));
        }
        tokens.insert("totalTokens".into(), json!(input + read + write + output));
        if let Some(m) = model {
            tokens.insert("model".into(), json!(m));
        }
        let mut cc = Map::new();
        cc.insert("usage".into(), Value::Object(tokens));
        let mut meta = Meta::new();
        meta.insert("claudeCode".into(), Value::Object(cc));
        let cost = msg
            .get("total_cost_usd")
            .and_then(Value::as_f64)
            .map(|amount| Cost::new(amount, "USD"));
        Some(
            UsageUpdate::new(self.context.unwrap_or(0), size)
                .cost(cost)
                .meta(meta),
        )
    }

    fn on_result(&mut self, msg: &Value, cancelled: bool) -> TurnEnd {
        if cancelled {
            return TurnEnd::Stop(StopReason::Cancelled);
        }
        if self.auth_failed {
            return TurnEnd::AuthRequired;
        }
        let subtype = msg.get("subtype").and_then(Value::as_str).unwrap_or("");
        match subtype {
            "success" => TurnEnd::Stop(match msg.get("stop_reason").and_then(Value::as_str) {
                Some("max_tokens") => StopReason::MaxTokens,
                Some("refusal") => StopReason::Refusal,
                _ => StopReason::EndTurn,
            }),
            "error_max_turns" => TurnEnd::Stop(StopReason::MaxTurnRequests),
            _ => {
                let detail = msg
                    .get("errors")
                    .and_then(Value::as_array)
                    .map(|e| {
                        e.iter()
                            .filter_map(Value::as_str)
                            .collect::<Vec<_>>()
                            .join("; ")
                    })
                    .filter(|s| !s.is_empty())
                    .or_else(|| msg.get("result").and_then(Value::as_str).map(str::to_owned))
                    .unwrap_or_default();
                TurnEnd::Error(format!("claude ended the turn with {subtype}: {detail}"))
            }
        }
    }
}
