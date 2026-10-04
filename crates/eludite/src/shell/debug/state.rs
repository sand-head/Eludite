//! The debugger's state machine, without GPUI: the mode, the session and stop generations, breakpoints, the stopped
//! thread's call stack, locals and watches, and the rules that make it safe for two drivers (PLAN.md 5.5).
//!
//! # The two-driver rules
//!
//! The user (keys, the margin, the windows) and agents (the same commands over MCP) drive one session:
//!
//! 1. **One queue.** Every command reaches this model on the UI thread, one at a time, through the command bus:
//!    the shell applies the user's directly and an agent's through its job queue. Nothing is applied concurrently.
//! 2. **Commands act on the state they were applied to, and are refused rather than queued.** Resuming commands
//!    (continue, the steps, Run To Cursor) and the commands that read a break (evaluate, select frame) need break
//!    mode. Applying one moves the model to `running` at once, before the adapter answers, so a second driver's
//!    step issued meanwhile is refused with a message naming the mode, the generation and the stop; it is never
//!    queued behind the first.
//! 3. **Stale stops are refused.** Each break increments `stop`. A command may quote the stop it saw; if the
//!    debuggee has moved on since (another driver stepped), it is refused as stale.
//! 4. **Old answers are dropped.** Adapter answers carry the session generation and the stop they were asked in;
//!    answers for an older one are never shown (CLAUDE.md invariant 12).
//! 5. **Both see the same state.** `eludite.debug.state` is built from this model, the one the windows render, and
//!    records who resumed the debuggee last (`last_driver`, `stopped.driver`).
//! 6. **Editing is always allowed.** Breakpoints, watches and exception settings change in any mode; the session
//!    picks them up at once.
//!
//! # Run control (brief 0026)
//!
//! Breakpoints are line breakpoints, tracepoints (a line breakpoint with a `log_message`) and function breakpoints
//! ([`FunctionBreakpoint`], by name). `run_until` and `trace` add `temporary` ones for the length of the call; they are
//! never persisted. `run_until`, `trace` (from break mode) and `set_next_statement` need break mode and are refused, not
//! queued, otherwise (`trace` with `run: start` needs no session); `set_variable` needs break mode. Exception settings
//! hold exception types too ([`ExceptionPlan`] turns them into DAP filters and filter options). What persists is
//! version 2 ([`Persisted`]); a version 1 file loads unchanged.
//!
//! # Attach, restart and who may drive (brief 0027)
//!
//! `attach` starts a session from a running process (allowed in design mode, and while Ctrl+F5's program runs, which
//! may be attached to); the session's `attached` flag makes Stop detach and Restart refused. `restart` needs a launched
//! session. [`DebugModel::agents_allowed`] is the session's Allow Agents to Drive switch: each session starts with the
//! setting's default, or with what the person chose while no session ran.
//!
//! # Several sessions (brief 0028)
//!
//! The shell debugs several processes at once; this model is one session's (the shell keeps the others aside and
//! swaps the one it works on in, `Debugger::enter`). What every session shares stays in place when they are swapped:
//! the breakpoints' definitions, the exception settings, the watch expressions, the startup projects and the Allow
//! Agents to Drive default. A breakpoint's binding (bound, hits, message, the adapter's id) is per session: the
//! current session's in its fields, the others' in [`Breakpoint::bindings`] ([`Breakpoints::switch_session`] moves
//! them). `run_until`'s and `trace`'s temporary points belong to the session that set them ([`Breakpoint::owner`]) and
//! are sent to that session's adapter only. The two-driver rules hold per session: each has its own generation (unique
//! across sessions), stop counter and mode.
//!
//! # Inspection (brief 0025)
//!
//! Reads never move what the windows show: `snapshot`, `stack`, `variables` and `exception_info` take the thread and
//! frame as parameters (proposal 0001 rule 3), and only `select_frame` changes [`DebugModel::thread`] and
//! [`DebugModel::frame`]. `output` reads the per-session rings ([`OutputRing`], one per [`OutputKind`]), `wait` and
//! `output` are never refused for the mode, `snapshot` needs a break or a running debuggee, and `pause` a running
//! one. [`DebugModel::summary`] is the stop summary as far as the model knows it (the selected frame, the loaded
//! members), which the UI thread answers with; agents' answers are completed through the adapter by the shell.

use std::collections::VecDeque;

use eludite_commands::CommandError;
use eludite_commands::debug::{
    BreakpointBrief, BreakpointKind, BreakpointRow, BreakpointSessionRow, Budget, CapabilitiesRow,
    ConsoleRow, DebugRequest, DebugState, ExceptionBrief, ExceptionSettingsRow, ExceptionTypeRow,
    FailedBreakpointRow, FrameRow, FramesBlock, HitCondition, LocalsBlock, LocationRow,
    OutputBlock, OutputKind, OutputLine, OutputPattern, SessionRow, StackFrameRow, StopSummary,
    StoppedRow, SummaryStopped, SummaryWatch, ThreadRow, TraceRun, VarRow, VariableRow, WatchRow,
    cut_value, null_spelling, pending_message,
};
use eludite_commands::project::StartupAction;
use eludite_dap::session::AdapterFamily;
use eludite_dap::types::{
    ExceptionFilterOptions, FunctionBreakpoint as DapFunction, SourceBreakpoint,
};
use eludite_editor::BreakpointGlyph;
use serde::{Deserialize, Serialize};

/// `eludite.debug.state` keeps this many lines of the program's output (the Output window keeps them all).
pub const CONSOLE_LINES: usize = 10_000;
/// Locals and members listed at most (the schema's bound).
pub const MAX_VARIABLES: usize = 500;
/// Lines each output ring keeps (`eludite.debug.output`).
pub const OUTPUT_RING_LINES: usize = 10_000;

/// One source's output in a session (brief 0025): the last [`OUTPUT_RING_LINES`] lines, numbered from 0 in the order
/// they were written, read by cursor.
#[derive(Debug, Clone, Default)]
pub struct OutputRing {
    lines: VecDeque<(String, Option<&'static str>)>,
    /// The sequence number of the next line.
    next: u64,
    /// The end of the text written so far that has no newline yet, and its stream.
    partial: Option<(String, Option<&'static str>)>,
}

/// A page of a ring: (lines, next cursor, dropped, total, more after `next`).
pub type RingPage = (Vec<OutputLine>, u64, u64, u64, bool);

impl OutputRing {
    pub fn next(&self) -> u64 {
        self.next
    }

    /// The sequence number of the oldest line kept.
    fn first(&self) -> u64 {
        self.next - self.lines.len() as u64
    }

    pub fn push_line(&mut self, text: impl Into<String>, stream: Option<&'static str>) {
        if self.lines.len() == OUTPUT_RING_LINES {
            self.lines.pop_front();
        }
        self.lines.push_back((text.into(), stream));
        self.next += 1;
    }

    /// Text as an adapter sends it: complete lines are kept, the rest waits for its newline.
    pub fn push_text(&mut self, text: &str, stream: Option<&'static str>) {
        let text = text.replace('\r', "");
        if let Some((_, s)) = &self.partial
            && *s != stream
        {
            self.flush();
        }
        let mut buf = self.partial.take().map(|(p, _)| p).unwrap_or_default();
        buf.push_str(&text);
        let mut parts: Vec<&str> = buf.split('\n').collect();
        let rest = parts.pop().unwrap_or_default().to_owned();
        for p in parts {
            self.push_line(p, stream);
        }
        if !rest.is_empty() {
            self.partial = Some((rest, stream));
        }
    }

    /// Keep a line without its newline (the session ended).
    pub fn flush(&mut self) {
        if let Some((p, s)) = self.partial.take() {
            self.push_line(p, s);
        }
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    fn line(seq: u64, (text, stream): &(String, Option<&'static str>)) -> OutputLine {
        OutputLine {
            seq,
            text: text.clone(),
            stream: stream.map(str::to_owned),
        }
    }

    /// Up to `max` lines from `since` on (those matching `pattern`). A cursor past the end (an older session's) reads
    /// from the start.
    pub fn read(&self, since: u64, max: usize, pattern: Option<&OutputPattern>) -> RingPage {
        let since = if since > self.next { 0 } else { since };
        let first = self.first();
        let dropped = first.saturating_sub(since);
        let mut out = Vec::new();
        let mut next = since.max(first);
        for (seq, l) in (first..).zip(&self.lines).skip((next - first) as usize) {
            if out.len() == max {
                break;
            }
            next = seq + 1;
            if pattern.is_none_or(|p| p.matches(&l.0)) {
                out.push(Self::line(seq, l));
            }
        }
        // Lines after `next` that the page left out: only matching ones count.
        let more = (first..)
            .zip(&self.lines)
            .skip((next - first) as usize)
            .any(|(_, l)| pattern.is_none_or(|p| p.matches(&l.0)));
        (out, next, dropped, self.next, more)
    }

    /// The last `max` lines.
    pub fn tail(&self, max: usize) -> RingPage {
        let since = self.next.saturating_sub(max as u64).max(self.first());
        let (lines, next, _, total, _) = self.read(since, max, None);
        (lines, next, 0, total, false)
    }

    /// As the stop summary lists it: from `since`, or the last `max` lines.
    pub fn block(&self, since: Option<u64>, max: usize) -> OutputBlock {
        let (lines, next, dropped, total, truncated) = match since {
            Some(s) => self.read(s, max, None),
            None => self.tail(max),
        };
        OutputBlock {
            lines,
            next,
            dropped,
            total,
            truncated,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Design,
    /// F5 or Ctrl+F5 builds the startup project before launching it (brief 0020).
    Building,
    Launching,
    Running,
    Break,
    Stopping,
    RunningWithoutDebugging,
}

impl Mode {
    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Design => "design",
            Mode::Building => "building",
            Mode::Launching => "launching",
            Mode::Running => "running",
            Mode::Break => "break",
            Mode::Stopping => "stopping",
            Mode::RunningWithoutDebugging => "running_without_debugging",
        }
    }
}

/// Visual Studio's tracepoint specials the debugger fills in (others, `$PID` and `$ADDRESS` among them, stay text).
pub const SPECIALS: [&str; 4] = ["$FUNCTION", "$CALLER", "$TID", "$TNAME"];

/// A tracepoint message: literal text, `{expression}`s and specials.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    Text(String),
    Expression(String),
    Special(&'static str),
}

/// Split a tracepoint message: `{expression}` (`{{` and `}}` are literal braces) and [`SPECIALS`].
pub fn parse_message(message: &str) -> Vec<Segment> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = message;
    while let Some(c) = rest.chars().next() {
        if let Some(r) = rest.strip_prefix("{{") {
            text.push('{');
            rest = r;
        } else if let Some(r) = rest.strip_prefix("}}") {
            text.push('}');
            rest = r;
        } else if c == '{'
            && let Some(end) = rest.find('}')
        {
            if !text.is_empty() {
                out.push(Segment::Text(std::mem::take(&mut text)));
            }
            out.push(Segment::Expression(rest[1..end].trim().to_owned()));
            rest = &rest[end + 1..];
        } else if let Some(sp) = SPECIALS.iter().find(|sp| {
            rest.starts_with(**sp)
                && !rest[sp.len()..].starts_with(|c: char| c.is_ascii_alphanumeric() || c == '_')
        }) {
            if !text.is_empty() {
                out.push(Segment::Text(std::mem::take(&mut text)));
            }
            out.push(Segment::Special(sp));
            rest = &rest[sp.len()..];
        } else {
            text.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    if !text.is_empty() {
        out.push(Segment::Text(text));
    }
    out
}

/// Whether a message uses a special: the debugger then prints it itself, since DAP log messages have none.
pub fn has_specials(message: &str) -> bool {
    parse_message(message)
        .iter()
        .any(|s| matches!(s, Segment::Special(_)))
}

/// Whether `line` is what an adapter printed for `message` (its `{expression}`s matching anything): how the lines an
/// adapter prints for log points are told apart from its other console output.
pub fn message_matches(message: &str, line: &str) -> bool {
    let segments = parse_message(message);
    let mut rest = line;
    let mut wild = false;
    let n = segments.len();
    for (i, seg) in segments.iter().enumerate() {
        match seg {
            Segment::Text(t) => {
                let last = i + 1 == n;
                if wild {
                    let found = if last {
                        rest.ends_with(t.as_str()).then(|| rest.len() - t.len())
                    } else {
                        rest.find(t.as_str())
                    };
                    match found {
                        Some(at) => rest = &rest[at + t.len()..],
                        None => return false,
                    }
                } else if let Some(r) = rest.strip_prefix(t.as_str()) {
                    rest = r;
                } else {
                    return false;
                }
                wild = false;
            }
            Segment::Expression(_) | Segment::Special(_) => wild = true,
        }
    }
    wild || rest.is_empty()
}

/// One breakpoint. `line` is 1-based; `path` is the normalized absolute path (the document id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    pub path: String,
    pub line: u32,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_condition: Option<HitCondition>,
    /// A tracepoint's message (brief 0026).
    pub log_message: Option<String>,
    /// Deleted at its first visible stop.
    pub remove_after: bool,
    /// `run_until`'s or `trace`'s for the length of that call: removed after it, never persisted.
    pub temporary: bool,
    /// The session's: bound by the adapter, times reached, the adapter's message and id.
    pub verified: bool,
    pub hits: u32,
    pub message: Option<String>,
    pub adapter_id: Option<i64>,
    /// The other sessions' bindings, by session id (brief 0028).
    pub bindings: Vec<(u32, Binding)>,
    /// A temporary point's session: only its adapter gets it (brief 0028).
    pub owner: Option<u32>,
}

/// A breakpoint's state in one session (brief 0028).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Binding {
    pub verified: bool,
    pub hits: u32,
    pub message: Option<String>,
    pub adapter_id: Option<i64>,
}

/// Move the current session's binding of a breakpoint aside under `old` and take `new`'s (brief 0028).
fn switch_binding(
    fields: (&mut bool, &mut u32, &mut Option<String>, &mut Option<i64>),
    bindings: &mut Vec<(u32, Binding)>,
    old: u32,
    new: u32,
) {
    let (verified, hits, message, adapter_id) = fields;
    let mine = Binding {
        verified: std::mem::take(verified),
        hits: std::mem::take(hits),
        message: message.take(),
        adapter_id: adapter_id.take(),
    };
    bindings.retain(|(id, _)| *id != old);
    if mine != Binding::default() {
        bindings.push((old, mine));
    }
    if let Some(i) = bindings.iter().position(|(id, _)| *id == new) {
        let (_, b) = bindings.remove(i);
        *verified = b.verified;
        *hits = b.hits;
        *message = b.message;
        *adapter_id = b.adapter_id;
    }
}

impl Breakpoint {
    pub fn new(path: &str, line: u32) -> Self {
        Self {
            path: path.to_owned(),
            line,
            enabled: true,
            condition: None,
            hit_condition: None,
            log_message: None,
            remove_after: false,
            temporary: false,
            verified: false,
            hits: 0,
            message: None,
            adapter_id: None,
            bindings: Vec::new(),
            owner: None,
        }
    }

    /// Bound in any session (brief 0028): the margin draws it bound.
    pub fn bound(&self) -> bool {
        self.verified || self.bindings.iter().any(|(_, b)| b.verified)
    }

    /// Whether the adapter prints this tracepoint's message (`log_points`: it has log points) rather than the debugger.
    pub fn adapter_logs(&self, log_points: bool) -> bool {
        log_points
            && self
                .log_message
                .as_deref()
                .is_some_and(|m| !has_specials(m))
    }

    /// The glyph the margin draws, `in_session` when a debugging session runs.
    pub fn glyph(&self, in_session: bool) -> BreakpointGlyph {
        if self.log_message.is_some() {
            return if !self.enabled {
                BreakpointGlyph::TracepointDisabled
            } else if in_session && !self.bound() {
                BreakpointGlyph::TracepointUnbound
            } else {
                BreakpointGlyph::Tracepoint
            };
        }
        if !self.enabled {
            BreakpointGlyph::Disabled
        } else if in_session && !self.bound() {
            BreakpointGlyph::Unbound
        } else if self.condition.is_some() || self.hit_condition.is_some() {
            BreakpointGlyph::Conditional
        } else {
            BreakpointGlyph::Enabled
        }
    }
}

/// A function breakpoint (brief 0026): a method by name, `Namespace.Type.Method`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionBreakpoint {
    pub name: String,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_condition: Option<HitCondition>,
    pub remove_after: bool,
    /// The session's, as for [`Breakpoint`].
    pub verified: bool,
    pub hits: u32,
    pub message: Option<String>,
    pub adapter_id: Option<i64>,
    /// The other sessions' bindings (brief 0028).
    pub bindings: Vec<(u32, Binding)>,
}

impl FunctionBreakpoint {
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            enabled: true,
            condition: None,
            hit_condition: None,
            remove_after: false,
            verified: false,
            hits: 0,
            message: None,
            adapter_id: None,
            bindings: Vec::new(),
        }
    }

