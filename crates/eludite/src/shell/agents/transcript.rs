//! The Agents window's transcript model: a flat list of rows, so the virtualized GPUI `list` re-measures only the rows
//! that changed (brief 0005's finding). Agent text is Markdown, one row per top-level block ([`AgentText`]): while it
//! streams only the last block changes and is parsed again; a block is complete once the next one starts. Tool calls fold every `tool_call_update`, their permission request, the Eludite MCP call that served them
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
//! Brief 0057: the agent's slash commands ([`Transcript::commands`]) are kept from its latest
//! `available_commands_update`, a new list replacing the old one in full; they make no row (the prompt box's slash
//! menu offers them, and `eludite.agents.*`'s state output lists them).
//!
//! Brief 0059: the person's prompts carry the local time they were sent; a thought records when its first and last
//! chunks came, so its collapsed line reads `Thinking…` while it streams and `Thought for 4 s` after
//! ([`ThoughtRow::label`]); a tool call's row is collapsed by default ([`ToolRow::expanded`]) and named by the
//! adapter's title ([`ToolRow::name`], the tool's own name in [`ToolRow::tool_name`]); the session's latest usage is
//! kept for the usage strip ([`Transcript::usage`]) until a new session starts ([`Transcript::new_session`]).
//!
//! Brief 0060: [`Transcript::from_json`] rebuilds the rows from the record [`Transcript::to_json`] writes (a session
//! kept in the per-workspace store): the prompts with their time (the record now carries it, and a thought its length
//! in seconds), the agent's Markdown, the thoughts collapsed, the tool calls as completed, denied or failed with their
//! arguments, result and note, the change links read from the note (`Change #3 Program.cs: accepted`), the plans,
//! the usage lines and the notices. A restored tool row keeps its record ([`ToolRow::restored`]), so the record of a
//! rebuilt transcript is the record it came from, and it is never audited again. [`Transcript::replay`] builds rows
//! from what an agent replays on `session/load` when the shell has no record.
//!
//! Brief 0024: a tool call whose result carries images (an `eludite.browser.screenshot` the agent took, or image
//! content in the agent's own tool results) shows them as thumbnails in its row ([`Thumb`], at most
//! [`THUMB_WIDTH`] pixels wide, decoded and scaled off the UI thread by [`decode_thumb`]).

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_acp::protocol::{
    AvailableCommand, PermissionOption, PlanEntry, RequestPermissionRequest, SessionUpdate,
    ToolCall, ToolCallStatus, Usage,
};
use eludite_commands::PermissionClass;
use eludite_ui::markdown::{self, Block};
use eludite_ui::transcript::{ToolStatus, money, tokens};
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

