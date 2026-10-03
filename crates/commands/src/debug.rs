//! The debugger's commands (brief 0018): `eludite.debug.start`, `stop`, `continue`, `step_over`, `step_into`,
//! `step_out`, `run_to_cursor`, `toggle_breakpoint`, `evaluate` and `state`, plus `select_frame` (the Call Stack and
//! Threads windows' clicks), `watch` (the Watch window's rows) and `exception_settings` (the Exception Settings
//! window), which every user-visible action needs to be a command (CLAUDE.md invariant 3).
//!
//! Brief 0025 (proposal 0001 brief A) adds the agent's inspection commands: `snapshot`, `stack`, `variables`,
//! `output`, `exception_info` and `wait` (read class: they run no debuggee code) and `pause` (Debug > Break All,
//! execute class). Their answers are budgeted (proposal 0001 rule 2): every list says `total` and `truncated`, values
//! are cut at `max_value_chars` ([`cut_value`]), and the commands that run the debuggee answer with the compact stop
//! summary ([`StopSummary`], `debug-stop-summary.output.json`) within the budget parameters ([`Budget`]).
//!
//! The keys, the Debug menu, the margin, the debugger windows and agents all run these, against one state machine
//! in the shell (PLAN.md 5.5): see [`DebugTarget`]. `stop`, `toggle_breakpoint`, `state`, `select_frame`, `watch` and
//! `exception_settings` answer with the debugger's state ([`DebugState`], `debug-state.output.json`), which is what
//! the windows show; `evaluate`, `stack`, `variables`, `output` and `exception_info` with their own outputs; the rest
//! with the stop summary.
//!
//! The schemas are the files in `protocol/schemas/debug-*.json` (checked in first, CLAUDE.md invariant 4).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const START: &str = "eludite.debug.start";
pub const STOP: &str = "eludite.debug.stop";
pub const CONTINUE: &str = "eludite.debug.continue";
pub const STEP_OVER: &str = "eludite.debug.step_over";
pub const STEP_INTO: &str = "eludite.debug.step_into";
pub const STEP_OUT: &str = "eludite.debug.step_out";
pub const RUN_TO_CURSOR: &str = "eludite.debug.run_to_cursor";
pub const TOGGLE_BREAKPOINT: &str = "eludite.debug.toggle_breakpoint";
pub const EVALUATE: &str = "eludite.debug.evaluate";
pub const STATE: &str = "eludite.debug.state";
pub const SELECT_FRAME: &str = "eludite.debug.select_frame";
pub const WATCH: &str = "eludite.debug.watch";
pub const EXCEPTION_SETTINGS: &str = "eludite.debug.exception_settings";
pub const SNAPSHOT: &str = "eludite.debug.snapshot";
pub const STACK: &str = "eludite.debug.stack";
pub const VARIABLES: &str = "eludite.debug.variables";
pub const OUTPUT: &str = "eludite.debug.output";
pub const EXCEPTION_INFO: &str = "eludite.debug.exception_info";
pub const PAUSE: &str = "eludite.debug.pause";
pub const WAIT: &str = "eludite.debug.wait";

pub const ALL: [&str; 20] = [
    START,
    STOP,
    CONTINUE,
    STEP_OVER,
    STEP_INTO,
    STEP_OUT,
    RUN_TO_CURSOR,
    TOGGLE_BREAKPOINT,
    EVALUATE,
    STATE,
    SELECT_FRAME,
    WATCH,
    EXCEPTION_SETTINGS,
    SNAPSHOT,
    STACK,
    VARIABLES,
    OUTPUT,
    EXCEPTION_INFO,
    PAUSE,
    WAIT,
];

/// The longest an agent's command may wait for the debuggee (`wait_ms`).
pub const MAX_WAIT_MS: u64 = 30_000;
/// How long `eludite.debug.wait` waits by default.
pub const DEFAULT_WAIT_MS: u64 = 5_000;
/// `eludite.debug.stack`'s page size, default and most.
pub const DEFAULT_STACK_COUNT: usize = 20;
pub const MAX_STACK_COUNT: usize = 200;
/// `eludite.debug.variables`' page size, default and most.
pub const DEFAULT_VARIABLES_COUNT: usize = 50;
pub const MAX_VARIABLES_COUNT: usize = 500;
/// `eludite.debug.output`'s page size, default and most.
pub const DEFAULT_OUTPUT_LINES: usize = 20;
pub const MAX_OUTPUT_LINES: usize = 1_000;
/// Inner exceptions listed at most, in depth.
pub const MAX_INNER_EXCEPTIONS: usize = 5;