    /// Whether a frame named `frame` (`App.Calc.Add(int a, int b)`) is in this function.
    pub fn matches_frame(&self, frame: &str) -> bool {
        let base = frame.split('(').next().unwrap_or_default().trim();
        let name = self.name.split('(').next().unwrap_or_default().trim();
        !name.is_empty() && (base == name || base.ends_with(&format!(".{name}")))
    }
}

/// The persisted file's version: 2 adds tracepoints, function breakpoints, Delete when hit and exception types (brief
/// 0026); 3 adds the multiple startup projects (brief 0028). Older files have none of them and load as is (a version 2
/// file's `startup_project` is the one startup project).
pub const PERSISTED_VERSION: u32 = 3;

/// One of the multiple startup projects as it persists (version 3): the project file's absolute path and its action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedStartup {
    pub path: String,
    pub action: StartupAction,
}

/// What persists per solution and per user (Visual Studio's .suo): breakpoints (without session state), exception
/// settings, watch expressions and the startup project (brief 0020).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Persisted {
    pub version: u32,
    pub breakpoints: Vec<PersistedBreakpoint>,
    pub exceptions: Option<ExceptionSettingsRow>,
    pub watches: Vec<String>,
    /// The absolute path of the startup project's file (Set as Startup Project); absent: the first executable one.
    /// With multiple startup projects, the first one with action `start` (what older versions read).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_project: Option<String>,
    /// Version 3: the multiple startup projects with their actions (brief 0028); empty: `startup_project` alone.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub startup_projects: Vec<PersistedStartup>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedBreakpoint {
    /// Empty for a function breakpoint.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub path: String,
    /// 0 for a function breakpoint.
    #[serde(default, skip_serializing_if = "is_zero")]
    pub line: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    /// Version 2: a tracepoint's message.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
    /// Version 2: a function breakpoint's function.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    /// Version 2: Delete when hit.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub remove_after: bool,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

fn yes() -> bool {
    true
}

/// A variable (or a watch: `name` is the expression) and its members, loaded lazily.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct VarNode {
    pub name: String,
    pub value: String,
    pub type_name: Option<String>,
    pub reference: i64,
    pub evaluate_name: Option<String>,
    /// The adapter's member counts, when it gives them.
    pub indexed: Option<i64>,
    pub named: Option<i64>,
    /// A watch that failed to evaluate: `value` is the message.
    pub error: bool,
    pub expanded: bool,
    pub loading: bool,
    pub children: Option<Vec<VarNode>>,
}

impl VarNode {
    /// A DAP variable as a node, its null spelled `null` on every adapter (brief 0034).
    pub fn from_dap(v: &eludite_dap::types::Variable) -> Self {
        Self {
            name: v.name.clone(),
            value: null_spelling(v.value.clone()),
            type_name: v.type_name.clone().filter(|t| !t.is_empty()),
            reference: v.variables_reference,
            evaluate_name: v.evaluate_name.clone(),
            indexed: v.indexed_variables,
            named: v.named_variables,
            ..Self::default()
        }
    }

    /// The row `eludite.debug.variables` and the stop summary list, its value cut at `max_chars`, with the members
    /// the model has loaded down to `depth` levels below it within `budget` rows (counted down).
    pub fn var_row(&self, max_chars: usize) -> VarRow {
        let (value, value_truncated) = cut_value(&self.value, max_chars);
        VarRow {
            name: self.name.clone(),
            value,
            type_name: self.type_name.clone(),
            reference: self.reference,
            // Left out when it is the name itself (most locals): a model's context is the budget.
            evaluate_name: self.evaluate_name.clone().filter(|e| *e != self.name),
            indexed: self.indexed,
            named: self.named,
            value_truncated,
            children: None,
            truncated: false,
        }
    }

    pub fn watch(expression: &str) -> Self {
        Self {
            name: expression.to_owned(),
            ..Self::default()
        }
    }

    fn row(&self) -> VariableRow {
        VariableRow {
            name: self.name.clone(),
            value: self.value.clone(),
            type_name: self.type_name.clone(),
            reference: self.reference,
            evaluate_name: self.evaluate_name.clone(),
        }
    }
}

/// The node at `path` (indices from the top).
pub fn node_mut<'a>(nodes: &'a mut [VarNode], path: &[usize]) -> Option<&'a mut VarNode> {
    let (first, rest) = path.split_first()?;
    let node = nodes.get_mut(*first)?;
    if rest.is_empty() {
        Some(node)
    } else {
        node_mut(node.children.as_deref_mut()?, rest)
    }
}

/// One row of a variables tree as a window lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlatRow {
    pub path: Vec<usize>,
    pub depth: usize,
    pub name: String,
    pub value: String,
    pub type_name: String,
    /// `None` for a leaf, else whether it is expanded.
    pub expanded: Option<bool>,
    pub error: bool,
}

/// The visible rows of `nodes` (expanded nodes' children follow them).
pub fn flatten(nodes: &[VarNode]) -> Vec<FlatRow> {
    fn walk(nodes: &[VarNode], prefix: &mut Vec<usize>, out: &mut Vec<FlatRow>) {
        for (i, n) in nodes.iter().enumerate() {
            prefix.push(i);
            out.push(FlatRow {
                path: prefix.clone(),
                depth: prefix.len() - 1,
                name: n.name.clone(),
                value: if n.loading && n.value.is_empty() {
                    "\u{2026}".into()
                } else {
                    n.value.clone()
                },
                type_name: n.type_name.clone().unwrap_or_default(),
                expanded: (n.reference > 0).then_some(n.expanded),
                error: n.error,
            });
            if n.expanded
                && let Some(c) = &n.children
            {
                walk(c, prefix, out);
            }
            prefix.pop();
        }
    }
    let mut out = Vec::new();
    walk(nodes, &mut Vec::new(), &mut out);
    out
}

/// For each row [`flatten`] lists, the variables reference of the value it is a member of (0 at the top: a frame's
/// variable), which `set_variable` of a member needs.
pub fn flatten_parents(nodes: &[VarNode]) -> Vec<i64> {
    fn walk(nodes: &[VarNode], parent: i64, out: &mut Vec<i64>) {
        for n in nodes {
            out.push(parent);
            if n.expanded
                && let Some(c) = &n.children
            {
                walk(c, n.reference, out);
            }
        }
    }
    let mut out = Vec::new();
    walk(nodes, 0, &mut out);
    out
}

/// A frame of the stopped thread's call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The adapter's frame id (new at every stop).
    pub id: i64,
    pub row: FrameRow,
    /// The adapter marks it `subtle` (external code, `eludite-dbg-mono`).
    pub subtle: bool,
}

impl Frame {
    /// The row `eludite.debug.stack` lists: frames without source are `external`.
    pub fn stack_row(&self) -> StackFrameRow {
        let r = &self.row;
        StackFrameRow {
            index: r.index,
            name: r.name.clone(),
            path: r.path.clone(),
            line: r.line,
            column: r.column,
            end_line: r.end_line,
            end_column: r.end_column,
            external: r.path.is_none() || self.subtle,
            source: r.source.clone(),
        }
    }

