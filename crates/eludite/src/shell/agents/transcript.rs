//! The Agents window's transcript model: a flat list of rows, so the virtualized GPUI `list` re-measures only the rows
//! that changed (brief 0005's finding: agent text is one row per line, and while it streams only the last line
//! changes). Tool calls fold every `tool_call_update`, their permission request, the Eludite MCP call that served them
//! (with its audit entry) and the pending changes they produced into one row.
//!
//! Brief 0027: an agent's debug command (`eludite.debug.*`) reads as the person would see it in the Debug toolbar and
//! the status bar ([`debug_line`]): `Step Over → stopped at Program.cs:42 (breakpoint)`, `Continue → exited (0)`,
//! `Run Until → interrupted by you`, a refusal as `Continue → refused: …`; the stop's location opens the file at the
//! line, and the summary the agent received is folded under it until expanded.
//!
//! Brief 0034: an agent's `usage_update` shows as one line under its turn ([`TurnUsage`]), the turn's totals as
//! `tokens: 260k in (208k cache read, 52k cache write), 2.9k out, $0.95` when the agent gives its tokens (eludite-claude-acp
//! forwards Claude Code's), else the context `53k of 200k tokens in context`; a later update of the same turn replaces it.
//! `toggle_breakpoint`'s compact answer reads `Toggle Breakpoint → added at Program.cs:13`.
//!
//! Brief 0024: a tool call whose result carries images (an `eludite.browser.screenshot` the agent took, or image
//! content in the agent's own tool results) shows them as thumbnails in its row ([`Thumb`], at most
//! [`THUMB_WIDTH`] pixels wide, decoded and scaled off the UI thread by [`decode_thumb`]).

use std::collections::HashMap;
use std::sync::Arc;

use eludite_acp::protocol::{
    PermissionOption, PlanEntry, RequestPermissionRequest, SessionUpdate, ToolCall, ToolCallStatus,
    Usage,
};
use eludite_commands::PermissionClass;
use eludite_ui::transcript::ToolStatus;
use serde_json::{Value, json};

/// What became of a tool call's permission request.
#[derive(Debug, Clone, PartialEq)]
pub enum Permission {
    /// Waiting for the user (request `key`).
    Asked {
        key: u64,
        class: PermissionClass,
    },
    Allowed {
        /// By policy, without asking.
        auto: bool,
        reason: String,
    },
    Denied {
        auto: bool,
        reason: String,
    },
}

/// The Eludite command an MCP tool call ran.
#[derive(Debug, Clone, PartialEq)]
pub struct McpLink {
    pub command: String,
    pub class: PermissionClass,
    pub ok: bool,
    pub ms: f64,
    /// The audit entry.
    pub audit: u64,
    /// An `eludite.debug.*` command, as the Debug toolbar and the status bar would say it (brief 0027).
    pub debug: Option<DebugLine>,
}

/// An agent's debug command in one line (brief 0027): the action and its result, and where it stopped.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugLine {
    pub text: String,
    /// The stop's file and line, which the row opens.
    pub location: Option<(String, u32)>,
}

/// The Debug menu's name for debug command `id` (`Start Without Debugging` for `start` with `debug: false`).
fn debug_action(id: &str, arguments: &Value) -> String {
    use eludite_commands::debug as d;
    let name = match id {
        d::START if arguments.get("debug") == Some(&Value::Bool(false)) => {
            "Start Without Debugging"
        }
        d::START => "Start Debugging",
        d::STOP => "Stop Debugging",
        d::CONTINUE => "Continue",
        d::STEP_OVER => "Step Over",
        d::STEP_INTO => "Step Into",
        d::STEP_OUT => "Step Out",
        d::RUN_TO_CURSOR => "Run To Cursor",
        d::PAUSE => "Break All",
        d::RESTART => "Restart",
        d::ATTACH => "Attach to Process",
        d::RUN_UNTIL => "Run Until",
        d::TRACE => "Trace",
        d::SET_VARIABLE => "Set Value",
        d::SET_NEXT_STATEMENT => "Set Next Statement",
        d::TOGGLE_BREAKPOINT => "Toggle Breakpoint",
        d::EVALUATE => "Evaluate",
        d::SNAPSHOT => "Snapshot",
        d::WAIT => "Wait",
        d::STACK => "Call Stack",
        d::VARIABLES => "Variables",
        d::OUTPUT => "Output",
        d::EXCEPTION_INFO => "Exception Information",
        d::PROCESSES => "Processes",
        d::ALLOW_AGENTS => "Allow Agents to Drive",
        d::STATE => "Debugger State",
        d::SELECT_FRAME => "Switch To Frame",
        d::WATCH => "Watch",
        d::EXCEPTION_SETTINGS => "Exception Settings",
        other => other.rsplit('.').next().unwrap_or(other),
    };
    name.to_owned()
}

/// A stop summary's or state's result as the status bar says it, with where it stopped.
fn summary_result(v: &Value) -> (String, Option<(String, u32)>) {
    if v.get("interrupted_by").and_then(Value::as_str) == Some("user") {
        return ("interrupted by you".into(), None);
    }
    let mode = v.get("mode").and_then(Value::as_str).unwrap_or_default();
    match mode {
        "break" => {
            let stopped = &v["stopped"];
            let reason = stopped["reason"].as_str().unwrap_or("break");
            let at = stopped
                .pointer("/location/path")
                .and_then(Value::as_str)
                .zip(stopped.pointer("/location/line").and_then(Value::as_u64));
            match at {
                Some((path, line)) => {
                    let name = std::path::Path::new(path)
                        .file_name()
                        .map_or(path.to_owned(), |n| n.to_string_lossy().into_owned());
                    (
                        format!("stopped at {name}:{line} ({reason})"),
                        Some((path.to_owned(), line as u32)),
                    )
                }
                None => (format!("stopped ({reason})"), None),
            }
        }
        "design" => match v.get("exit_code").and_then(Value::as_i64) {
            Some(code) => (format!("exited ({code})"), None),
            None => ("ended".into(), None),
        },
        "running" if v.get("timed_out") == Some(&Value::Bool(true)) => {
            ("still running".into(), None)
        }
        "" => ("done".into(), None),
        other => (other.replace('_', " "), None),
    }
}

