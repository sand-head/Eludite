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

use eludite_commands::CommandError;
use eludite_commands::debug::{
    BreakpointRow, ConsoleRow, DebugRequest, DebugState, ExceptionSettingsRow, FrameRow,
    HitCondition, SessionRow, StoppedRow, ThreadRow, VariableRow, WatchRow,
};
use eludite_dap::types::SourceBreakpoint;
use eludite_editor::BreakpointGlyph;
use serde::{Deserialize, Serialize};

/// `eludite.debug.state` keeps this many lines of the program's output (the Output window keeps them all).
pub const CONSOLE_LINES: usize = 10_000;
/// Locals and members listed at most (the schema's bound).
pub const MAX_VARIABLES: usize = 500;

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

/// One breakpoint. `line` is 1-based; `path` is the normalized absolute path (the document id).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    pub path: String,
    pub line: u32,
    pub enabled: bool,
    pub condition: Option<String>,
    pub hit_condition: Option<HitCondition>,
    /// The session's: bound by the adapter, times reached, the adapter's message and id.
    pub verified: bool,
    pub hits: u32,
    pub message: Option<String>,
    pub adapter_id: Option<i64>,
}

impl Breakpoint {
    pub fn new(path: &str, line: u32) -> Self {
        Self {
            path: path.to_owned(),
            line,
            enabled: true,
            condition: None,
            hit_condition: None,
            verified: false,
            hits: 0,
            message: None,
            adapter_id: None,
        }
    }

    /// The glyph the margin draws, `in_session` when a debugging session runs.
    pub fn glyph(&self, in_session: bool) -> BreakpointGlyph {
        if !self.enabled {
            BreakpointGlyph::Disabled
        } else if in_session && !self.verified {
            BreakpointGlyph::Unbound
        } else if self.condition.is_some() || self.hit_condition.is_some() {
            BreakpointGlyph::Conditional
        } else {
            BreakpointGlyph::Enabled
        }
    }
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
    #[serde(skip_serializing_if = "Option::is_none")]
    pub startup_project: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PersistedBreakpoint {
    pub path: String,
    pub line: u32,
    #[serde(default = "yes")]
    pub enabled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
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
    /// A watch that failed to evaluate: `value` is the message.
    pub error: bool,
    pub expanded: bool,
    pub loading: bool,
    pub children: Option<Vec<VarNode>>,
}

impl VarNode {
    pub fn from_dap(v: &eludite_dap::types::Variable) -> Self {
        Self {
            name: v.name.clone(),
            value: v.value.clone(),
            type_name: v.type_name.clone().filter(|t| !t.is_empty()),
            reference: v.variables_reference,
            evaluate_name: v.evaluate_name.clone(),
            ..Self::default()
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

/// A frame of the stopped thread's call stack.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Frame {
    /// The adapter's frame id (new at every stop).
    pub id: i64,
    pub row: FrameRow,
}

impl Frame {
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
            },
        }
    }
}