    pub fn from_dap(index: usize, f: &eludite_dap::types::StackFrame) -> Self {
        let line = |v: i64| u32::try_from(v).ok().filter(|v| *v > 0);
        let has_source = f.path().is_some();
        Self {
            id: f.id,
            row: FrameRow {
                index,
                name: f.name.clone(),
                path: f.path().map(str::to_owned),
                line: line(f.line).filter(|_| has_source),
                column: line(f.column).filter(|_| has_source),
                end_line: f.end_line.and_then(line).filter(|_| has_source),
                end_column: f.end_column.and_then(line).filter(|_| has_source),
                // Through a source map (brief 0038): the original file is the frame's path.
                source: f.generated.as_ref().zip(f.path()).map(|(g, p)| {
                    eludite_commands::debug::FrameSourceRow {
                        original: p.to_owned(),
                        generated: Some(g.path.clone()),
                        generated_line: g.line.and_then(line),
                        generated_column: g.column.and_then(line),
                    }
                }),
            },
            subtle: f.presentation_hint.as_deref() == Some("subtle"),
        }
    }
}

/// The breakpoints, sorted by path and line, and the function breakpoints in the order they were added.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakpoints {
    list: Vec<Breakpoint>,
    functions: Vec<FunctionBreakpoint>,
    /// The session whose binding the fields hold (brief 0028; 0 before any).
    current: u32,
}

impl Breakpoints {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn all(&self) -> &[Breakpoint] {
        &self.list
    }

    fn sort(&mut self) {
        self.list
            .sort_by(|a, b| (a.path.as_str(), a.line).cmp(&(b.path.as_str(), b.line)));
    }

    pub fn at(&self, path: &str, line: u32) -> Option<&Breakpoint> {
        self.list.iter().find(|b| b.path == path && b.line == line)
    }

    pub fn at_mut(&mut self, path: &str, line: u32) -> Option<&mut Breakpoint> {
        self.list
            .iter_mut()
            .find(|b| b.path == path && b.line == line)
    }

    /// Take the breakpoint off the line (`trace` keeps it to put it back).
    pub fn take(&mut self, path: &str, line: u32) -> Option<Breakpoint> {
        let i = self
            .list
            .iter()
            .position(|b| b.path == path && b.line == line)?;
        Some(self.list.remove(i))
    }

    /// Put a breakpoint (back) on its line, replacing any there.
    pub fn put(&mut self, b: Breakpoint) {
        self.delete(&b.path, b.line);
        self.list.push(b);
        self.sort();
    }

    /// Make `session` the one whose binding the fields hold: the current one's moves to `bindings` (brief 0028).
    pub fn switch_session(&mut self, session: u32) {
        let old = self.current;
        if old == session {
            return;
        }
        for b in &mut self.list {
            switch_binding(
                (
                    &mut b.verified,
                    &mut b.hits,
                    &mut b.message,
                    &mut b.adapter_id,
                ),
                &mut b.bindings,
                old,
                session,
            );
        }
        for f in &mut self.functions {
            switch_binding(
                (
                    &mut f.verified,
                    &mut f.hits,
                    &mut f.message,
                    &mut f.adapter_id,
                ),
                &mut f.bindings,
                old,
                session,
            );
        }
        self.current = session;
    }

    /// A session is gone: its bindings with it (and its temporary points).
    pub fn forget_session(&mut self, session: u32) {
        for b in &mut self.list {
            b.bindings.retain(|(id, _)| *id != session);
        }
        for f in &mut self.functions {
            f.bindings.retain(|(id, _)| *id != session);
        }
        self.list
            .retain(|b| !(b.temporary && b.owner == Some(session)));
    }

    /// Every line breakpoint, mutably.
    pub fn iter_mut(&mut self) -> impl Iterator<Item = &mut Breakpoint> {
        self.list.iter_mut()
    }

    /// Delete the temporary breakpoints `remove` picks; returns the files that changed.
    pub fn remove_temporary(&mut self, remove: impl Fn(&Breakpoint) -> bool) -> Vec<String> {
        let mut files: Vec<String> = Vec::new();
        self.list.retain(|b| {
            let drop = b.temporary && remove(b);
            if drop && !files.contains(&b.path) {
                files.push(b.path.clone());
            }
            !drop
        });
        files
    }

    pub fn functions(&self) -> &[FunctionBreakpoint] {
        &self.functions
    }

    pub fn function_mut(&mut self, name: &str) -> Option<&mut FunctionBreakpoint> {
        self.functions.iter_mut().find(|f| f.name == name)
    }

    /// The function breakpoint on `name`, created if needed.
    pub fn ensure_function(&mut self, name: &str) -> &mut FunctionBreakpoint {
        if self.function_mut(name).is_none() {
            self.functions.push(FunctionBreakpoint::new(name));
        }
        self.function_mut(name).expect("just ensured")
    }

    pub fn delete_function(&mut self, name: &str) -> bool {
        let before = self.functions.len();
        self.functions.retain(|f| f.name != name);
        self.functions.len() != before
    }

    /// The enabled function breakpoints as `setFunctionBreakpoints` sends them (hit conditions only when the adapter
    /// supports them), with their names in the same order.
    pub fn function_breakpoints(&self, hit_conditions: bool) -> (Vec<String>, Vec<DapFunction>) {
        self.functions
            .iter()
            .filter(|f| f.enabled)
            .map(|f| {
                (
                    f.name.clone(),
                    DapFunction {
                        name: f.name.clone(),
                        condition: f.condition.clone(),
                        hit_condition: hit_conditions
                            .then(|| f.hit_condition.map(|h| h.to_string()))
                            .flatten(),
                    },
                )
            })
            .unzip()
    }

    /// The adapter's answer for the function breakpoints sent, in order.
    pub fn apply_function_answer(
        &mut self,
        names: &[String],
        answer: &[eludite_dap::types::Breakpoint],
    ) {
        for (name, a) in names.iter().zip(answer) {
            if let Some(f) = self.function_mut(name) {
                f.verified = a.verified;
                f.adapter_id = a.id;
                f.message = a.message.clone().filter(|_| !a.verified);
            }
        }
    }

    /// Add a breakpoint on the line, or delete the one there. True when added.
    pub fn toggle(&mut self, path: &str, line: u32) -> bool {
        if self.delete(path, line) {
            return false;
        }
        self.list.push(Breakpoint::new(path, line));
        self.sort();
        true
    }

    /// The breakpoint on the line, created if needed.
    pub fn ensure(&mut self, path: &str, line: u32) -> &mut Breakpoint {
        if self.at(path, line).is_none() {
            self.list.push(Breakpoint::new(path, line));
            self.sort();
        }
        self.at_mut(path, line).expect("just ensured")
    }

    pub fn delete(&mut self, path: &str, line: u32) -> bool {
        let before = self.list.len();
        self.list.retain(|b| !(b.path == path && b.line == line));
        self.list.len() != before
    }

    /// Delete every breakpoint, function breakpoints too; returns the files that had some.
    pub fn delete_all(&mut self) -> Vec<String> {
        let files = self.files();
        self.list.clear();
        self.functions.clear();
        files
    }

    /// The files with breakpoints.
    pub fn files(&self) -> Vec<String> {
        let mut f: Vec<String> = self.list.iter().map(|b| b.path.clone()).collect();
        f.dedup();
        f
    }

    /// The enabled breakpoints of `path` in line order, as `setBreakpoints` sends them, plus `extra` (Run To
    /// Cursor's one-shot line). Hit conditions go to the adapter only when it supports them; otherwise the shell
    /// counts hits itself.
    pub fn source_breakpoints(
        &self,
        path: &str,
        hit_conditions: bool,
        log_points: bool,
        extra: Option<u32>,
    ) -> (Vec<u32>, Vec<SourceBreakpoint>) {
        let mut lines = Vec::new();
        let mut out = Vec::new();
        // Another session's temporary points are not this one's (brief 0028).
        let mine = |b: &&Breakpoint| !b.temporary || b.owner.is_none_or(|o| o == self.current);
        for b in self
            .list
            .iter()
            .filter(|b| b.path == path && b.enabled)
            .filter(mine)
        {
            lines.push(b.line);
            out.push(SourceBreakpoint {
                line: i64::from(b.line),
                column: None,
                condition: b.condition.clone(),
                hit_condition: hit_conditions
                    .then(|| b.hit_condition.map(|h| h.to_string()))
                    .flatten(),
                // A tracepoint the adapter prints; otherwise it breaks and the debugger prints and resumes.
                log_message: b
                    .adapter_logs(log_points)
                    .then(|| b.log_message.clone())
                    .flatten(),
            });
        }
        if let Some(l) = extra.filter(|l| !lines.contains(l)) {
            lines.push(l);
            out.push(SourceBreakpoint {
                line: i64::from(l),
                ..Default::default()
            });
        }
        (lines, out)
    }

    /// The adapter's answer for the lines sent for `path`, in order.
    pub fn apply_answer(
        &mut self,
        path: &str,
        lines: &[u32],
        answer: &[eludite_dap::types::Breakpoint],
    ) {
        for (line, a) in lines.iter().zip(answer) {
            if let Some(b) = self.at_mut(path, *line) {
                b.verified = a.verified;
                b.adapter_id = a.id;
                b.message = a.message.clone();
            }
        }
    }

    /// A `breakpoint` event: the adapter bound (or unbound) breakpoint `id`. True when one changed.
    pub fn apply_event(&mut self, bp: &eludite_dap::types::Breakpoint) -> bool {
        let Some(id) = bp.id else { return false };
        if let Some(b) = self.list.iter_mut().find(|b| b.adapter_id == Some(id)) {
            b.verified = bp.verified;
            b.message = bp.message.clone().filter(|_| !bp.verified);
            return true;
        }
        match self.functions.iter_mut().find(|f| f.adapter_id == Some(id)) {
            Some(f) => {
                f.verified = bp.verified;
                f.message = bp.message.clone().filter(|_| !bp.verified);
                true
            }
            None => false,
        }
    }

    /// The current session's breakpoints its adapter refused or whose condition it rejected (brief 0036): not bound,
    /// with a message that is not a pending one. `run_until`'s and `trace`'s temporary points are reported by those
    /// commands (`points_failed`), not here.
    pub fn failed(&self) -> Vec<FailedBreakpointRow> {
        let refused = |verified: bool, enabled: bool, message: &Option<String>| {
            message
                .as_deref()
                .filter(|m| enabled && !verified && !m.trim().is_empty() && !pending_message(m))
                .map(str::to_owned)
        };
        let lines = self.list.iter().filter(|b| !b.temporary).filter_map(|b| {
            Some(FailedBreakpointRow {
                path: Some(b.path.clone()),
                line: Some(b.line),
                function: None,
                session: self.current,
                message: refused(b.verified, b.enabled, &b.message)?,
            })
        });
        let functions = self.functions.iter().filter_map(|f| {
            Some(FailedBreakpointRow {
                path: None,
                line: None,
                function: Some(f.name.clone()),
                session: self.current,
                message: refused(f.verified, f.enabled, &f.message)?,
            })
        });
        lines.chain(functions).collect()
    }

    /// The binding of the breakpoint on a line in the current session: (bound, the adapter's message).
    pub fn binding(&self, path: &str, line: u32) -> Option<(bool, Option<String>)> {
        self.at(path, line).map(|b| (b.verified, b.message.clone()))
    }

    /// Forget the session's state (a new session starts).
    pub fn reset_session(&mut self) {
        for b in &mut self.list {
            b.verified = false;
            b.hits = 0;
            b.message = None;
            b.adapter_id = None;
        }
        for f in &mut self.functions {
            f.verified = false;
            f.hits = 0;
            f.message = None;
            f.adapter_id = None;
        }
    }