/// How an agent's call of debug command `command` reads in its row (brief 0027); `None` for other commands.
pub fn debug_line(
    command: &str,
    arguments: &Value,
    outcome: &Result<Value, String>,
) -> Option<DebugLine> {
    use eludite_commands::debug as d;
    if !command.starts_with("eludite.debug.") {
        return None;
    }
    let action = debug_action(command, arguments);
    let (result, location) = match outcome {
        Err(e) => {
            let e = e
                .strip_prefix("command failed: ")
                .unwrap_or(e)
                .lines()
                .next()
                .unwrap_or_default();
            (format!("refused: {e}"), None)
        }
        Ok(v) => match command {
            d::TRACE => {
                let lines = v["lines"].as_array().map_or(0, Vec::len);
                let lines = format!("{lines} line{}", if lines == 1 { "" } else { "s" });
                match v["stopped_by"].as_str().unwrap_or_default() {
                    "interrupted" => ("interrupted by you".into(), None),
                    "terminated" => match v["exit_code"].as_i64() {
                        Some(c) => (format!("{lines}, exited ({c})"), None),
                        None => (format!("{lines}, ended"), None),
                    },
                    "stopped" => {
                        let (s, at) = summary_result(&v["summary"]);
                        (format!("{lines}, {s}"), at)
                    }
                    "hits" => (format!("{lines}, hit count reached"), None),
                    other => (format!("{lines}, {other}"), None),
                }
            }
            d::EVALUATE => match v["state"].as_str() {
                Some("failed") => (
                    format!("failed: {}", v["message"].as_str().unwrap_or_default()),
                    None,
                ),
                _ => (
                    format!(
                        "{} = {}",
                        v["expression"].as_str().unwrap_or_default(),
                        v["result"].as_str().unwrap_or("…")
                    ),
                    None,
                ),
            },
            d::SET_VARIABLE => (
                format!(
                    "{} = {}",
                    v["name"].as_str().unwrap_or_default(),
                    v["value"].as_str().unwrap_or_default()
                ),
                None,
            ),
            d::PROCESSES => (
                format!("{} processes", v["total"].as_u64().unwrap_or_default()),
                None,
            ),
            d::ALLOW_AGENTS => (
                if v["agents_allowed"] == Value::Bool(true) {
                    "agents allowed".into()
                } else {
                    "agents not allowed".into()
                },
                None,
            ),
            // The compact answer (brief 0034): what happened to which breakpoint, which the row opens.
            d::TOGGLE_BREAKPOINT => {
                let action = v["action"].as_str().unwrap_or("done").replace('_', " ");
                let b = &v["breakpoint"];
                match (
                    b["path"].as_str(),
                    b["line"].as_u64(),
                    b["function"].as_str(),
                ) {
                    (Some(path), Some(line), _) => {
                        let name = std::path::Path::new(path)
                            .file_name()
                            .map_or(path.to_owned(), |n| n.to_string_lossy().into_owned());
                        (
                            format!("{action} at {name}:{line}"),
                            Some((path.to_owned(), line as u32)),
                        )
                    }
                    (_, _, Some(function)) => (format!("{action} on {function}"), None),
                    _ => (action, None),
                }
            }
            // The commands that answer the whole state change what the debugger shows, not where it is.
            d::WATCH | d::SELECT_FRAME | d::EXCEPTION_SETTINGS | d::STATE => ("done".into(), None),
            d::STOP => (
                match v["session"]["attached"].as_bool() {
                    _ if v["mode"] == "design" => "ended".into(),
                    Some(true) => "detaching".into(),
                    _ => "stopping".into(),
                },
                None,
            ),
            _ if v.get("mode").is_some() => summary_result(v),
            _ => ("done".into(), None),
        },
    };
    Some(DebugLine {
        text: format!("{action} \u{2192} {result}"),
        location,
    })
}

/// The widest a thumbnail is, in pixels.
pub const THUMB_WIDTH: u32 = 160;

/// An image a tool call returned, as its row shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Thumb {
    /// A hash of the encoded image, so the same image from the agent's content and Eludite's own result shows once.
    pub id: u64,
    /// `image/png`, `image/jpeg`.
    pub mime: String,
    /// The encoded image, for opening it.
    pub bytes: Arc<Vec<u8>>,
    /// The full image's size.
    pub width: u32,
    pub height: u32,
    /// The scaled image, in BGRA, at most [`THUMB_WIDTH`] wide.
    pub render: Arc<gpui::RenderImage>,
}

impl Thumb {
    /// The thumbnail's size in pixels.
    pub fn thumb_size(&self) -> (u32, u32) {
        let s = self.render.size(0);
        (s.width.0 as u32, s.height.0 as u32)
    }
}

/// An image to decode: base64 data and its media type.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImageData {
    pub data: String,
    pub mime: String,
}

/// The image content blocks of a tool call's ACP content (`{"type": "content", "content": {"type": "image", ...}}`).
pub fn content_images(content: &[Value]) -> Vec<ImageData> {
    content
        .iter()
        .filter(|c| c["type"] == "content" && c["content"]["type"] == "image")
        .filter_map(|c| {
            Some(ImageData {
                data: c["content"]["data"].as_str()?.to_owned(),
                mime: c["content"]["mimeType"]
                    .as_str()
                    .unwrap_or("image/png")
                    .to_owned(),
            })
        })
        .collect()
}

