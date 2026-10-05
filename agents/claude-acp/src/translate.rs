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
//!
//! [`SessionOptions`] (brief 0057) holds a session's modes and config options:
//! the two modes the adapter offers (`default`, shown as "Manual", and
//! `plan`), the `model` option from the `initialize` reply's `models`, and the
//! `effort` option from the current model's `supportedEffortLevels`. It is
//! pure too; the agent sends the control requests and local commands that
//! change them. A `result` whose `local_command` is `model` or `effort` (the
//! person typed `/model X` or `/effort X`) moves the option to X when X is
//! one of its values ([`SessionOptions::on_local_command`]); the model of a
//! turn's usage corrects the model option ([`SessionOptions::on_turn_model`]).

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use agent_client_protocol::schema::v1::{
    AvailableCommand, AvailableCommandInput, AvailableCommandsUpdate, ConfigOptionUpdate,
    ContentBlock, ContentChunk, Cost, CurrentModeUpdate, Meta, SessionConfigOption,
    SessionConfigOptionCategory, SessionConfigSelectOption, SessionMode, SessionModeState,
    SessionUpdate, StopReason, ToolCall, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields,
    ToolKind, UnstructuredCommandInput, UsageUpdate,
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

/// The modes the adapter offers: (id, name, description). `claude`'s
/// `acceptEdits`, `auto` and `bypassPermissions` are not offered: they stop it
/// from raising `can_use_tool`, which is how the client reviews edits and
/// applies its own policy.
pub const MODES: [(&str, &str, &str); 2] = [
    (
        "default",
        "Manual",
        "Eludite reviews each edit and prompts as the policy says",
    ),
    ("plan", "Plan", "Plans before making changes"),
];

/// The `model` config option's id.
pub const MODEL_OPTION: &str = "model";
/// The `effort` config option's id.
pub const EFFORT_OPTION: &str = "effort";
/// The value of either option that leaves the choice to `claude`.
pub const DEFAULT_VALUE: &str = "default";

/// A session's modes and config options (see the module docs).
#[derive(Debug, Clone, PartialEq)]
pub struct SessionOptions {
    /// `claude`'s `models` from its `initialize` reply, as it lists them.
    models: Vec<Value>,
    pub mode: String,
    /// The model option's value (`default` when the session chose none).
    pub model: String,
    /// The effort option's value (`default` when the session chose none).
    pub effort: String,
}

/// `low` -> `Low`.
fn capitalized(s: &str) -> String {
    let mut c = s.chars();
    c.next()
        .map(|f| f.to_uppercase().chain(c).collect())
        .unwrap_or_default()
}

impl SessionOptions {
    /// The options of a session launched with `model` and `effort` (`None`:
    /// `claude`'s own). A model given as a resolved name (`--model
    /// claude-opus-5-5`) is shown as the entry that resolves to it.
    pub fn new(models: Vec<Value>, model: Option<&str>, effort: Option<&str>) -> Self {
        let mut o = Self {
            models,
            mode: MODES[0].0.into(),
            model: DEFAULT_VALUE.into(),
            effort: effort.unwrap_or(DEFAULT_VALUE).into(),
        };
        if let Some(m) = model {
            o.model = o
                .entry(m)
                .or_else(|| {
                    o.models
                        .iter()
                        .find(|e| e.get("resolvedModel").and_then(Value::as_str) == Some(m))
                })
                .and_then(|e| e.get("value").and_then(Value::as_str))
                .unwrap_or(m)
                .to_owned();
        }
        o
    }

    fn entry(&self, value: &str) -> Option<&Value> {
        self.models
            .iter()
            .find(|e| e.get("value").and_then(Value::as_str) == Some(value))
    }

    /// The current model's effort levels: empty when it does not support
    /// effort (or is not in the list).
    pub fn effort_levels(&self) -> Vec<String> {
        self.entry(&self.model)
            .filter(|e| e.get("supportsEffort").and_then(Value::as_bool) == Some(true))
            .and_then(|e| e.get("supportedEffortLevels").and_then(Value::as_array))
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect()
    }

    pub fn has_mode(&self, id: &str) -> bool {
        MODES.iter().any(|(m, _, _)| *m == id)
    }

    pub fn has_model(&self, value: &str) -> bool {
        self.entry(value).is_some()
    }

    /// ACP's `modes`.
    pub fn modes(&self) -> SessionModeState {
        SessionModeState::new(
            self.mode.clone(),
            MODES
                .iter()
                .map(|(id, name, d)| SessionMode::new(*id, *name).description(d.to_string()))
                .collect(),
        )
    }

    /// ACP's `configOptions`: `model` when `claude` listed models, `effort`
    /// when the current model supports effort.
    pub fn config_options(&self) -> Vec<SessionConfigOption> {
        let mut out = Vec::new();
        if !self.models.is_empty() {
            let choices: Vec<SessionConfigSelectOption> = self
                .models
                .iter()
                .filter_map(|e| {
                    let value = e.get("value").and_then(Value::as_str)?;
                    let name = e
                        .get("displayName")
                        .and_then(Value::as_str)
                        .unwrap_or(value);
                    Some(
                        SessionConfigSelectOption::new(value.to_owned(), name).description(
                            e.get("description")
                                .and_then(Value::as_str)
                                .map(str::to_owned),
                        ),
                    )
                })
                .collect();
            out.push(
                SessionConfigOption::select(MODEL_OPTION, "Model", self.model.clone(), choices)
                    .description("The model Claude Code uses".to_owned())
                    .category(SessionConfigOptionCategory::Model),
            );
        }
        let levels = self.effort_levels();
        if !levels.is_empty() {
            let choices: Vec<SessionConfigSelectOption> = std::iter::once(
                SessionConfigSelectOption::new(DEFAULT_VALUE, "Default")
                    .description("The model decides".to_owned()),
            )
            .chain(
                levels
                    .iter()
                    .map(|l| SessionConfigSelectOption::new(l.clone(), capitalized(l))),
            )
            .collect();
            out.push(
                SessionConfigOption::select(EFFORT_OPTION, "Effort", self.effort.clone(), choices)
                    .description("How much the model thinks before it answers".to_owned())
                    .category(SessionConfigOptionCategory::ThoughtLevel),
            );
        }
        out
    }

    /// The model is now `value` (one of the list): the effort stays when the
    /// new model supports it, else it is `default`.
    pub fn set_model(&mut self, value: &str) {
        self.model = value.to_owned();
        if !self.effort_levels().contains(&self.effort) {
            self.effort = DEFAULT_VALUE.into();
        }
    }

    /// Whether `value` is an effort `/effort` can set for the current model
    /// (`default` is not: `claude` has no way back to the model's own effort
    /// in a session).
    pub fn can_set_effort(&self, value: &str) -> bool {
        value != DEFAULT_VALUE && self.effort_levels().iter().any(|l| l == value)
    }

    /// A local command typed as a prompt (`/model X`, `/effort X`) ended with
    /// a `result` whose `local_command` is `command`: move the option to X
    /// when it is one of its values. Returns whether `command` is `model` or
    /// `effort` (the client is told either way).
    pub fn on_local_command(&mut self, command: &str, prompt: &str) -> bool {
        let arg = prompt
            .trim()
            .strip_prefix('/')
            .and_then(|p| p.strip_prefix(command))
            .filter(|rest| rest.is_empty() || rest.starts_with(char::is_whitespace))
            .map(str::trim)
            .unwrap_or("");
        match command {
            MODEL_OPTION => {
                if self.has_model(arg) {
                    self.set_model(arg);
                }
                true
            }
            EFFORT_OPTION => {
                if self.can_set_effort(arg) {
                    self.effort = arg.to_owned();
                }
                true
            }
            _ => false,
        }
    }

    /// A turn ran on `resolved` (its usage's model): when the current model
    /// option resolves to another model, move it to the first entry that
    /// resolves to this one. Returns whether it moved.
    pub fn on_turn_model(&mut self, resolved: &str) -> bool {
        let resolves = |e: &Value| e.get("resolvedModel").and_then(Value::as_str) == Some(resolved);
        if self.entry(&self.model).is_some_and(resolves) {
            return false;
        }
        let Some(value) = self
            .models
            .iter()
            .find(|e| resolves(e))
            .and_then(|e| e.get("value").and_then(Value::as_str))
            .map(str::to_owned)
        else {
            return false;
        };
        self.set_model(&value);
        true
    }

    /// The `config_option_update` with every option as it is now.
    pub fn update(&self) -> SessionUpdate {
        SessionUpdate::ConfigOptionUpdate(ConfigOptionUpdate::new(self.config_options()))
    }

    /// The `current_mode_update` for the current mode.
    pub fn mode_update(&self) -> SessionUpdate {
        SessionUpdate::CurrentModeUpdate(CurrentModeUpdate::new(self.mode.clone()))
    }
}

/// The local command a `result` message reports (`local_command`: `effort`,
/// `model`, ...), when it is one.
pub fn local_command(msg: &Value) -> Option<&str> {
    (msg.get("type").and_then(Value::as_str) == Some("result"))
        .then(|| msg.get("local_command").and_then(Value::as_str))
        .flatten()
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

    /// The model of the turn's last top-level model call, when one was made.
    pub fn model(&self) -> Option<&str> {
        self.model.as_deref()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Three of `claude` 2.1.289's `models` (tests/fixtures/claude-2.1.289-options.jsonl), and one without effort.
    fn models() -> Vec<Value> {
        let levels = json!(["low", "medium", "high", "xhigh", "max"]);
        vec![
            json!({"value": "default", "resolvedModel": "claude-fable-5-1", "displayName": "Default (recommended)",
                   "description": "Fable 5.1", "supportsEffort": true, "supportedEffortLevels": levels}),
            json!({"value": "opus", "resolvedModel": "claude-opus-5-5", "displayName": "Opus 5.5",
                   "description": "For complex work and everyday tasks", "supportsEffort": true,
                   "supportedEffortLevels": levels}),
            json!({"value": "claude-opus-4-6", "resolvedModel": "claude-opus-4-6", "displayName": "Opus 4.6",
                   "supportsEffort": true, "supportedEffortLevels": ["low", "medium", "high", "max"]}),
            json!({"value": "haiku", "resolvedModel": "claude-haiku-4-5-20251001", "displayName": "Haiku 4.5",
                   "description": "Fastest for quick answers"}),
        ]
    }

    fn option(o: &SessionOptions, id: &str) -> Option<Value> {
        o.config_options()
            .into_iter()
            .map(|c| serde_json::to_value(c).unwrap())
            .find(|c| c["id"] == id)
    }

    fn values(option: &Value) -> Vec<String> {
        option["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["value"].as_str().unwrap().to_owned())
            .collect()
    }

    #[test]
    fn options_list_the_models_and_the_current_models_effort_levels() {
        let o = SessionOptions::new(models(), None, None);
        let modes = serde_json::to_value(o.modes()).unwrap();
        assert_eq!(
            modes,
            json!({"currentModeId": "default", "availableModes": [
                {"id": "default", "name": "Manual", "description": "Eludite reviews each edit and prompts as the policy says"},
                {"id": "plan", "name": "Plan", "description": "Plans before making changes"}
            ]})
        );
        let model = option(&o, "model").unwrap();
        assert_eq!(model["category"], "model");
        assert_eq!(model["type"], "select");
        assert_eq!(model["currentValue"], "default");
        assert_eq!(
            model["options"][1],
            json!({"value": "opus", "name": "Opus 5.5", "description": "For complex work and everyday tasks"})
        );
        assert!(model["options"][2].get("description").is_none());
        let effort = option(&o, "effort").unwrap();
        assert_eq!(effort["category"], "thought_level");
        assert_eq!(effort["currentValue"], "default");
        assert_eq!(
            values(&effort),
            ["default", "low", "medium", "high", "xhigh", "max"]
        );
        let names: Vec<&str> = effort["options"]
            .as_array()
            .unwrap()
            .iter()
            .map(|c| c["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Default", "Low", "Medium", "High", "Xhigh", "Max"]);
        // No models: no options at all (an older `claude`, or a recording redacted to none).
        assert!(
            SessionOptions::new(Vec::new(), Some("opus"), None)
                .config_options()
                .is_empty()
        );
    }

    #[test]
    fn the_launch_model_and_effort_are_current_and_a_model_change_keeps_a_supported_effort() {
        // A resolved name is shown as its entry; the launch effort is current.
        let mut o = SessionOptions::new(models(), Some("claude-opus-5-5"), Some("xhigh"));
        assert_eq!((o.model.as_str(), o.effort.as_str()), ("opus", "xhigh"));
        assert_eq!(option(&o, "effort").unwrap()["currentValue"], "xhigh");
        // Opus 4.6 has no xhigh: the effort goes back to default, and its list is its own.
        o.set_model("claude-opus-4-6");
        assert_eq!(o.effort, "default");
        assert_eq!(
            values(&option(&o, "effort").unwrap()),
            ["default", "low", "medium", "high", "max"]
        );
        o.effort = "high".into();
        o.set_model("opus");
        assert_eq!(o.effort, "high", "a supported effort stays");
        // A model without effort has no effort option.
        o.set_model("haiku");
        assert!(option(&o, "effort").is_none());
        assert_eq!(o.effort, "default");
        assert!(!o.can_set_effort("high"));
        // `default` is never set by `/effort`.
        o.set_model("opus");
        assert!(
            o.can_set_effort("max") && !o.can_set_effort("default") && !o.can_set_effort("huge")
        );
        // A model not in the list is kept as given, without effort.
        let other = SessionOptions::new(models(), Some("my-model"), None);
        assert_eq!(other.model, "my-model");
        assert!(option(&other, "effort").is_none());
    }

    #[test]
    fn local_commands_and_the_turns_model_move_the_options() {
        let mut o = SessionOptions::new(models(), None, None);
        assert!(o.on_local_command("model", "/model opus"));
        assert_eq!(o.model, "opus");
        assert!(o.on_local_command("effort", "/effort  high "));
        assert_eq!(o.effort, "high");
        // Not one of the values: left as it was, but still a model or effort command.
        assert!(o.on_local_command("model", "/model"));
        assert!(o.on_local_command("effort", "/effort auto"));
        assert_eq!((o.model.as_str(), o.effort.as_str()), ("opus", "high"));
        assert!(!o.on_local_command("compact", "/compact"));
        // `/modelx opus` is another command.
        assert!(o.on_local_command("model", "/modelx haiku"));
        assert_eq!(o.model, "opus");
        // The turn's model: unchanged when the option resolves to it, else the first entry that does.
        assert!(!o.on_turn_model("claude-opus-5-5"));
        assert!(o.on_turn_model("claude-fable-5-1"));
        assert_eq!(o.model, "default");
        assert!(!o.on_turn_model("<synthetic>"));
        assert_eq!(o.model, "default");
        // The local command's `result`.
        assert_eq!(
            local_command(&json!({"type": "result", "local_command": "effort"})),
            Some("effort")
        );
        assert_eq!(local_command(&json!({"type": "result"})), None);
        assert_eq!(
            local_command(&json!({"type": "assistant", "local_command": "effort"})),
            None
        );
    }
}