/// The last `n` non-empty lines of `text`, each at most 120 characters: the excerpt a terminal row shows.
fn excerpt(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines[lines.len().saturating_sub(n)..]
        .iter()
        .map(|l| {
            if l.chars().count() > 120 {
                format!("{}\u{2026}", l.chars().take(120).collect::<String>())
            } else {
                (*l).to_owned()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// An agent's terminal command in its row (brief 0041): `send` shows the text typed, `wait` what ended it and the
/// last lines that came (a text thumbnail of the screen), `read` the same excerpt.
fn terminal_line(command: &str, arguments: &Value, outcome: &Result<Value, String>) -> DebugLine {
    use eludite_commands::terminal as t;
    let v = match outcome {
        Err(e) => {
            let e = e
                .strip_prefix("command failed: ")
                .unwrap_or(e)
                .lines()
                .next()
                .unwrap_or_default();
            let name = command.rsplit('.').next().unwrap_or(command);
            return DebugLine {
                text: format!("Terminal {name} refused: {e}"),
                location: None,
            };
        }
        Ok(v) => v,
    };
    let term = v["terminal"].as_str().unwrap_or("terminal");
    let text = match command {
        t::SEND => {
            let typed = arguments["text"].as_str().unwrap_or_default();
            let enter = if arguments.get("newline") == Some(&Value::Bool(false)) {
                ""
            } else {
                " \u{21B5}"
            };
            format!("Typed `{}`{enter} in {term}", excerpt(typed, 3))
        }
        t::WAIT => {
            let matched = v["matched"].as_str().unwrap_or_default();
            let what = match (matched, v["exit_code"].as_i64()) {
                ("prompt", Some(c)) => format!("prompt (exit {c})"),
                ("interrupted", _) => "interrupted by the person".to_owned(),
                (m, _) => m.to_owned(),
            };
            let body = excerpt(v["text"].as_str().unwrap_or_default(), 5);
            format!(
                "Waited {} ms in {term}: {what}{}{body}",
                v["elapsed_ms"].as_u64().unwrap_or(0),
                if body.is_empty() { "" } else { "\n" }
            )
        }
        t::READ => {
            let body = excerpt(v["text"].as_str().unwrap_or_default(), 5);
            format!(
                "Read {term}{}{body}",
                if body.is_empty() { "" } else { "\n" }
            )
        }
        t::OPEN => format!(
            "Opened {} ({term}) in {}",
            v["name"].as_str().unwrap_or_default(),
            v["cwd"].as_str().unwrap_or_default()
        ),
        t::CLOSE => format!("Closed {term}"),
        t::CLEAR => format!("Cleared {term}"),
        t::RESIZE => format!(
            "Resized {term} to {}x{}",
            v["cols"].as_u64().unwrap_or(0),
            v["rows"].as_u64().unwrap_or(0)
        ),
        _ => "Listed the terminals".to_owned(),
    };
    DebugLine {
        text,
        location: None,
    }
}

/// How an agent's call of debug command `command` reads in its row (brief 0027); `None` for other commands.
pub fn debug_line(
    command: &str,
    arguments: &Value,
    outcome: &Result<Value, String>,
) -> Option<DebugLine> {
    use eludite_commands::debug as d;
    // An agent's terminal command (brief 0041): what it typed, and after a wait an excerpt of what came.
    if command.starts_with("eludite.terminal.") {
        return Some(terminal_line(command, arguments, outcome));
    }
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
    /// The card shows its arguments and result (brief 0059; collapsed by default). A debug command's row (brief 0027)
    /// shows the summary the agent received under its line by the same state, through its "Show snapshot" link.
    pub expanded: bool,
    /// The record this row was rebuilt from (brief 0060): its status, note and debug line come from it, and so does
    /// its record. `None` for a row of a live session.
    pub restored: Option<Box<Value>>,
}

impl ToolRow {
    /// What the card's line says (brief 0059): the title the adapter gave (`ls`, `Read Program.cs`), else the tool's
    /// name.
    pub fn name(&self) -> String {
        self.call
            .title
            .clone()
            .filter(|t| !t.trim().is_empty())
            .unwrap_or_else(|| self.tool_name())
    }

    /// The tool's own name (`Bash`, `mcp__eludite__diagnostics-list`), else its title: what the audit records and the
    /// card's tooltip says.
    pub fn tool_name(&self) -> String {
        self.call
            .agent_tool_name()
            .map(str::to_owned)
            .or_else(|| self.call.title.clone())
            .unwrap_or_else(|| self.call.tool_call_id.clone())
    }

    /// The status the transcript shows (brief 0016 Contract). A restored row (brief 0060) is denied, failed or
    /// completed, as its record says.
    pub fn status(&self) -> ToolStatus {
        if let Some(r) = &self.restored {
            return match r["status"].as_str() {
                Some("denied") => ToolStatus::Denied,
                Some("failed") => ToolStatus::Failed,
                _ => ToolStatus::Completed,
            };
        }
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
        if let Some(r) = &self.restored {
            return r["note"].as_str().map(str::to_owned);
        }
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

impl ToolRow {
    /// An Eludite debug command's line (brief 0027): from the MCP call that served it, or from a restored row's
    /// record.
    pub fn debug_line(&self) -> Option<DebugLine> {
        if let Some(d) = self.mcp.as_ref().and_then(|m| m.debug.clone()) {
            return Some(d);
        }
        let r = self.restored.as_ref()?;
        Some(DebugLine {
            text: r["debug"].as_str()?.to_owned(),
            location: r["debug_location"]["path"].as_str().map(|p| {
                (
                    p.to_owned(),
                    r["debug_location"]["line"].as_u64().unwrap_or(1) as u32,
                )
            }),
        })
    }
}

/// The changes a note names (`Change #3 Program.cs: accepted`), as a row's `changes`: (id, file name, state).
fn changes_in_note(note: &str) -> Vec<(u64, String, String)> {
    note.split(" \u{2022} ")
        .filter_map(|part| {
            let rest = part.strip_prefix("Change #")?;
            let (id, rest) = rest.split_once(' ')?;
            let (name, state) = rest.rsplit_once(": ")?;
            Some((id.parse().ok()?, name.to_owned(), state.to_owned()))
        })
        .collect()
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
    /// The person's prompt and the local time it was sent (`14:02`).
    User {
        text: String,
        time: String,
    },
    /// One top-level Markdown block of agent text.
    Agent(AgentText),
    Thought(ThoughtRow),
    Tool(Box<ToolRow>),
    Plan(Vec<PlanEntry>),
    /// The turn's usage (brief 0034).
    Usage(TurnUsage),
    Notice(String),
    Error(String),
}

/// The agent's thinking (brief 0059): its text, whether it is shown, and when its first and last chunks came.
#[derive(Debug, Clone, PartialEq)]
pub struct ThoughtRow {
    pub text: String,
    pub expanded: bool,
    pub first: Instant,
    pub last: Instant,
    /// Another row followed it or the turn ended.
    pub done: bool,
}

impl ThoughtRow {
    /// How long the agent thought, from its first chunk to its last.
    pub fn duration(&self) -> Duration {
        self.last.duration_since(self.first)
    }

    /// The collapsed line: `Thinking…` while it streams, `Thought for 4 s` after (at least 1 s).
    pub fn label(&self) -> String {
        if self.done {
            format!(
                "Thought for {} s",
                self.duration().as_secs_f64().round().max(1.) as u64
            )
        } else {
            "Thinking\u{2026}".into()
        }
    }
}

/// The local time of day as `14:02`. The UTC offset comes from libgit2 (`git_signature_now` reads the system's
/// time zone), which the shell already links through `eludite-git`; without it the time is UTC.
pub fn local_time() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs() as i64);
    let offset = eludite_git::git2::Signature::now("eludite", "eludite@localhost")
        .map_or(0, |s| s.when().offset_minutes());
    let minutes = (now + i64::from(offset) * 60).rem_euclid(86_400) / 60;
    format!("{:02}:{:02}", minutes / 60, minutes % 60)
}

/// One top-level block of the agent's Markdown: its source, exactly as streamed (with the blank lines after it), and
/// the blocks parsed from it.
#[derive(Debug, Clone, PartialEq)]
pub struct AgentText {
    pub source: String,
    pub blocks: Vec<Block>,
}

impl AgentText {
    fn new(source: &str) -> Self {
        Self {
            source: source.to_owned(),
            blocks: markdown::parse(source),
        }
    }
}

/// A turn's usage as its line shows it (brief 0034).
#[derive(Debug, Clone, PartialEq)]
pub struct TurnUsage {
    pub usage: Usage,
    /// The turn's own cost: the agent's running total less the previous turn's.
    pub cost: Option<(f64, String)>,
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
            s.push_str(", ");
            s.push_str(&money(*amount, currency));
        }
        s
    }

    /// The line as the transcript draws it under its turn (brief 0059): `260k in · 2.9k out · $0.17`, else the
    /// context as [`TurnUsage::text`] says it.
    pub fn short(&self) -> String {
        let u = &self.usage;
        let mut parts = match &u.turn {
            Some(t) => vec![
                format!("{} in", tokens(t.input_total())),
                format!("{} out", tokens(t.output_tokens)),
            ],
            None if u.size > 0 => vec![format!("{} of {} tokens", tokens(u.used), tokens(u.size))],
            None => vec![format!("{} tokens", tokens(u.used))],
        };
        if let Some((amount, currency)) = &self.cost {
            parts.push(money(*amount, currency));
        }
        parts.join(" \u{B7} ")
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
    /// The last row is an agent block still receiving text.
    agent_open: bool,
    thought_open: bool,
    dirty_from: Option<usize>,
    synced_len: usize,
    /// This turn's usage row (brief 0034), replaced by the turn's later updates.
    usage_row: Option<usize>,
    /// The agent's running cost at the end of the previous turn.
    cost_before: Option<f64>,
    /// The agent's slash commands, from its latest `available_commands_update` (brief 0057).
    pub commands: Vec<AvailableCommand>,
    /// The session's latest usage, for the usage strip (brief 0059).
    pub usage: Option<TurnUsage>,
    /// The thought row still receiving text, if any.
    thought_row: Option<usize>,
    /// The last row is a prompt an agent's replay is still sending (brief 0060).
    user_open: bool,
}

impl Transcript {
    fn mark(&mut self, ix: usize) {
        self.dirty_from = Some(self.dirty_from.map_or(ix, |d| d.min(ix)));
    }

    fn push(&mut self, row: Row) {
        self.user_open = false;
        self.close_agent();
        self.close_thought();
        self.mark(self.rows.len());
        self.rows.push(row);
    }

    /// The open thought ended: its line says how long it took.
    fn close_thought(&mut self) {
        self.thought_open = false;
        if let Some(ix) = self.thought_row.take()
            && let Some(Row::Thought(t)) = self.rows.get_mut(ix)
        {
            t.done = true;
            self.mark(ix);
        }
    }

    /// The turn ended: the open agent block and thought are complete.
    pub fn end_turn(&mut self) {
        self.close_agent();
        self.close_thought();
    }

    /// A new agent session starts (Start, Restart): its slash commands and usage are its own.
    pub fn new_session(&mut self) {
        self.end_turn();
        self.commands.clear();
        self.usage = None;
        self.usage_row = None;
        self.cost_before = None;
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
        self.push(Row::User {
            text: text.to_owned(),
            time: local_time(),
        });
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
        let turn = TurnUsage { usage, cost };
        self.usage = Some(turn.clone());
        let row = Row::Usage(turn);
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

    /// Agent text: appended to the open block, which is split at the blocks that start in it on complete lines.
    /// Every piece but the last is complete; the last stays open.
    fn agent_text(&mut self, text: &str) {
        let open = match self.rows.last() {
            Some(Row::Agent(last)) if self.agent_open => Some(last.source.clone()),
            _ => None,
        };
        let source = match &open {
            Some(last) => format!("{last}{text}"),
            None => text.to_owned(),
        };
        // Only complete lines decide where blocks start; the partial line stays in the open block.
        let complete = source.rfind('\n').map_or(0, |at| at + 1);
        self.split_agent(&source, &source[..complete], open.is_some());
        self.agent_open = true;
    }

    /// The agent's text ended (another row follows): its open block is split at every block start, the last line
    /// included.
    fn close_agent(&mut self) {
        if !std::mem::take(&mut self.agent_open) {
            return;
        }
        if let Some(Row::Agent(last)) = self.rows.last() {
            let source = last.source.clone();
            self.split_agent(&source, &source, true);
        }
    }

    /// Put `source` into rows, one per block starting in `decided`, a prefix of it; the first replaces the last row
    /// when `replace`.
    fn split_agent(&mut self, source: &str, decided: &str, replace: bool) {
        self.agent_open = false;
        let mut cuts: Vec<usize> = markdown::block_starts(decided)
            .into_iter()
            .filter(|&at| at > 0)
            .collect();
        cuts.push(source.len());
        let mut from = 0;
        for (i, to) in cuts.into_iter().enumerate() {
            let piece = AgentText::new(&source[from..to]);
            if i == 0 && replace {
                let ix = self.rows.len() - 1;
                if self.rows[ix] != Row::Agent(piece.clone()) {
                    self.rows[ix] = Row::Agent(piece);
                    self.mark(ix);
                }
            } else {
                self.push(Row::Agent(piece));
            }
            from = to;
        }
    }

    fn thought_text(&mut self, text: &str) {
        let now = Instant::now();
        if self.thought_open
            && let Some(Row::Thought(last)) = self.rows.last_mut()
        {
            last.text.push_str(text);
            last.last = now;
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::Thought(ThoughtRow {
                text: text.to_owned(),
                expanded: false,
                first: now,
                last: now,
                done: false,
            }));
            self.thought_open = true;
            self.thought_row = Some(self.rows.len() - 1);
        }
    }

    /// Expand or collapse the thinking block at `ix`.
    pub fn toggle_thought(&mut self, ix: usize) {
        if let Some(Row::Thought(t)) = self.rows.get_mut(ix) {
            t.expanded = !t.expanded;
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
            restored: None,
        })));
        ix
    }

    /// Expand or collapse the tool call card at `ix` (brief 0059); for a debug command's row, show or fold the
    /// summary the agent received (brief 0027).
    pub fn toggle_tool(&mut self, ix: usize) {
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
                    // A live update makes a restored row live again (brief 0060).
                    row.restored = None;
                    row.call.apply(t.clone());
                }
            }
            other => {
                if let Some(usage) = other.usage() {
                    self.usage(usage);
                } else if let Some(commands) = other.available_commands() {
                    self.commands = commands;
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
            .filter(|t| t.audit.is_none() && t.mcp.is_none() && t.restored.is_none())
            .filter(|t| {
                !t.tool_name()
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
                    name: t.tool_name(),
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

    /// All agent text as streamed; agent messages separated by `\n`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn agent_message(&self) -> String {
        let mut out = String::new();
        let mut prev_agent = false;
        for r in &self.rows {
            if let Row::Agent(t) = r {
                out.push_str(&t.source);
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
                agent.push_str(&t.source);
                continue;
            }
            flush(&mut agent, &mut out);
            out.push(match r {
                Row::User { text, time } => {
                    let mut map = serde_json::Map::new();
                    map.insert("user".into(), Value::String(text.clone()));
                    map.insert("time".into(), Value::String(time.clone()));
                    Value::Object(map)
                }
                Row::Thought(t) => {
                    json!({"thought": t.text, "seconds": t.duration().as_secs_f64().round() as u64})
                }
                Row::Tool(t) if t.restored.is_some() => {
                    let mut call = (**t.restored.as_ref().expect("restored")).clone();
                    call["expanded"] = json!(t.expanded);
                    json!({ "tool_call": call })
                }
                Row::Tool(t) => {
                    // Built member by member: `json!` would serialize the arguments' value into a new one (brief
                    // 0060 writes the record every 2 s while a session changes).
                    let opt = |v: Option<String>| v.map_or(Value::Null, Value::String);
                    let mut map = serde_json::Map::new();
                    map.insert("id".into(), Value::String(t.call.tool_call_id.clone()));
                    map.insert("tool".into(), Value::String(t.tool_name()));
                    map.insert("title".into(), Value::String(t.name()));
                    map.insert("kind".into(), opt(t.call.kind.clone()));
                    map.insert("status".into(), Value::from(t.status().label()));
                    map.insert(
                        "arguments".into(),
                        t.call.raw_input.clone().unwrap_or(Value::Null),
                    );
                    map.insert("result".into(), Value::String(t.call.content_text()));
                    map.insert("note".into(), opt(t.note()));
                    map.insert("expanded".into(), Value::Bool(t.expanded));
                    let mut call = Value::Object(map);
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

    /// The transcript [`Transcript::to_json`] recorded (brief 0060): every row rebuilt, the agent's text split into its
    /// Markdown blocks again, the thoughts collapsed, the tool rows with their record ([`ToolRow::restored`]), the last
    /// usage line the usage strip's. Entries it does not know are skipped. Its record is `record`.
    pub fn from_json(record: &Value) -> Transcript {
        let mut t = Transcript::default();
        let now = Instant::now();
        for entry in record.as_array().into_iter().flatten() {
            let str_of = |k: &str| entry.get(k).and_then(Value::as_str);
            if let Some(text) = str_of("user") {
                t.end_turn_usage();
                t.push(Row::User {
                    text: text.to_owned(),
                    time: str_of("time").unwrap_or_default().to_owned(),
                });
            } else if let Some(text) = str_of("agent") {
                t.agent_text(text);
                t.close_agent();
            } else if let Some(text) = str_of("thought") {
                let secs = entry.get("seconds").and_then(Value::as_u64).unwrap_or(0);
                let first = now.checked_sub(Duration::from_secs(secs)).unwrap_or(now);
                t.push(Row::Thought(ThoughtRow {
                    text: text.to_owned(),
                    expanded: false,
                    first,
                    last: first + Duration::from_secs(secs),
                    done: true,
                }));
            } else if let Some(call) = entry.get("tool_call").filter(|c| c.is_object()) {
                let id = call["id"].as_str().unwrap_or_default().to_owned();
                let content = call["result"].as_str().filter(|r| !r.is_empty()).map(|r| {
                    vec![json!({"type": "content", "content": {"type": "text", "text": r}})]
                });
                let tool = call["tool"].as_str().unwrap_or_default();
                let row = ToolRow {
                    call: ToolCall {
                        tool_call_id: id.clone(),
                        title: call["title"].as_str().map(str::to_owned),
                        kind: call["kind"].as_str().map(str::to_owned),
                        status: Some(match call["status"].as_str() {
                            Some("failed") => ToolCallStatus::Failed,
                            _ => ToolCallStatus::Completed,
                        }),
                        content,
                        raw_input: Some(call["arguments"].clone()).filter(|a| !a.is_null()),
                        raw_output: None,
                        meta: Some(json!({"claudeCode": {"toolName": tool}})),
                    },
                    permission: None,
                    options: Vec::new(),
                    mcp: None,
                    changes: call["note"]
                        .as_str()
                        .map(changes_in_note)
                        .unwrap_or_default(),
                    audit: None,
                    images: Vec::new(),
                    expanded: call["expanded"].as_bool().unwrap_or(false),
                    restored: Some(Box::new(call.clone())),
                };
                t.tools.insert(id, t.rows.len());
                t.push(Row::Tool(Box::new(row)));
            } else if let Some(entries) = entry.get("plan") {
                if let Ok(entries) = serde_json::from_value::<Vec<PlanEntry>>(entries.clone()) {
                    t.push(Row::Plan(entries));
                }
            } else if let Some(u) = entry.get("usage").filter(|u| u.is_object()) {
                let num = |k: &str| u.get(k).and_then(Value::as_u64);
                let money = |v: &Value| {
                    Some((
                        v.get("amount")?.as_f64()?,
                        v.get("currency")?.as_str()?.to_owned(),
                    ))
                };
                let turn =
                    num("input_tokens").map(|input_tokens| eludite_acp::protocol::TurnTokens {
                        input_tokens,
                        cached_read_tokens: num("cached_read_tokens").unwrap_or(0),
                        cached_write_tokens: num("cached_write_tokens").unwrap_or(0),
                        output_tokens: num("output_tokens").unwrap_or(0),
                        thought_tokens: num("thought_tokens"),
                        model: u.get("model").and_then(Value::as_str).map(str::to_owned),
                    });
                let turn = TurnUsage {
                    usage: Usage {
                        used: num("used").unwrap_or(0),
                        size: num("size").unwrap_or(0),
                        cost: u.get("session_cost").and_then(money),
                        turn,
                    },
                    cost: u.get("cost").and_then(money),
                };
                t.usage = Some(turn.clone());
                t.cost_before = turn.usage.cost.as_ref().map(|(amount, _)| *amount);
                t.push(Row::Usage(turn));
            } else if let Some(text) = str_of("notice") {
                t.notice(text);
            } else if let Some(text) = str_of("error") {
                t.error(text);
            }
        }
        t.end_turn();
        t
    }

    /// What an agent replays on `session/load` (brief 0060), when the shell has no record of the session: its prompts
    /// as prompt rows (no time), everything else as [`Transcript::apply`] shows it.
    pub fn replay(&mut self, update: &SessionUpdate) {
        let SessionUpdate::UserMessageChunk(c) = update else {
            return self.apply(update);
        };
        let Some(text) = c.as_text() else {
            return;
        };
        if self.user_open
            && let Some(Row::User { text: last, .. }) = self.rows.last_mut()
        {
            last.push_str(text);
            self.mark(self.rows.len() - 1);
        } else {
            self.push(Row::User {
                text: text.to_owned(),
                time: String::new(),
            });
            self.user_open = true;
        }
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
    fn agent_blocks_split_and_only_the_tail_is_dirty() {
        let mut t = Transcript::default();
        t.user("hi");
        t.take_splice();
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Hello ",
        )));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "world\n\nsecond\n",
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
            "line",
        )));
        assert_eq!(
            t.take_splice(),
            Some(Splice {
                start: 2,
                old_end: 3,
                new_end: 3
            })
        );
        assert_eq!(t.agent_message(), "Hello world\n\nsecond\nline");
        // A partial line waits for its line break to start a block; the agent's text ending splits it.
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "\n\nthird",
        )));
        assert_eq!(t.rows.len(), 3);
        t.notice("Turn ended");
        assert_eq!(t.rows.len(), 5);
        assert_eq!(t.agent_message(), "Hello world\n\nsecond\nline\n\nthird\n");
    }

    #[test]
    fn agent_markdown_streams_into_blocks() {
        let mut t = Transcript::default();
        let md = "## Plan\nI will:\n\n1. read `a.rs`\n2. fix it\n\n```rust\nfn a() {}\n\nfn b() {}\n```\n| a | b |\n|---|---|\n| 1 | 2 |\n| 3 | 4 |\n\nDone.";
        // Streamed a byte at a time: every split of lines, fences and table rows (a lone `|` ends a table).
        for chunk in md.as_bytes().chunks(1) {
            t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
                std::str::from_utf8(chunk).unwrap(),
            )));
        }
        assert_eq!(t.agent_message(), md);
        t.notice("Turn ended");
        let blocks: Vec<&Block> = t
            .rows
            .iter()
            .flat_map(|r| match r {
                Row::Agent(a) => a.blocks.iter().collect::<Vec<_>>(),
                _ => Vec::new(),
            })
            .collect();
        // The same blocks as parsing the whole message at once, one row each.
        assert_eq!(blocks, markdown::parse(md).iter().collect::<Vec<_>>());
        assert_eq!(t.rows.len(), 7);
        assert!(matches!(blocks[0], Block::Heading { level: 2, .. }));
        assert!(matches!(blocks[2], Block::List { start: Some(1), .. }));
        assert_eq!(*blocks[3], Block::Code("fn a() {}\n\nfn b() {}".into()));
        assert!(matches!(blocks[4], Block::Table { rows, .. } if rows.len() == 2));
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
        assert_eq!(t.rows.len(), 1);
        let Row::Thought(row) = &t.rows[0] else {
            panic!("{:?}", t.rows[0]);
        };
        assert_eq!(row.text, "Let me think.");
        assert!(!row.expanded);
        // Brief 0059: `Thinking…` while it streams.
        assert_eq!(row.label(), "Thinking\u{2026}");
        t.toggle_thought(0);
        assert!(matches!(&t.rows[0], Row::Thought(r) if r.expanded));
        // The agent's text after it ends the thought: its line says how long it took (at least a second).
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Done.",
        )));
        let Row::Thought(row) = &t.rows[0] else {
            panic!()
        };
        assert!(row.done);
        assert_eq!(row.label(), "Thought for 1 s");
        let four = ThoughtRow {
            last: row.first + Duration::from_millis(4_400),
            ..row.clone()
        };
        assert_eq!(four.label(), "Thought for 4 s");
        // A thought that is the turn's last row ends with the turn.
        t.apply(&SessionUpdate::AgentThoughtChunk(ContentBlock::text(
            "More.",
        )));
        assert!(matches!(t.rows.last(), Some(Row::Thought(r)) if !r.done));
        t.end_turn();
        assert!(matches!(t.rows.last(), Some(Row::Thought(r)) if r.done));
    }

    #[test]
    fn prompts_carry_their_time_and_cards_their_title() {
        let mut t = Transcript::default();
        t.user("hi");
        let Row::User { text, time } = &t.rows[0] else {
            panic!()
        };
        assert_eq!(text, "hi");
        assert_eq!(time.len(), 5, "{time}");
        assert_eq!(&time[2..3], ":");
        assert_eq!(t.to_json()[0], json!({"user": "hi", "time": time}));
        // The card says the adapter's title; the audit and the record's `tool` keep the tool's own name.
        let mut ls = call("l", "Bash");
        ls.title = Some("ls".into());
        ls.kind = Some("execute".into());
        t.apply(&SessionUpdate::ToolCall(ls));
        let row = t.tool("l").unwrap();
        assert_eq!(row.name(), "ls");
        assert_eq!(row.tool_name(), "Bash");
        assert!(!row.expanded);
        let ix = t.rows.len() - 1;
        t.toggle_tool(ix);
        let record = t.to_json();
        let card = &record.as_array().unwrap().last().unwrap()["tool_call"];
        assert_eq!(card["tool"], "Bash");
        assert_eq!(card["title"], "ls");
        assert_eq!(card["expanded"], true);
        // A call without a title is named by its tool.
        let mut bare = call("b", "Read");
        bare.title = None;
        t.apply(&SessionUpdate::ToolCall(bare));
        assert_eq!(t.tool("b").unwrap().name(), "Read");
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
        // Brief 0059: the strip reads the session's latest update; the row draws the short form.
        let latest = t.usage.clone().unwrap();
        assert_eq!(latest.usage.used, 53_000);
        assert_eq!(latest.usage.cost, Some((1.2012, "USD".into())));
        assert_eq!(latest.short(), "53k of 200k tokens \u{B7} $0.25");
        match &t.rows[t.rows.len() - 1] {
            Row::Usage(u) => assert_eq!(u.short(), "53k of 200k tokens \u{B7} $0.25"),
            other => panic!("{other:?}"),
        }
        // A new session starts with no usage and its own running cost.
        t.new_session();
        assert!(t.usage.is_none());
        t.apply(&usage_update(eludite_acp::fake_agent::stream_usage()));
        assert_eq!(
            t.usage.as_ref().unwrap().short(),
            "260k in \u{B7} 2.9k out \u{B7} $0.95"
        );
    }

    /// Brief 0041: an agent's terminal commands read as what it typed and what came back.
    #[test]
    fn terminal_commands_read_as_typing_and_an_excerpt() {
        let l = debug_line(
            "eludite.terminal.send",
            &json!({"text": "dotnet build"}),
            &Ok(json!({"terminal": "term1", "mark": 0, "bytes": 13})),
        )
        .unwrap();
        assert_eq!(l.text, "Typed `dotnet build` \u{21B5} in term1");
        let out = (1..=8)
            .map(|i| format!("line {i}"))
            .collect::<Vec<_>>()
            .join("\n");
        let l = debug_line(
            "eludite.terminal.wait",
            &json!({}),
            &Ok(
                json!({"terminal": "term1", "matched": "prompt", "exit_code": 0, "elapsed_ms": 42,
                       "text": out, "integration": true, "truncated": false, "mark": 9}),
            ),
        )
        .unwrap();
        assert_eq!(
            l.text,
            "Waited 42 ms in term1: prompt (exit 0)\nline 4\nline 5\nline 6\nline 7\nline 8"
        );
        let l = debug_line(
            "eludite.terminal.send",
            &json!({"text": "ls"}),
            &Err("command failed: the person typed into term1 while you waited".into()),
        )
        .unwrap();
        assert!(
            l.text
                .starts_with("Terminal send refused: the person typed")
        );
    }

    #[test]
    fn the_latest_command_list_replaces_the_last_one_and_makes_no_row() {
        let mut t = Transcript::default();
        t.apply(&usage_update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": eludite_acp::fake_agent::stream_commands()
        })));
        let names = |t: &Transcript| {
            t.commands
                .iter()
                .map(|c| c.name.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(names(&t), ["compact", "model"]);
        assert_eq!(
            t.commands[0].hint(),
            Some("<optional custom summarization instructions>")
        );
        assert!(t.rows.is_empty());
        t.apply(&usage_update(json!({
            "sessionUpdate": "available_commands_update",
            "availableCommands": [{"name": "clear", "description": "Clear the conversation"}]
        })));
        assert_eq!(names(&t), ["clear"]);
        t.apply(&usage_update(json!({
            "sessionUpdate": "available_commands_update", "availableCommands": []
        })));
        assert!(t.commands.is_empty());
        assert!(t.rows.is_empty());
    }
}