    /// The session ended: nothing is bound any more (hits stay until the next session).
    pub fn unbind(&mut self) {
        for b in &mut self.list {
            b.verified = false;
            b.adapter_id = None;
            b.message = None;
        }
        for f in &mut self.functions {
            f.verified = false;
            f.adapter_id = None;
            f.message = None;
        }
    }

    /// The margin glyphs of `path`: (0-based row, glyph).
    pub fn glyphs(&self, path: &str, in_session: bool) -> Vec<(u32, BreakpointGlyph)> {
        self.list
            .iter()
            .filter(|b| b.path == path)
            .map(|b| (b.line.saturating_sub(1), b.glyph(in_session)))
            .collect()
    }

    /// The text moved the breakpoints of `path` to these (1-based) lines, in order. False when nothing changed or
    /// the count differs (two breakpoints merged into one line: left alone).
    pub fn moved(&mut self, path: &str, lines: &[u32]) -> bool {
        let mut current: Vec<&mut Breakpoint> =
            self.list.iter_mut().filter(|b| b.path == path).collect();
        if current.len() != lines.len() || current.iter().zip(lines).all(|(b, l)| b.line == *l) {
            return false;
        }
        for (b, l) in current.iter_mut().zip(lines) {
            b.line = *l;
        }
        self.sort();
        true
    }

    /// What persists: every breakpoint but the temporary ones, then the function breakpoints.
    pub fn to_persisted(&self) -> Vec<PersistedBreakpoint> {
        let lines = self
            .list
            .iter()
            .filter(|b| !b.temporary)
            .map(|b| PersistedBreakpoint {
                path: b.path.clone(),
                line: b.line,
                enabled: b.enabled,
                condition: b.condition.clone(),
                hit_condition: b.hit_condition.map(|h| h.to_string()),
                log_message: b.log_message.clone(),
                function: None,
                remove_after: b.remove_after,
            });
        let functions = self.functions.iter().map(|f| PersistedBreakpoint {
            path: String::new(),
            line: 0,
            enabled: f.enabled,
            condition: f.condition.clone(),
            hit_condition: f.hit_condition.map(|h| h.to_string()),
            log_message: None,
            function: Some(f.name.clone()),
            remove_after: f.remove_after,
        });
        lines.chain(functions).collect()
    }

    pub fn from_persisted(rows: &[PersistedBreakpoint]) -> Self {
        let condition =
            |r: &PersistedBreakpoint| r.condition.clone().filter(|c| !c.trim().is_empty());
        let mut b = Self {
            list: rows
                .iter()
                .filter(|r| r.function.is_none() && r.line >= 1 && !r.path.is_empty())
                .map(|r| Breakpoint {
                    enabled: r.enabled,
                    condition: condition(r),
                    hit_condition: r.hit_condition.as_deref().and_then(HitCondition::parse),
                    log_message: r.log_message.clone().filter(|m| !m.is_empty()),
                    remove_after: r.remove_after,
                    ..Breakpoint::new(&r.path, r.line)
                })
                .collect(),
            functions: Vec::new(),
            current: 0,
        };
        for r in rows {
            if let Some(name) = r.function.as_deref().filter(|n| !n.trim().is_empty())
                && b.function_mut(name).is_none()
            {
                b.functions.push(FunctionBreakpoint {
                    enabled: r.enabled,
                    condition: condition(r),
                    hit_condition: r.hit_condition.as_deref().and_then(HitCondition::parse),
                    remove_after: r.remove_after,
                    ..FunctionBreakpoint::new(name)
                });
            }
        }
        b.sort();
        b.list.dedup_by(|a, b| a.path == b.path && a.line == b.line);
        b
    }

    pub fn rows(&self) -> Vec<BreakpointRow> {
        self.rows_for(&[])
    }

    /// The rows with each breakpoint's binding in the live sessions `live` (brief 0028): `sessions` per session, the
    /// row's `verified` bound in any, its `hits` summed; with no live session, as the current one left them. A line
    /// breakpoint lists only the sessions whose adapter takes its file (brief 0038, [`AdapterFamily::takes`]).
    pub fn rows_for(&self, live: &[(u32, AdapterFamily)]) -> Vec<BreakpointRow> {
        let current = self.current;
        let per = |verified: bool,
                   hits: u32,
                   message: &Option<String>,
                   bindings: &[(u32, Binding)],
                   path: Option<&str>|
         -> (bool, u32, Vec<BreakpointSessionRow>) {
            if live.is_empty() {
                return (verified, hits, Vec::new());
            }
            let rows: Vec<BreakpointSessionRow> = live
                .iter()
                .filter(|(_, f)| path.is_none_or(|p| f.takes(p)))
                .map(|(id, _)| {
                    if *id == current {
                        BreakpointSessionRow {
                            session: *id,
                            verified,
                            hits,
                            message: message.clone().filter(|_| !verified),
                        }
                    } else {
                        let b = bindings
                            .iter()
                            .find(|(s, _)| s == id)
                            .map(|(_, b)| b.clone())
                            .unwrap_or_default();
                        BreakpointSessionRow {
                            session: *id,
                            verified: b.verified,
                            hits: b.hits,
                            message: b.message.filter(|_| !b.verified),
                        }
                    }
                })
                .collect();
            (
                rows.iter().any(|r| r.verified),
                rows.iter().map(|r| r.hits).sum(),
                rows,
            )
        };
        let rows: Vec<BreakpointRow> = self.rows_one();
        rows.into_iter()
            .zip(
                self.list
                    .iter()
                    .map(|b| per(b.verified, b.hits, &b.message, &b.bindings, Some(&b.path)))
                    .chain(
                        self.functions
                            .iter()
                            .map(|f| per(f.verified, f.hits, &f.message, &f.bindings, None)),
                    ),
            )
            .map(|(mut row, (verified, hits, sessions))| {
                row.verified = verified;
                row.hits = hits;
                row.sessions = sessions;
                row
            })
            .collect()
    }

    fn rows_one(&self) -> Vec<BreakpointRow> {
        let lines = self.list.iter().map(|b| BreakpointRow {
            kind: if b.log_message.is_some() {
                BreakpointKind::Tracepoint
            } else {
                BreakpointKind::Line
            },
            path: Some(b.path.clone()),
            line: Some(b.line),
            function: None,
            enabled: b.enabled,
            verified: b.verified,
            condition: b.condition.clone(),
            hit_condition: b.hit_condition.map(|h| h.to_string()),
            hits: b.hits,
            message: b.message.clone(),
            log_message: b.log_message.clone(),
            remove_after: b.remove_after,
            temporary: b.temporary,
            sessions: Vec::new(),
        });
        let functions = self.functions.iter().map(|f| BreakpointRow {
            kind: BreakpointKind::Function,
            path: None,
            line: None,
            function: Some(f.name.clone()),
            enabled: f.enabled,
            verified: f.verified,
            condition: f.condition.clone(),
            hit_condition: f.hit_condition.map(|h| h.to_string()),
            hits: f.hits,
            message: f.message.clone(),
            log_message: None,
            remove_after: f.remove_after,
            temporary: false,
            sessions: Vec::new(),
        });
        lines.chain(functions).collect()
    }
}

/// The DAP exception filters for these settings (netcoredbg's `all` and `user-unhandled`): a category box that is on
/// is its filter without a condition; while it is off, the types checked in that column are the filter's condition
/// (comma-separated, a `filterOptions` entry), so `options` is empty without types.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExceptionPlan {
    pub filters: Vec<String>,
    pub options: Vec<ExceptionFilterOptions>,
}

impl ExceptionPlan {
    /// `setExceptionBreakpoints`' arguments; the options only when the adapter takes them.
    pub fn arguments(&self, filter_options: bool) -> serde_json::Value {
        let none: [ExceptionFilterOptions; 0] = [];
        eludite_dap::session::set_exception_breakpoints_arguments(
            &self.filters,
            if filter_options { &self.options } else { &none },
        )
    }
}

pub fn exception_plan(e: &ExceptionSettingsRow) -> ExceptionPlan {
    let mut plan = ExceptionPlan::default();
    let mut filter = |on: bool, id: &str, pick: fn(&ExceptionTypeRow) -> bool| {
        if on {
            plan.filters.push(id.to_owned());
            return;
        }
        let types: Vec<&str> = e
            .types
            .iter()
            .filter(|t| pick(t))
            .map(|t| t.type_name.as_str())
            .collect();
        if !types.is_empty() {
            plan.options.push(ExceptionFilterOptions {
                filter_id: id.to_owned(),
                condition: Some(types.join(", ")),
            });
        }
    };
    filter(e.break_when_thrown, "all", |t| t.break_when_thrown);
    filter(e.break_when_user_unhandled, "user-unhandled", |t| {
        t.break_when_user_unhandled
    });
    plan
}

/// The exception plan for a session of `family` (brief 0038): vscode-js-debug's `all` and `uncaught` from the two
/// boxes (its filter options take JavaScript conditions, so the .NET exception types are not sent); a browser session
/// gets none; the others the .NET plan above.
pub fn exception_plan_for(family: AdapterFamily, e: &ExceptionSettingsRow) -> ExceptionPlan {
    match family {
        AdapterFamily::Javascript => ExceptionPlan {
            filters: eludite_dap::attach::js_exception_filters(
                e.break_when_thrown,
                e.break_when_user_unhandled,
            ),
            options: Vec::new(),
        },
        AdapterFamily::Browser => ExceptionPlan::default(),
        _ => exception_plan(e),
    }
}

/// The scope the Locals window shows: the first one named `Local` (vscode-js-debug lists `Block: f`, then `Local: f`:
/// the function's variables; brief 0038), else the first that is not expensive, else the first.
pub fn locals_scope(scopes: &[eludite_dap::types::Scope]) -> Option<&eludite_dap::types::Scope> {
    scopes
        .iter()
        .find(|s| !s.expensive && s.name.starts_with("Local"))
        .or_else(|| scopes.iter().find(|s| !s.expensive))
}

/// The DAP exception filters without conditions (an adapter without filter options gets these only).
#[cfg_attr(not(test), allow(dead_code))]
pub fn exception_filters(e: &ExceptionSettingsRow) -> Vec<String> {
    exception_plan(e).filters
}