/// Decode an image and scale it to at most [`THUMB_WIDTH`] wide. Slow (milliseconds to tens of them): call it off
/// the UI thread. `None` when the data is not a PNG or JPEG image.
pub fn decode_thumb(image: &ImageData) -> Option<Thumb> {
    use std::hash::{Hash, Hasher};
    let bytes = eludite_browser::browser::base64_decode(&image.data)?;
    let decoded = image::load_from_memory(&bytes).ok()?;
    let (width, height) = (decoded.width(), decoded.height());
    let small = if width > THUMB_WIDTH {
        let h =
            ((u64::from(height) * u64::from(THUMB_WIDTH)) / u64::from(width.max(1))).max(1) as u32;
        decoded.thumbnail_exact(THUMB_WIDTH, h)
    } else {
        decoded
    };
    let mut rgba = small.to_rgba8();
    for px in rgba.pixels_mut() {
        px.0.swap(0, 2);
    }
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    image.data.hash(&mut hasher);
    Some(Thumb {
        id: hasher.finish(),
        mime: image.mime.clone(),
        bytes: Arc::new(bytes),
        width,
        height,
        render: Arc::new(gpui::RenderImage::new([image::Frame::new(rgba)])),
    })
}

#[derive(Debug, Clone, PartialEq)]
pub struct ToolRow {
    pub call: ToolCall,
    pub permission: Option<Permission>,
    pub options: Vec<PermissionOption>,
    pub mcp: Option<McpLink>,
    /// Pending changes it proposed: (id, path, state label).
    pub changes: Vec<(u64, String, String)>,
    /// The audit entry of an agent's own tool (Eludite's tools have theirs in `mcp`).
    pub audit: Option<u64>,
    /// Images its result carried, as thumbnails.
    pub images: Vec<Thumb>,
    /// A debug command's result (the summary the agent received) is shown (brief 0027; folded by default).
    pub expanded: bool,
}

impl ToolRow {
    pub fn name(&self) -> String {
        self.call
            .agent_tool_name()
            .map(str::to_owned)
            .or_else(|| self.call.title.clone())
            .unwrap_or_else(|| self.call.tool_call_id.clone())
    }

    /// The status the transcript shows (brief 0016 Contract).
    pub fn status(&self) -> ToolStatus {
        match &self.permission {
            Some(Permission::Asked { .. }) => return ToolStatus::AwaitingPermission,
            Some(Permission::Denied { .. }) => return ToolStatus::Denied,
            _ => {}
        }
        if self.changes.iter().any(|(_, _, s)| s == "pending") {
            return ToolStatus::AwaitingReview;
        }
        match self.call.status {
            Some(ToolCallStatus::Failed) => ToolStatus::Failed,
            Some(ToolCallStatus::Completed) => ToolStatus::Completed,
            Some(ToolCallStatus::InProgress) => ToolStatus::Running,
            None | Some(ToolCallStatus::Pending) => match &self.permission {
                Some(Permission::Allowed { auto: true, .. }) => ToolStatus::AllowedWithoutPrompt,
                Some(Permission::Allowed { .. }) => ToolStatus::Running,
                _ => ToolStatus::Pending,
            },
        }
    }

    /// The note under the card: how permission was decided, the command and audit entry, the changes.
    pub fn note(&self) -> Option<String> {
        let mut parts = Vec::new();
        match &self.permission {
            Some(Permission::Allowed { auto: true, reason }) => {
                parts.push(format!("Allowed without prompt: {reason}"))
            }
            Some(Permission::Allowed {
                auto: false,
                reason,
            }) => parts.push(format!("Allowed by you{}", suffix(reason))),
            Some(Permission::Denied { auto, reason }) => parts.push(format!(
                "Denied{}{}",
                if *auto { " by policy" } else { " by you" },
                suffix(reason)
            )),
            Some(Permission::Asked { class, .. }) => {
                parts.push(format!("Awaiting your answer (class {})", class.as_str()))
            }
            None => {}
        }
        if let Some(seq) = self.audit {
            parts.push(format!("audit #{seq}"));
        }
        if let Some(m) = &self.mcp {
            parts.push(format!(
                "{} ({}) audit #{} {:.1} ms",
                m.command,
                m.class.as_str(),
                m.audit,
                m.ms
            ));
        }
        for (id, path, state) in &self.changes {
            let name = std::path::Path::new(path)
                .file_name()
                .map_or(path.clone(), |n| n.to_string_lossy().into_owned());
            parts.push(format!("Change #{id} {name}: {state}"));
        }
        (!parts.is_empty()).then(|| parts.join(" \u{2022} "))
    }
}