#[cfg(test)]
mod record_tests {
    use super::tests::png;
    use super::*;
    use eludite_acp::protocol::ContentBlock;

    /// A transcript with every kind of row: prompts, Markdown, a thought, tool calls (completed with an MCP link and a
    /// debug line, denied, failed, with a change and a thumbnail), a plan, a usage line, a notice and an error.
    fn rich() -> Transcript {
        let mut t = Transcript::default();
        t.notice("Starting Fake agent");
        t.user("Fix the\nfailing test");
        t.apply(&SessionUpdate::AgentThoughtChunk(ContentBlock::text(
            "Let me look.",
        )));
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "## Plan\n\nI will read `a.rs`.\n\n```rust\nfn a() {}\n```\n",
        )));
        let plan: SessionUpdate =
            serde_json::from_value(json!({"sessionUpdate": "plan", "entries": [
            {"content": "Read", "priority": "high", "status": "completed"},
            {"content": "Fix", "priority": "high", "status": "in_progress"}]}))
            .unwrap();
        t.apply(&plan);
        let mut read = ToolCall {
            tool_call_id: "r".into(),
            title: Some("Step over".into()),
            kind: Some("execute".into()),
            status: Some(ToolCallStatus::Completed),
            raw_input: Some(json!({"session": 1})),
            meta: Some(
                json!({"claudeCode": {"toolName": "mcp__eludite__eludite-debug-step_over"}}),
            ),
            ..Default::default()
        };
        read.content = Some(vec![
            json!({"type": "content", "content": {"type": "text", "text": "stopped"}}),
        ]);
        t.apply(&SessionUpdate::ToolCall(read));
        t.link_mcp(
            Some("r"),
            "eludite-debug-step_over",
            McpLink {
                command: "eludite.debug.step_over".into(),
                class: PermissionClass::Execute,
                ok: true,
                ms: 3.25,
                audit: 7,
                debug: Some(DebugLine {
                    text: "Step Over \u{2192} stopped at Program.cs:42 (breakpoint)".into(),
                    location: Some(("/s/Program.cs".into(), 42)),
                }),
            },
        );
        let shell = ToolCall {
            tool_call_id: "s".into(),
            title: Some("rm -rf obj/".into()),
            kind: Some("execute".into()),
            raw_input: Some(json!({"command": "rm -rf obj/"})),
            meta: Some(json!({"claudeCode": {"toolName": "Bash"}})),
            ..Default::default()
        };
        t.apply(&SessionUpdate::ToolCall(shell));
        let req: RequestPermissionRequest = serde_json::from_value(json!({
            "sessionId": "x", "toolCall": {"toolCallId": "s"}, "options": []}))
        .unwrap();
        t.permission(
            &req,
            Permission::Denied {
                auto: false,
                reason: "Denied".into(),
            },
        );
        let edit = ToolCall {
            tool_call_id: "e".into(),
            title: Some("Write Program.cs".into()),
            kind: Some("edit".into()),
            status: Some(ToolCallStatus::Completed),
            meta: Some(json!({"claudeCode": {"toolName": "Write"}})),
            ..Default::default()
        };
        t.apply(&SessionUpdate::ToolCall(edit));
        t.change("e", 3, "/s/Program.cs", "accepted");
        t.set_audit("e", 9);
        let thumb = decode_thumb(&ImageData {
            data: png(4, 2),
            mime: "image/png".into(),
        })
        .unwrap();
        t.add_thumbs("e", vec![thumb]);
        let failed = ToolCall {
            tool_call_id: "f".into(),
            title: Some("Read missing.rs".into()),
            kind: Some("read".into()),
            status: Some(ToolCallStatus::Failed),
            meta: Some(json!({"claudeCode": {"toolName": "Read"}})),
            ..Default::default()
        };
        t.apply(&SessionUpdate::ToolCall(failed));
        t.toggle_tool(t.rows.len() - 1);
        let usage: SessionUpdate =
            serde_json::from_value(eludite_acp::fake_agent::stream_usage()).unwrap();
        t.apply(&usage);
        t.end_turn();
        t.error("The turn failed: boom");
        t.user("Again");
        t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
            "Done.",
        )));
        t.end_turn();
        t
    }

    /// Brief 0060: a transcript's record rebuilds into rows whose record is the same; the rows read as the shown ones
    /// did (titles, statuses, notes, the debug line, the change links), thoughts collapsed and done, and no restored
    /// call is audited again.
    #[test]
    fn a_record_rebuilds_the_rows_it_was_made_from() {
        let t = rich();
        let record = t.to_json();
        let rebuilt = Transcript::from_json(&record);
        assert_eq!(rebuilt.to_json(), record);
        // And again: the round trip is stable.
        assert_eq!(Transcript::from_json(&rebuilt.to_json()).to_json(), record);
        assert_eq!(rebuilt.rows.len(), t.rows.len());
        assert_eq!(rebuilt.agent_message(), t.agent_message());
        for (a, b) in t.tools().zip(rebuilt.tools()) {
            assert_eq!(a.name(), b.name());
            assert_eq!(a.tool_name(), b.tool_name());
            assert_eq!(a.note(), b.note());
            assert_eq!(a.expanded, b.expanded);
            assert_eq!(a.call.content_text(), b.call.content_text());
            assert_eq!(a.debug_line(), b.debug_line());
        }
        let status = |id: &str| rebuilt.tool(id).unwrap().status();
        assert_eq!(status("r"), ToolStatus::Completed);
        assert_eq!(status("s"), ToolStatus::Denied);
        assert_eq!(status("f"), ToolStatus::Failed);
        assert_eq!(
            rebuilt.tool("e").unwrap().changes,
            [(3, "Program.cs".to_owned(), "accepted".to_owned())]
        );
        assert!(rebuilt.unaudited().is_empty());
        assert!(rebuilt.asked().is_empty());
        let Some(Row::User { text, time }) = rebuilt.rows.get(1) else {
            panic!("{:?}", rebuilt.rows.get(1))
        };
        assert_eq!(text, "Fix the\nfailing test");
        assert_eq!(time.len(), 5);
        let thought = rebuilt
            .rows
            .iter()
            .find_map(|r| match r {
                Row::Thought(t) => Some(t),
                _ => None,
            })
            .unwrap();
        assert!(thought.done && !thought.expanded);
        assert_eq!(thought.label(), "Thought for 1 s");
        // The usage strip's numbers come back.
        assert_eq!(rebuilt.usage, t.usage);
        // A live update makes a restored row live again.
        let mut live = rebuilt;
        let update = ToolCall {
            tool_call_id: "f".into(),
            status: Some(ToolCallStatus::Completed),
            ..Default::default()
        };
        live.apply(&SessionUpdate::ToolCallUpdate(update));
        assert!(live.tool("f").unwrap().restored.is_none());
        // Entries it does not know are skipped.
        let odd =
            Transcript::from_json(&json!([{"user": "hi", "time": "10:00"}, {"mystery": 1}, 3]));
        assert_eq!(odd.rows.len(), 1);
        assert!(
            Transcript::from_json(&json!({"not": "a list"}))
                .rows
                .is_empty()
        );
    }

    /// Brief 0060: an agent's replay builds rows when the shell has no record: the prompts (their chunks joined),
    /// the answer, the tool calls.
    #[test]
    fn a_replay_builds_prompts_and_answers() {
        let mut t = Transcript::default();
        for u in [
            SessionUpdate::UserMessageChunk(ContentBlock::text("Fix ")),
            SessionUpdate::UserMessageChunk(ContentBlock::text("it")),
            SessionUpdate::AgentMessageChunk(ContentBlock::text("Done.")),
            SessionUpdate::UserMessageChunk(ContentBlock::text("Thanks")),
        ] {
            t.replay(&u);
        }
        t.end_turn();
        let record = t.to_json();
        assert_eq!(record[0], json!({"user": "Fix it", "time": ""}));
        assert_eq!(record[1], json!({"agent": "Done."}));
        assert_eq!(record[2]["user"], "Thanks");
    }

    /// Brief 0060's budget: a 2,000-row record rebuilds in under 50 ms in a debug build; it serializes in under 10 ms
    /// (the brief's budget, for the shipped build: a debug build takes 11 to 12 ms on the 4-core VM of brief 0060's
    /// report, so a debug run is held to twice it). Asserted while the machine is not loaded; printed always.
    #[test]
    fn a_two_thousand_row_record_serializes_and_rebuilds_within_budget() {
        let mut t = Transcript::default();
        let mut n = 0;
        while t.rows.len() < 2000 {
            n += 1;
            t.user(&format!("Prompt {n}"));
            t.apply(&SessionUpdate::AgentMessageChunk(ContentBlock::text(
                format!("Answer {n}: some **bold** text and `code`.\n\nA second paragraph.\n"),
            )));
            let call = ToolCall {
                tool_call_id: format!("t{n}"),
                title: Some(format!("Read file{n}.rs")),
                kind: Some("read".into()),
                status: Some(ToolCallStatus::Completed),
                raw_input: Some(json!({"file_path": format!("/s/file{n}.rs")})),
                content: Some(vec![
                    json!({"type": "content", "content": {"type": "text", "text": "fn main() {}\n"}}),
                ]),
                meta: Some(json!({"claudeCode": {"toolName": "Read"}})),
                ..Default::default()
            };
            t.apply(&SessionUpdate::ToolCall(call));
            t.end_turn();
        }
        let started = Instant::now();
        let record = t.to_json();
        let serialize = started.elapsed();
        let started = Instant::now();
        let rebuilt = Transcript::from_json(&record);
        let rebuild = started.elapsed();
        assert_eq!(rebuilt.rows.len(), t.rows.len());
        eprintln!(
            "2000-row record: serialized in {:.2} ms, rebuilt in {:.2} ms",
            serialize.as_secs_f64() * 1e3,
            rebuild.as_secs_f64() * 1e3
        );
        let quiet = std::fs::read_to_string("/proc/loadavg")
            .ok()
            .and_then(|l| l.split_whitespace().next()?.parse::<f64>().ok())
            .is_some_and(|load| {
                load < std::thread::available_parallelism().map_or(1, |n| n.get()) as f64
            });
        if quiet {
            let budget = if cfg!(debug_assertions) { 20 } else { 10 };
            assert!(serialize < Duration::from_millis(budget), "{serialize:?}");
            assert!(rebuild < Duration::from_millis(50), "{rebuild:?}");
        }
    }
}