const STATE_OUTPUT: &str = include_str!("../../../protocol/schemas/debug-state.output.json");
const SUMMARY_OUTPUT: &str =
    include_str!("../../../protocol/schemas/debug-stop-summary.output.json");

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    macro_rules! input {
        ($f:literal) => {
            include_str!(concat!("../../../protocol/schemas/", $f))
        };
    }
    match id {
        // Running the program, resuming it and evaluating expressions (which can call methods) execute code.
        START => (
            "Debug: Start Debugging",
            input!("debug-start.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        STOP => (
            "Debug: Stop Debugging",
            input!("debug-stop.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        CONTINUE => (
            "Debug: Continue",
            input!("debug-continue.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        STEP_OVER => (
            "Debug: Step Over",
            input!("debug-step-over.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        STEP_INTO => (
            "Debug: Step Into",
            input!("debug-step-into.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        STEP_OUT => (
            "Debug: Step Out",
            input!("debug-step-out.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        RUN_TO_CURSOR => (
            "Debug: Run To Cursor",
            input!("debug-run-to-cursor.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        EVALUATE => (
            "Debug: Evaluate Expression",
            input!("debug-evaluate.input.json"),
            input!("debug-evaluate.output.json"),
            Execute,
        ),
        // Break All stops the debuggee where it is: it changes the process's state.
        PAUSE => (
            "Debug: Break All",
            input!("debug-pause.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        // Reads that cannot run debuggee code (proposal 0001 rule 7).
        SNAPSHOT => (
            "Debug: Snapshot",
            input!("debug-snapshot.input.json"),
            SUMMARY_OUTPUT,
            Read,
        ),
        STACK => (
            "Debug: Call Stack",
            input!("debug-stack.input.json"),
            input!("debug-stack.output.json"),
            Read,
        ),
        VARIABLES => (
            "Debug: Variables",
            input!("debug-variables.input.json"),
            input!("debug-variables.output.json"),
            Read,
        ),
        OUTPUT => (
            "Debug: Program Output",
            input!("debug-output.input.json"),
            input!("debug-output.output.json"),
            Read,
        ),
        EXCEPTION_INFO => (
            "Debug: Exception Information",
            input!("debug-exception-info.input.json"),
            input!("debug-exception-info.output.json"),
            Read,
        ),
        WAIT => (
            "Debug: Wait",
            input!("debug-wait.input.json"),
            SUMMARY_OUTPUT,
            Read,
        ),
        // Breakpoints, watches, frame selection and exception settings change what the debugger shows and where it
        // stops, not files or processes.
        TOGGLE_BREAKPOINT => (
            "Debug: Toggle Breakpoint",
            input!("debug-toggle-breakpoint.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        STATE => (
            "Debug: Debugger State",
            input!("debug-state.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        SELECT_FRAME => (
            "Debug: Switch To Frame",
            input!("debug-select-frame.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        WATCH => (
            "Debug: Add or Remove Watch",
            input!("debug-watch.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        EXCEPTION_SETTINGS => (
            "Debug: Exception Settings",
            input!("debug-exception-settings.input.json"),
            STATE_OUTPUT,
            Read,
        ),
        other => unreachable!("not a debug command: {other}"),
    }
}

/// Which step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StepKind {
    Over,
    Into,
    Out,
}

impl StepKind {
    /// The DAP request.
    pub fn dap_command(self) -> &'static str {
        match self {
            StepKind::Over => "next",
            StepKind::Into => "stepIn",
            StepKind::Out => "stepOut",
        }
    }

    pub fn command(self) -> &'static str {
        match self {
            StepKind::Over => STEP_OVER,
            StepKind::Into => STEP_INTO,
            StepKind::Out => STEP_OUT,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointAction {
    #[default]
    Toggle,
    Set,
    Delete,
    DeleteAll,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum EvalContext {
    #[default]
    Watch,
    Hover,
    Repl,
}

impl EvalContext {
    pub fn as_str(self) -> &'static str {
        match self {
            EvalContext::Watch => "watch",
            EvalContext::Hover => "hover",
            EvalContext::Repl => "repl",
        }
    }
}

/// Visual Studio's Hit Count: break when the count is equal to, at least, or a multiple of `count`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HitCondition {
    Equal(u32),
    AtLeast(u32),
    MultipleOf(u32),
}

impl HitCondition {
    /// `N`, `>=N` or `%N` with N at least 1.
    pub fn parse(s: &str) -> Option<HitCondition> {
        let s = s.trim();
        let (make, rest): (fn(u32) -> HitCondition, &str) = if let Some(r) = s.strip_prefix(">=") {
            (HitCondition::AtLeast, r)
        } else if let Some(r) = s.strip_prefix('%') {
            (HitCondition::MultipleOf, r)
        } else {
            (HitCondition::Equal, s)
        };
        let n: u32 = rest.trim().parse().ok().filter(|n| *n >= 1)?;
        Some(make(n))
    }

    /// Whether the `hits`th hit breaks.
    pub fn breaks_on(self, hits: u32) -> bool {
        match self {
            HitCondition::Equal(n) => hits == n,
            HitCondition::AtLeast(n) => hits >= n,
            HitCondition::MultipleOf(n) => hits.is_multiple_of(n),
        }
    }
}

impl std::fmt::Display for HitCondition {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HitCondition::Equal(n) => write!(f, "{n}"),
            HitCondition::AtLeast(n) => write!(f, ">={n}"),
            HitCondition::MultipleOf(n) => write!(f, "%{n}"),
        }
    }
}

/// The stop summary's budget parameters (proposal 0001 rule 2), with their defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// Levels of the locals expanded (1: the top level).
    pub depth: usize,
    /// Variable rows at most, every level counted.
    pub max_variables: usize,
    pub max_value_chars: usize,
    pub max_frames: usize,
    pub max_output_lines: usize,
    /// List the program's output from this cursor on; `None`: the last `max_output_lines` lines.
    pub output_since: Option<u64>,
}

impl Budget {
    pub const MAX_DEPTH: usize = 5;
    pub const MAX_VARIABLES: usize = 500;
    pub const MAX_VALUE_CHARS: usize = 10_000;
    pub const MAX_FRAMES: usize = 200;
    pub const MAX_OUTPUT_LINES: usize = 1_000;
}

impl Default for Budget {
    fn default() -> Self {
        Self {
            depth: 1,
            max_variables: 50,
            max_value_chars: 200,
            max_frames: 10,
            max_output_lines: 20,
            output_since: None,
        }
    }
}

/// `value` cut at `max` characters, ending with `… (N chars)`, and whether it was cut.
pub fn cut_value(value: &str, max: usize) -> (String, bool) {
    let n = value.chars().count();
    if n <= max {
        return (value.to_owned(), false);
    }
    let mut cut: String = value.chars().take(max).collect();
    cut.push_str(&format!("\u{2026} ({n} chars)"));
    (cut, true)
}

/// `eludite.debug.variables`' `scope`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ScopeKind {
    #[default]
    Locals,
    Arguments,
    This,
}

/// What `eludite.debug.variables` lists.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VariablesTarget {
    /// The members of a value an earlier row gave.
    Reference(i64),
    /// A frame's variables: thread (default: the one that stopped), frame index (default 0), scope.
    Frame {
        thread: Option<i64>,
        frame: Option<usize>,
        scope: ScopeKind,
    },
}

/// `eludite.debug.output`'s `source`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputKind {
    /// The debuggee's stdout and stderr.
    #[default]
    Program,
    /// The debugger's own messages.
    Debug,
    /// The debug adapter's stderr and console messages.
    Adapter,
}

impl OutputKind {
    pub const ALL: [OutputKind; 3] = [OutputKind::Program, OutputKind::Debug, OutputKind::Adapter];

    pub fn index(self) -> usize {
        self as usize
    }
}

/// `eludite.debug.wait`'s `until`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitUntil {
    Stopped,
    Terminated,
    Output,
    #[default]
    Any,
}

/// `eludite.debug.output`'s `pattern`: a substring, or `/regex/` (a small regular-expression engine: literals,
/// `.`, classes `[a-z]` and `[^...]`, `\d \w \s` and their negations, `* + ?`, `|`, groups, `^` and `$`).
#[derive(Debug, Clone)]
pub struct OutputPattern {
    source: String,
    regex: Option<pattern::Regex>,
}

impl PartialEq for OutputPattern {
    fn eq(&self, other: &Self) -> bool {
        self.source == other.source
    }
}

impl OutputPattern {
    pub fn parse(source: &str) -> Result<Self, String> {
        let regex = match source.strip_prefix('/').and_then(|r| r.strip_suffix('/')) {
            Some(r) if source.len() >= 2 => Some(pattern::Regex::new(r)?),
            _ => None,
        };
        Ok(Self {
            source: source.to_owned(),
            regex,
        })
    }

    pub fn matches(&self, line: &str) -> bool {
        match &self.regex {
            Some(r) => r.is_match(line),
            None => line.contains(&self.source),
        }
    }
}

/// A parsed, validated debug command.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugRequest {
    Start {
        project: Option<String>,
        debug: bool,
        profile: Option<String>,
        /// Build the project first (brief 0020); `None`: the setting `build.beforeRun`.
        build: Option<bool>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Stop,
    Continue {
        stop: Option<u64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Step {
        kind: StepKind,
        thread: Option<i64>,
        stop: Option<u64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    RunToCursor {
        path: Option<String>,
        line: Option<u32>,
        stop: Option<u64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Breakpoint {
        path: Option<String>,
        line: Option<u32>,
        action: BreakpointAction,
        enabled: Option<bool>,
        /// `Some("")` removes the condition.
        condition: Option<String>,
        /// `Some(None)` removes the hit condition.
        hit_condition: Option<Option<HitCondition>>,
    },
    Evaluate {
        expression: String,
        frame: Option<usize>,
        context: EvalContext,
        expand: bool,
        stop: Option<u64>,
    },
    State,
    SelectFrame {
        thread: Option<i64>,
        frame: Option<usize>,
        stop: Option<u64>,
    },
    AddWatch(String),
    RemoveWatch(usize),
    ExceptionSettings {
        break_when_thrown: Option<bool>,
        break_when_user_unhandled: Option<bool>,
    },
    Snapshot {
        thread: Option<i64>,
        frame: Option<usize>,
        budget: Budget,
    },
    Stack {
        thread: Option<i64>,
        start: usize,
        count: usize,
        all_threads: bool,
        stop: Option<u64>,
    },
    Variables {
        target: VariablesTarget,
        start: usize,
        count: usize,
        depth: usize,
        filter: Option<String>,
        max_value_chars: usize,
        stop: Option<u64>,
    },
    Output {
        source: OutputKind,
        since: u64,
        max_lines: usize,
        pattern: Option<OutputPattern>,
    },
    ExceptionInfo {
        thread: Option<i64>,
        stop: Option<u64>,
    },
    Pause {
        thread: Option<i64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Wait {
        until: WaitUntil,
        wait_ms: u64,
        stop: Option<u64>,
        budget: Budget,
    },
}

impl DebugRequest {
    pub fn command(&self) -> &'static str {
        match self {
            DebugRequest::Start { .. } => START,
            DebugRequest::Stop => STOP,
            DebugRequest::Continue { .. } => CONTINUE,
            DebugRequest::Step { kind, .. } => kind.command(),
            DebugRequest::RunToCursor { .. } => RUN_TO_CURSOR,
            DebugRequest::Breakpoint { .. } => TOGGLE_BREAKPOINT,
            DebugRequest::Evaluate { .. } => EVALUATE,
            DebugRequest::State => STATE,
            DebugRequest::SelectFrame { .. } => SELECT_FRAME,
            DebugRequest::AddWatch(_) | DebugRequest::RemoveWatch(_) => WATCH,
            DebugRequest::ExceptionSettings { .. } => EXCEPTION_SETTINGS,
            DebugRequest::Snapshot { .. } => SNAPSHOT,
            DebugRequest::Stack { .. } => STACK,
            DebugRequest::Variables { .. } => VARIABLES,
            DebugRequest::Output { .. } => OUTPUT,
            DebugRequest::ExceptionInfo { .. } => EXCEPTION_INFO,
            DebugRequest::Pause { .. } => PAUSE,
            DebugRequest::Wait { .. } => WAIT,
        }
    }

    /// How long an agent asked to wait for the debuggee.
    pub fn wait_ms(&self) -> Option<u64> {
        match self {
            DebugRequest::Start { wait_ms, .. }
            | DebugRequest::Continue { wait_ms, .. }
            | DebugRequest::Step { wait_ms, .. }
            | DebugRequest::RunToCursor { wait_ms, .. }
            | DebugRequest::Pause { wait_ms, .. } => *wait_ms,
            DebugRequest::Wait { wait_ms, .. } => Some(*wait_ms),
            _ => None,
        }
    }

    /// The budget of a command that answers with the stop summary.
    pub fn budget(&self) -> Option<Budget> {
        match self {
            DebugRequest::Start { budget, .. }
            | DebugRequest::Continue { budget, .. }
            | DebugRequest::Step { budget, .. }
            | DebugRequest::RunToCursor { budget, .. }
            | DebugRequest::Snapshot { budget, .. }
            | DebugRequest::Pause { budget, .. }
            | DebugRequest::Wait { budget, .. } => Some(*budget),
            _ => None,
        }
    }

    /// Whether the command resumes or starts the debuggee (an agent's call waits for it to settle).
    pub fn resumes(&self) -> bool {
        matches!(
            self,
            DebugRequest::Start { .. }
                | DebugRequest::Continue { .. }
                | DebugRequest::Step { .. }
                | DebugRequest::RunToCursor { .. }
                | DebugRequest::Stop
        )
    }
}

/// `debug-state.output.json`'s variables.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariableRow {
    pub name: String,
    pub value: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionRow {
    pub project: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub debug: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// What runs the program (brief 0022): `coreclr`, `mono` or `netfx`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_mode: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StoppedRow {
    pub reason: String,
    pub thread: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<ExceptionRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ThreadRow {
    pub id: i64,
    pub name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameRow {
    pub index: usize,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct WatchRow {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointRow {
    pub path: String,
    pub line: u32,
    pub enabled: bool,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    pub hits: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionSettingsRow {
    pub break_when_thrown: bool,
    pub break_when_user_unhandled: bool,
}

impl Default for ExceptionSettingsRow {
    /// Visual Studio's default: break on exceptions user code does not handle, not on every throw.
    fn default() -> Self {
        Self {
            break_when_thrown: false,
            break_when_user_unhandled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConsoleRow {
    pub lines: u64,
    pub tail: Vec<String>,
    /// The program output's next sequence number (`eludite.debug.output`'s `since`).
    #[serde(default)]
    pub next: u64,
}

/// `debug-state.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DebugState {
    /// `design`, `launching`, `running`, `break`, `stopping` or `running_without_debugging`.
    pub mode: String,
    pub generation: u64,
    pub stop: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<SessionRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<StoppedRow>,
    pub threads: Vec<ThreadRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub thread: Option<i64>,
    pub frames: Vec<FrameRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frame: Option<usize>,
    pub locals: Vec<VariableRow>,
    pub watches: Vec<WatchRow>,
    pub breakpoints: Vec<BreakpointRow>,
    pub exceptions: ExceptionSettingsRow,
    pub console: ConsoleRow,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_driver: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The session's adapter's capabilities (brief 0025).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilitiesRow>,
    /// The last command that started, resumed or paused the debuggee came from an agent.
    #[serde(default)]
    pub agent_driving: bool,
}

/// `debug-state.output.json`'s `$defs/capabilities`: what the session's adapter supports.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CapabilitiesRow {
    /// `coreclr`, `mono` or `fake`.
    pub adapter: String,
    pub pause: bool,
    pub set_variable: bool,
    pub exception_info: bool,
    pub function_breakpoints: bool,
    /// `adapter` or `shell`.
    pub log_points: String,
    /// `adapter` or `shell`.
    pub hit_conditions: String,
    pub exception_filter_options: bool,
    pub set_next_statement: bool,
    pub data_breakpoints: bool,
    pub step_back: bool,
    pub restart: bool,
    pub terminate: bool,
    pub modules: bool,
    pub memory: bool,
    pub disassembly: bool,
    pub delayed_stack_loading: bool,
    pub variable_paging: bool,
}

fn is_false(b: &bool) -> bool {
    !*b
}

/// A frame as `eludite.debug.stack` and the stop summary list it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackFrameRow {
    pub index: usize,
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// A frame without source (Visual Studio's [External Code]).
    #[serde(default, skip_serializing_if = "is_false")]
    pub external: bool,
}

/// A variable as `eludite.debug.variables` and the stop summary list it.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VarRow {
    pub name: String,
    pub value: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evaluate_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub indexed: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub named: Option<i64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub value_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<VarRow>>,
    /// Not every member is in `children`.
    #[serde(default, skip_serializing_if = "is_false")]
    pub truncated: bool,
}

/// The stop summary's `stopped.location`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocationRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    pub function: String,
}

/// The stop summary's `stopped.exception`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionBrief {
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_mode: Option<String>,
}

/// The stop summary's `stopped.breakpoint`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointBrief {
    pub path: String,
    pub line: u32,
    pub hits: u32,
}

/// The stop summary's `stopped`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryStopped {
    pub reason: String,
    pub thread: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub location: Option<LocationRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exception: Option<ExceptionBrief>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<BreakpointBrief>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub driver: Option<String>,
}

/// The stop summary's `frames`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FramesBlock {
    pub thread: i64,
    pub rows: Vec<StackFrameRow>,
    pub total: usize,
    pub truncated: bool,
}

/// The stop summary's `locals`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LocalsBlock {
    pub thread: i64,
    pub frame: usize,
    pub rows: Vec<VarRow>,
    pub total: usize,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<usize>,
}

/// The stop summary's `watches`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SummaryWatch {
    pub expression: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub reference: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub value_truncated: bool,
}

/// One output line.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputLine {
    pub seq: u64,
    pub text: String,
    /// `stdout` or `stderr`, when known.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stream: Option<String>,
}

/// The stop summary's `output`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputBlock {
    pub lines: Vec<OutputLine>,
    pub next: u64,
    pub dropped: u64,
    pub total: u64,
    pub truncated: bool,
}

/// `debug-stop-summary.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StopSummary {
    pub mode: String,
    pub generation: u64,
    pub stop: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<SummaryStopped>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub frames: Option<FramesBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub locals: Option<LocalsBlock>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub watches: Option<Vec<SummaryWatch>>,
    pub output: OutputBlock,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<CapabilitiesRow>,
    pub agent_driving: bool,
    pub truncated: bool,
    /// `eludite.debug.wait`: `stopped`, `terminated` or `output`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub satisfied: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timed_out: Option<bool>,
}

/// One thread's page of `debug-stack.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackThread {
    pub id: i64,
    pub name: String,
    pub frames: Vec<StackFrameRow>,
    pub total: usize,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<usize>,
}