/// The breakpoints, sorted by path and line.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Breakpoints {
    list: Vec<Breakpoint>,
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

    /// Delete every breakpoint; returns the files that had some.
    pub fn delete_all(&mut self) -> Vec<String> {
        let files = self.files();
        self.list.clear();
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
        extra: Option<u32>,
    ) -> (Vec<u32>, Vec<SourceBreakpoint>) {
        let mut lines = Vec::new();
        let mut out = Vec::new();
        for b in self.list.iter().filter(|b| b.path == path && b.enabled) {
            lines.push(b.line);
            out.push(SourceBreakpoint {
                line: i64::from(b.line),
                column: None,
                condition: b.condition.clone(),
                hit_condition: hit_conditions
                    .then(|| b.hit_condition.map(|h| h.to_string()))
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
        match self.list.iter_mut().find(|b| b.adapter_id == Some(id)) {
            Some(b) => {
                b.verified = bp.verified;
                b.message = bp.message.clone().filter(|_| !bp.verified);
                true
            }
            None => false,
        }
    }

    /// Forget the session's state (a new session starts).
    pub fn reset_session(&mut self) {
        for b in &mut self.list {
            b.verified = false;
            b.hits = 0;
            b.message = None;
            b.adapter_id = None;
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

    pub fn to_persisted(&self) -> Vec<PersistedBreakpoint> {
        self.list
            .iter()
            .map(|b| PersistedBreakpoint {
                path: b.path.clone(),
                line: b.line,
                enabled: b.enabled,
                condition: b.condition.clone(),
                hit_condition: b.hit_condition.map(|h| h.to_string()),
            })
            .collect()
    }

    pub fn from_persisted(rows: &[PersistedBreakpoint]) -> Self {
        let mut b = Self {
            list: rows
                .iter()
                .filter(|r| r.line >= 1)
                .map(|r| Breakpoint {
                    enabled: r.enabled,
                    condition: r.condition.clone().filter(|c| !c.trim().is_empty()),
                    hit_condition: r.hit_condition.as_deref().and_then(HitCondition::parse),
                    ..Breakpoint::new(&r.path, r.line)
                })
                .collect(),
        };
        b.sort();
        b.list.dedup_by(|a, b| a.path == b.path && a.line == b.line);
        b
    }

    pub fn rows(&self) -> Vec<BreakpointRow> {
        self.list
            .iter()
            .map(|b| BreakpointRow {
                path: b.path.clone(),
                line: b.line,
                enabled: b.enabled,
                verified: b.verified,
                condition: b.condition.clone(),
                hit_condition: b.hit_condition.map(|h| h.to_string()),
                hits: b.hits,
                message: b.message.clone(),
            })
            .collect()
    }
}

/// The DAP exception filters for these settings (netcoredbg's `all` and `user-unhandled`).
pub fn exception_filters(e: &ExceptionSettingsRow) -> Vec<String> {
    let mut f = Vec::new();
    if e.break_when_thrown {
        f.push("all".to_owned());
    }
    if e.break_when_user_unhandled {
        f.push("user-unhandled".to_owned());
    }
    f
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
            _ => Ok(()),
        }
    }

    /// Done moving: an agent waiting on a resume can answer (no session, a program run without debugging, or a
    /// break with its locals loaded).
    pub fn settled(&self) -> bool {
        match self.mode {
            Mode::Design | Mode::RunningWithoutDebugging => true,
            Mode::Break => !self.locals_loading,
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
        self.frame = 0;
        self.locals.clear();
        self.locals_loading = false;
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
    }

    /// The session ended.
    pub fn end(&mut self) {
        self.mode = Mode::Design;
        self.threads.clear();
        self.thread = None;
        self.clear_break();
        for b in &mut self.breakpoints.list {
            b.verified = false;
            b.adapter_id = None;
            b.message = None;
        }
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
            exceptions: self.exceptions,
            console: ConsoleRow {
                lines: self.console_total,
                tail: self.console.iter().skip(tail_from).cloned().collect(),
            },
            last_driver: self.last_driver.clone(),
            message: self.message.clone(),
        }
    }

    /// What persists.
    pub fn persisted(&self) -> Persisted {
        Persisted {
            version: 1,
            breakpoints: self.breakpoints.to_persisted(),
            exceptions: Some(self.exceptions),
            watches: self.watches.iter().map(|w| w.name.clone()).collect(),
            startup_project: self.startup_project.clone(),
        }
    }

    /// Load what persisted for a solution (replacing the breakpoints, settings and watches).
    pub fn restore(&mut self, p: &Persisted) {
        self.breakpoints = Breakpoints::from_persisted(&p.breakpoints);
        self.exceptions = p.exceptions.unwrap_or_default();
        self.watches = p.watches.iter().map(|w| VarNode::watch(w)).collect();
        self.startup_project = p.startup_project.clone();
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
                project: None,
                debug: true,
                profile: None,
                build: None,
                wait_ms: None
            })
            .is_ok()
        );
        m.begin(Mode::Launching, "user");
        assert_eq!(m.generation, 1);
        assert!(
            m.check(&DebugRequest::Start {
                project: None,
                debug: true,
                profile: None,
                build: None,
                wait_ms: None
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
        let (lines, sent) = b.source_breakpoints("/s/A.cs", false, Some(4));
        assert_eq!(lines, [2, 9, 4]);
        assert_eq!(sent[1].condition.as_deref(), Some("i > 3"));
        assert_eq!(sent[1].hit_condition, None);
        let (_, sent) = b.source_breakpoints("/s/A.cs", true, None);
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
}