/// The whole debugger state.
#[derive(Debug, Clone)]
pub struct DebugModel {
    pub mode: Mode,
    pub generation: u64,
    pub stop: u64,
    pub session: Option<SessionRow>,
    pub stopped: Option<StoppedRow>,
    pub threads: Vec<ThreadRow>,
    pub thread: Option<i64>,
    pub frames: Vec<Frame>,
    pub frame: usize,
    pub locals: Vec<VarNode>,
    pub locals_loading: bool,
    pub watches: Vec<VarNode>,
    pub breakpoints: Breakpoints,
    pub exceptions: ExceptionSettingsRow,
    pub console: std::collections::VecDeque<String>,
    pub console_total: u64,
    pub last_driver: Option<String>,
    pub message: Option<String>,
    /// The project Set as Startup Project chose (brief 0020): an absolute project file path.
    pub startup_project: Option<String>,
    /// The multiple startup projects (brief 0028): absolute project file paths with their actions; empty when one
    /// startup project (or none) is set.
    pub startup_projects: Vec<(String, StartupAction)>,
    /// How many frames the selected thread's stack has (the adapter's `totalFrames`, or the frames read).
    pub frames_total: usize,
    /// The selected frame's locals: the scope's variables reference and how many top-level rows it has.
    pub locals_reference: i64,
    pub locals_total: usize,
    /// At an exception stop: the adapter's `exceptionInfo` answer, and whether it is still awaited.
    pub exception_info: Option<eludite_dap::types::ExceptionInfoResponse>,
    pub exception_loading: bool,
    /// The program's output, the debugger's messages and the adapter's (brief 0025), per session.
    pub outputs: [OutputRing; 3],
    /// The program's exit code, once the adapter reported it.
    pub exit_code: Option<i64>,
    /// What the session's adapter supports, once its handshake ended.
    pub capabilities: Option<CapabilitiesRow>,
    /// Agents may drive this session (Debug > Allow Agents to Drive; brief 0027).
    pub agents_allowed: bool,
    /// What each new session starts with (the setting `debugger.allowAgentsByDefault`).
    pub agents_default: bool,
    /// Chosen while no session ran: the next session starts with it.
    pub agents_next: Option<bool>,
    /// The session's failed breakpoints when it ended, for the end-of-session summary (brief 0036).
    pub ended_failed: Vec<FailedBreakpointRow>,
    /// `run_until`'s points removed at the last stop or at the end, with their binding then: (path, line, bound, the
    /// adapter's message), for its `points_failed` (brief 0036).
    pub removed_points: Vec<(String, u32, bool, Option<String>)>,
    /// The launch's browser step (brief 0037): the readiness watch the session's program output feeds; cancelled when
    /// the session ends.
    pub browser_watch: Option<std::sync::Arc<eludite_dap::launch::ServerWatch>>,
    /// What the session's launch opens, as its launch thread planned it (a restart through the adapter's `restart`
    /// runs the step again with it).
    pub browser_plan: Option<eludite_dap::launch::BrowserLaunch>,
    /// The start's `browser`, kept for Restart (brief 0037).
    pub browser_choice: Option<eludite_commands::debug::BrowserChoice>,
    /// The tab the session's page opened in: a restart navigates it instead of opening another.
    pub browser_reuse: Option<String>,
    /// From Kestrel's listening line to the page opened (brief 0037's budget).
    pub browser_latency: Option<std::time::Duration>,
    /// The versions of the session's adapter and its runtime (brief 0038: `vscode-js-debug 1.140.0, node v22.12.0`).
    pub adapter_version: Option<String>,
    /// A server session's page to debug once it opens (brief 0038): the start's `browser` entry, or the implied one of
    /// `browser: built_in` with the setting debugger.attachBrowser on.
    pub browser_entry: Option<eludite_commands::debug::BrowserEntry>,
    /// What came of that attach.
    pub browser_attach: Option<BrowserAttach>,
    /// A browser session started for a server session's page: that session (its answer and message say how it went).
    pub attached_for: Option<u32>,
    /// Stop on a browser session waits for its children to end before it detaches the session itself
    /// (vscode-js-debug answers the parent's `disconnect` once its children are gone; brief 0038).
    pub detach_after_children: bool,
}

/// A server session's browser attach (brief 0038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BrowserAttach {
    /// The browser session (it attaches, then runs).
    Session(u32),
    /// Why it could not attach (no Node.js, no vscode-js-debug, no tab).
    Failed(String),
}

impl Default for DebugModel {
    fn default() -> Self {
        Self {
            mode: Mode::Design,
            generation: 0,
            stop: 0,
            session: None,
            stopped: None,
            threads: Vec::new(),
            thread: None,
            frames: Vec::new(),
            frame: 0,
            locals: Vec::new(),
            locals_loading: false,
            watches: Vec::new(),
            breakpoints: Breakpoints::default(),
            exceptions: ExceptionSettingsRow::default(),
            console: Default::default(),
            console_total: 0,
            last_driver: None,
            message: None,
            startup_project: None,
            startup_projects: Vec::new(),
            frames_total: 0,
            locals_reference: 0,
            locals_total: 0,
            exception_info: None,
            exception_loading: false,
            outputs: Default::default(),
            exit_code: None,
            capabilities: None,
            agents_allowed: true,
            agents_default: true,
            agents_next: None,
            ended_failed: Vec::new(),
            removed_points: Vec::new(),
            browser_watch: None,
            browser_plan: None,
            browser_choice: None,
            browser_reuse: None,
            browser_latency: None,
            adapter_version: None,
            browser_entry: None,
            browser_attach: None,
            attached_for: None,
            detach_after_children: false,
        }
    }
}

fn refused(message: String) -> CommandError {
    CommandError::Failed(message)
}

impl DebugModel {
    /// Rules 2 and 3 of the module docs: is `request` allowed now?
    pub fn check(&self, request: &DebugRequest) -> Result<(), CommandError> {
        let mode = self.mode.as_str();
        let (g, s) = (self.generation, self.stop);
        let needs_break = |verb: &str, stop: Option<u64>| -> Result<(), CommandError> {
            // The stop the caller saw is over once another one came or the debuggee resumed.
            if let Some(seen) = stop
                && (seen != s || self.mode != Mode::Break)
            {
                return Err(refused(format!(
                    "stale: {verb} was issued against stop {seen}, but the debuggee has moved on (now stop {s}, \
                     {mode}, generation {g}); read eludite.debug.state and decide again"
                )));
            }
            if self.mode != Mode::Break {
                return Err(refused(format!(
                    "cannot {verb}: the debuggee is not in break mode (it is {mode}, generation {g}, stop {s}); \
                     commands are refused, not queued, until it breaks"
                )));
            }
            Ok(())
        };
        match request {
            DebugRequest::Start { .. } if self.mode != Mode::Design => Err(refused(format!(
                "a session is already {mode} (generation {g}); stop it (eludite.debug.stop) or resume it \
                 (eludite.debug.continue)"
            ))),
            DebugRequest::Stop if self.mode == Mode::Design => {
                Err(refused("there is no debugging session to stop".into()))
            }
            DebugRequest::Stop if self.mode == Mode::Stopping => Err(refused(format!(
                "the session is already stopping (generation {g})"
            ))),
            DebugRequest::Continue { stop, .. } => needs_break("continue", *stop),
            DebugRequest::Step { kind, stop, .. } => needs_break(
                match kind {
                    eludite_commands::debug::StepKind::Over => "step over",
                    eludite_commands::debug::StepKind::Into => "step into",
                    eludite_commands::debug::StepKind::Out => "step out",
                },
                *stop,
            ),
            DebugRequest::RunToCursor { stop, .. } => needs_break("run to cursor", *stop),
            DebugRequest::Evaluate { stop, .. } => needs_break("evaluate", *stop),
            DebugRequest::SelectFrame { stop, .. } => needs_break("select a frame", *stop),
            DebugRequest::Stack { stop, .. } => needs_break("read the call stack", *stop),
            DebugRequest::Variables { stop, .. } => needs_break("read variables", *stop),
            DebugRequest::ExceptionInfo { stop, .. } => {
                needs_break("read the exception", *stop)?;
                match &self.stopped {
                    Some(s) if s.reason == "exception" => Ok(()),
                    Some(s) => Err(refused(format!(
                        "the debuggee stopped for `{}`, not an exception (stop {s_}); there is no exception to read",
                        s.reason,
                        s_ = self.stop
                    ))),
                    None => Err(refused("the debuggee has not stopped".into())),
                }
            }
            DebugRequest::Snapshot { .. } => match self.mode {
                Mode::Break | Mode::Running | Mode::RunningWithoutDebugging => Ok(()),
                _ => Err(refused(format!(
                    "cannot take a snapshot: the debuggee is not in break mode or running (it is {mode}, \
                     generation {g}, stop {s}); eludite.debug.wait waits for it"
                ))),
            },
            DebugRequest::RunUntil { stop, .. } => needs_break("run until", *stop),
            DebugRequest::Trace {
                run: TraceRun::Continue,
                stop,
                ..
            } => needs_break("trace", *stop),
            DebugRequest::Trace {
                run: TraceRun::Start,
                ..
            } if self.mode != Mode::Design => Err(refused(format!(
                "cannot trace with `run: start`: a session is already {mode} (generation {g}); use `run: continue` \
                 from break mode, or stop it first"
            ))),
            DebugRequest::SetVariable { stop, .. } => needs_break("set a value", *stop),
            DebugRequest::SetNextStatement { stop, .. } => {
                needs_break("set the next statement", *stop)
            }
            DebugRequest::Attach {
                target: eludite_commands::debug::AttachTarget::Dialog,
                ..
            } => Ok(()),
            DebugRequest::Attach { .. }
                if !matches!(self.mode, Mode::Design | Mode::RunningWithoutDebugging) =>
            {
                Err(refused(format!(
                    "a debugging session is already {mode} (generation {g}); stop it (eludite.debug.stop) before \
                     attaching to another process"
                )))
            }
            DebugRequest::Restart { .. } if self.mode == Mode::Design => Err(refused(
                "there is no debugging session to restart; start one with eludite.debug.start".into(),
            )),
            DebugRequest::Restart { .. } if self.attached() => Err(refused(
                "Restart is not available for an attached session (Visual Studio disables it too): detach with \
                 eludite.debug.stop and attach again"
                    .into(),
            )),
            DebugRequest::Restart { .. } if matches!(self.mode, Mode::Stopping | Mode::Building) => {
                Err(refused(format!(
                    "the session is {mode} (generation {g}); restart it once it has started or ended"
                )))
            }
            DebugRequest::Pause { .. } if self.mode != Mode::Running => Err(refused(format!(
                "cannot break all: the debuggee is not running (it is {mode}, generation {g}, stop {s}); Break All \
                 needs a running debuggee"
            ))),
            _ => Ok(()),
        }
    }

    /// The last command that started, resumed or paused the debuggee came from an agent.
    pub fn agent_driving(&self) -> bool {
        self.last_driver
            .as_deref()
            .is_some_and(|d| d.starts_with("agent:"))
    }

    pub fn output(&self, kind: OutputKind) -> &OutputRing {
        &self.outputs[kind.index()]
    }

    pub fn output_mut(&mut self, kind: OutputKind) -> &mut OutputRing {
        &mut self.outputs[kind.index()]
    }

    /// Done moving: an agent waiting on a resume can answer (no session, a program run without debugging, or a
    /// break with its locals loaded).
    pub fn settled(&self) -> bool {
        match self.mode {
            Mode::Design | Mode::RunningWithoutDebugging => true,
            Mode::Break => {
                !self.locals_loading
                    && !self.exception_loading
                    && !self.watches.iter().any(|w| w.loading)
            }
            _ => false,
        }
    }

    /// Leave break mode (a resume): the stack, locals and watch values belong to the stop that ended.
    pub fn resume(&mut self, driver: &str) {
        self.mode = Mode::Running;
        self.last_driver = Some(driver.to_owned());
        self.clear_break();
    }

    fn clear_break(&mut self) {
        self.stopped = None;
        self.frames.clear();
        self.frames_total = 0;
        self.frame = 0;
        self.locals.clear();
        self.locals_loading = false;
        self.locals_reference = 0;
        self.locals_total = 0;
        self.exception_info = None;
        self.exception_loading = false;
        for w in &mut self.watches {
            *w = VarNode::watch(&w.name);
        }
    }

    /// A new session: generation up, the session state cleared.
    pub fn begin(&mut self, mode: Mode, driver: &str) {
        self.generation += 1;
        self.mode = mode;
        self.session = None;
        self.threads.clear();
        self.thread = None;
        self.message = None;
        self.last_driver = Some(driver.to_owned());
        self.breakpoints.reset_session();
        self.clear_break();
        for r in &mut self.outputs {
            r.clear();
        }
        self.exit_code = None;
        self.capabilities = None;
        self.ended_failed.clear();
        self.removed_points.clear();
        self.adapter_version = None;
        self.browser_attach = None;
        self.detach_after_children = false;
        self.agents_allowed = self.agents_next.take().unwrap_or(self.agents_default);
    }

    /// Which files' breakpoints the session's adapter gets (brief 0038).
    pub fn family(&self) -> eludite_dap::session::AdapterFamily {
        let s = self.session.as_ref();
        eludite_dap::session::AdapterFamily::of_runtime(
            s.and_then(|s| s.runtime.as_deref()),
            s.is_some_and(|s| s.parent.is_some()),
        )
    }

    /// Whether the session attached to a running process.
    pub fn attached(&self) -> bool {
        self.session.as_ref().is_some_and(|s| s.attached)
    }

    /// The session ended.
    pub fn end(&mut self) {
        for r in &mut self.outputs {
            r.flush();
        }
        self.mode = Mode::Design;
        self.threads.clear();
        self.thread = None;
        self.clear_break();
        // What failed stays in the end-of-session summary (brief 0036); the bindings go.
        self.ended_failed = self.breakpoints.failed();
        self.breakpoints.unbind();
    }