/// `debug-stack.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StackOutput {
    pub threads: Vec<StackThread>,
    pub stop: u64,
}

/// `debug-variables.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VariablesOutput {
    pub rows: Vec<VarRow>,
    pub total: usize,
    pub truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next: Option<usize>,
    pub stop: u64,
}

/// `debug-output.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputPage {
    pub source: OutputKind,
    pub lines: Vec<OutputLine>,
    pub next: u64,
    pub dropped: u64,
    pub total: u64,
    pub truncated: bool,
}

/// `debug-exception-info.output.json`'s `details`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionDetailsRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub full_type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub inner_exceptions: Vec<ExceptionDetailsRow>,
}

/// `debug-exception-info.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionInfoOutput {
    pub supported: bool,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub break_mode: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<ExceptionDetailsRow>,
    pub thread: i64,
    pub stop: u64,
}

/// `debug-evaluate.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EvaluateOutput {
    pub expression: String,
    /// `done`, `pending` or `failed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<String>,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub children: Option<Vec<VariableRow>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub stop: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugOutput {
    State(Box<DebugState>),
    Evaluate(EvaluateOutput),
    Summary(Box<StopSummary>),
    Stack(StackOutput),
    Variables(VariablesOutput),
    Output(OutputPage),
    ExceptionInfo(ExceptionInfoOutput),
}

impl DebugOutput {
    pub fn to_json(&self) -> Value {
        match self {
            DebugOutput::State(s) => serde_json::to_value(s),
            DebugOutput::Evaluate(e) => serde_json::to_value(e),
            DebugOutput::Summary(s) => serde_json::to_value(s),
            DebugOutput::Stack(s) => serde_json::to_value(s),
            DebugOutput::Variables(v) => serde_json::to_value(v),
            DebugOutput::Output(o) => serde_json::to_value(o),
            DebugOutput::ExceptionInfo(e) => serde_json::to_value(e),
        }
        .expect("debug outputs serialize")
    }
}