fn suffix(reason: &str) -> String {
    if reason.is_empty() {
        String::new()
    } else {
        format!(": {reason}")
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum Row {
    User(String),
    /// One line of agent text.
    Agent(String),
    Thought {
        text: String,
        expanded: bool,
    },
    Tool(Box<ToolRow>),
    Plan(Vec<PlanEntry>),
    /// The turn's usage (brief 0034).
    Usage(TurnUsage),
    Notice(String),
    Error(String),
}

/// A turn's usage as its line shows it (brief 0034).
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsage {
    pub usage: Usage,
    /// The turn's own cost: the agent's running total less the previous turn's.
    pub cost: Option<(f64, String)>,
}

/// `1234` as `1.2k`, `259826` as `260k`, `1500000` as `1.5M`.
fn tokens(n: u64) -> String {
    let n = n as f64;
    if n < 1e3 {
        format!("{n}")
    } else if n < 1e4 {
        format!("{:.1}k", n / 1e3)
    } else if n < 1e6 {
        format!("{:.0}k", n / 1e3)
    } else {
        format!("{:.1}M", n / 1e6)
    }
}

impl TurnUsage {
    /// The line under the turn.
    pub fn text(&self) -> String {
        let u = &self.usage;
        let mut s = match &u.turn {
            Some(t) => {
                let cache = match (t.cached_read_tokens, t.cached_write_tokens) {
                    (0, 0) => String::new(),
                    (r, w) => format!(" ({} cache read, {} cache write)", tokens(r), tokens(w)),
                };
                format!(
                    "tokens: {} in{cache}, {} out",
                    tokens(t.input_total()),
                    tokens(t.output_tokens)
                )
            }
            None if u.size > 0 => {
                format!("context: {} of {} tokens", tokens(u.used), tokens(u.size))
            }
            None => format!("context: {} tokens", tokens(u.used)),
        };
        if let Some((amount, currency)) = &self.cost {
            if currency == "USD" {
                s.push_str(&format!(", ${amount:.2}"));
            } else {
                s.push_str(&format!(", {amount:.2} {currency}"));
            }
        }
        s
    }

    /// The record's form (`--transcript-out`).
    pub fn to_json(&self) -> Value {
        let u = &self.usage;
        let mut v = json!({"text": self.text(), "used": u.used, "size": u.size});
        if let Some(t) = &u.turn {
            v["input_tokens"] = json!(t.input_tokens);
            v["cached_read_tokens"] = json!(t.cached_read_tokens);
            v["cached_write_tokens"] = json!(t.cached_write_tokens);
            v["input_total"] = json!(t.input_total());
            v["output_tokens"] = json!(t.output_tokens);
            if let Some(n) = t.thought_tokens {
                v["thought_tokens"] = json!(n);
            }
            if let Some(m) = &t.model {
                v["model"] = json!(m);
            }
        }
        if let Some((amount, currency)) = &u.cost {
            v["session_cost"] = json!({"amount": amount, "currency": currency});
        }
        if let Some((amount, currency)) = &self.cost {
            v["cost"] = json!({"amount": amount, "currency": currency});
        }
        v
    }
}

/// An agent's own tool call that ended, to audit.
#[derive(Debug, Clone, PartialEq)]
pub struct EndedTool {
    pub id: String,
    pub name: String,
    pub kind: Option<String>,
    pub input: Option<Value>,
    pub ok: bool,
}

/// Rows `start..old_end` were replaced by rows `start..new_end`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Splice {
    pub start: usize,
    pub old_end: usize,
    pub new_end: usize,
}

#[derive(Debug, Default)]
pub struct Transcript {
    pub rows: Vec<Row>,
    tools: HashMap<String, usize>,
    /// The last row is an agent line still receiving text.
    agent_open: bool,
    thought_open: bool,
    dirty_from: Option<usize>,
    synced_len: usize,
    /// This turn's usage row (brief 0034), replaced by the turn's later updates.
    usage_row: Option<usize>,
    /// The agent's running cost at the end of the previous turn.
    cost_before: Option<f64>,
}

impl Transcript {
    fn mark(&mut self, ix: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
    }

    fn push(&mut self, row: Row) {
        self.agent_open = false;
        self.thought_open = false;
        self.mark(self.rows.len());
        self.rows.push(row);
    }

    /// What changed since the last call, for `ListState::splice`.
    pub fn take_splice(&mut self) -> Option<Splice> {
        let start = self.dirty_from.take()?;
        let s = Splice {
            start,
            old_end: self.synced_len.max(start),
            new_end: self.rows.len(),
        };
        self.synced_len = self.rows.len();
        Some(s)
    }

    pub fn user(&mut self, text: &str) {
        self.end_turn_usage();
        self.push(Row::User(text.to_owned()));
    }

    /// A new turn: the last one's running cost is the base of the next one's.
    fn end_turn_usage(&mut self) {
        if let Some(Row::Usage(u)) = self.usage_row.and_then(|ix| self.rows.get(ix))
            && let Some((amount, _)) = &u.usage.cost
        {
            self.cost_before = Some(*amount);
        }
        self.usage_row = None;
    }

    /// A `usage_update`: the turn's line, made at the first and replaced by the next.
    fn usage(&mut self, usage: Usage) {
        let cost = usage.cost.clone().map(|(amount, currency)| {
            ((amount - self.cost_before.unwrap_or(0.)).max(0.), currency)
        });
        let row = Row::Usage(TurnUsage { usage, cost });
        match self.usage_row.filter(|ix| *ix < self.rows.len()) {
            Some(ix) => {
                self.rows[ix] = row;
                self.mark(ix);
            }
            None => {
                self.usage_row = Some(self.rows.len());
                self.push(row);
            }
        }
    }

    pub fn notice(&mut self, text: impl Into<String>) {
        self.push(Row::Notice(text.into()));
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.push(Row::Error(text.into()));
    }

    fn agent_text(&mut self, text: &str) {
        let mut lines = text.split('\n');
        let first = lines.next().unwrap_or_default();
        if self.agent_open
            && let Some(Row::Agent(last)) = self.rows.last_mut()
        {
            last.push_str(first);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Agent(first.to_owned()));
        }
        for line in lines {
            self.push(Row::Agent(line.to_owned()));
        }
        self.agent_open = true;
    }

    fn thought_text(&mut self, text: &str) {
        if self.thought_open
            && let Some(Row::Thought { text: last, .. }) = self.rows.last_mut()
        {
            last.push_str(text);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Thought {
                text: text.to_owned(),
                expanded: false,
            });
            self.thought_open = true;
        }
    }

    /// Expand or collapse the thinking block at `ix`.
    pub fn toggle_thought(&mut self, ix: usize) {
        if let Some(Row::Thought { expanded, .. }) = self.rows.get_mut(ix) {
            *expanded = !*expanded;
            self.mark(ix);
        }
    }

    fn tool_index(&mut self, call: &ToolCall) -> usize {
        if let Some(&ix) = self.tools.get(&call.tool_call_id) {
            return ix;
        }
        let ix = self.rows.len();
        self.tools.insert(call.tool_call_id.clone(), ix);
        self.push(Row::Tool(Box::new(ToolRow {
            call: call.clone(),
            permission: None,
            options: Vec::new(),
            mcp: None,
            changes: Vec::new(),
            audit: None,
            images: Vec::new(),
            expanded: false,
        })));
        ix
    }

    /// Show or fold the result of the debug command in row `ix` (brief 0027).
    pub fn toggle_result(&mut self, ix: usize) {
        if let Some(row) = self.tool_mut(ix) {
            row.expanded = !row.expanded;
        }
    }

    /// The row of `call`, made if it was not announced.
    pub fn ensure_tool(&mut self, call: &ToolCall) {
        let ix = self.tool_index(call);
        if let Some(row) = self.tool_mut(ix)
            && row.call.raw_input.is_none()
        {
            row.call.raw_input = call.raw_input.clone();
        }
    }

    fn tool_mut(&mut self, ix: usize) -> Option<&mut ToolRow> {
        self.mark(ix);
        match self.rows.get_mut(ix) {
            Some(Row::Tool(t)) => Some(t),
            _ => None,
        }
    }

    pub fn apply(&mut self, update: &SessionUpdate) {
        match update {
            SessionUpdate::AgentMessageChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.agent_text(t);
                }
            }
            SessionUpdate::AgentThoughtChunk(c) => {
                if let Some(t) = c.as_text() {
                    self.thought_text(t);
                }
            }
            SessionUpdate::UserMessageChunk(_) => {}
            SessionUpdate::ToolCall(t) | SessionUpdate::ToolCallUpdate(t) => {
                let known = self.tools.contains_key(&t.tool_call_id);
                if !known && matches!(update, SessionUpdate::ToolCallUpdate(_)) {
                    return;
                }
                let ix = self.tool_index(t);
                if known && let Some(row) = self.tool_mut(ix) {
                    row.call.apply(t.clone());
                }
            }
            other => {
                if let Some(usage) = other.usage() {
                    self.usage(usage);
                } else if let Some(entries) = other.plan_entries() {
                    // A plan replaces the previous one when it is the last row.
                    if let Some(Row::Plan(last)) = self.rows.last_mut() {
                        *last = entries;
                        self.mark(self.rows.len() - 1);
                    } else {
                        self.push(Row::Plan(entries));
                    }
                }
            }
        }
    }

    /// A permission request: attach it to its tool call's row (made if the call was not announced).
    pub fn permission(&mut self, req: &RequestPermissionRequest, state: Permission) {
        let ix = self.tool_index(&req.tool_call);
        let options = req.options.clone();
        if let Some(row) = self.tool_mut(ix) {
            if row.call.raw_input.is_none() {
                row.call.raw_input = req.tool_call.raw_input.clone();
            }
            row.permission = Some(state);
            row.options = options;
        }
    }

    /// The answer to request `key`.
    pub fn answer(&mut self, key: u64, state: Permission) -> bool {
        let ix = self.rows.iter().position(|r| {
            matches!(r, Row::Tool(t) if matches!(t.permission, Some(Permission::Asked { key: k, .. }) if k == key))
        });
        match ix.and_then(|ix| self.tool_mut(ix)) {
            Some(row) => {
                row.permission = Some(state);
                true
            }
            None => false,
        }
    }

    /// The tool call row an Eludite MCP call served: the one named in the call's `_meta`, else the newest call of
    /// `mcp__eludite__<tool>` not linked yet.
    pub fn link_mcp(
        &mut self,
        tool_call: Option<&str>,
        tool: &str,
        link: McpLink,
    ) -> Option<String> {
        let full = format!("mcp__{}__{tool}", super::endpoint::MCP_SERVER_NAME);
        let ix = match tool_call.and_then(|id| self.tools.get(id).copied()) {
            Some(ix) => Some(ix),
            None => self.rows.iter().rposition(|r| {
                matches!(r, Row::Tool(t) if t.mcp.is_none() && t.call.agent_tool_name() == Some(full.as_str()))
            }),
        }?;
        let row = self.tool_mut(ix)?;
        row.mcp = Some(link);
        Some(row.call.tool_call_id.clone())
    }

    /// The MCP gate asks about an Eludite command: attach the request to the call's row (named in the call's `_meta`,
    /// else the newest unanswered call of `mcp__eludite__<tool>`), or add a row when the agent did not announce it.
    pub fn ask_mcp(
        &mut self,
        tool_call: Option<&str>,
        tool: &str,
        input: &Value,
        state: Permission,
    ) {
        let full = format!("mcp__{}__{tool}", super::endpoint::MCP_SERVER_NAME);
        let ix = tool_call
            .and_then(|id| self.tools.get(id).copied())
            .or_else(|| {
                self.rows.iter().rposition(|r| {
                    matches!(r, Row::Tool(t) if t.mcp.is_none()
                        && !matches!(t.permission, Some(Permission::Asked { .. }))
                        && t.call.agent_tool_name() == Some(full.as_str()))
                })
            });
        let ix = match ix {
            Some(ix) => ix,
            None => {
                let key = match state {
                    Permission::Asked { key, .. } => key,
                    _ => 0,
                };
                self.tool_index(&ToolCall {
                    tool_call_id: tool_call
                        .map_or_else(|| format!("eludite-ask-{key}"), str::to_owned),
                    title: Some(full.clone()),
                    kind: Some("other".into()),
                    raw_input: Some(input.clone()),
                    meta: Some(json!({"claudeCode": {"toolName": full}})),
                    ..Default::default()
                })
            }
        };
        if let Some(row) = self.tool_mut(ix) {
            row.permission = Some(state);
        }
    }

    /// The agent's own tool calls that ended (completed or failed) and have no audit entry yet.
    pub fn unaudited(&self) -> Vec<EndedTool> {
        self.tools()
            .filter(|t| t.audit.is_none() && t.mcp.is_none())
            .filter(|t| {
                !t.name()
                    .starts_with(&format!("mcp__{}__", super::endpoint::MCP_SERVER_NAME))
            })
            .filter_map(|t| {
                let ok = match t.call.status? {
                    ToolCallStatus::Completed => true,
                    ToolCallStatus::Failed => false,
                    _ => return None,
                };
                Some(EndedTool {
                    id: t.call.tool_call_id.clone(),
                    name: t.name(),
                    kind: t.call.kind.clone(),
                    input: t.call.raw_input.clone(),
                    ok,
                })
            })
            .collect()
    }

    /// Thumbnails for tool call `id` (one per image; an image the row has already is skipped).
    pub fn add_thumbs(&mut self, id: &str, thumbs: Vec<Thumb>) -> bool {
        let Some(&ix) = self.tools.get(id) else {
            return false;
        };
        let Some(row) = self.tool_mut(ix) else {
            return false;
        };
        for t in thumbs {
            if !row.images.iter().any(|i| i.id == t.id) {
                row.images.push(t);
            }
        }
        true
    }

    /// Link tool call `id` to its audit entry.
    pub fn set_audit(&mut self, id: &str, seq: u64) {
        if let Some(&ix) = self.tools.get(id)
            && let Some(row) = self.tool_mut(ix)
        {
            row.audit = Some(seq);
        }
    }

    /// Record a pending change (or its new state) on its tool call's row.
    pub fn change(&mut self, tool_call: &str, id: u64, path: &str, state: &str) {
        let Some(&ix) = self.tools.get(tool_call) else {
            return;
        };
        if let Some(row) = self.tool_mut(ix) {
            match row.changes.iter_mut().find(|c| c.0 == id) {
                Some(c) => c.2 = state.to_owned(),
                None => row.changes.push((id, path.to_owned(), state.to_owned())),
            }
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn tool(&self, tool_call_id: &str) -> Option<&ToolRow> {
        match self.rows.get(*self.tools.get(tool_call_id)?) {
            Some(Row::Tool(t)) => Some(t),
            _ => None,
        }
    }

    pub fn tools(&self) -> impl Iterator<Item = &ToolRow> {
        self.rows.iter().filter_map(|r| match r {
            Row::Tool(t) => Some(&**t),
            _ => None,
        })
    }

    /// The keys of permission requests waiting for an answer, oldest first.
    pub fn asked(&self) -> Vec<u64> {
        self.tools()
            .filter_map(|t| match t.permission {
                Some(Permission::Asked { key, .. }) => Some(key),
                _ => None,
            })
            .collect()
    }

    /// All agent text, lines joined with `\n`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agent_message(&self) -> String {
        let mut out = String::new();
        let mut prev_agent = false;
        for r in &self.rows {
            if let Row::Agent(t) = r {
                if prev_agent {
                    out.push('\n');
                }
                out.push_str(t);
                prev_agent = true;
            } else {
                if prev_agent {
                    out.push('\n');
                }
                prev_agent = false;
            }
        }
        out
    }

    /// The transcript as JSON (the manual run's record).
    pub fn to_json(&self) -> Value {
        let mut out = Vec::new();
        let mut agent = String::new();
        let flush = |agent: &mut String, out: &mut Vec<Value>| {
            if !agent.is_empty() {
                out.push(json!({"agent": std::mem::take(agent)}));
            }
        };
        for r in &self.rows {
            if let Row::Agent(t) = r {
                if !agent.is_empty() {
                    agent.push('\n');
                }
                agent.push_str(t);
                continue;
            }
            flush(&mut agent, &mut out);
            out.push(match r {
                Row::User(t) => json!({"user": t}),
                Row::Thought { text, .. } => json!({"thought": text}),
                Row::Tool(t) => {
                    let mut call = json!({
                        "id": t.call.tool_call_id, "tool": t.name(), "kind": t.call.kind,
                        "status": t.status().label(), "arguments": t.call.raw_input,
                        "result": t.call.content_text(), "note": t.note()
                    });
                    if let Some(d) = t.mcp.as_ref().and_then(|m| m.debug.as_ref()) {
                        call["debug"] = json!(d.text);
                        if let Some((path, line)) = &d.location {
                            call["debug_location"] = json!({"path": path, "line": line});
                        }
                    }
                    if !t.images.is_empty() {
                        call["images"] = t
                            .images
                            .iter()
                            .map(|i| {
                                let (w, h) = i.thumb_size();
                                json!({"mime": i.mime, "width": i.width, "height": i.height,
                                    "thumb_width": w, "thumb_height": h})
                            })
                            .collect();
                    }
                    json!({ "tool_call": call })
                }
                Row::Plan(entries) => json!({"plan": entries}),
                Row::Usage(u) => json!({"usage": u.to_json()}),
                Row::Notice(t) => json!({"notice": t}),
                Row::Error(t) => json!({"error": t}),
                Row::Agent(_) => unreachable!(),
            });
        }
        flush(&mut agent, &mut out);
        Value::Array(out)
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use eludite_acp::protocol::ContentBlock;

    fn call(id: &str, tool: &str) -> ToolCall {
        ToolCall {
            tool_call_id: id.into(),
            title: Some(tool.into()),
            status: Some(ToolCallStatus::Pending),
            meta: Some(json!({"claudeCode": {"toolName": tool}})),
            ..Default::default()
        }
    }

    #[test]
    fn agent_lines_split_and_only_the_tail_is_dirty() {
        let mut t = Transcript::default();
        t.user("hi");
        t.take_splice();
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Hello ",
        )));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "world\nsecond",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 1,
                old_end: 1,
                new_end: 3
            })
        );
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            " line",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 2,
                old_end: 3,
                new_end: 3
            })
        );
        assert_eq!(t.agent_message(), "Hello world\nsecond line");
    }

    /// A `w` by `h` PNG, base64.
    pub(crate) fn png(w: u32, h: u32) -> String {
        let img = image::RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        eludite_browser::browser::base64_encode(out.get_ref())
    }

    #[test]
    fn images_become_thumbnails_once_per_row() {
        let data = png(640, 400);
        let content = vec![
            json!({"type": "content", "content": {"type": "text", "text": "{}"}}),
            json!({"type": "content", "content": {"type": "image", "data": data, "mimeType": "image/png"}}),
        ];
        let images = content_images(&content);
        assert_eq!(images.len(), 1);
        let thumb = decode_thumb(&images[0]).unwrap();
        assert_eq!((thumb.width, thumb.height), (640, 400));
        assert_eq!(thumb.thumb_size(), (THUMB_WIDTH, 100));
        // BGRA: the red and blue channels are swapped.
        assert_eq!(&thumb.render.as_bytes(0).unwrap()[..4], &[30, 20, 10, 255]);
        let small = decode_thumb(&ImageData {
            data: png(20, 10),
            mime: "image/png".into(),
        })
        .unwrap();
        assert_eq!(
            small.thumb_size(),
            (20, 10),
            "small images are not scaled up"
        );
        assert!(
            decode_thumb(&ImageData {
                data: "bm90IGFuIGltYWdl".into(),
                mime: "image/png".into()
            })
            .is_none()
        );

        let mut t = Transcript::default();
        t.apply(&SessionUpdate::ToolCall(call(
            "s",
            "mcp__eludite__eludite-browser-screenshot",
        )));
        assert!(t.add_thumbs("s", vec![thumb.clone()]));
        assert!(
            t.add_thumbs("s", vec![thumb.clone()]),
            "the same image again"
        );
        assert!(!t.add_thumbs("nope", vec![thumb]));
        assert_eq!(t.tool("s").unwrap().images.len(), 1);
        let json = t.to_json();
        assert_eq!(
            json[0]["tool_call"]["images"][0]["thumb_width"],
            THUMB_WIDTH
        );
    }

    #[test]
    fn thinking_is_one_collapsed_block_and_toggles() {
        let mut t = Transcript::default();
        for piece in ["Let me ", "think."] {
            t.apply(&SessionUpdate::AgentThoughtChunk(ContentBlock::text(piece)));
        }
        assert_eq!(
            t.rows,
            [Row::Thought {
                text: "Let me think.".into(),
                expanded: false
            }]
        );
        t.toggle_thought(0);
        assert!(matches!(t.rows[0], Row::Thought { expanded: true, .. }));
    }

    #[test]
    fn tool_rows_fold_updates_permissions_mcp_calls_and_changes() {
        let mut t = Transcript::default();
        let read = call("a", "mcp__eludite__diagnostics-list");
        t.apply(&SessionUpdate::ToolCall(read.clone()));
        let req = RequestPermissionRequest {
            session_id: "s".into(),
            tool_call: read.clone(),
            options: Vec::new(),
        };
        t.permission(
            &req,
            Permission::Allowed {
                auto: true,
                reason: "diagnostics.list is class read".into(),
            },
        );
        assert_eq!(
            t.tool("a").unwrap().status(),
            ToolStatus::AllowedWithoutPrompt
        );
        let linked = t.link_mcp(
            None,
            "diagnostics-list",
            McpLink {
                command: "diagnostics.list".into(),
                class: PermissionClass::Read,
                ok: true,
                ms: 0.1,
                audit: 3,
                debug: None,
            },
        );
        assert_eq!(linked.as_deref(), Some("a"));
        t.apply(&SessionUpdate::ToolCallUpdate(ToolCall {
            tool_call_id: "a".into(),
            status: Some(ToolCallStatus::Completed),
            ..Default::default()
        }));
        let row = t.tool("a").unwrap();
        assert_eq!(row.status(), ToolStatus::Completed);
        let note = row.note().unwrap();
        assert!(
            note.contains("Allowed without prompt: diagnostics.list is class read"),
            "{note}"
        );
        assert!(note.contains("audit #3"), "{note}");

        // An unannounced call asking permission gets a row; the answer finds it by key.
        let shell = call("b", "Bash");
        t.permission(
            &RequestPermissionRequest {
                session_id: "s".into(),
                tool_call: shell,
                options: Vec::new(),
            },
            Permission::Asked {
                key: 9,
                class: PermissionClass::Execute,
            },
        );
        assert_eq!(t.asked(), [9]);
        assert_eq!(
            t.tool("b").unwrap().status(),
            ToolStatus::AwaitingPermission
        );
        assert!(t.answer(
            9,
            Permission::Denied {
                auto: false,
                reason: String::new()
            }
        ));
        assert_eq!(t.tool("b").unwrap().status(), ToolStatus::Denied);
        assert!(t.asked().is_empty());

        // An edit tool awaiting review, then accepted.
        t.apply(&SessionUpdate::ToolCall(call(
            "c",
            "mcp__eludite__eludite-workspace-apply_edit",
        )));
        t.change("c", 1, "/s/A.cs", "pending");
        assert_eq!(t.tool("c").unwrap().status(), ToolStatus::AwaitingReview);
        t.change("c", 1, "/s/A.cs", "accepted");
        assert!(
            t.tool("c")
                .unwrap()
                .note()
                .unwrap()
                .contains("Change #1 A.cs: accepted")
        );
        // Updates for unknown calls are ignored; plans replace the previous plan at the tail.
        t.apply(&SessionUpdate::ToolCallUpdate(call("zzz", "x")));
        assert!(t.tool("zzz").is_none());
        let plan = |s: &str| SessionUpdate::Other {
            kind: "plan".into(),
            raw: json!({"sessionUpdate": "plan", "entries": [{"content": "x", "priority": "high", "status": s}]}),
        };
        t.apply(&plan("pending"));
        t.apply(&plan("completed"));
        assert!(matches!(t.rows.last(), Some(Row::Plan(e)) if e[0].status == "completed"));
        assert_eq!(
            t.rows.iter().filter(|r| matches!(r, Row::Plan(_))).count(),
            1
        );
        let json = t.to_json();
        assert_eq!(json[0]["tool_call"]["status"], "completed");
    }

    /// Brief 0027: agents' debug commands read as the Debug toolbar and the status bar would say them.
    #[test]
    fn debug_commands_read_as_the_debug_toolbar_would() {
        let stop = json!({"mode": "break", "generation": 1, "stop": 3,
            "stopped": {"reason": "breakpoint", "thread": 1,
                        "location": {"path": "/s/src/App/Program.cs", "line": 42, "function": "Main"}}});
        let l = debug_line("eludite.debug.step_over", &json!({}), &Ok(stop.clone())).unwrap();
        assert_eq!(
            l.text,
            "Step Over \u{2192} stopped at Program.cs:42 (breakpoint)"
        );
        assert_eq!(l.location, Some(("/s/src/App/Program.cs".into(), 42)));
        let exited = json!({"mode": "design", "generation": 1, "stop": 3, "exit_code": 0});
        assert_eq!(
            debug_line("eludite.debug.continue", &json!({}), &Ok(exited))
                .unwrap()
                .text,
            "Continue \u{2192} exited (0)"
        );
        let cut = json!({"mode": "running", "generation": 1, "stop": 3, "interrupted_by": "user"});
        let l = debug_line("eludite.debug.run_until", &json!({}), &Ok(cut)).unwrap();
        assert_eq!(l.text, "Run Until \u{2192} interrupted by you");
        assert_eq!(l.location, None);
        let refused = debug_line(
            "eludite.debug.continue",
            &json!({}),
            &Err("command failed: agents are not allowed to drive this session (Debug > Allow Agents to Drive)".into()),
        )
        .unwrap();
        assert_eq!(
            refused.text,
            "Continue \u{2192} refused: agents are not allowed to drive this session (Debug > Allow Agents to Drive)"
        );
        assert_eq!(
            debug_line(
                "eludite.debug.start",
                &json!({"debug": false}),
                &Ok(json!({"mode": "running_without_debugging"}))
            )
            .unwrap()
            .text,
            "Start Without Debugging \u{2192} running without debugging"
        );
        assert_eq!(
            debug_line(
                "eludite.debug.trace",
                &json!({}),
                &Ok(json!({"lines": [{"text": "a"}, {"text": "b"}], "stopped_by": "terminated", "exit_code": 3}))
            )
            .unwrap()
            .text,
            "Trace \u{2192} 2 lines, exited (3)"
        );
        assert_eq!(
            debug_line(
                "eludite.debug.trace",
                &json!({}),
                &Ok(json!({"lines": [], "stopped_by": "interrupted"}))
            )
            .unwrap()
            .text,
            "Trace \u{2192} interrupted by you"
        );
        assert_eq!(
            debug_line(
                "eludite.debug.evaluate",
                &json!({}),
                &Ok(json!({"expression": "a + b", "state": "done", "result": "3"}))
            )
            .unwrap()
            .text,
            "Evaluate \u{2192} a + b = 3"
        );
        assert_eq!(
            debug_line(
                "eludite.debug.processes",
                &json!({}),
                &Ok(json!({"total": 7}))
            )
            .unwrap()
            .text,
            "Processes \u{2192} 7 processes"
        );
        assert!(debug_line("eludite.browser.navigate", &json!({}), &Ok(json!({}))).is_none());
        // The compact toggle_breakpoint answer (brief 0034).
        let l = debug_line(
            "eludite.debug.toggle_breakpoint",
            &json!({}),
            &Ok(json!({"action": "added", "verified": false, "breakpoints_total": 1,
                "breakpoint": {"kind": "line", "path": "/s/OffByOne/Program.cs", "line": 13, "enabled": true, "verified": false, "hits": 0}})),
        )
        .unwrap();
        assert_eq!(l.text, "Toggle Breakpoint \u{2192} added at Program.cs:13");
        assert_eq!(l.location, Some(("/s/OffByOne/Program.cs".into(), 13)));
        assert_eq!(
            debug_line(
                "eludite.debug.toggle_breakpoint",
                &json!({}),
                &Ok(json!({"action": "deleted_all", "verified": false, "breakpoints_total": 0}))
            )
            .unwrap()
            .text,
            "Toggle Breakpoint \u{2192} deleted all"
        );
    }

    fn usage_update(v: Value) -> SessionUpdate {
        serde_json::from_value(v).unwrap()
    }

    #[test]
    fn a_usage_update_is_one_line_under_its_turn() {
        let mut t = Transcript::default();
        t.user("debug it");
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Done.",
        )));
        t.apply(&usage_update(eludite_acp::fake_agent::stream_usage()));
        let text = |t: &Transcript| match t.rows.last() {
            Some(Row::Usage(u)) => u.text(),
            other => panic!("{other:?}"),
        };
        assert_eq!(
            text(&t),
            "tokens: 260k in (208k cache read, 52k cache write), 2.9k out, $0.95"
        );
        let rows = t.rows.len();
        // A later update of the same turn replaces the line.
        let mut later = eludite_acp::fake_agent::stream_usage();
        later["_meta"]["claudeCode"]["usage"]["outputTokens"] = json!(3_100);
        t.apply(&usage_update(later));
        assert_eq!(t.rows.len(), rows);
        assert!(text(&t).contains("3.1k out"), "{}", text(&t));
        let record = t.to_json();
        let usage = &record.as_array().unwrap().last().unwrap()["usage"];
        assert_eq!(usage["input_total"], 162 + 207_671 + 51_993);
        assert_eq!(usage["cached_read_tokens"], 207_671);
        assert_eq!(usage["output_tokens"], 3_100);
        assert_eq!(usage["model"], "claude-fable-5-1");
        assert_eq!(usage["cost"]["amount"], 0.9512);
        // The next turn's cost is the running total's growth; an agent without token counts shows its context.
        t.user("again");
        t.apply(&usage_update(
            json!({"sessionUpdate": "usage_update", "used": 53_000, "size": 200_000,
            "cost": {"amount": 1.2012, "currency": "USD"}}),
        ));
        assert_eq!(text(&t), "context: 53k of 200k tokens, $0.25");
        assert_eq!(
            t.rows.iter().filter(|r| matches!(r, Row::Usage(_))).count(),
            2
        );
    }
}