    pub fn push_console(&mut self, line: impl Into<String>) {
        if self.console.len() == CONSOLE_LINES {
            self.console.pop_front();
        }
        self.console.push_back(line.into());
        self.console_total += 1;
    }

    /// `eludite.debug.state`.
    pub fn state(&self) -> DebugState {
        let tail_from = self.console.len().saturating_sub(20);
        DebugState {
            mode: self.mode.as_str().into(),
            generation: self.generation,
            stop: self.stop,
            session: self.session.clone(),
            stopped: self.stopped.clone(),
            threads: self.threads.clone(),
            thread: self.thread,
            frames: self
                .frames
                .iter()
                .take(200)
                .map(|f| f.row.clone())
                .collect(),
            frame: (!self.frames.is_empty()).then_some(self.frame),
            locals: self
                .locals
                .iter()
                .take(MAX_VARIABLES)
                .map(VarNode::row)
                .collect(),
            watches: self
                .watches
                .iter()
                .map(|w| WatchRow {
                    expression: w.name.clone(),
                    value: (!w.error && !w.value.is_empty()).then(|| w.value.clone()),
                    type_name: w.type_name.clone(),
                    reference: w.reference,
                    error: w.error.then(|| w.value.clone()),
                })
                .collect(),
            breakpoints: self.breakpoints.rows(),
            exceptions: self.exceptions.clone(),
            console: ConsoleRow {
                lines: self.console_total,
                tail: self.console.iter().skip(tail_from).cloned().collect(),
                next: self.output(OutputKind::Program).next(),
            },
            last_driver: self.last_driver.clone(),
            message: self.message.clone(),
            capabilities: self.capabilities.clone(),
            agent_driving: self.agent_driving(),
            agents_allowed: self.agents_allowed,
            sessions: Vec::new(),
        }
    }

    /// The stop's `stopped` block, its location the top of `top` (the stopped thread's frames).
    pub fn summary_stopped(&self, top: Option<&StackFrameRow>) -> Option<SummaryStopped> {
        let s = self.stopped.as_ref()?;
        let location = top.map(|f| LocationRow {
            path: f.path.clone(),
            line: f.line,
            column: f.column,
            end_line: f.end_line,
            end_column: f.end_column,
            function: f.name.clone(),
        });
        let exception = (s.reason == "exception").then(|| {
            let e = s.exception.clone().unwrap_or_default();
            ExceptionBrief {
                type_name: e.id,
                message: e.description.or_else(|| s.description.clone()),
                break_mode: e.break_mode,
            }
        });
        let breakpoint = (s.reason == "breakpoint")
            .then(|| {
                let f = top?;
                let path = crate::shell::documents::normalize_path(std::path::Path::new(
                    f.path.as_deref()?,
                ))
                .to_string_lossy()
                .into_owned();
                let b = self.breakpoints.at(&path, f.line?)?;
                Some(BreakpointBrief {
                    path: b.path.clone(),
                    line: b.line,
                    hits: b.hits,
                })
            })
            .flatten();
        Some(SummaryStopped {
            reason: s.reason.clone(),
            thread: s.thread,
            location,
            exception,
            breakpoint,
            driver: s.driver.clone(),
        })
    }

    /// The watches as the stop summary lists them.
    pub fn summary_watches(&self, max_chars: usize) -> Vec<SummaryWatch> {
        self.watches
            .iter()
            .map(|w| {
                let (value, value_truncated) = if !w.error && !w.value.is_empty() {
                    let (v, cut) = cut_value(&w.value, max_chars);
                    (Some(v), cut)
                } else {
                    (None, false)
                };
                SummaryWatch {
                    expression: w.name.clone(),
                    value,
                    type_name: w.type_name.clone(),
                    reference: w.reference,
                    error: w.error.then(|| w.value.clone()),
                    value_truncated,
                }
            })
            .collect()
    }

    /// Everything of the stop summary but the break's frames and locals: the mode, the output, the end of the session,
    /// the capabilities and who drives.
    pub fn summary_base(&self, budget: &Budget) -> StopSummary {
        let output = self
            .output(OutputKind::Program)
            .block(budget.output_since, budget.max_output_lines);
        let ended = self.mode == Mode::Design && self.generation > 0;
        StopSummary {
            session: None,
            browser: self.session.as_ref().and_then(|s| s.browser.clone()),
            sessions: Vec::new(),
            mode: self.mode.as_str().into(),
            generation: self.generation,
            stop: self.stop,
            stopped: None,
            frames: None,
            locals: None,
            watches: None,
            truncated: output.truncated,
            output,
            exit_code: self.exit_code.filter(|_| ended),
            message: if ended {
                Some(
                    self.message
                        .clone()
                        .unwrap_or_else(|| match self.exit_code {
                            Some(c) => {
                                format!("The session ended: the program exited with code {c}.")
                            }
                            None => "The session ended.".into(),
                        }),
                )
            } else if self.generation == 0 {
                Some("There is no debugging session.".into())
            } else {
                self.message.clone()
            },
            capabilities: self.capabilities.clone(),
            agent_driving: self.agent_driving(),
            satisfied: None,
            timed_out: None,
            interrupted_by: None,
            breakpoints_failed: if ended {
                self.ended_failed.clone()
            } else if self.generation > 0 && self.mode != Mode::Design {
                self.breakpoints.failed()
            } else {
                Vec::new()
            },
            points_failed: Vec::new(),
        }
    }

    /// The stop summary as far as the model knows it: the selected thread's frames and the selected frame's locals
    /// with the members the Locals window has loaded (what the UI thread answers; agents' answers read the adapter).
    pub fn summary(&self, budget: &Budget) -> StopSummary {
        let mut out = self.summary_base(budget);
        if self.mode != Mode::Break {
            return out;
        }
        let rows: Vec<StackFrameRow> = self
            .frames
            .iter()
            .take(budget.max_frames)
            .map(Frame::stack_row)
            .collect();
        let thread = self.thread.unwrap_or_default();
        let total = self.frames_total.max(self.frames.len());
        let stopped_thread = self.stopped.as_ref().map(|s| s.thread);
        let top = (stopped_thread == self.thread)
            .then(|| self.frames.first().map(Frame::stack_row))
            .flatten();
        out.stopped = self.summary_stopped(top.as_ref());
        let frames = FramesBlock {
            thread,
            truncated: rows.len() < total,
            rows,
            total,
        };
        let mut left = budget.max_variables;
        let mut cut = false;
        let rows = model_rows(
            &self.locals,
            budget.depth,
            &mut left,
            budget.max_value_chars,
            &mut cut,
        );
        let total = self.locals_total.max(self.locals.len());
        let next = (rows.len() < total).then_some(rows.len());
        let locals = LocalsBlock {
            thread,
            frame: self.frame,
            truncated: cut || next.is_some(),
            rows,
            total,
            next,
        };
        out.truncated |= frames.truncated || locals.truncated;
        out.frames = Some(frames);
        out.locals = Some(locals);
        out.watches = Some(self.summary_watches(budget.max_value_chars));
        out
    }

    /// What persists.
    pub fn persisted(&self) -> Persisted {
        Persisted {
            version: PERSISTED_VERSION,
            breakpoints: self.breakpoints.to_persisted(),
            exceptions: Some(self.exceptions.clone()),
            watches: self.watches.iter().map(|w| w.name.clone()).collect(),
            startup_project: self.startup_project.clone(),
            startup_projects: self
                .startup_projects
                .iter()
                .map(|(path, action)| PersistedStartup {
                    path: path.clone(),
                    action: *action,
                })
                .collect(),
        }
    }

    /// Load what persisted for a solution (replacing the breakpoints, settings and watches).
    pub fn restore(&mut self, p: &Persisted) {
        let current = self.breakpoints.current;
        self.breakpoints = Breakpoints::from_persisted(&p.breakpoints);
        self.breakpoints.current = current;
        self.exceptions = p.exceptions.clone().unwrap_or_default();
        self.watches = p.watches.iter().map(|w| VarNode::watch(w)).collect();
        self.startup_project = p.startup_project.clone();
        self.startup_projects = p
            .startup_projects
            .iter()
            .filter(|s| !s.path.is_empty())
            .map(|s| (s.path.clone(), s.action))
            .collect();
    }

    /// The projects F5 starts, with their actions (brief 0028): the multiple startup projects that start, else the
    /// one startup project (`None`: the default one).
    pub fn startup_set(&self) -> Vec<(String, StartupAction)> {
        if self.startup_projects.is_empty() {
            return self
                .startup_project
                .iter()
                .map(|p| (p.clone(), StartupAction::Start))
                .collect();
        }
        self.startup_projects
            .iter()
            .filter(|(_, a)| *a != StartupAction::None)
            .cloned()
            .collect()
    }
}

/// `nodes` as rows within `left` rows (counted down), breadth-first to `depth` levels, using the members the model
/// has loaded; `cut` is set when something was left out for the budget.
pub fn model_rows(
    nodes: &[VarNode],
    depth: usize,
    left: &mut usize,
    max_chars: usize,
    cut: &mut bool,
) -> Vec<VarRow> {
    let take = nodes.len().min(*left);
    *cut |= take < nodes.len();
    *left -= take;
    let mut rows: Vec<VarRow> = nodes[..take].iter().map(|n| n.var_row(max_chars)).collect();
    let mut frontier: Vec<(Vec<usize>, &VarNode)> = nodes[..take]
        .iter()
        .enumerate()
        .map(|(i, n)| (vec![i], n))
        .collect();
    for _ in 1..depth {
        let mut next = Vec::new();
        for (path, node) in frontier {
            let Some(children) = node.children.as_deref() else {
                continue;
            };
            let row = row_at_mut(&mut rows, &path).expect("a row of the frontier");
            let n = children.len().min(*left);
            *left -= n;
            if n < children.len() {
                row.truncated = true;
                *cut = true;
            }
            if n == 0 && !children.is_empty() {
                continue;
            }
            row.children = Some(children[..n].iter().map(|c| c.var_row(max_chars)).collect());
            for (i, c) in children[..n].iter().enumerate() {
                let mut p = path.clone();
                p.push(i);
                next.push((p, c));
            }
        }
        frontier = next;
    }
    rows
}