/// Whatever owns the debugger (the shell). Called on the invoking thread.
pub trait DebugTarget: Send + Sync {
    fn apply(&self, request: DebugRequest) -> Result<DebugOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StartIn {
    project: Option<String>,
    debug: Option<bool>,
    profile: Option<String>,
    build: Option<bool>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ResumeIn {
    stop: Option<u64>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StepIn {
    thread: Option<i64>,
    stop: Option<u64>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct CursorIn {
    path: Option<String>,
    line: Option<u32>,
    stop: Option<u64>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct BreakpointIn {
    path: Option<String>,
    line: Option<u32>,
    #[serde(default)]
    action: BreakpointAction,
    enabled: Option<bool>,
    condition: Option<String>,
    hit_condition: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EvaluateIn {
    expression: String,
    frame: Option<usize>,
    #[serde(default)]
    context: EvalContext,
    #[serde(default)]
    expand: bool,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SelectFrameIn {
    thread: Option<i64>,
    frame: Option<usize>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WatchIn {
    add: Option<String>,
    remove: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ExceptionsIn {
    break_when_thrown: Option<bool>,
    break_when_user_unhandled: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SnapshotIn {
    thread: Option<i64>,
    frame: Option<usize>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StackIn {
    thread: Option<i64>,
    start: Option<usize>,
    count: Option<usize>,
    #[serde(default)]
    all_threads: bool,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct VariablesIn {
    reference: Option<i64>,
    thread: Option<i64>,
    frame: Option<usize>,
    scope: Option<ScopeKind>,
    start: Option<usize>,
    count: Option<usize>,
    depth: Option<usize>,
    filter: Option<String>,
    max_value_chars: Option<usize>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OutputIn {
    #[serde(default)]
    source: OutputKind,
    since: Option<u64>,
    max_lines: Option<usize>,
    pattern: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ExceptionInfoIn {
    thread: Option<i64>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct PauseIn {
    thread: Option<i64>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WaitIn {
    #[serde(default)]
    until: WaitUntil,
    wait_ms: Option<u64>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(m.into())
}

/// A count in `min..=max`, `default` when absent.
fn bounded(
    field: &str,
    v: Option<usize>,
    default: usize,
    min: usize,
    max: usize,
) -> Result<usize, CommandError> {
    match v {
        None => Ok(default),
        Some(n) if (min..=max).contains(&n) => Ok(n),
        Some(_) => Err(invalid(format!("`{field}` is {min} to {max}"))),
    }
}

/// Take the stop summary's budget parameters out of `value` (so the rest parses with its own fields), validated.
fn take_budget(value: &mut Value) -> Result<Budget, CommandError> {
    let d = Budget::default();
    let Some(obj) = value.as_object_mut() else {
        return Ok(d);
    };
    let mut num = |key: &str| -> Result<Option<u64>, CommandError> {
        match obj.remove(key) {
            None | Some(Value::Null) => Ok(None),
            Some(v) => v
                .as_u64()
                .map(Some)
                .ok_or_else(|| invalid(format!("`{key}` is a non-negative integer"))),
        }
    };
    let size = |v: Option<u64>| v.map(|n| usize::try_from(n).unwrap_or(usize::MAX));
    let depth = size(num("depth")?);
    let max_variables = size(num("max_variables")?);
    let max_value_chars = size(num("max_value_chars")?);
    let max_frames = size(num("max_frames")?);
    let max_output_lines = size(num("max_output_lines")?);
    let output_since = num("output_since")?;
    Ok(Budget {
        depth: bounded("depth", depth, d.depth, 1, Budget::MAX_DEPTH)?,
        max_variables: bounded(
            "max_variables",
            max_variables,
            d.max_variables,
            1,
            Budget::MAX_VARIABLES,
        )?,
        max_value_chars: bounded(
            "max_value_chars",
            max_value_chars,
            d.max_value_chars,
            1,
            Budget::MAX_VALUE_CHARS,
        )?,
        max_frames: bounded(
            "max_frames",
            max_frames,
            d.max_frames,
            1,
            Budget::MAX_FRAMES,
        )?,
        max_output_lines: bounded(
            "max_output_lines",
            max_output_lines,
            d.max_output_lines,
            0,
            Budget::MAX_OUTPUT_LINES,
        )?,
        output_since,
    })
}

fn check_wait(w: Option<u64>) -> Result<Option<u64>, CommandError> {
    match w {
        Some(ms) if ms > MAX_WAIT_MS => Err(invalid(format!("`wait_ms` is at most {MAX_WAIT_MS}"))),
        w => Ok(w),
    }
}

fn check_stop(s: Option<u64>) -> Result<Option<u64>, CommandError> {
    match s {
        Some(0) => Err(invalid("`stop` is at least 1")),
        s => Ok(s),
    }
}

fn check_line(l: Option<u32>) -> Result<Option<u32>, CommandError> {
    match l {
        Some(0) => Err(invalid("`line` is at least 1")),
        l => Ok(l),
    }
}

fn non_empty(field: &str, s: Option<String>) -> Result<Option<String>, CommandError> {
    match s {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{field}` must not be empty"))),
        s => Ok(s),
    }
}

/// Parse and validate the input of debug command `id`.
pub fn parse(id: &str, mut value: Value) -> Result<DebugRequest, CommandError> {
    // The commands that answer with the stop summary take its budget parameters.
    let budget = match id {
        START | CONTINUE | STEP_OVER | STEP_INTO | STEP_OUT | RUN_TO_CURSOR | SNAPSHOT | PAUSE
        | WAIT => take_budget(&mut value)?,
        _ => Budget::default(),
    };
    Ok(match id {
        START => {
            let i: StartIn = input(value)?;
            DebugRequest::Start {
                project: non_empty("project", i.project)?,
                debug: i.debug.unwrap_or(true),
                profile: non_empty("profile", i.profile)?,
                build: i.build,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        STOP => {
            let _: Empty = input(value)?;
            DebugRequest::Stop
        }
        CONTINUE => {
            let i: ResumeIn = input(value)?;
            DebugRequest::Continue {
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        STEP_OVER | STEP_INTO | STEP_OUT => {
            let i: StepIn = input(value)?;
            DebugRequest::Step {
                kind: match id {
                    STEP_OVER => StepKind::Over,
                    STEP_INTO => StepKind::Into,
                    _ => StepKind::Out,
                },
                thread: i.thread,
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        RUN_TO_CURSOR => {
            let i: CursorIn = input(value)?;
            DebugRequest::RunToCursor {
                path: non_empty("path", i.path)?,
                line: check_line(i.line)?,
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        TOGGLE_BREAKPOINT => {
            let i: BreakpointIn = input(value)?;
            let editing = i.enabled.is_some() || i.condition.is_some() || i.hit_condition.is_some();
            if editing && i.action != BreakpointAction::Set {
                return Err(invalid(
                    "`enabled`, `condition` and `hit_condition` go with `action: \"set\"`",
                ));
            }
            let hit_condition = match i.hit_condition {
                None => None,
                Some(s) if s.trim().is_empty() => Some(None),
                Some(s) => Some(Some(HitCondition::parse(&s).ok_or_else(|| {
                    invalid(format!(
                        "`hit_condition` is N, >=N or %N with N at least 1, not `{s}`"
                    ))
                })?)),
            };
            DebugRequest::Breakpoint {
                path: non_empty("path", i.path)?,
                line: check_line(i.line)?,
                action: i.action,
                enabled: i.enabled,
                condition: i.condition,
                hit_condition,
            }
        }
        EVALUATE => {
            let i: EvaluateIn =
                serde_json::from_value(value).map_err(|e| invalid(e.to_string()))?;
            if i.expression.trim().is_empty() {
                return Err(invalid("`expression` must not be empty"));
            }
            DebugRequest::Evaluate {
                expression: i.expression,
                frame: i.frame,
                context: i.context,
                expand: i.expand,
                stop: check_stop(i.stop)?,
            }
        }
        STATE => {
            let _: Empty = input(value)?;
            DebugRequest::State
        }
        SELECT_FRAME => {
            let i: SelectFrameIn = input(value)?;
            DebugRequest::SelectFrame {
                thread: i.thread,
                frame: i.frame,
                stop: check_stop(i.stop)?,
            }
        }
        WATCH => {
            let i: WatchIn = input(value)?;
            match (i.add, i.remove) {
                (Some(e), None) if !e.trim().is_empty() => DebugRequest::AddWatch(e),
                (None, Some(ix)) => DebugRequest::RemoveWatch(ix),
                _ => {
                    return Err(invalid(
                        "give exactly one of `add` (an expression) and `remove`",
                    ));
                }
            }
        }
        EXCEPTION_SETTINGS => {
            let i: ExceptionsIn = input(value)?;
            DebugRequest::ExceptionSettings {
                break_when_thrown: i.break_when_thrown,
                break_when_user_unhandled: i.break_when_user_unhandled,
            }
        }
        SNAPSHOT => {
            let i: SnapshotIn = input(value)?;
            DebugRequest::Snapshot {
                thread: i.thread,
                frame: i.frame,
                budget,
            }
        }
        STACK => {
            let i: StackIn = input(value)?;
            if i.all_threads && i.thread.is_some() {
                return Err(invalid("give `thread` or `all_threads`, not both"));
            }
            DebugRequest::Stack {
                thread: i.thread,
                start: i.start.unwrap_or(0),
                count: bounded("count", i.count, DEFAULT_STACK_COUNT, 1, MAX_STACK_COUNT)?,
                all_threads: i.all_threads,
                stop: check_stop(i.stop)?,
            }
        }
        VARIABLES => {
            let i: VariablesIn = input(value)?;
            let target = match i.reference {
                Some(r) if i.thread.is_some() || i.frame.is_some() || i.scope.is_some() => {
                    return Err(invalid(format!(
                        "give `reference` ({r}) or `thread`, `frame` and `scope`, not both"
                    )));
                }
                Some(r) if r < 1 => return Err(invalid("`reference` is at least 1")),
                Some(r) => VariablesTarget::Reference(r),
                None => VariablesTarget::Frame {
                    thread: i.thread,
                    frame: i.frame,
                    scope: i.scope.unwrap_or_default(),
                },
            };
            DebugRequest::Variables {
                target,
                start: i.start.unwrap_or(0),
                count: bounded(
                    "count",
                    i.count,
                    DEFAULT_VARIABLES_COUNT,
                    1,
                    MAX_VARIABLES_COUNT,
                )?,
                depth: bounded("depth", i.depth, 1, 1, Budget::MAX_DEPTH)?,
                filter: non_empty("filter", i.filter)?,
                max_value_chars: bounded(
                    "max_value_chars",
                    i.max_value_chars,
                    Budget::default().max_value_chars,
                    1,
                    Budget::MAX_VALUE_CHARS,
                )?,
                stop: check_stop(i.stop)?,
            }
        }
        OUTPUT => {
            let i: OutputIn = input(value)?;
            let pattern = match non_empty("pattern", i.pattern)? {
                Some(p) => Some(
                    OutputPattern::parse(&p).map_err(|e| invalid(format!("`pattern` {p}: {e}")))?,
                ),
                None => None,
            };
            DebugRequest::Output {
                source: i.source,
                since: i.since.unwrap_or(0),
                max_lines: bounded(
                    "max_lines",
                    i.max_lines,
                    DEFAULT_OUTPUT_LINES,
                    1,
                    MAX_OUTPUT_LINES,
                )?,
                pattern,
            }
        }
        EXCEPTION_INFO => {
            let i: ExceptionInfoIn = input(value)?;
            DebugRequest::ExceptionInfo {
                thread: i.thread,
                stop: check_stop(i.stop)?,
            }
        }
        PAUSE => {
            let i: PauseIn = input(value)?;
            DebugRequest::Pause {
                thread: i.thread,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        WAIT => {
            let i: WaitIn = input(value)?;
            DebugRequest::Wait {
                until: i.until,
                wait_ms: check_wait(i.wait_ms)?.unwrap_or(DEFAULT_WAIT_MS),
                stop: check_stop(i.stop)?,
                budget,
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of debug command `id` (one of [`ALL`]). All are agent-visible: agent-driven debugging
/// (PLAN.md 5.5) runs the same commands as the keys and windows.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register every debug command, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn DebugTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

/// `eludite.debug.output`'s `/regex/` patterns, over the `regex` crate (MIT OR Apache-2.0, already in the build):
/// whether a pattern matches anywhere in a line. A line is matched on its first [`pattern::MAX_CHARS`] characters.
mod pattern {
    /// Characters of a line a regular expression looks at.
    pub const MAX_CHARS: usize = 4_096;

    #[derive(Debug, Clone)]
    pub struct Regex(regex::Regex);

    impl Regex {
        pub fn new(source: &str) -> Result<Self, String> {
            regex::Regex::new(source)
                .map(Self)
                .map_err(|e| format!("invalid regular expression `{source}`: {e}"))
        }

        pub fn is_match(&self, line: &str) -> bool {
            let end = line
                .char_indices()
                .nth(MAX_CHARS)
                .map_or(line.len(), |(i, _)| i);
            self.0.is_match(&line[..end])
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// `value` against `schema`: required members present, no member the schema does not define (where
    /// `additionalProperties` is false), enums, integer bounds and types, recursively through arrays and local
    /// `$ref`s.
    fn conforms(schema: &str, value: &Value) {
        let root: Value = serde_json::from_str(schema).unwrap();
        check(&root, &root, value, "$");
    }

    fn check(root: &Value, schema: &Value, value: &Value, at: &str) {
        if let Some(r) = schema["$ref"].as_str() {
            let path = r.strip_prefix("#/").expect("a local $ref");
            let mut target = root;
            for part in path.split('/') {
                target = &target[part];
            }
            assert!(!target.is_null(), "{at}: no {r}");
            return check(root, target, value, at);
        }
        if let Some(e) = schema["enum"].as_array() {
            assert!(e.contains(value), "{at}: {value} not in {e:?}");
        }
        match schema["type"].as_str() {
            Some("object") => {
                let obj = value
                    .as_object()
                    .unwrap_or_else(|| panic!("{at}: not an object"));
                for r in schema["required"].as_array().into_iter().flatten() {
                    assert!(obj.contains_key(r.as_str().unwrap()), "{at}: missing {r}");
                }
                for (k, v) in obj {
                    let p = &schema["properties"][k];
                    if p.is_null() {
                        assert!(
                            schema["additionalProperties"] != Value::Bool(false),
                            "{at}: unexpected {k}"
                        );
                        continue;
                    }
                    check(root, p, v, &format!("{at}.{k}"));
                }
            }
            Some("array") => {
                let a = value
                    .as_array()
                    .unwrap_or_else(|| panic!("{at}: not an array"));
                if let Some(max) = schema["maxItems"].as_u64() {
                    assert!(a.len() as u64 <= max, "{at}: {} items", a.len());
                }
                for (i, v) in a.iter().enumerate() {
                    check(root, &schema["items"], v, &format!("{at}[{i}]"));
                }
            }
            Some("integer") => {
                let n = value
                    .as_i64()
                    .unwrap_or_else(|| panic!("{at}: not an integer"));
                if let Some(min) = schema["minimum"].as_i64() {
                    assert!(n >= min, "{at}: {n} < {min}");
                }
                if let Some(max) = schema["maximum"].as_i64() {
                    assert!(n <= max, "{at}: {n} > {max}");
                }
            }
            Some("string") => assert!(value.is_string(), "{at}: not a string"),
            Some("boolean") => assert!(value.is_boolean(), "{at}: not a boolean"),
            _ => {}
        }
    }

    fn schema_keys_match(schema: &str, value: &Value) {
        let schema: Value = serde_json::from_str(schema).unwrap();
        for r in schema["required"].as_array().unwrap() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "missing {r}");
        }
        for k in value.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "unexpected {k}");
        }
    }

    #[test]
    fn parses_and_validates_every_command() {
        assert_eq!(
            parse(START, json!({})).unwrap(),
            DebugRequest::Start {
                project: None,
                debug: true,
                profile: None,
                build: None,
                wait_ms: None,
                budget: Budget::default()
            }
        );
        assert_eq!(
            parse(
                START,
                json!({"debug": false, "project": "App", "profile": "App", "build": false})
            )
            .unwrap(),
            DebugRequest::Start {
                project: Some("App".into()),
                debug: false,
                profile: Some("App".into()),
                build: Some(false),
                wait_ms: None,
                budget: Budget::default()
            }
        );
        assert!(parse(START, json!({"project": " "})).is_err());
        assert!(parse(START, json!({"wait_ms": 40000})).is_err());
        assert_eq!(parse(STOP, Value::Null).unwrap(), DebugRequest::Stop);
        assert!(parse(STOP, json!({"x": 1})).is_err());
        assert_eq!(
            parse(STEP_INTO, json!({"stop": 3, "wait_ms": 100})).unwrap(),
            DebugRequest::Step {
                kind: StepKind::Into,
                thread: None,
                stop: Some(3),
                wait_ms: Some(100),
                budget: Budget::default()
            }
        );
        assert!(parse(STEP_OUT, json!({"stop": 0})).is_err());
        assert_eq!(
            parse(CONTINUE, json!({})).unwrap(),
            DebugRequest::Continue {
                stop: None,
                wait_ms: None,
                budget: Budget::default()
            }
        );
        assert!(parse(RUN_TO_CURSOR, json!({"line": 0})).is_err());
        assert_eq!(
            parse(TOGGLE_BREAKPOINT, json!({"path": "A.cs", "line": 4})).unwrap(),
            DebugRequest::Breakpoint {
                path: Some("A.cs".into()),
                line: Some(4),
                action: BreakpointAction::Toggle,
                enabled: None,
                condition: None,
                hit_condition: None
            }
        );
        assert_eq!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "condition": "x > 1", "hit_condition": ">=3", "enabled": false})
            )
            .unwrap(),
            DebugRequest::Breakpoint {
                path: None,
                line: None,
                action: BreakpointAction::Set,
                enabled: Some(false),
                condition: Some("x > 1".into()),
                hit_condition: Some(Some(HitCondition::AtLeast(3)))
            }
        );
        assert!(matches!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "hit_condition": ""})
            )
            .unwrap(),
            DebugRequest::Breakpoint {
                hit_condition: Some(None),
                ..
            }
        ));
        assert!(parse(TOGGLE_BREAKPOINT, json!({"condition": "x"})).is_err());
        assert!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "hit_condition": "0"})
            )
            .is_err()
        );
        assert!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "hit_condition": "~2"})
            )
            .is_err()
        );
        assert!(parse(TOGGLE_BREAKPOINT, json!({"action": "nope"})).is_err());
        assert_eq!(
            parse(
                EVALUATE,
                json!({"expression": "x", "expand": true, "context": "hover"})
            )
            .unwrap(),
            DebugRequest::Evaluate {
                expression: "x".into(),
                frame: None,
                context: EvalContext::Hover,
                expand: true,
                stop: None
            }
        );
        assert!(parse(EVALUATE, json!({})).is_err());
        assert!(parse(EVALUATE, json!({"expression": ""})).is_err());
        assert_eq!(parse(STATE, json!({})).unwrap(), DebugRequest::State);
        assert_eq!(
            parse(SELECT_FRAME, json!({"frame": 1})).unwrap(),
            DebugRequest::SelectFrame {
                thread: None,
                frame: Some(1),
                stop: None
            }
        );
        assert_eq!(
            parse(WATCH, json!({"add": "order.Name"})).unwrap(),
            DebugRequest::AddWatch("order.Name".into())
        );
        assert_eq!(
            parse(WATCH, json!({"remove": 0})).unwrap(),
            DebugRequest::RemoveWatch(0)
        );
        assert!(parse(WATCH, json!({"add": "x", "remove": 0})).is_err());
        assert!(parse(WATCH, json!({})).is_err());
        assert_eq!(
            parse(EXCEPTION_SETTINGS, json!({"break_when_thrown": true})).unwrap(),
            DebugRequest::ExceptionSettings {
                break_when_thrown: Some(true),
                break_when_user_unhandled: None
            }
        );
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible, "{id}");
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["type"], "object");
        }
        assert_eq!(spec(STATE).permission, PermissionClass::Read);
        assert_eq!(spec(STEP_OVER).permission, PermissionClass::Execute);
        assert_eq!(spec(EVALUATE).permission, PermissionClass::Execute);
        assert_eq!(spec(TOGGLE_BREAKPOINT).permission, PermissionClass::Read);
    }

    #[test]
    fn hit_conditions() {
        assert_eq!(HitCondition::parse("3"), Some(HitCondition::Equal(3)));
        assert_eq!(HitCondition::parse(">= 2"), Some(HitCondition::AtLeast(2)));
        assert_eq!(HitCondition::parse("%4"), Some(HitCondition::MultipleOf(4)));
        assert_eq!(HitCondition::parse("0"), None);
        assert_eq!(HitCondition::parse("x"), None);
        let eq = HitCondition::Equal(2);
        assert_eq!(
            (1..=4).map(|h| eq.breaks_on(h)).collect::<Vec<_>>(),
            [false, true, false, false]
        );
        let ge = HitCondition::AtLeast(2);
        assert_eq!(
            (1..=3).map(|h| ge.breaks_on(h)).collect::<Vec<_>>(),
            [false, true, true]
        );
        let m = HitCondition::MultipleOf(2);
        assert_eq!(
            (1..=4).map(|h| m.breaks_on(h)).collect::<Vec<_>>(),
            [false, true, false, true]
        );
        for h in [eq, ge, m] {
            assert_eq!(HitCondition::parse(&h.to_string()), Some(h));
        }
    }

    #[test]
    fn outputs_follow_their_schemas() {
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "break".into(),
            generation: 2,
            stop: 3,
            session: Some(SessionRow {
                project: "/s/App.csproj".into(),
                program: "/s/bin/Debug/net10.0/App.dll".into(),
                args: vec![],
                cwd: "/s".into(),
                profile: None,
                debug: true,
                adapter: Some("netcoredbg".into()),
                runtime: Some("coreclr".into()),
                process_id: Some(7),
            }),
            stopped: Some(StoppedRow {
                reason: "breakpoint".into(),
                thread: 1,
                description: None,
                exception: None,
                driver: Some("user".into()),
            }),
            threads: vec![ThreadRow {
                id: 1,
                name: "Main Thread".into(),
            }],
            thread: Some(1),
            frames: vec![FrameRow {
                index: 0,
                name: "Main".into(),
                path: Some("/s/Program.cs".into()),
                line: Some(5),
                ..Default::default()
            }],
            frame: Some(0),
            locals: vec![VariableRow {
                name: "x".into(),
                value: "1".into(),
                type_name: Some("int".into()),
                reference: 0,
                evaluate_name: Some("x".into()),
            }],
            watches: vec![WatchRow {
                expression: "x".into(),
                value: Some("1".into()),
                type_name: None,
                reference: 0,
                error: None,
            }],
            breakpoints: vec![BreakpointRow {
                path: "/s/Program.cs".into(),
                line: 5,
                enabled: true,
                verified: true,
                condition: None,
                hit_condition: Some("2".into()),
                hits: 2,
                message: None,
            }],
            exceptions: ExceptionSettingsRow::default(),
            console: ConsoleRow {
                lines: 1,
                tail: vec!["hello".into()],
                next: 1,
            },
            last_driver: Some("agent:Claude Code".into()),
            message: None,
            capabilities: None,
            agent_driving: true,
        }))
        .to_json();
        schema_keys_match(STATE_OUTPUT, &state);
        for (k, v) in [
            ("session", &state["session"]),
            ("stopped", &state["stopped"]),
        ] {
            let schema: Value = serde_json::from_str(STATE_OUTPUT).unwrap();
            for key in v.as_object().unwrap().keys() {
                assert!(
                    schema["properties"][k]["properties"].get(key).is_some(),
                    "{k}.{key}"
                );
            }
        }
        let eval = DebugOutput::Evaluate(EvaluateOutput {
            expression: "order".into(),
            state: "done".into(),
            result: Some("{App.Order}".into()),
            type_name: Some("App.Order".into()),
            reference: Some(3),
            children: Some(vec![]),
            message: None,
            stop: 1,
        })
        .to_json();
        schema_keys_match(
            include_str!("../../../protocol/schemas/debug-evaluate.output.json"),
            &eval,
        );
    }

    #[test]
    fn the_inspection_commands_parse_and_validate() {
        // The stop summary's budget: defaults, every parameter, and the maxima.
        let d = Budget::default();
        assert_eq!(
            (
                d.depth,
                d.max_variables,
                d.max_value_chars,
                d.max_frames,
                d.max_output_lines
            ),
            (1, 50, 200, 10, 20)
        );
        assert_eq!(
            parse(SNAPSHOT, json!({})).unwrap(),
            DebugRequest::Snapshot {
                thread: None,
                frame: None,
                budget: d
            }
        );
        let all = json!({"thread": 2, "frame": 1, "depth": 5, "max_variables": 500, "max_value_chars": 10000,
                         "max_frames": 200, "max_output_lines": 1000, "output_since": 7});
        assert_eq!(
            parse(SNAPSHOT, all).unwrap(),
            DebugRequest::Snapshot {
                thread: Some(2),
                frame: Some(1),
                budget: Budget {
                    depth: 5,
                    max_variables: 500,
                    max_value_chars: 10_000,
                    max_frames: 200,
                    max_output_lines: 1000,
                    output_since: Some(7)
                }
            }
        );
        for bad in [
            json!({"depth": 0}),
            json!({"depth": 6}),
            json!({"max_variables": 501}),
            json!({"max_variables": 0}),
            json!({"max_value_chars": 10001}),
            json!({"max_frames": 201}),
            json!({"max_output_lines": 1001}),
            json!({"output_since": -1}),
            json!({"depth": "2"}),
            json!({"nope": 1}),
        ] {
            assert!(parse(SNAPSHOT, bad.clone()).is_err(), "{bad}");
        }
        assert!(
            parse(SNAPSHOT, json!({"depth": 6}))
                .unwrap_err()
                .to_string()
                .contains("`depth` is 1 to 5")
        );
        // Every command answering with the summary takes the budget; the others refuse it.
        for id in [
            START,
            CONTINUE,
            STEP_OVER,
            STEP_INTO,
            STEP_OUT,
            RUN_TO_CURSOR,
            PAUSE,
            WAIT,
        ] {
            let r = parse(id, json!({"max_frames": 3, "max_output_lines": 0})).unwrap();
            let b = r.budget().unwrap();
            assert_eq!((b.max_frames, b.max_output_lines), (3, 0), "{id}");
        }
        assert!(parse(STATE, json!({"max_frames": 3})).is_err());
        assert!(parse(STACK, json!({"depth": 2})).is_err());

        // stack
        assert_eq!(
            parse(STACK, json!({})).unwrap(),
            DebugRequest::Stack {
                thread: None,
                start: 0,
                count: 20,
                all_threads: false,
                stop: None
            }
        );
        assert_eq!(
            parse(
                STACK,
                json!({"thread": 3, "start": 10, "count": 200, "stop": 2})
            )
            .unwrap(),
            DebugRequest::Stack {
                thread: Some(3),
                start: 10,
                count: 200,
                all_threads: false,
                stop: Some(2)
            }
        );
        assert!(parse(STACK, json!({"count": 201})).is_err());
        assert!(parse(STACK, json!({"count": 0})).is_err());
        assert!(parse(STACK, json!({"thread": 1, "all_threads": true})).is_err());

        // variables: a reference, or a frame and a scope.
        assert_eq!(
            parse(VARIABLES, json!({})).unwrap(),
            DebugRequest::Variables {
                target: VariablesTarget::Frame {
                    thread: None,
                    frame: None,
                    scope: ScopeKind::Locals
                },
                start: 0,
                count: 50,
                depth: 1,
                filter: None,
                max_value_chars: 200,
                stop: None
            }
        );
        assert_eq!(
            parse(
                VARIABLES,
                json!({"reference": 9, "start": 50, "count": 500, "depth": 2, "filter": "ord",
                       "max_value_chars": 20, "stop": 4})
            )
            .unwrap(),
            DebugRequest::Variables {
                target: VariablesTarget::Reference(9),
                start: 50,
                count: 500,
                depth: 2,
                filter: Some("ord".into()),
                max_value_chars: 20,
                stop: Some(4)
            }
        );
        for (scope, kind) in [
            ("locals", ScopeKind::Locals),
            ("arguments", ScopeKind::Arguments),
            ("this", ScopeKind::This),
        ] {
            assert!(matches!(
                parse(VARIABLES, json!({"thread": 1, "frame": 2, "scope": scope})).unwrap(),
                DebugRequest::Variables {
                    target: VariablesTarget::Frame { thread: Some(1), frame: Some(2), scope: s },
                    ..
                } if s == kind
            ));
        }
        for bad in [
            json!({"scope": "statics"}),
            json!({"reference": 3, "frame": 1}),
            json!({"reference": 3, "scope": "locals"}),
            json!({"reference": 0}),
            json!({"count": 501}),
            json!({"depth": 6}),
            json!({"filter": ""}),
            json!({"max_value_chars": 0}),
        ] {
            assert!(parse(VARIABLES, bad.clone()).is_err(), "{bad}");
        }

        // output
        assert_eq!(
            parse(OUTPUT, json!({})).unwrap(),
            DebugRequest::Output {
                source: OutputKind::Program,
                since: 0,
                max_lines: 20,
                pattern: None
            }
        );
        let DebugRequest::Output {
            source, pattern, ..
        } = parse(
            OUTPUT,
            json!({"source": "debug", "since": 3, "max_lines": 1000, "pattern": "/^res(ult)? \\d+$/"}),
        )
        .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(source, OutputKind::Debug);
        let pattern = pattern.unwrap();
        assert!(pattern.matches("result 10") && pattern.matches("res 3"));
        assert!(!pattern.matches("the result 10") && !pattern.matches("result x"));
        let plain = OutputPattern::parse("a.b").unwrap();
        assert!(
            plain.matches("xa.by") && !plain.matches("axb"),
            "a substring"
        );
        assert!(parse(OUTPUT, json!({"source": "adapter"})).is_ok());
        assert!(parse(OUTPUT, json!({"source": "stderr"})).is_err());
        assert!(parse(OUTPUT, json!({"max_lines": 1001})).is_err());
        assert!(parse(OUTPUT, json!({"max_lines": 0})).is_err());
        let e = parse(OUTPUT, json!({"pattern": "/(a/"}))
            .unwrap_err()
            .to_string();
        assert!(e.contains("unclosed"), "{e}");

        // exception_info, pause, wait
        assert_eq!(
            parse(EXCEPTION_INFO, json!({"thread": 1, "stop": 2})).unwrap(),
            DebugRequest::ExceptionInfo {
                thread: Some(1),
                stop: Some(2)
            }
        );
        assert!(parse(EXCEPTION_INFO, json!({"stop": 0})).is_err());
        assert_eq!(
            parse(PAUSE, Value::Null).unwrap(),
            DebugRequest::Pause {
                thread: None,
                wait_ms: None,
                budget: d
            }
        );
        assert!(parse(PAUSE, json!({"wait_ms": 30001})).is_err());
        assert_eq!(
            parse(WAIT, json!({})).unwrap(),
            DebugRequest::Wait {
                until: WaitUntil::Any,
                wait_ms: 5000,
                stop: None,
                budget: d
            }
        );
        for (until, u) in [
            ("stopped", WaitUntil::Stopped),
            ("terminated", WaitUntil::Terminated),
            ("output", WaitUntil::Output),
            ("any", WaitUntil::Any),
        ] {
            assert!(matches!(
                parse(WAIT, json!({"until": until, "wait_ms": 30000, "stop": 3})).unwrap(),
                DebugRequest::Wait { until: x, wait_ms: 30000, stop: Some(3), .. } if x == u
            ));
        }
        assert!(parse(WAIT, json!({"until": "exited"})).is_err());
        assert!(parse(WAIT, json!({"wait_ms": 30001})).is_err());
        assert_eq!(parse(WAIT, json!({})).unwrap().wait_ms(), Some(5000));

        // Classes per proposal 0001 rule 7; every new command is on the bus for agents.
        for id in [SNAPSHOT, STACK, VARIABLES, OUTPUT, EXCEPTION_INFO, WAIT] {
            assert_eq!(spec(id).permission, PermissionClass::Read, "{id}");
        }
        assert_eq!(spec(PAUSE).permission, PermissionClass::Execute);
        assert_eq!(spec(PAUSE).title, "Debug: Break All");
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible, "{id}");
            assert!(
                s.input_schema["description"].as_str().unwrap().len() > 40,
                "{id}"
            );
        }
        for id in [
            START,
            CONTINUE,
            STEP_OVER,
            STEP_INTO,
            STEP_OUT,
            RUN_TO_CURSOR,
            SNAPSHOT,
            PAUSE,
            WAIT,
        ] {
            assert_eq!(
                spec(id).output_schema["title"],
                "eludite.debug stop summary",
                "{id}"
            );
            for k in [
                "depth",
                "max_variables",
                "max_value_chars",
                "max_frames",
                "max_output_lines",
                "output_since",
            ] {
                assert!(
                    spec(id).input_schema["properties"].get(k).is_some(),
                    "{id} {k}"
                );
            }
        }
    }

    #[test]
    fn regular_expressions_match_as_documented() {
        let m = |re: &str, line: &str| pattern::Regex::new(re).unwrap().is_match(line);
        assert!(m("abc", "xxabcxx"));
        assert!(!m("^abc", "xxabc"));
        assert!(m("^abc$", "abc") && !m("^abc$", "abcd"));
        assert!(m("a.c", "abc") && !m("a.c", "ac"));
        assert!(m("ab*c", "ac") && m("ab*c", "abbbc"));
        assert!(m("ab+c", "abc") && !m("ab+c", "ac"));
        assert!(m("colou?r", "color") && m("colou?r", "colour"));
        assert!(m("^(cat|dog)s?$", "dogs") && !m("^(cat|dog)s?$", "cow"));
        assert!(m("[a-c]x", "bx") && !m("[a-c]x", "dx"));
        assert!(m("[^0-9]", "a") && !m("^[^0-9]+$", "a1"));
        assert!(m("\\d{3}-\\d{4}", "call 555-1234") && !m("^\\d{3}$", "12"));
        assert!(m("^\\w+\\s\\w+$", "hello world") && !m("^\\S+$", "a b"));
        assert!(m("a{2,}", "caab") && !m("a{2,}", "cab"));
        assert!(m("^a{1,2}$", "aa") && !m("^a{1,2}$", "aaa"));
        assert!(m("\\.cs$", "Program.cs") && !m("\\.cs$", "Programcs"));
        assert!(m("(a*)*b", "aaab") && !m("^(a*)*$", "aaac"));
        assert!(m("(?:ab)+", "abab") && m("(?i)error", "An ERROR here"));
        assert!(m("", "anything"));
        for bad in ["(a", "a)", "[a", "*a", "a{3,1}", "[z-a]", "\\"] {
            assert!(pattern::Regex::new(bad).is_err(), "{bad}");
        }
        // Long lines are matched on their first 4,096 characters.
        let long = "a".repeat(100_000);
        assert!(m("^a*$", &long[..pattern::MAX_CHARS]));
        assert!(m("a+", &long) && m("^.*a$", &long[..4000]));
        // A `b` after the 4,096th character is never seen; one before it is.
        assert!(m("^a*$", &format!("{}b", &long[..4096])));
        assert!(!m("^a*$", &format!("{}b", &long[..4000])));
        assert!(m("^(ab)+$", &"ab".repeat(2000)));
    }

    #[test]
    fn values_are_cut_at_the_budget() {
        assert_eq!(cut_value("short", 200), ("short".to_owned(), false));
        let (v, cut) = cut_value(&"x".repeat(1000), 200);
        assert!(cut);
        assert_eq!(v, format!("{}\u{2026} (1000 chars)", "x".repeat(200)));
        let (v, _) = cut_value("\u{e9}\u{e9}\u{e9}", 2);
        assert_eq!(v, "\u{e9}\u{e9}\u{2026} (3 chars)");
    }

    #[test]
    fn the_inspection_outputs_follow_their_schemas() {
        let row = VarRow {
            name: "order".into(),
            value: "{App.Order}".into(),
            type_name: Some("App.Order".into()),
            reference: 4,
            evaluate_name: Some("order".into()),
            indexed: None,
            named: Some(2),
            value_truncated: false,
            children: Some(vec![VarRow {
                name: "Name".into(),
                value: "\"aaaa\u{2026} (900 chars)\"".into(),
                reference: 0,
                value_truncated: true,
                ..Default::default()
            }]),
            truncated: true,
        };
        let frame = StackFrameRow {
            index: 0,
            name: "App.Program.Main()".into(),
            path: Some("/s/Program.cs".into()),
            line: Some(5),
            column: Some(9),
            end_line: Some(5),
            end_column: Some(20),
            external: false,
        };
        let native = StackFrameRow {
            index: 1,
            name: "[Native Frames]".into(),
            external: true,
            ..Default::default()
        };
        let caps = CapabilitiesRow {
            adapter: "fake".into(),
            pause: true,
            log_points: "shell".into(),
            hit_conditions: "shell".into(),
            ..Default::default()
        };
        let output = OutputBlock {
            lines: vec![OutputLine {
                seq: 4,
                text: "hello".into(),
                stream: Some("stdout".into()),
            }],
            next: 5,
            dropped: 0,
            total: 5,
            truncated: false,
        };
        let summary = DebugOutput::Summary(Box::new(StopSummary {
            mode: "break".into(),
            generation: 1,
            stop: 3,
            stopped: Some(SummaryStopped {
                reason: "exception".into(),
                thread: 1,
                location: Some(LocationRow {
                    path: Some("/s/Program.cs".into()),
                    line: Some(5),
                    column: Some(9),
                    end_line: Some(5),
                    end_column: Some(20),
                    function: "App.Program.Main()".into(),
                }),
                exception: Some(ExceptionBrief {
                    type_name: Some("System.InvalidOperationException".into()),
                    message: Some("boom".into()),
                    break_mode: Some("always".into()),
                }),
                breakpoint: Some(BreakpointBrief {
                    path: "/s/Program.cs".into(),
                    line: 5,
                    hits: 1,
                }),
                driver: Some("agent:Claude Code".into()),
            }),
            frames: Some(FramesBlock {
                thread: 1,
                rows: vec![frame.clone(), native.clone()],
                total: 30,
                truncated: true,
            }),
            locals: Some(LocalsBlock {
                thread: 1,
                frame: 0,
                rows: vec![row.clone()],
                total: 120,
                truncated: true,
                next: Some(50),
            }),
            watches: Some(vec![SummaryWatch {
                expression: "x".into(),
                value: Some("1".into()),
                type_name: Some("int".into()),
                reference: 0,
                error: None,
                value_truncated: false,
            }]),
            output: output.clone(),
            exit_code: None,
            message: None,
            capabilities: Some(caps.clone()),
            agent_driving: true,
            truncated: true,
            satisfied: Some("stopped".into()),
            timed_out: None,
        }))
        .to_json();
        conforms(SUMMARY_OUTPUT, &summary);
        assert_eq!(summary["frames"]["rows"][1]["external"], true);
        assert!(summary["frames"]["rows"][0].get("external").is_none());
        let ended = DebugOutput::Summary(Box::new(StopSummary {
            mode: "design".into(),
            exit_code: Some(3),
            message: Some("The program exited with code 3.".into()),
            timed_out: Some(false),
            ..Default::default()
        }))
        .to_json();
        conforms(SUMMARY_OUTPUT, &ended);
        let stack = DebugOutput::Stack(StackOutput {
            threads: vec![StackThread {
                id: 1,
                name: "Main Thread".into(),
                frames: vec![frame, native],
                total: 31,
                truncated: true,
                next: Some(2),
            }],
            stop: 3,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-stack.output.json"),
            &stack,
        );
        let vars = DebugOutput::Variables(VariablesOutput {
            rows: vec![row],
            total: 10_000,
            truncated: true,
            next: Some(50),
            stop: 3,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-variables.output.json"),
            &vars,
        );
        let out = DebugOutput::Output(OutputPage {
            source: OutputKind::Adapter,
            lines: output.lines,
            next: 5,
            dropped: 2,
            total: 5,
            truncated: false,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-output.output.json"),
            &out,
        );
        assert_eq!(out["source"], "adapter");
        let info = DebugOutput::ExceptionInfo(ExceptionInfoOutput {
            supported: true,
            type_name: Some("System.InvalidOperationException".into()),
            message: Some("boom".into()),
            break_mode: Some("always".into()),
            details: Some(ExceptionDetailsRow {
                message: Some("boom".into()),
                type_name: Some("InvalidOperationException".into()),
                full_type_name: Some("System.InvalidOperationException".into()),
                stack_trace: Some("   at Main()".into()),
                inner_exceptions: vec![ExceptionDetailsRow {
                    message: Some("inner".into()),
                    ..Default::default()
                }],
            }),
            thread: 1,
            stop: 3,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-exception-info.output.json"),
            &info,
        );
        // The state gains the capabilities, `agent_driving` and the console's cursor.
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "running".into(),
            capabilities: Some(caps),
            agent_driving: true,
            console: ConsoleRow {
                lines: 2,
                tail: vec!["a".into(), "b".into()],
                next: 1,
            },
            ..Default::default()
        }))
        .to_json();
        conforms(STATE_OUTPUT, &state);
        assert_eq!(state["capabilities"]["adapter"], "fake");
        assert_eq!(state["console"]["next"], 1);
    }
}