/// The row at `path` (indices from the top, through `children`).
pub fn row_at_mut<'a>(rows: &'a mut [VarRow], path: &[usize]) -> Option<&'a mut VarRow> {
    let (first, rest) = path.split_first()?;
    let row = rows.get_mut(*first)?;
    if rest.is_empty() {
        Some(row)
    } else {
        row_at_mut(row.children.as_deref_mut()?, rest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eludite_commands::debug::{EvalContext, StepKind};

    fn step(stop: Option<u64>) -> DebugRequest {
        DebugRequest::Step {
            kind: StepKind::Over,
            thread: None,
            stop,
            wait_ms: None,
            budget: Default::default(),
        }
    }

    #[test]
    fn two_drivers_are_refused_not_queued() {
        let mut m = DebugModel::default();
        // Design: nothing to step, stop or evaluate; starting is fine.
        let e = m.check(&step(None)).unwrap_err().to_string();
        assert!(
            e.contains("not in break mode") && e.contains("design"),
            "{e}"
        );
        assert!(m.check(&DebugRequest::Stop).is_err());
        assert!(
            m.check(&DebugRequest::Start {
                compound: None,
                browsers: Vec::new(),
                project: None,
                debug: true,
                profile: None,
                framework: None,
                build: None,
                cargo: Default::default(),
                browser: None,
                wait_ms: None,
                budget: Default::default()
            })
            .is_ok()
        );
        m.begin(Mode::Launching, "user");
        assert_eq!(m.generation, 1);
        assert!(
            m.check(&DebugRequest::Start {
                compound: None,
                browsers: Vec::new(),
                project: None,
                debug: true,
                profile: None,
                framework: None,
                build: None,
                cargo: Default::default(),
                browser: None,
                wait_ms: None,
                budget: Default::default()
            })
            .unwrap_err()
            .to_string()
            .contains("already launching")
        );
        // A break: stop 1. Both drivers may step...
        m.mode = Mode::Break;
        m.stop = 1;
        assert!(m.check(&step(None)).is_ok());
        assert!(m.check(&step(Some(1))).is_ok());
        // ...the first one's step is applied: the model runs at once, and the second driver's step is refused.
        m.resume("user");
        let e = m.check(&step(None)).unwrap_err().to_string();
        assert!(
            e.contains("running") && e.contains("refused, not queued"),
            "{e}"
        );
        // Quoting the stop it saw, it learns the stop is over.
        let e = m.check(&step(Some(1))).unwrap_err().to_string();
        assert!(e.contains("stale:") && e.contains("running"), "{e}");
        let e = m
            .check(&DebugRequest::Evaluate {
                expression: "x".into(),
                frame: None,
                context: EvalContext::Watch,
                expand: false,
                stop: None,
            })
            .unwrap_err()
            .to_string();
        assert!(e.contains("cannot evaluate"), "{e}");
        // The next break is stop 2: a command quoting stop 1 is stale even in break mode.
        m.mode = Mode::Break;
        m.stop = 2;
        let e = m.check(&step(Some(1))).unwrap_err().to_string();
        assert!(e.contains("stale:") && e.contains("now stop 2"), "{e}");
        assert!(m.check(&step(Some(2))).is_ok());
        // Editing and reading are always allowed.
        m.resume("agent:Claude Code");
        assert!(m.check(&DebugRequest::State).is_ok());
        assert!(m.check(&DebugRequest::AddWatch("x".into())).is_ok());
        assert_eq!(m.state().last_driver.as_deref(), Some("agent:Claude Code"));
        assert!(!m.settled());
        m.mode = Mode::Break;
        m.locals_loading = true;
        assert!(!m.settled());
        m.locals_loading = false;
        assert!(m.settled());
    }

    #[test]
    fn breakpoints_toggle_persist_follow_edits_and_bind() {
        let mut b = Breakpoints::default();
        assert!(b.toggle("/s/A.cs", 5));
        assert!(b.toggle("/s/A.cs", 2));
        assert!(b.toggle("/s/B.cs", 1));
        assert!(!b.toggle("/s/A.cs", 5));
        assert!(b.toggle("/s/A.cs", 9));
        {
            let x = b.ensure("/s/A.cs", 9);
            x.condition = Some("i > 3".into());
            x.hit_condition = HitCondition::parse(">=2");
        }
        b.ensure("/s/A.cs", 2).enabled = false;
        assert_eq!(
            b.glyphs("/s/A.cs", false),
            [
                (1, BreakpointGlyph::Disabled),
                (8, BreakpointGlyph::Conditional)
            ]
        );
        // Unbound while a session has not bound it.
        b.ensure("/s/A.cs", 2).enabled = true;
        assert_eq!(b.glyphs("/s/A.cs", true)[0], (1, BreakpointGlyph::Unbound));
        // setBreakpoints: enabled ones in line order; hit conditions only if the adapter supports them.
        let (lines, sent) = b.source_breakpoints("/s/A.cs", false, false, Some(4));
        assert_eq!(lines, [2, 9, 4]);
        assert_eq!(sent[1].condition.as_deref(), Some("i > 3"));
        assert_eq!(sent[1].hit_condition, None);
        let (_, sent) = b.source_breakpoints("/s/A.cs", true, false, None);
        assert_eq!(sent[1].hit_condition.as_deref(), Some(">=2"));
        let answer = vec![
            eludite_dap::types::Breakpoint {
                id: Some(7),
                verified: false,
                message: Some("pending".into()),
                ..Default::default()
            },
            eludite_dap::types::Breakpoint {
                id: Some(8),
                verified: true,
                ..Default::default()
            },
        ];
        b.apply_answer("/s/A.cs", &lines, &answer);
        assert!(!b.at("/s/A.cs", 2).unwrap().verified);
        assert!(b.at("/s/A.cs", 9).unwrap().verified);
        assert!(b.apply_event(&eludite_dap::types::Breakpoint {
            id: Some(7),
            verified: true,
            ..Default::default()
        }));
        assert!(b.at("/s/A.cs", 2).unwrap().verified);
        // Lines inserted above move them.
        assert!(b.moved("/s/A.cs", &[4, 11]));
        assert_eq!(
            b.all().iter().map(|x| x.line).collect::<Vec<_>>(),
            [4, 11, 1]
        );
        assert!(!b.moved("/s/A.cs", &[4, 11]));
        assert!(!b.moved("/s/A.cs", &[4]));
        // Round trip through the persisted form keeps what persists and drops the session's state.
        let p = b.to_persisted();
        let back = Breakpoints::from_persisted(&p);
        assert_eq!(back.to_persisted(), p);
        assert!(back.all().iter().all(|x| !x.verified && x.hits == 0));
        assert_eq!(
            back.at("/s/A.cs", 11).unwrap().hit_condition,
            Some(HitCondition::AtLeast(2))
        );
        assert_eq!(b.delete_all(), ["/s/A.cs", "/s/B.cs"]);
        assert!(b.all().is_empty());
    }

    #[test]
    fn variables_flatten_by_expansion() {
        let mut nodes = vec![
            VarNode {
                name: "order".into(),
                value: "{Order}".into(),
                reference: 3,
                ..Default::default()
            },
            VarNode {
                name: "x".into(),
                value: "1".into(),
                ..Default::default()
            },
        ];
        assert_eq!(flatten(&nodes).len(), 2);
        assert_eq!(flatten(&nodes)[0].expanded, Some(false));
        assert_eq!(flatten(&nodes)[1].expanded, None);
        let n = node_mut(&mut nodes, &[0]).unwrap();
        n.expanded = true;
        n.children = Some(vec![VarNode {
            name: "Id".into(),
            value: "7".into(),
            ..Default::default()
        }]);
        let rows = flatten(&nodes);
        assert_eq!(
            rows.iter()
                .map(|r| (r.depth, r.name.as_str()))
                .collect::<Vec<_>>(),
            [(0, "order"), (1, "Id"), (0, "x")]
        );
        assert_eq!(rows[1].path, [0, 0]);
        assert_eq!(node_mut(&mut nodes, &[0, 0]).unwrap().value, "7");
        assert!(node_mut(&mut nodes, &[1, 0]).is_none());
    }

    #[test]
    fn state_follows_the_schema_and_persists() {
        let mut m = DebugModel::default();
        m.breakpoints.toggle("/s/A.cs", 3);
        m.watches.push(VarNode::watch("order.Name"));
        m.exceptions.break_when_thrown = true;
        for i in 0..30 {
            m.push_console(format!("line {i}"));
        }
        let s = m.state();
        assert_eq!(s.mode, "design");
        assert_eq!(s.console.lines, 30);
        assert_eq!(s.console.tail.len(), 20);
        assert_eq!(s.console.tail[19], "line 29");
        let v = serde_json::to_value(&s).unwrap();
        let schema: serde_json::Value = serde_json::from_str(include_str!(
            "../../../../../protocol/schemas/debug-state.output.json"
        ))
        .unwrap();
        for k in v.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let p = m.persisted();
        let text = serde_json::to_string(&p).unwrap();
        let mut m2 = DebugModel::default();
        m2.restore(&serde_json::from_str(&text).unwrap());
        assert_eq!(m2.persisted(), p);
        assert_eq!(exception_filters(&m2.exceptions), ["all", "user-unhandled"]);
    }

    #[test]
    fn output_rings_read_by_cursor_and_count_what_they_drop() {
        let mut r = OutputRing::default();
        r.push_text("one\ntw", Some("stdout"));
        r.push_text("o\r\nthree", Some("stdout"));
        // A partial line from another stream ends the first.
        r.push_text("err\n", Some("stderr"));
        let (lines, next, dropped, total, more) = r.read(0, 10, None);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["one", "two", "three", "err"]);
        assert_eq!(lines[3].stream.as_deref(), Some("stderr"));
        assert_eq!((next, dropped, total, more), (4, 0, 4, false));
        // Pages: forward from a cursor, `more` when lines follow.
        let (lines, next, _, _, more) = r.read(1, 2, None);
        assert_eq!(lines[0].seq, 1);
        assert_eq!((next, more), (3, true));
        // A pattern: the cursor passes the lines it skipped.
        let p = OutputPattern::parse("/^t/").unwrap();
        let (lines, next, _, _, more) = r.read(0, 10, Some(&p));
        assert_eq!(lines.iter().map(|l| l.seq).collect::<Vec<_>>(), [1, 2]);
        assert_eq!((next, more), (4, false));
        // The tail.
        let b = r.block(None, 2);
        assert_eq!(b.lines.iter().map(|l| l.seq).collect::<Vec<_>>(), [2, 3]);
        assert_eq!((b.next, b.truncated), (4, false));
        // Overflow: the oldest lines go, and a cursor before them hears how many.
        for i in 0..OUTPUT_RING_LINES {
            r.push_line(format!("l{i}"), None);
        }
        let (lines, next, dropped, total, more) = r.read(0, 3, None);
        assert_eq!(dropped, 4);
        assert_eq!(lines[0].seq, 4);
        assert_eq!(lines[0].text, "l0");
        assert_eq!((next, total, more), (7, OUTPUT_RING_LINES as u64 + 4, true));
        // A cursor from an older session (past the end) reads from the start.
        let mut fresh = OutputRing::default();
        fresh.push_line("a", None);
        assert_eq!(fresh.read(500, 10, None).0.len(), 1);
        // The session's end keeps a line without its newline.
        fresh.push_text("half", None);
        assert_eq!(fresh.next(), 1);
        fresh.flush();
        assert_eq!(fresh.read(1, 10, None).0[0].text, "half");
    }

    #[test]
    fn inspection_commands_are_refused_by_mode_and_rows_follow_the_budget() {
        use eludite_commands::debug::{OutputKind, ScopeKind, VariablesTarget, WaitUntil};
        let mut m = DebugModel::default();
        let b = Budget::default();
        let snapshot = DebugRequest::Snapshot {
            thread: None,
            frame: None,
            budget: b,
        };
        let pause = DebugRequest::Pause {
            thread: None,
            wait_ms: None,
            budget: b,
        };
        let vars = DebugRequest::Variables {
            target: VariablesTarget::Frame {
                thread: None,
                frame: None,
                scope: ScopeKind::Locals,
            },
            start: 0,
            count: 50,
            depth: 1,
            filter: None,
            max_value_chars: 200,
            stop: Some(1),
        };
        let wait = DebugRequest::Wait {
            until: WaitUntil::Any,
            wait_ms: 10,
            stop: None,
            budget: b,
        };
        let output = DebugRequest::Output {
            source: OutputKind::Program,
            since: 0,
            max_lines: 20,
            pattern: None,
        };
        let info = DebugRequest::ExceptionInfo {
            thread: None,
            stop: None,
        };
        // Design: snapshot and pause are refused; wait and output never are.
        assert!(
            m.check(&snapshot)
                .unwrap_err()
                .to_string()
                .contains("snapshot")
        );
        assert!(
            m.check(&pause)
                .unwrap_err()
                .to_string()
                .contains("cannot break all")
        );
        assert!(m.check(&wait).is_ok() && m.check(&output).is_ok());
        // Running: snapshot (the cheap poll) and pause.
        m.begin(Mode::Running, "agent:A");
        assert!(m.agent_driving());
        assert!(m.check(&snapshot).is_ok() && m.check(&pause).is_ok());
        assert!(m.check(&vars).unwrap_err().to_string().contains("stale"));
        // Break: pause is refused; variables quoting an older stop are stale; exception_info needs an exception.
        m.mode = Mode::Break;
        m.stop = 2;
        m.stopped = Some(StoppedRow {
            reason: "breakpoint".into(),
            thread: 1,
            ..Default::default()
        });
        assert!(
            m.check(&pause)
                .unwrap_err()
                .to_string()
                .contains("not running")
        );
        assert!(m.check(&vars).unwrap_err().to_string().contains("stale"));
        let e = m.check(&info).unwrap_err().to_string();
        assert!(e.contains("`breakpoint`, not an exception"), "{e}");
        m.stopped.as_mut().unwrap().reason = "exception".into();
        assert!(m.check(&info).is_ok());
        m.resume("user");
        assert!(!m.agent_driving());
        assert!(!m.state().agent_driving);

        // Rows within the budget, breadth-first over what the model has loaded.
        let node = |name: &str, children: Option<Vec<VarNode>>| VarNode {
            name: name.into(),
            value: "v".repeat(10),
            reference: i64::from(children.is_some()),
            evaluate_name: Some(name.into()),
            children,
            ..Default::default()
        };
        let nodes = vec![
            node(
                "a",
                Some(vec![
                    node("a0", None),
                    node("a1", Some(vec![node("a10", None)])),
                ]),
            ),
            node("b", Some(vec![node("b0", None)])),
            node("c", None),
        ];
        let (mut left, mut cut) = (5, false);
        let rows = model_rows(&nodes, 3, &mut left, 4, &mut cut);
        assert_eq!(rows.len(), 3);
        assert_eq!(
            rows[0].children.as_ref().unwrap().len(),
            2,
            "a's members first"
        );
        assert!(
            rows[1].children.is_none() && rows[1].truncated,
            "the budget ran out"
        );
        assert!(rows[0].children.as_ref().unwrap()[1].truncated);
        assert!(cut && left == 0);
        assert_eq!(rows[2].value, "vvvv\u{2026} (10 chars)");
        assert!(rows[2].value_truncated);
        assert_eq!(rows[2].evaluate_name, None, "the name itself is left out");
    }

    #[test]
    fn tracepoint_messages_parse_match_and_exception_types_plan() {
        assert_eq!(
            parse_message("i={i} {{x}} in $FUNCTION $PID $TIDY"),
            [
                Segment::Text("i=".into()),
                Segment::Expression("i".into()),
                Segment::Text(" {x} in ".into()),
                Segment::Special("$FUNCTION"),
                Segment::Text(" $PID $TIDY".into()),
            ]
        );
        assert!(has_specials("$TNAME: {x}") && !has_specials("x = {x}"));
        // The adapter's lines are told apart from its other console output.
        assert!(message_matches("i={i} total={total}", "i=3 total=6"));
        assert!(message_matches("{i}", "anything"));
        assert!(!message_matches("i={i} total={total}", "Loaded assembly x"));
        assert!(!message_matches("i={i}!", "i=3"));
        assert!(message_matches("done", "done") && !message_matches("done", "done twice"));
        // Exception types: a category box that is on is its filter; off, its checked types are the condition.
        let mut e = ExceptionSettingsRow::default();
        assert_eq!(exception_plan(&e).filters, ["user-unhandled"]);
        e.types = vec![
            ExceptionTypeRow {
                type_name: "A.X".into(),
                break_when_thrown: true,
                break_when_user_unhandled: false,
            },
            ExceptionTypeRow {
                type_name: "A.Y".into(),
                break_when_thrown: true,
                break_when_user_unhandled: true,
            },
        ];
        let plan = exception_plan(&e);
        assert_eq!(plan.filters, ["user-unhandled"]);
        assert_eq!(plan.options.len(), 1);
        assert_eq!(plan.options[0].filter_id, "all");
        assert_eq!(plan.options[0].condition.as_deref(), Some("A.X, A.Y"));
        e.break_when_user_unhandled = false;
        let plan = exception_plan(&e);
        assert!(plan.filters.is_empty());
        assert_eq!(plan.options[1].condition.as_deref(), Some("A.Y"));
        assert_eq!(
            plan.arguments(false),
            serde_json::json!({"filters": []}),
            "an adapter without filter options gets none"
        );
        // A version 1 file reads into version 2's shape.
        let v1: Persisted = serde_json::from_str(
            r#"{"version": 1, "breakpoints": [{"path": "/s/A.cs", "line": 3, "condition": "x"}],
                "exceptions": {"break_when_thrown": true, "break_when_user_unhandled": true}, "watches": []}"#,
        )
        .unwrap();
        let b = Breakpoints::from_persisted(&v1.breakpoints);
        assert_eq!(b.all()[0].condition.as_deref(), Some("x"));
        assert!(b.all()[0].log_message.is_none() && b.functions().is_empty());
        assert!(v1.exceptions.unwrap().types.is_empty());
    }

    /// Brief 0028: a breakpoint's binding per session moves aside when another session is current and back when it
    /// returns; the rows list each live session's binding, `verified` bound in any and `hits` summed; another
    /// session's temporary points are not sent; a session that is gone takes its bindings and points with it.
    #[test]
    fn breakpoint_bindings_are_per_session() {
        let mut b = Breakpoints::default();
        b.switch_session(1);
        b.toggle("/s/a.cs", 6);
        b.toggle("/s/a.cs", 9);
        let bound = |verified: bool| eludite_dap::types::Breakpoint {
            id: Some(10),
            verified,
            line: Some(6),
            ..Default::default()
        };
        b.apply_answer("/s/a.cs", &[6, 9], &[bound(true), bound(false)]);
        b.at_mut("/s/a.cs", 6).unwrap().hits = 2;
        // Session 2 is current: nothing bound there yet.
        b.switch_session(2);
        assert!(!b.at("/s/a.cs", 6).unwrap().verified);
        assert_eq!(b.at("/s/a.cs", 6).unwrap().hits, 0);
        assert!(b.at("/s/a.cs", 6).unwrap().bound(), "bound in session 1");
        b.apply_answer("/s/a.cs", &[6], &[bound(false)]);
        b.at_mut("/s/a.cs", 6).unwrap().hits = 1;
        let rows = b.rows_for(&[(1, AdapterFamily::Other), (2, AdapterFamily::Other)]);
        assert!(rows[0].verified);
        assert_eq!(rows[0].hits, 3);
        assert_eq!(
            rows[0]
                .sessions
                .iter()
                .map(|r| (r.session, r.verified, r.hits))
                .collect::<Vec<_>>(),
            [(1, true, 2), (2, false, 1)]
        );
        assert!(!rows[1].verified);
        // Without a live session the rows are as the current one left them.
        assert!(b.rows_for(&[])[0].sessions.is_empty());
        // Back to session 1: its binding again.
        b.switch_session(1);
        assert!(b.at("/s/a.cs", 6).unwrap().verified);
        assert_eq!(b.at("/s/a.cs", 6).unwrap().hits, 2);
        // A temporary point of session 2 is not session 1's to send.
        b.put(Breakpoint {
            temporary: true,
            owner: Some(2),
            ..Breakpoint::new("/s/a.cs", 12)
        });
        let (lines, _) = b.source_breakpoints("/s/a.cs", false, false, None);
        assert_eq!(lines, [6, 9]);
        b.switch_session(2);
        let (lines, _) = b.source_breakpoints("/s/a.cs", false, false, None);
        assert_eq!(lines, [6, 9, 12]);
        b.switch_session(1);
        b.forget_session(2);
        assert!(
            b.at("/s/a.cs", 12).is_none(),
            "its temporary point went with it"
        );
        assert!(b.at("/s/a.cs", 6).unwrap().bindings.is_empty());
    }

    /// Brief 0028: the persisted file's version 3 keeps the multiple startup projects with their actions; a version 2
    /// file (one `startup_project`) loads as that one startup project and saves as version 3.
    #[test]
    fn multiple_startup_projects_persist_and_a_version_2_file_migrates() {
        let v2: Persisted = serde_json::from_value(serde_json::json!({
            "version": 2,
            "breakpoints": [{"path": "/s/a.cs", "line": 3}],
            "watches": ["x"],
            "startup_project": "/s/Tool/Tool.csproj"
        }))
        .unwrap();
        let mut m = DebugModel::default();
        m.restore(&v2);
        assert_eq!(
            m.startup_set(),
            [("/s/Tool/Tool.csproj".to_owned(), StartupAction::Start)]
        );
        let saved = m.persisted();
        assert_eq!(saved.version, 3);
        assert!(saved.startup_projects.is_empty());
        let text = serde_json::to_string(&saved).unwrap();
        assert!(!text.contains("startup_projects"), "{text}");
        assert!(text.contains("\"startup_project\":\"/s/Tool/Tool.csproj\""));
        // Version 3: the actions round-trip; F5's set leaves out `none`.
        m.startup_projects = vec![
            ("/s/App/App.csproj".into(), StartupAction::Start),
            (
                "/s/Web/Web.csproj".into(),
                StartupAction::StartWithoutDebugging,
            ),
            ("/s/Tool/Tool.csproj".into(), StartupAction::None),
        ];
        m.startup_project = Some("/s/App/App.csproj".into());
        let text = serde_json::to_string(&m.persisted()).unwrap();
        assert!(
            text.contains("\"action\":\"start_without_debugging\""),
            "{text}"
        );
        let back: Persisted = serde_json::from_str(&text).unwrap();
        let mut n = DebugModel::default();
        n.restore(&back);
        assert_eq!(n.startup_projects, m.startup_projects);
        assert_eq!(
            n.startup_set(),
            [
                ("/s/App/App.csproj".to_owned(), StartupAction::Start),
                (
                    "/s/Web/Web.csproj".to_owned(),
                    StartupAction::StartWithoutDebugging
                )
            ]
        );
    }

    /// Brief 0038: a line breakpoint's row lists only the sessions whose adapter takes its file; the exception plan
    /// of a vscode-js-debug session is `all` and `uncaught`, without the .NET exception types; the Locals scope is
    /// js-debug's `Local:` one.
    #[test]
    fn rows_name_only_the_adapters_that_take_the_file_and_js_gets_its_filters() {
        let mut b = Breakpoints::default();
        b.toggle("/w/App/Program.cs", 5);
        b.toggle("/w/wwwroot/app.ts", 25);
        b.toggle("/w/notes.txt", 1);
        let live = [
            (1, AdapterFamily::Dotnet),
            (2, AdapterFamily::Browser),
            (3, AdapterFamily::Javascript),
        ];
        let rows = b.rows_for(&live);
        let sessions = |path: &str| -> Vec<u32> {
            rows.iter()
                .find(|r| r.path.as_deref() == Some(path))
                .unwrap()
                .sessions
                .iter()
                .map(|s| s.session)
                .collect()
        };
        assert_eq!(sessions("/w/App/Program.cs"), [1]);
        assert_eq!(sessions("/w/wwwroot/app.ts"), [3]);
        assert_eq!(sessions("/w/notes.txt"), [1, 3]);
        let e = ExceptionSettingsRow {
            break_when_thrown: false,
            break_when_user_unhandled: true,
            types: vec![ExceptionTypeRow {
                type_name: "System.InvalidOperationException".into(),
                break_when_thrown: true,
                break_when_user_unhandled: false,
            }],
            ..ExceptionSettingsRow::default()
        };
        let js = exception_plan_for(AdapterFamily::Javascript, &e);
        assert_eq!(
            (js.filters.clone(), js.options.len()),
            (vec!["uncaught".to_owned()], 0)
        );
        assert_eq!(
            exception_plan_for(AdapterFamily::Browser, &e),
            ExceptionPlan::default()
        );
        assert_eq!(
            exception_plan_for(AdapterFamily::Dotnet, &e),
            exception_plan(&e)
        );
        let scope = |name: &str, expensive: bool| eludite_dap::types::Scope {
            name: name.into(),
            expensive,
            ..Default::default()
        };
        let js_scopes = [
            scope("Block: total", false),
            scope("Local: total", false),
            scope("Global", true),
        ];
        assert_eq!(locals_scope(&js_scopes).unwrap().name, "Local: total");
        let net = [scope("Locals", false)];
        assert_eq!(locals_scope(&net).unwrap().name, "Locals");
        assert!(locals_scope(&[scope("Global", true)]).is_none());
    }
}
