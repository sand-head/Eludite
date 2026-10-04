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
//! Brief 0026 (proposal 0001 brief B) adds run control: tracepoints (`toggle_breakpoint`'s `log_message`), function
//! breakpoints (`function`), exception settings per type (`exception_settings`' `types`), and the execute-class
//! commands `run_until` (one-shot breakpoints, resume, the next stop's summary), `trace` (install tracepoints, run,
//! collect their lines: [`TraceOutput`]), `set_variable` ([`SetVariableOutput`]) and `set_next_statement`. A
//! `log_message` with `{expression}` runs debuggee code at each hit, so `toggle_breakpoint` is of the execute class (the
//! class is per command; the policy's rules by input refine it).
//!
//! Brief 0027 (proposal 0001 brief C) adds `attach` (Debug > Attach to Process..., by `pid` or `process_name`;
//! [`AttachTarget`]), `processes` (the candidates, read class: [`ProcessesOutput`]), `restart` (Debug > Restart) and
//! `allow_agents` (Debug > Allow Agents to Drive: [`AllowAgentsOutput`]), `agents_allowed` in the state, `attached` on the
//! session, and `interrupted_by` on the stop summary (the person took over while an agent's command waited). The
//! commands that drive a session, run debuggee code or attach register escalation hooks ([`escalation`], ADR-0009) that
//! apply the solution policy's `debug` object ([`crate::policy::DebugPolicy`]); an attach to a process Eludite did not
//! start is dangerous.
//!
//! Brief 0028 (proposal 0001 brief D) debugs several processes at once: `session` (an id) on every command that acts
//! on a session ([`parse_with_session`], [`SESSION_COMMANDS`]; default: the active session), `sessions` (read class:
//! [`SessionsOutput`]), `start`'s `compound` ([`Compound`]: the solution's multiple startup projects or a list), and
//! `sessions` in the state, `session` and `sessions` on the stop summary, and each breakpoint's binding per session
//! ([`BreakpointSessionRow`]).
//!
//! Brief 0034 tunes the answers from the proving run: `toggle_breakpoint` answers with the changed breakpoint only
//! ([`ToggleBreakpointOutput`], `debug-toggle-breakpoint.output.json`) instead of the whole state, and a null reference
//! reads `null` on every adapter ([`null_spelling`]).
//!
//! Brief 0036 makes a breakpoint that cannot stop say so: the adapter's reason is on its row, `toggle_breakpoint`
//! answers it (`message`) once a live session's adapter answered, and the stop summary lists the session's failed
//! breakpoints (`breakpoints_failed`) and `run_until`'s points that never bound (`points_failed`), as `trace` does
//! ([`FailedBreakpointRow`]).
//!
//! The keys, the Debug menu, the margin, the debugger windows and agents all run these, against one state machine
//! in the shell (PLAN.md 5.5): see [`DebugTarget`]. `stop`, `state`, `select_frame`, `watch` and
//! `exception_settings` answer with the debugger's state ([`DebugState`], `debug-state.output.json`), which is what
//! the windows show; `toggle_breakpoint`, `evaluate`, `stack`, `variables`, `output` and `exception_info` with their
//! own outputs; the rest with the stop summary.
//!
//! The schemas are the files in `protocol/schemas/debug-*.json` (checked in first, CLAUDE.md invariant 4).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{DebugCall, PolicyView};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, EscalationHook, PermissionClass,
};

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
pub const RUN_UNTIL: &str = "eludite.debug.run_until";
pub const TRACE: &str = "eludite.debug.trace";
pub const SET_VARIABLE: &str = "eludite.debug.set_variable";
pub const SET_NEXT_STATEMENT: &str = "eludite.debug.set_next_statement";
pub const ATTACH: &str = "eludite.debug.attach";
pub const PROCESSES: &str = "eludite.debug.processes";
pub const RESTART: &str = "eludite.debug.restart";
pub const ALLOW_AGENTS: &str = "eludite.debug.allow_agents";
pub const SESSIONS: &str = "eludite.debug.sessions";

pub const ALL: [&str; 29] = [
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
    RUN_UNTIL,
    TRACE,
    SET_VARIABLE,
    SET_NEXT_STATEMENT,
    ATTACH,
    PROCESSES,
    RESTART,
    ALLOW_AGENTS,
    SESSIONS,
];

/// The commands that act on one debugging session and take `session` (brief 0028). `toggle_breakpoint` and
/// `exception_settings` change the shared settings of every session; `attach` adds a session; `processes` and
/// `sessions` act on none.
pub const SESSION_COMMANDS: [&str; 23] = [
    STOP,
    CONTINUE,
    STEP_OVER,
    STEP_INTO,
    STEP_OUT,
    RUN_TO_CURSOR,
    EVALUATE,
    STATE,
    SELECT_FRAME,
    WATCH,
    SNAPSHOT,
    STACK,
    VARIABLES,
    OUTPUT,
    EXCEPTION_INFO,
    PAUSE,
    WAIT,
    RUN_UNTIL,
    TRACE,
    SET_VARIABLE,
    SET_NEXT_STATEMENT,
    RESTART,
    ALLOW_AGENTS,
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
/// `run_until`'s and `trace`'s points, at most.
pub const MAX_POINTS: usize = 50;
/// How long `trace` collects by default.
pub const DEFAULT_TRACE_WAIT_MS: u64 = 10_000;
/// `trace`'s `max_hits`, default and most (also the most `count` of `until: hits`).
pub const DEFAULT_MAX_HITS: usize = 1_000;
pub const MAX_HITS: usize = 10_000;
/// A trace line's text is cut at this many characters.
pub const MAX_TRACE_TEXT: usize = 1_000;
/// Exception types in the settings, at most.
pub const MAX_EXCEPTION_TYPES: usize = 100;
/// `processes` lists at most this many rows.
pub const MAX_PROCESSES: usize = 500;
/// A process's command line is cut at this many characters.
pub const MAX_COMMAND_LINE: usize = 500;
/// The most projects a compound start lists (brief 0028).
pub const MAX_COMPOUND: usize = 20;
/// What an agent's command is refused with while Allow Agents to Drive is off.
pub const AGENTS_NOT_ALLOWED: &str =
    "agents are not allowed to drive this session (Debug > Allow Agents to Drive)";

const STATE_OUTPUT: &str = include_str!("../../../protocol/schemas/debug-state.output.json");
const SUMMARY_OUTPUT: &str =
    include_str!("../../../protocol/schemas/debug-stop-summary.output.json");
const TOGGLE_OUTPUT: &str =
    include_str!("../../../protocol/schemas/debug-toggle-breakpoint.output.json");

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
        // Run control (brief 0026): each runs the debuggee or changes its state.
        RUN_UNTIL => (
            "Debug: Run Until",
            input!("debug-run-until.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        TRACE => (
            "Debug: Trace",
            input!("debug-trace.input.json"),
            input!("debug-trace.output.json"),
            Execute,
        ),
        SET_VARIABLE => (
            "Debug: Set Value",
            input!("debug-set-variable.input.json"),
            input!("debug-set-variable.output.json"),
            Execute,
        ),
        SET_NEXT_STATEMENT => (
            "Debug: Set Next Statement",
            input!("debug-set-next-statement.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        // A tracepoint's `{expression}` runs debuggee code at every hit (brief 0026): the class is per command, so
        // toggle_breakpoint is execute; the policy's rules by input (`log_message`) refine it.
        TOGGLE_BREAKPOINT => (
            "Debug: Toggle Breakpoint",
            input!("debug-toggle-breakpoint.input.json"),
            TOGGLE_OUTPUT,
            Execute,
        ),
        // Attach and restart start sessions (brief 0027); attaching to a process Eludite did not start is dangerous,
        // through the escalation hook.
        ATTACH => (
            "Debug: Attach to Process",
            input!("debug-attach.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        RESTART => (
            "Debug: Restart",
            input!("debug-restart.input.json"),
            SUMMARY_OUTPUT,
            Execute,
        ),
        PROCESSES => (
            "Debug: Processes",
            input!("debug-processes.input.json"),
            input!("debug-processes.output.json"),
            Read,
        ),
        // Who may drive the session: the person's control; an agent may only turn it off.
        ALLOW_AGENTS => (
            "Debug: Allow Agents to Drive",
            input!("debug-allow-agents.input.json"),
            input!("debug-allow-agents.output.json"),
            Execute,
        ),
        // The list of sessions (brief 0028) runs no debuggee code.
        SESSIONS => (
            "Debug: Sessions",
            input!("debug-sessions.input.json"),
            input!("debug-sessions.output.json"),
            Read,
        ),
        // Watches, frame selection and exception settings change what the debugger shows and where it stops, not
        // files or processes.
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

/// The one spelling of a null reference in a value (brief 0034): eludite-dbg-mono's `(null)` reads `null`, as
/// netcoredbg, lldb-dap's C# formatters and Visual Studio's C# Locals window show it. Every other value is unchanged.
pub fn null_spelling(value: String) -> String {
    if value == "(null)" {
        "null".to_owned()
    } else {
        value
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

/// What `eludite.debug.start` asks of a Cargo package (brief 0029): the binary target (or test target), whether to
/// debug the test executable, and the arguments (with `test`, the harness's filter). `.NET` projects take none.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CargoOptions {
    pub target: Option<String>,
    pub test: bool,
    pub args: Option<Vec<String>>,
}

impl CargoOptions {
    /// Whether any is given (a .NET project refuses them).
    pub fn is_set(&self) -> bool {
        self.target.is_some() || self.test || self.args.is_some()
    }
}

/// Where `eludite.debug.start` opens a web project's page (brief 0037).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BrowserChoice {
    /// The Web Browser window (the system browser when its engine is not found).
    BuiltIn,
    /// The system browser (`eludite.browser.open_external`): Debug > Start in External Browser.
    External,
    /// No page.
    None,
}

/// The page a launch opened or is opening (`debug-state.output.json`'s `$defs/browser`; brief 0037).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionBrowser {
    /// The Web Browser window's tab, once it opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    pub url: String,
    /// `embedded` (a tab of the Web Browser window) or `system` (the system browser, no tab).
    pub engine: String,
    /// `waiting`, `opened` or `failed`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl SessionBrowser {
    /// Still waiting for the server.
    pub fn waiting(&self) -> bool {
        self.state == "waiting"
    }
}

/// A point of `run_until`: stop at `line` of `path` (when `condition` holds).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RunPoint {
    pub path: String,
    pub line: u32,
    #[serde(default)]
    pub condition: Option<String>,
}

/// A point of `trace`: print `message` at `line` of `path` (when `condition` holds) and continue.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePoint {
    pub path: String,
    pub line: u32,
    pub message: String,
    #[serde(default)]
    pub condition: Option<String>,
}

/// `trace`'s `run`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TraceRun {
    #[default]
    Continue,
    Start,
}

/// `trace`'s `until` (with `count` for `hits`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TraceUntil {
    #[default]
    Terminated,
    Stopped,
    Hits(usize),
}

/// What `start` takes, for `trace` with `run: start`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StartParams {
    pub project: Option<String>,
    pub profile: Option<String>,
    pub build: Option<bool>,
}

/// What `set_variable` changes: a variable of a frame, or a member of a value by its reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SetTarget {
    Frame {
        thread: Option<i64>,
        frame: Option<usize>,
    },
    Reference(i64),
}

/// `exception_settings`' `types` entries (and `debug-state.output.json`'s rows).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionTypeRow {
    #[serde(rename = "type")]
    pub type_name: String,
    pub break_when_thrown: bool,
    pub break_when_user_unhandled: bool,
}

/// One project of a compound start (brief 0028): the project, whether to debug it, its launch profile.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompoundEntry {
    pub project: String,
    #[serde(default = "yes")]
    pub debug: bool,
    #[serde(default)]
    pub profile: Option<String>,
}

/// A compound's `browser` entry (brief 0038): attach vscode-js-debug to a server entry's page once it is up.
#[derive(Debug, Clone, PartialEq, Eq, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BrowserEntry {
    /// The server entry it waits for (its `project`); `None`: the compound's first debugged web project.
    #[serde(default)]
    pub project: Option<String>,
    #[serde(default)]
    pub tab: Option<String>,
    #[serde(default)]
    pub url: Option<String>,
    #[serde(default)]
    pub web_root: Option<String>,
}

/// One item of `compound`'s list: a project, or a `browser` entry.
#[derive(Deserialize)]
#[serde(untagged)]
enum CompoundItem {
    Browser { browser: BrowserEntry },
    Project(CompoundEntry),
}

/// `start`'s `compound` (brief 0028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Compound {
    /// The solution's multiple startup projects with their actions.
    Startup,
    /// These projects, each in its own session.
    Projects(Vec<CompoundEntry>),
}

/// Which process (or page) `attach` names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachTarget {
    Pid(u32),
    /// A unique process name (compared without regard to case, the extension optional).
    Name(String),
    /// A tab of Eludite's browser, debugged with vscode-js-debug (brief 0038).
    Tab(String),
    /// A page url of Eludite's browser, or a Chrome DevTools websocket url (brief 0038).
    Url(String),
    /// Neither: from the UI, the Attach to Process dialog opens.
    Dialog,
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
        /// A Cargo package's target, test executable and arguments (brief 0029).
        cargo: CargoOptions,
        /// Several projects, each in its own session (brief 0028).
        compound: Option<Compound>,
        /// The compound's `browser` entries (brief 0038).
        browsers: Vec<BrowserEntry>,
        /// Where a web project's page opens (brief 0037); `None`: its launch profile and the settings decide.
        browser: Option<BrowserChoice>,
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
        /// A tracepoint's message; `Some("")` turns it back into a breakpoint.
        log_message: Option<String>,
        /// A function breakpoint by name (no path or line).
        function: Option<String>,
        remove_after: Option<bool>,
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
        /// The Rust panics row (brief 0029).
        break_on_rust_panic: Option<bool>,
        /// Types added or changed.
        types: Vec<ExceptionTypeRow>,
        /// A type removed.
        remove: Option<String>,
        /// Every type removed first.
        clear: bool,
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
    RunUntil {
        points: Vec<RunPoint>,
        remove_after: bool,
        stop: Option<u64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Trace {
        points: Vec<TracePoint>,
        run: TraceRun,
        start: StartParams,
        until: TraceUntil,
        wait_ms: u64,
        max_hits: usize,
        stop: Option<u64>,
        budget: Budget,
    },
    SetVariable {
        target: SetTarget,
        name: String,
        value: String,
        stop: Option<u64>,
    },
    SetNextStatement {
        path: Option<String>,
        line: Option<u32>,
        thread: Option<i64>,
        stop: Option<u64>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Attach {
        target: AttachTarget,
        /// `coreclr`, `mono`, `netfx` or `lldb`; `None`: from the process's runtime.
        adapter: Option<String>,
        /// An adapter listening at (host, port) instead of one started here.
        transport: Option<(String, u16)>,
        /// A Mono program's debugger agent: (address, port).
        mono: Option<(String, u16)>,
        /// A page's web root for vscode-js-debug (brief 0038).
        web_root: Option<String>,
        wait_ms: Option<u64>,
        budget: Budget,
    },
    Processes {
        filter: Option<String>,
    },
    Restart {
        wait_ms: Option<u64>,
        budget: Budget,
    },
    AllowAgents {
        enabled: bool,
    },
    /// The live sessions (brief 0028).
    Sessions,
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
            DebugRequest::RunUntil { .. } => RUN_UNTIL,
            DebugRequest::Trace { .. } => TRACE,
            DebugRequest::SetVariable { .. } => SET_VARIABLE,
            DebugRequest::SetNextStatement { .. } => SET_NEXT_STATEMENT,
            DebugRequest::Attach { .. } => ATTACH,
            DebugRequest::Processes { .. } => PROCESSES,
            DebugRequest::Restart { .. } => RESTART,
            DebugRequest::AllowAgents { .. } => ALLOW_AGENTS,
            DebugRequest::Sessions => SESSIONS,
        }
    }

    /// How long an agent asked to wait for the debuggee.
    pub fn wait_ms(&self) -> Option<u64> {
        match self {
            DebugRequest::Start { wait_ms, .. }
            | DebugRequest::Continue { wait_ms, .. }
            | DebugRequest::Step { wait_ms, .. }
            | DebugRequest::RunToCursor { wait_ms, .. }
            | DebugRequest::Pause { wait_ms, .. }
            | DebugRequest::RunUntil { wait_ms, .. }
            | DebugRequest::SetNextStatement { wait_ms, .. }
            | DebugRequest::Attach { wait_ms, .. }
            | DebugRequest::Restart { wait_ms, .. } => *wait_ms,
            DebugRequest::Wait { wait_ms, .. } | DebugRequest::Trace { wait_ms, .. } => {
                Some(*wait_ms)
            }
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
            | DebugRequest::Wait { budget, .. }
            | DebugRequest::RunUntil { budget, .. }
            | DebugRequest::Trace { budget, .. }
            | DebugRequest::SetNextStatement { budget, .. }
            | DebugRequest::Attach { budget, .. }
            | DebugRequest::Restart { budget, .. } => Some(*budget),
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
                | DebugRequest::RunUntil { .. }
                | DebugRequest::Trace { .. }
                | DebugRequest::SetNextStatement { .. }
                | DebugRequest::Stop
                | DebugRequest::Attach { .. }
                | DebugRequest::Restart { .. }
        )
    }

    /// Whether Allow Agents to Drive governs it (brief 0027): the commands that start, attach to, restart, resume or
    /// change the session; a breakpoint edit only for a tracepoint message with an expression. Reads, watches, frame
    /// selection, exception settings, plain breakpoints and `evaluate` are not driving.
    pub fn drives(&self) -> bool {
        match self {
            DebugRequest::Breakpoint { log_message, .. } => {
                log_message.as_deref().is_some_and(has_expression)
            }
            DebugRequest::Pause { .. } | DebugRequest::SetVariable { .. } => true,
            DebugRequest::Attach {
                target: AttachTarget::Dialog,
                ..
            } => false,
            other => other.resumes(),
        }
    }
}

/// Whether a tracepoint message has an `{expression}` (`{{` and `}}` are literal braces).
pub fn has_expression(message: &str) -> bool {
    message.replace("{{", "").replace("}}", "").contains('{')
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
    /// The session's id (brief 0028).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u32>,
    pub project: String,
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub profile: Option<String>,
    pub debug: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// What runs the program (brief 0022): `coreclr`, `mono` or `netfx`; `native` for a Cargo package (brief 0029).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i64>,
    /// Attached to a running process (brief 0027): Stop detaches.
    #[serde(default, skip_serializing_if = "is_false")]
    pub attached: bool,
    /// A child session's parent (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    /// A browser session's tab and page url (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    /// The web project's page (brief 0037).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<SessionBrowser>,
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
    /// Through a source map: the original and generated places (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<FrameSourceRow>,
}

/// A frame's `source` when a source map applies (brief 0038): the original file (the frame's `path`) and the generated
/// script's place.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FrameSourceRow {
    pub original: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generated_column: Option<u32>,
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

/// A breakpoint row's `kind`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointKind {
    #[default]
    Line,
    Tracepoint,
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointRow {
    #[serde(default)]
    pub kind: BreakpointKind,
    /// Absent for a function breakpoint.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub enabled: bool,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub hit_condition: Option<String>,
    pub hits: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub log_message: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub remove_after: bool,
    /// `run_until`'s or `trace`'s, for the length of that call.
    #[serde(default, skip_serializing_if = "is_false")]
    pub temporary: bool,
    /// Its binding in each live session (brief 0028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<BreakpointSessionRow>,
}

/// What `toggle_breakpoint` did (`debug-toggle-breakpoint.output.json`'s `action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BreakpointEdit {
    Added,
    Changed,
    Deleted,
    DeletedAll,
}

/// `debug-toggle-breakpoint.output.json` (brief 0034): the compact answer of `toggle_breakpoint`, the changed
/// breakpoint's row instead of the whole state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToggleBreakpointOutput {
    pub action: BreakpointEdit,
    /// Absent after a delete.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub breakpoint: Option<BreakpointRow>,
    /// Bound by a live session's adapter now.
    pub verified: bool,
    /// Sent to a live session's adapter, whose answer comes later.
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
    /// Why a live session's adapter refused it, once it answered (brief 0036).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// The active session, while one runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    pub breakpoints_total: usize,
}

/// A breakpoint's binding in one session (`debug-state.output.json`'s `breakpoints[].sessions`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BreakpointSessionRow {
    pub session: u32,
    pub verified: bool,
    pub hits: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// A debugging session as `eludite.debug.sessions` and the state's `sessions` list it (brief 0028).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionInfo {
    pub id: u32,
    pub name: String,
    pub mode: String,
    pub active: bool,
    pub generation: u64,
    pub stop: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter: Option<String>,
    /// The adapter's and its runtime's versions (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub adapter_version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<i64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub attached: bool,
    /// A child session's parent (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    /// A browser session's tab and page url (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tab: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    #[serde(default = "yes")]
    pub agents_allowed: bool,
    /// In break mode: the stop's reason.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stopped: Option<String>,
}

/// `debug-sessions.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SessionsOutput {
    pub sessions: Vec<SessionInfo>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub active: Option<u32>,
}

/// A session of a compound start's answer (`debug-stop-summary.output.json`'s `sessions`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompoundSessionRow {
    pub id: u32,
    pub name: String,
    pub mode: String,
}

impl BreakpointRow {
    /// How the Breakpoints window names it: `Program.cs, line 6`, or the function.
    pub fn label(&self) -> String {
        match (&self.function, &self.path, self.line) {
            (Some(f), _, _) => f.clone(),
            (None, Some(p), Some(l)) => format!(
                "{}, line {l}",
                std::path::Path::new(p)
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| p.clone())
            ),
            _ => String::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExceptionSettingsRow {
    pub break_when_thrown: bool,
    pub break_when_user_unhandled: bool,
    /// Rust panics (brief 0029): a native session breaks at `rust_panic`. Absent from files saved before: on.
    #[serde(default = "rust_panics_default")]
    pub break_on_rust_panic: bool,
    /// Exception types under the category (brief 0026).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub types: Vec<ExceptionTypeRow>,
}

fn rust_panics_default() -> bool {
    true
}

impl Default for ExceptionSettingsRow {
    /// Visual Studio's default: break on exceptions user code does not handle, not on every throw; and on Rust panics
    /// (what rust-lldb users set).
    fn default() -> Self {
        Self {
            break_when_thrown: false,
            break_when_user_unhandled: true,
            break_on_rust_panic: true,
            types: Vec::new(),
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
    /// Agents may drive the session (Debug > Allow Agents to Drive; brief 0027).
    #[serde(default = "yes")]
    pub agents_allowed: bool,
    /// Every live session (brief 0028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<SessionInfo>,
}

fn yes() -> bool {
    true
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
    /// Through a source map: the original and generated places (brief 0038).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<FrameSourceRow>,
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
    /// The session it describes (brief 0028).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
    /// The session's web page (brief 0037): a start with `wait_ms` names the tab it opened.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub browser: Option<SessionBrowser>,
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
    /// `user`: the person resumed, paused, stopped or restarted the debuggee while this agent's command waited
    /// (proposal 0001 rule 5).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_by: Option<String>,
    /// A compound start that timed out before any session broke: every session's mode (brief 0028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub sessions: Vec<CompoundSessionRow>,
    /// The session's breakpoints that its adapter refused or whose condition it rejected (brief 0036).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub breakpoints_failed: Vec<FailedBreakpointRow>,
    /// `run_until`: its points the adapter had not bound when it answered (brief 0036).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points_failed: Vec<FailedBreakpointRow>,
}

/// A breakpoint, or a `run_until` or `trace` point, that a session's adapter did not bind or whose condition it
/// rejected (`debug-stop-summary.output.json`'s and `debug-trace.output.json`'s `failed`; brief 0036).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FailedBreakpointRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub function: Option<String>,
    pub session: u32,
    pub message: String,
}

/// Whether an adapter's message for a breakpoint it has not bound says it is still pending (its code has not loaded,
/// the session has not started) rather than refused: netcoredbg's "pending", eludite-dbg-mono's "will not currently be
/// hit", "could not yet be bound" and "will bind when its code loads" (brief 0036); vscode-js-debug's
/// "breakpoint.provisionalBreakpoint" (set before the page's script loaded) and "Unbound breakpoint" (no loaded script
/// has the line yet; brief 0038). Anything else is a refusal.
pub fn pending_message(message: &str) -> bool {
    let m = message.to_ascii_lowercase();
    [
        "pending",
        "provisional",
        "unbound breakpoint",
        "not currently be hit",
        "not yet",
        "will bind",
        "will be resolved",
        "when its code loads",
    ]
    .iter()
    .any(|p| m.contains(p))
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

/// One line of `debug-trace.output.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceLine {
    pub seq: u64,
    pub path: String,
    pub line: u32,
    pub hit: u32,
    pub time_ms: f64,
    pub text: String,
}

/// One point of `debug-trace.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TracePointRow {
    pub path: String,
    pub line: u32,
    pub hits: u32,
    pub verified: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `debug-trace.output.json`.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TraceOutput {
    pub lines: Vec<TraceLine>,
    pub hits: u64,
    pub truncated: bool,
    /// `terminated`, `stopped`, `hits` or `timeout`.
    pub stopped_by: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<Box<StopSummary>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i64>,
    pub points: Vec<TracePointRow>,
    /// The points that never bound, with why (brief 0036).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub points_failed: Vec<FailedBreakpointRow>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub overhead_ms_per_hit: Option<f64>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub emulated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generation: Option<u64>,
}

/// `debug-set-variable.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetVariableOutput {
    pub name: String,
    pub value: String,
    #[serde(rename = "type", default, skip_serializing_if = "Option::is_none")]
    pub type_name: Option<String>,
    pub reference: i64,
    /// `setVariable` or `setExpression`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request: Option<String>,
    #[serde(default, skip_serializing_if = "is_false")]
    pub pending: bool,
    pub stop: u64,
}

/// One row of `debug-processes.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessRow {
    pub pid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parent: Option<u32>,
    pub name: String,
    pub command_line: String,
    /// `dotnet`, `netfx`, `mono`, `native` or `unknown`.
    pub runtime: String,
    pub launched_by_eludite: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debugger_agent: Option<String>,
}

/// `debug-processes.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProcessesOutput {
    pub processes: Vec<ProcessRow>,
    pub total: usize,
    pub truncated: bool,
    /// The browser's tabs, attachable by `tab` (brief 0038); absent while no browser runs.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tabs: Option<Vec<AttachTabRow>>,
}

/// A tab `eludite.debug.processes` lists (brief 0038).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachTabRow {
    pub id: String,
    pub title: String,
    pub url: String,
    /// The debugging session attached to it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session: Option<u32>,
}

/// `debug-allow-agents.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AllowAgentsOutput {
    pub agents_allowed: bool,
    pub default: bool,
    pub mode: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum DebugOutput {
    State(Box<DebugState>),
    Breakpoint(Box<ToggleBreakpointOutput>),
    Evaluate(EvaluateOutput),
    Summary(Box<StopSummary>),
    Stack(StackOutput),
    Variables(VariablesOutput),
    Output(OutputPage),
    ExceptionInfo(ExceptionInfoOutput),
    Trace(Box<TraceOutput>),
    SetVariable(SetVariableOutput),
    Processes(ProcessesOutput),
    AllowAgents(AllowAgentsOutput),
    Sessions(SessionsOutput),
}

impl DebugOutput {
    pub fn to_json(&self) -> Value {
        match self {
            DebugOutput::State(s) => serde_json::to_value(s),
            DebugOutput::Breakpoint(b) => serde_json::to_value(b),
            DebugOutput::Evaluate(e) => serde_json::to_value(e),
            DebugOutput::Summary(s) => serde_json::to_value(s),
            DebugOutput::Stack(s) => serde_json::to_value(s),
            DebugOutput::Variables(v) => serde_json::to_value(v),
            DebugOutput::Output(o) => serde_json::to_value(o),
            DebugOutput::ExceptionInfo(e) => serde_json::to_value(e),
            DebugOutput::Trace(t) => serde_json::to_value(t),
            DebugOutput::SetVariable(v) => serde_json::to_value(v),
            DebugOutput::Processes(p) => serde_json::to_value(p),
            DebugOutput::AllowAgents(a) => serde_json::to_value(a),
            DebugOutput::Sessions(s) => serde_json::to_value(s),
        }
        .expect("debug outputs serialize")
    }
}

/// Whatever owns the debugger (the shell). Called on the invoking thread with the session the call named (brief 0028;
/// `None`: the active one, or for `stop` every one).
pub trait DebugTarget: Send + Sync {
    fn apply(
        &self,
        session: Option<u32>,
        request: DebugRequest,
    ) -> Result<DebugOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StartIn {
    project: Option<String>,
    compound: Option<Value>,
    debug: Option<bool>,
    profile: Option<String>,
    build: Option<bool>,
    target: Option<String>,
    test: Option<bool>,
    args: Option<Vec<String>>,
    browser: Option<BrowserChoice>,
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
    log_message: Option<String>,
    function: Option<String>,
    remove_after: Option<bool>,
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
    break_on_rust_panic: Option<bool>,
    #[serde(default)]
    types: Vec<ExceptionTypeIn>,
    remove: Option<String>,
    #[serde(default)]
    clear: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ExceptionTypeIn {
    #[serde(rename = "type")]
    type_name: String,
    break_when_thrown: Option<bool>,
    break_when_user_unhandled: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RunUntilIn {
    #[serde(default)]
    points: Vec<RunPoint>,
    remove_after: Option<bool>,
    wait_ms: Option<u64>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default, PartialEq, Eq, Clone, Copy)]
#[serde(rename_all = "snake_case")]
enum UntilIn {
    #[default]
    Terminated,
    Stopped,
    Hits,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TraceIn {
    #[serde(default)]
    points: Vec<TracePoint>,
    #[serde(default)]
    run: TraceRun,
    project: Option<String>,
    profile: Option<String>,
    build: Option<bool>,
    #[serde(default)]
    until: UntilIn,
    count: Option<usize>,
    wait_ms: Option<u64>,
    max_hits: Option<usize>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SetVariableIn {
    #[serde(default)]
    name: String,
    #[serde(default)]
    value: String,
    reference: Option<i64>,
    thread: Option<i64>,
    frame: Option<usize>,
    stop: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NextStatementIn {
    path: Option<String>,
    line: Option<u32>,
    thread: Option<i64>,
    stop: Option<u64>,
    wait_ms: Option<u64>,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct TransportIn {
    kind: String,
    host: String,
    port: u16,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MonoIn {
    address: Option<String>,
    port: u16,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AttachIn {
    pid: Option<u32>,
    process_name: Option<String>,
    tab: Option<String>,
    url: Option<String>,
    web_root: Option<String>,
    adapter: Option<String>,
    transport: Option<TransportIn>,
    mono: Option<MonoIn>,
    wait_ms: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProcessesIn {
    filter: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RestartIn {
    wait_ms: Option<u64>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AllowAgentsIn {
    enabled: bool,
}

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

/// A tab id (`t1`), when given (brief 0038).
fn check_tab(tab: Option<&str>) -> Result<(), CommandError> {
    match tab {
        Some(t)
            if !(t.len() > 1
                && t.starts_with('t')
                && t[1..].bytes().all(|b| b.is_ascii_digit())) =>
        {
            Err(invalid(format!(
                "`tab` is a tab id as eludite.browser.tabs lists it (`t1`), not `{t}`"
            )))
        }
        _ => Ok(()),
    }
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

/// Parse and validate the input of debug command `id`, without its `session` (see [`parse_with_session`]).
pub fn parse(id: &str, value: Value) -> Result<DebugRequest, CommandError> {
    parse_with_session(id, value).map(|(_, r)| r)
}

/// Parse and validate the input of debug command `id` and the session it names (brief 0028): `session` is an id from
/// 1 on, taken by the commands of [`SESSION_COMMANDS`] only.
pub fn parse_with_session(
    id: &str,
    mut value: Value,
) -> Result<(Option<u32>, DebugRequest), CommandError> {
    let session = match value.as_object_mut().and_then(|o| o.remove("session")) {
        None | Some(Value::Null) => None,
        Some(_) if !SESSION_COMMANDS.contains(&id) => {
            return Err(invalid(format!(
                "{id} takes no `session`: it acts on every session or adds one"
            )));
        }
        Some(v) => Some(
            v.as_u64()
                .filter(|n| *n >= 1)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(|| invalid("`session` is a session id, an integer from 1"))?,
        ),
    };
    parse_request(id, value).map(|r| (session, r))
}

fn parse_request(id: &str, mut value: Value) -> Result<DebugRequest, CommandError> {
    // The commands that answer with the stop summary take its budget parameters.
    let budget = match id {
        START | CONTINUE | STEP_OVER | STEP_INTO | STEP_OUT | RUN_TO_CURSOR | SNAPSHOT | PAUSE
        | WAIT | RUN_UNTIL | TRACE | SET_NEXT_STATEMENT | ATTACH | RESTART => {
            take_budget(&mut value)?
        }
        _ => Budget::default(),
    };
    Ok(match id {
        START => {
            let i: StartIn = input(value)?;
            if i.args.as_ref().is_some_and(|a| a.len() > 100) {
                return Err(invalid("`args` takes at most 100 arguments"));
            }
            let mut browsers = Vec::new();
            let compound = match i.compound {
                None | Some(Value::Null) => None,
                Some(Value::String(s)) if s == "startup" => Some(Compound::Startup),
                Some(Value::String(s)) => {
                    return Err(invalid(format!(
                        "`compound` is \"startup\" or a list of projects, not `{s}`"
                    )));
                }
                Some(v @ Value::Array(_)) => {
                    let items: Vec<CompoundItem> = serde_json::from_value(v).map_err(|_| {
                        invalid(
                            "`compound` lists `{ project, debug?, profile? }` and `{ browser: { project?, tab?, \
                             url?, web_root? } }` entries",
                        )
                    })?;
                    if items.is_empty() || items.len() > MAX_COMPOUND {
                        return Err(invalid(format!(
                            "`compound` lists 1 to {MAX_COMPOUND} projects"
                        )));
                    }
                    let mut entries = Vec::new();
                    for item in items {
                        match item {
                            CompoundItem::Project(e) => entries.push(e),
                            CompoundItem::Browser { browser } => {
                                check_tab(browser.tab.as_deref())?;
                                non_empty("browser.project", browser.project.clone())?;
                                non_empty("browser.url", browser.url.clone())?;
                                non_empty("browser.web_root", browser.web_root.clone())?;
                                browsers.push(browser);
                            }
                        }
                    }
                    if entries.is_empty() {
                        return Err(invalid(
                            "`compound` needs a project to start; a `browser` entry attaches to a project's page",
                        ));
                    }
                    for e in &entries {
                        if e.project.trim().is_empty() {
                            return Err(invalid("a compound project must not be empty"));
                        }
                        non_empty("profile", e.profile.clone())?;
                    }
                    Some(Compound::Projects(entries))
                }
                Some(_) => {
                    return Err(invalid(
                        "`compound` is \"startup\" or a list of `{ project, debug?, profile? }`",
                    ));
                }
            };
            if compound.is_some()
                && (i.project.is_some()
                    || i.profile.is_some()
                    || i.target.is_some()
                    || i.test.is_some()
                    || i.args.is_some())
            {
                return Err(invalid(
                    "`compound` names its projects: not with `project`, `profile`, `target`, `test` or `args`",
                ));
            }
            DebugRequest::Start {
                compound,
                browsers,
                project: non_empty("project", i.project)?,
                debug: i.debug.unwrap_or(true),
                profile: non_empty("profile", i.profile)?,
                build: i.build,
                cargo: CargoOptions {
                    target: non_empty("target", i.target)?,
                    test: i.test.unwrap_or(false),
                    args: i.args,
                },
                browser: i.browser,
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
            let editing = i.enabled.is_some()
                || i.condition.is_some()
                || i.hit_condition.is_some()
                || i.log_message.is_some()
                || i.remove_after.is_some();
            if editing && i.action != BreakpointAction::Set {
                return Err(invalid(
                    "`enabled`, `condition`, `hit_condition`, `log_message` and `remove_after` go with `action: \"set\"`",
                ));
            }
            let function = match i.function {
                Some(f) if f.trim().is_empty() => {
                    return Err(invalid("`function` must not be empty"));
                }
                Some(f) => {
                    if i.path.is_some() || i.line.is_some() {
                        return Err(invalid(
                            "a function breakpoint has no `path` or `line`: give `function` alone",
                        ));
                    }
                    if !matches!(i.action, BreakpointAction::Set | BreakpointAction::Delete) {
                        return Err(invalid(
                            "`function` goes with `action: \"set\"` or `\"delete\"`",
                        ));
                    }
                    if i.log_message.is_some() {
                        return Err(invalid(
                            "`log_message` is for line breakpoints: a function breakpoint has no message (DAP \
                             function breakpoints carry none)",
                        ));
                    }
                    Some(f.trim().to_owned())
                }
                None => None,
            };
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
                log_message: i.log_message,
                function,
                remove_after: i.remove_after,
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
            if i.types.len() > MAX_EXCEPTION_TYPES {
                return Err(invalid(format!(
                    "`types` lists at most {MAX_EXCEPTION_TYPES} types"
                )));
            }
            let check_type = |t: &str| -> Result<String, CommandError> {
                let t = t.trim();
                if t.is_empty() || t.contains(|c: char| c == ',' || c.is_whitespace()) {
                    return Err(invalid(format!(
                        "`{t}` is not an exception type name (`System.InvalidOperationException`)"
                    )));
                }
                Ok(t.to_owned())
            };
            let mut types = Vec::new();
            for t in i.types {
                types.push(ExceptionTypeRow {
                    type_name: check_type(&t.type_name)?,
                    break_when_thrown: t.break_when_thrown.unwrap_or(true),
                    break_when_user_unhandled: t.break_when_user_unhandled.unwrap_or(true),
                });
            }
            DebugRequest::ExceptionSettings {
                break_when_thrown: i.break_when_thrown,
                break_when_user_unhandled: i.break_when_user_unhandled,
                break_on_rust_panic: i.break_on_rust_panic,
                types,
                remove: i.remove.as_deref().map(check_type).transpose()?,
                clear: i.clear,
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
        RUN_UNTIL => {
            let i: RunUntilIn = input(value)?;
            check_points(i.points.len())?;
            for p in &i.points {
                check_point(&p.path, p.line)?;
            }
            DebugRequest::RunUntil {
                points: i.points,
                remove_after: i.remove_after.unwrap_or(true),
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        TRACE => {
            let i: TraceIn = input(value)?;
            check_points(i.points.len())?;
            for p in &i.points {
                check_point(&p.path, p.line)?;
                if p.message.trim().is_empty() {
                    return Err(invalid("a point's `message` must not be empty"));
                }
            }
            let starts = i.project.is_some() || i.profile.is_some() || i.build.is_some();
            if starts && i.run != TraceRun::Start {
                return Err(invalid(
                    "`project`, `profile` and `build` go with `run: \"start\"`",
                ));
            }
            if i.run == TraceRun::Start && i.stop.is_some() {
                return Err(invalid(
                    "`stop` goes with `run: \"continue\"`: `run: \"start\"` starts a session",
                ));
            }
            let until = match (i.until, i.count) {
                (UntilIn::Hits, Some(n)) if (1..=MAX_HITS).contains(&n) => TraceUntil::Hits(n),
                (UntilIn::Hits, Some(_)) => {
                    return Err(invalid(format!("`count` is 1 to {MAX_HITS}")));
                }
                (UntilIn::Hits, None) => {
                    return Err(invalid("`until: \"hits\"` needs `count`"));
                }
                (_, Some(_)) => return Err(invalid("`count` goes with `until: \"hits\"`")),
                (UntilIn::Terminated, None) => TraceUntil::Terminated,
                (UntilIn::Stopped, None) => TraceUntil::Stopped,
            };
            DebugRequest::Trace {
                points: i.points,
                run: i.run,
                start: StartParams {
                    project: non_empty("project", i.project)?,
                    profile: non_empty("profile", i.profile)?,
                    build: i.build,
                },
                until,
                wait_ms: check_wait(i.wait_ms)?.unwrap_or(DEFAULT_TRACE_WAIT_MS),
                max_hits: bounded("max_hits", i.max_hits, DEFAULT_MAX_HITS, 1, MAX_HITS)?,
                stop: check_stop(i.stop)?,
                budget,
            }
        }
        SET_VARIABLE => {
            let i: SetVariableIn = input(value)?;
            if i.name.trim().is_empty() {
                return Err(invalid("`name` is required"));
            }
            if i.value.trim().is_empty() {
                return Err(invalid("`value` is required (a C# expression)"));
            }
            let target = match i.reference {
                Some(_) if i.thread.is_some() || i.frame.is_some() => {
                    return Err(invalid(
                        "give `reference` or `thread` and `frame`, not both",
                    ));
                }
                Some(r) if r < 1 => return Err(invalid("`reference` is at least 1")),
                Some(r) => SetTarget::Reference(r),
                None => SetTarget::Frame {
                    thread: i.thread,
                    frame: i.frame,
                },
            };
            DebugRequest::SetVariable {
                target,
                name: i.name.trim().to_owned(),
                value: i.value,
                stop: check_stop(i.stop)?,
            }
        }
        SET_NEXT_STATEMENT => {
            let i: NextStatementIn = input(value)?;
            DebugRequest::SetNextStatement {
                path: non_empty("path", i.path)?,
                line: check_line(i.line)?,
                thread: i.thread,
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        ATTACH => {
            let i: AttachIn = input(value)?;
            let tab = i.tab.clone();
            check_tab(tab.as_deref())?;
            let url = non_empty("url", i.url.clone())?;
            if url.as_ref().is_some_and(|u| u.chars().count() > 2048) {
                return Err(invalid("`url` is at most 2048 characters"));
            }
            let process = i.pid.is_some() || i.process_name.is_some();
            let page = tab.is_some() || url.is_some();
            if process && page || tab.is_some() && url.is_some() {
                return Err(invalid(
                    "name one target: `pid`, `process_name`, `tab` or `url`",
                ));
            }
            let target = match (i.pid, non_empty("process_name", i.process_name)?, tab, url) {
                (Some(0), ..) => return Err(invalid("`pid` is at least 1")),
                (Some(pid), ..) => AttachTarget::Pid(pid),
                (None, Some(name), ..) => AttachTarget::Name(name.trim().to_owned()),
                (None, None, Some(tab), _) => AttachTarget::Tab(tab),
                (None, None, None, Some(url)) => AttachTarget::Url(url),
                (None, None, None, None) => AttachTarget::Dialog,
            };
            let adapter = match i.adapter.as_deref() {
                None if page => Some("javascript".to_owned()),
                None => i.mono.as_ref().map(|_| "mono".to_owned()),
                Some("javascript") if process => {
                    return Err(invalid(
                        "a browser is attached by tab or url, not by process: pass `tab` (eludite.browser.tabs' \
                         id) or `url` with adapter `javascript`",
                    ));
                }
                Some(a) if page && a != "javascript" => {
                    return Err(invalid(format!(
                        "a page (`tab` or `url`) is debugged with adapter `javascript`, not `{a}`"
                    )));
                }
                Some(a @ ("coreclr" | "mono" | "netfx" | "lldb" | "javascript")) => {
                    Some(a.to_owned())
                }
                Some(other) => {
                    return Err(invalid(format!(
                        "`adapter` is coreclr, mono, netfx, lldb or javascript, not `{other}`"
                    )));
                }
            };
            let js = adapter.as_deref() == Some("javascript");
            if js && (i.mono.is_some() || i.transport.is_some()) {
                return Err(invalid(
                    "`mono` and `transport` are for a process; a page needs `tab` or `url`",
                ));
            }
            if i.web_root.is_some() && !js {
                return Err(invalid("`web_root` is for a page (`tab` or `url`)"));
            }
            let web_root = non_empty("web_root", i.web_root)?;
            if i.mono.is_some() && adapter.as_deref() != Some("mono") {
                return Err(invalid("`mono` is for adapter `mono`"));
            }
            let transport = match i.transport {
                None => None,
                Some(t) if t.kind != "tcp" => {
                    return Err(invalid(format!(
                        "`transport.kind` is `tcp`, not `{}`",
                        t.kind
                    )));
                }
                Some(t) if t.host.trim().is_empty() || t.port == 0 => {
                    return Err(invalid("`transport` needs a `host` and a `port` from 1"));
                }
                Some(t) => Some((t.host, t.port)),
            };
            let mono = match i.mono {
                Some(m) if m.port == 0 => return Err(invalid("`mono.port` is at least 1")),
                Some(m) => Some((
                    m.address
                        .filter(|a| !a.trim().is_empty())
                        .unwrap_or_else(|| "127.0.0.1".into()),
                    m.port,
                )),
                None => None,
            };
            DebugRequest::Attach {
                target,
                adapter,
                transport,
                mono,
                web_root,
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        PROCESSES => {
            let i: ProcessesIn = input(value)?;
            let filter = non_empty("filter", i.filter)?;
            if filter.as_ref().is_some_and(|f| f.chars().count() > 200) {
                return Err(invalid("`filter` is at most 200 characters"));
            }
            DebugRequest::Processes { filter }
        }
        RESTART => {
            let i: RestartIn = input(value)?;
            DebugRequest::Restart {
                wait_ms: check_wait(i.wait_ms)?,
                budget,
            }
        }
        SESSIONS => {
            let _: Empty = input(value)?;
            DebugRequest::Sessions
        }
        ALLOW_AGENTS => {
            if value.is_null() {
                return Err(invalid("`enabled` is required"));
            }
            let i: AllowAgentsIn = serde_json::from_value(value)
                .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
            DebugRequest::AllowAgents { enabled: i.enabled }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

fn check_points(n: usize) -> Result<(), CommandError> {
    if (1..=MAX_POINTS).contains(&n) {
        Ok(())
    } else {
        Err(invalid(format!("`points` lists 1 to {MAX_POINTS} points")))
    }
}

fn check_point(path: &str, line: u32) -> Result<(), CommandError> {
    if path.trim().is_empty() {
        return Err(invalid("a point's `path` must not be empty"));
    }
    if line == 0 {
        return Err(invalid("a point's `line` is at least 1"));
    }
    Ok(())
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

// ---- escalation (ADR-0009, brief 0027) ----

/// What call `input` of debug command `id` does, for the `debug` policy; `None` for commands it does not govern
/// (reads, watches, frame selection, exception settings, plain breakpoints, `allow_agents`).
pub fn debug_call(id: &str, input: &Value, view: &PolicyView) -> Option<DebugCall> {
    let message_expression = |v: &Value| v.as_str().is_some_and(has_expression);
    Some(match id {
        START | RESTART | STOP | CONTINUE | STEP_OVER | STEP_INTO | STEP_OUT | RUN_TO_CURSOR
        | RUN_UNTIL | PAUSE | SET_NEXT_STATEMENT => DebugCall {
            drive: true,
            ..DebugCall::default()
        },
        SET_VARIABLE => DebugCall {
            drive: true,
            evaluate: true,
            attach: None,
        },
        EVALUATE => DebugCall {
            evaluate: true,
            ..DebugCall::default()
        },
        TRACE => DebugCall {
            drive: true,
            evaluate: input
                .get("points")
                .and_then(Value::as_array)
                .is_some_and(|ps| ps.iter().any(|p| message_expression(&p["message"]))),
            attach: None,
        },
        TOGGLE_BREAKPOINT if input.get("log_message").is_some_and(message_expression) => {
            DebugCall {
                drive: true,
                evaluate: true,
                attach: None,
            }
        }
        // A tab of the browser Eludite runs is Eludite's; a url may be any browser's (brief 0038).
        ATTACH if input.get("tab").is_some_and(Value::is_string) => DebugCall {
            drive: true,
            evaluate: false,
            attach: Some(false),
        },
        ATTACH if input.get("url").is_some_and(Value::is_string) => DebugCall {
            drive: true,
            evaluate: false,
            attach: Some(true),
        },
        ATTACH => {
            let pid = input
                .get("pid")
                .and_then(Value::as_u64)
                .and_then(|p| u32::try_from(p).ok());
            let name = input.get("process_name").and_then(Value::as_str);
            if pid.is_none() && name.is_none() {
                // The dialog: it attaches nothing itself.
                return None;
            }
            // A process on another machine (`transport`) is never one Eludite started here.
            let ours = input.get("transport").is_none_or(Value::is_null)
                && view
                    .launched()
                    .contains(pid, if pid.is_some() { None } else { name });
            DebugCall {
                drive: true,
                evaluate: false,
                attach: Some(!ours),
            }
        }
        _ => return None,
    })
}

/// The escalation hook of debug command `id`, if it has one: the solution policy's `debug` object applied to the
/// call (ADR-0009), tool rules first.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    debug_call(id, &json_object(id), &PolicyView::of(Default::default()))?;
    let tool = id.replace('.', "-");
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        let call = debug_call(id, input, view)?;
        view.debug()
            .decide_for(call, &view.policy().rules, &tool, input)
    }))
}

/// An input that makes [`debug_call`] answer for every command it governs (to know which commands have hooks).
fn json_object(id: &str) -> Value {
    match id {
        TOGGLE_BREAKPOINT => serde_json::json!({"log_message": "{x}"}),
        ATTACH => serde_json::json!({"pid": 1}),
        _ => serde_json::json!({}),
    }
}

/// Register every debug command, applying them to `target`, with their escalation hooks.
pub fn register(registry: &CommandRegistry, target: Arc<dyn DebugTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let (session, request) = parse_with_session(id, input)?;
            target.apply(session, request).map(|out| out.to_json())
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
                compound: None,
                browsers: Vec::new(),
                project: None,
                debug: true,
                profile: None,
                build: None,
                cargo: CargoOptions::default(),
                browser: None,
                wait_ms: None,
                budget: Budget::default()
            }
        );
        // A Cargo package's test executable with a filter (brief 0029).
        assert_eq!(
            parse(
                START,
                json!({"project": "app", "test": true, "args": ["my_test"], "target": "app"})
            )
            .unwrap(),
            DebugRequest::Start {
                compound: None,
                browsers: Vec::new(),
                project: Some("app".into()),
                debug: true,
                profile: None,
                build: None,
                cargo: CargoOptions {
                    target: Some("app".into()),
                    test: true,
                    args: Some(vec!["my_test".into()])
                },
                browser: None,
                wait_ms: None,
                budget: Budget::default()
            }
        );
        assert!(parse(START, json!({"target": ""})).is_err());
        assert!(parse(START, json!({"args": "my_test"})).is_err());
        assert_eq!(
            parse(
                START,
                json!({"debug": false, "project": "App", "profile": "App", "build": false})
            )
            .unwrap(),
            DebugRequest::Start {
                compound: None,
                browsers: Vec::new(),
                project: Some("App".into()),
                debug: false,
                profile: Some("App".into()),
                build: Some(false),
                cargo: CargoOptions::default(),
                browser: None,
                wait_ms: None,
                budget: Budget::default()
            }
        );
        assert!(parse(START, json!({"project": " "})).is_err());
        assert!(parse(START, json!({"wait_ms": 40000})).is_err());
        // Where a web project's page opens (brief 0037).
        for (v, want) in [
            ("built_in", BrowserChoice::BuiltIn),
            ("external", BrowserChoice::External),
            ("none", BrowserChoice::None),
        ] {
            assert!(matches!(
                parse(START, json!({"browser": v})).unwrap(),
                DebugRequest::Start { browser: Some(b), .. } if b == want
            ));
        }
        assert!(parse(START, json!({"browser": "chrome"})).is_err());
        assert!(parse(START, json!({"browser": true})).is_err());
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
                hit_condition: None,
                log_message: None,
                function: None,
                remove_after: None
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
                hit_condition: Some(Some(HitCondition::AtLeast(3))),
                log_message: None,
                function: None,
                remove_after: None
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
                break_when_user_unhandled: None,
                break_on_rust_panic: None,
                types: Vec::new(),
                remove: None,
                clear: false
            }
        );
        assert_eq!(
            parse(EXCEPTION_SETTINGS, json!({"break_on_rust_panic": false})).unwrap(),
            DebugRequest::ExceptionSettings {
                break_when_thrown: None,
                break_when_user_unhandled: None,
                break_on_rust_panic: Some(false),
                types: Vec::new(),
                remove: None,
                clear: false
            }
        );
        // Settings saved before brief 0029 have no Rust panics row: it is on.
        let old: ExceptionSettingsRow = serde_json::from_value(
            json!({"break_when_thrown": false, "break_when_user_unhandled": true}),
        )
        .unwrap();
        assert!(old.break_on_rust_panic);
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible, "{id}");
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["type"], "object");
        }
        assert_eq!(spec(STATE).permission, PermissionClass::Read);
        assert_eq!(spec(STEP_OVER).permission, PermissionClass::Execute);
        assert_eq!(spec(EVALUATE).permission, PermissionClass::Execute);
        // A tracepoint's `{expression}` runs code (brief 0026): the command is execute.
        assert_eq!(spec(TOGGLE_BREAKPOINT).permission, PermissionClass::Execute);
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
                id: Some(1),
                project: "/s/App.csproj".into(),
                program: "/s/bin/Debug/net10.0/App.dll".into(),
                args: vec![],
                cwd: "/s".into(),
                profile: None,
                debug: true,
                adapter: Some("netcoredbg".into()),
                runtime: Some("coreclr".into()),
                process_id: Some(7),
                attached: true,
                parent: None,
                tab: None,
                url: None,
                // Brief 0037: the web project's page.
                browser: Some(SessionBrowser {
                    tab: Some("t1".into()),
                    url: "http://localhost:5180/".into(),
                    engine: "embedded".into(),
                    state: "opened".into(),
                    message: None,
                }),
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
                path: Some("/s/Program.cs".into()),
                line: Some(5),
                enabled: true,
                verified: true,
                condition: None,
                hit_condition: Some("2".into()),
                hits: 2,
                ..Default::default()
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
            agents_allowed: false,
            sessions: Vec::new(),
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
    fn a_null_reads_null_on_every_adapter() {
        // eludite-dbg-mono's spelling is rewritten; netcoredbg's and every other value are unchanged.
        assert_eq!(null_spelling("(null)".into()), "null");
        assert_eq!(null_spelling("null".into()), "null");
        for kept in ["\"(null)\"", "(null) ", "{(null)}", "0", "", "Count = 5"] {
            assert_eq!(null_spelling(kept.into()), kept);
        }
    }

    #[test]
    fn the_toggle_breakpoint_answer_is_compact_and_follows_its_schema() {
        assert_eq!(
            spec(TOGGLE_BREAKPOINT).output_schema["title"],
            "eludite.debug.toggle_breakpoint output"
        );
        let row = BreakpointRow {
            kind: BreakpointKind::Line,
            path: Some("/s/OffByOne/Program.cs".into()),
            line: Some(13),
            enabled: true,
            verified: true,
            condition: Some("i == count - 1".into()),
            hits: 0,
            remove_after: true,
            sessions: vec![BreakpointSessionRow {
                session: 1,
                verified: true,
                hits: 0,
                message: None,
            }],
            ..BreakpointRow::default()
        };
        let added = DebugOutput::Breakpoint(Box::new(ToggleBreakpointOutput {
            action: BreakpointEdit::Added,
            breakpoint: Some(row),
            verified: true,
            pending: true,
            message: None,
            session: Some(1),
            breakpoints_total: 3,
        }))
        .to_json();
        conforms(TOGGLE_OUTPUT, &added);
        assert_eq!(added["action"], "added");
        assert_eq!(added["breakpoint"]["line"], 13);
        assert_eq!(added["breakpoint"]["remove_after"], true);
        assert_eq!(added["breakpoints_total"], 3);
        assert!(added.to_string().len() < 500, "{added}");
        // Before any session, and after a delete: no row, nothing bound, nothing pending.
        let deleted = DebugOutput::Breakpoint(Box::new(ToggleBreakpointOutput {
            action: BreakpointEdit::Deleted,
            breakpoint: None,
            verified: false,
            pending: false,
            message: None,
            session: None,
            breakpoints_total: 0,
        }))
        .to_json();
        conforms(TOGGLE_OUTPUT, &deleted);
        assert_eq!(
            deleted,
            json!({"action": "deleted", "verified": false, "breakpoints_total": 0})
        );
        let all = serde_json::to_value(BreakpointEdit::DeletedAll).unwrap();
        assert_eq!(all, "deleted_all");
        // The state is no longer its answer.
        assert_ne!(
            spec(TOGGLE_BREAKPOINT).output_schema["title"],
            spec(STATE).output_schema["title"]
        );
    }

    #[test]
    fn a_breakpoint_that_cannot_stop_says_so_in_the_answers() {
        // The toggle answer with a live session's refusal: under 600 B with the message (brief 0036's budget).
        let message = "Unknown identifier: Coin";
        let row = BreakpointRow {
            kind: BreakpointKind::Line,
            path: Some("/home/user/work/MissingCase/Program.cs".into()),
            line: Some(28),
            enabled: true,
            verified: false,
            condition: Some("coin == Coin.Quarter".into()),
            message: Some(message.into()),
            remove_after: true,
            sessions: vec![BreakpointSessionRow {
                session: 1,
                verified: false,
                hits: 0,
                message: Some(message.into()),
            }],
            ..BreakpointRow::default()
        };
        let answer = DebugOutput::Breakpoint(Box::new(ToggleBreakpointOutput {
            action: BreakpointEdit::Added,
            breakpoint: Some(row),
            verified: false,
            pending: false,
            message: Some(message.into()),
            session: Some(1),
            breakpoints_total: 1,
        }))
        .to_json();
        conforms(TOGGLE_OUTPUT, &answer);
        assert_eq!(answer["message"], message);
        assert!(answer.to_string().len() < 600, "{answer}");
        // The stop summary's lists, and trace's.
        let failed = FailedBreakpointRow {
            path: Some("/s/Program.cs".into()),
            line: Some(28),
            function: None,
            session: 1,
            message: message.into(),
        };
        let summary = DebugOutput::Summary(Box::new(StopSummary {
            mode: "design".into(),
            generation: 1,
            exit_code: Some(1),
            message: Some("The session ended: the program exited with code 1.".into()),
            breakpoints_failed: vec![failed.clone()],
            points_failed: vec![FailedBreakpointRow {
                function: Some("App.Coins.Cents".into()),
                path: None,
                line: None,
                ..failed.clone()
            }],
            ..StopSummary::default()
        }))
        .to_json();
        conforms(SUMMARY_OUTPUT, &summary);
        assert_eq!(summary["breakpoints_failed"][0]["line"], 28);
        assert_eq!(summary["points_failed"][0]["function"], "App.Coins.Cents");
        let empty = DebugOutput::Summary(Box::new(StopSummary {
            mode: "running".into(),
            ..StopSummary::default()
        }))
        .to_json();
        assert!(empty.get("breakpoints_failed").is_none() && empty.get("points_failed").is_none());
        // A start's answer names the page it opened (brief 0037), and one still waiting or failed.
        for (tab, state, message) in [
            (Some("t2"), "opened", None),
            (None, "waiting", None),
            (
                None,
                "failed",
                Some("the server did not answer within 30 s"),
            ),
        ] {
            let web = DebugOutput::Summary(Box::new(StopSummary {
                mode: "running".into(),
                session: Some(1),
                browser: Some(SessionBrowser {
                    tab: tab.map(Into::into),
                    url: "http://localhost:5180/".into(),
                    engine: "embedded".into(),
                    state: state.into(),
                    message: message.map(Into::into),
                }),
                ..StopSummary::default()
            }))
            .to_json();
            conforms(SUMMARY_OUTPUT, &web);
            assert_eq!(web["browser"]["state"], state);
        }
        assert!(empty.get("browser").is_none());
        let trace = DebugOutput::Trace(Box::new(TraceOutput {
            stopped_by: "terminated".into(),
            points: vec![TracePointRow {
                path: "/s/Program.cs".into(),
                line: 28,
                hits: 0,
                verified: false,
                message: Some(message.into()),
            }],
            points_failed: vec![failed],
            ..TraceOutput::default()
        }))
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-trace.output.json"),
            &trace,
        );
        // Pending is not failed.
        for pending in [
            "The breakpoint will not currently be hit",
            "The breakpoint could not yet be bound to a valid location",
            "The breakpoint will bind when its code loads.",
            "The breakpoint is pending and will be resolved when debugging starts.",
            "breakpoint.provisionalBreakpoint",
            "Unbound breakpoint",
        ] {
            assert!(pending_message(pending), "{pending}");
        }
        for failed in [
            message,
            "The breakpoint location is invalid. Perhaps the source line does not contain any statements",
            "The breakpoint could not be bound",
            "'Kind' is ambiguous between A.Kind and B.Kind: qualify it",
        ] {
            assert!(!pending_message(failed), "{failed}");
        }
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
            source: None,
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
            session: Some(1),
            browser: None,
            sessions: Vec::new(),
            breakpoints_failed: Vec::new(),
            points_failed: Vec::new(),
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
            interrupted_by: None,
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

    #[test]
    fn the_run_control_commands_parse_and_validate() {
        // Tracepoints, function breakpoints and Delete when hit.
        assert!(matches!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "path": "A.cs", "line": 3, "log_message": "x = {x}", "remove_after": true})
            )
            .unwrap(),
            DebugRequest::Breakpoint { log_message: Some(ref m), remove_after: Some(true), function: None, .. }
                if m == "x = {x}"
        ));
        // An empty message turns it back into a breakpoint (parsed as given).
        assert!(matches!(
            parse(TOGGLE_BREAKPOINT, json!({"action": "set", "log_message": ""})).unwrap(),
            DebugRequest::Breakpoint { log_message: Some(ref m), .. } if m.is_empty()
        ));
        assert!(parse(TOGGLE_BREAKPOINT, json!({"log_message": "x"})).is_err());
        assert!(parse(TOGGLE_BREAKPOINT, json!({"remove_after": true})).is_err());
        assert_eq!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "set", "function": " App.Calc.Add ", "condition": "a == 1", "hit_condition": "2"})
            )
            .unwrap(),
            DebugRequest::Breakpoint {
                path: None,
                line: None,
                action: BreakpointAction::Set,
                enabled: None,
                condition: Some("a == 1".into()),
                hit_condition: Some(Some(HitCondition::Equal(2))),
                log_message: None,
                function: Some("App.Calc.Add".into()),
                remove_after: None,
            }
        );
        assert!(
            parse(
                TOGGLE_BREAKPOINT,
                json!({"action": "delete", "function": "App.Calc.Add"})
            )
            .is_ok()
        );
        for bad in [
            json!({"action": "set", "function": "App.Calc.Add", "log_message": "x"}),
            json!({"action": "set", "function": "App.Calc.Add", "path": "A.cs"}),
            json!({"action": "set", "function": "App.Calc.Add", "line": 3}),
            json!({"function": "App.Calc.Add"}),
            json!({"action": "delete_all", "function": "App.Calc.Add"}),
            json!({"action": "set", "function": " "}),
        ] {
            let e = parse(TOGGLE_BREAKPOINT, bad.clone())
                .unwrap_err()
                .to_string();
            assert!(!e.is_empty(), "{bad}");
        }
        let e = parse(
            TOGGLE_BREAKPOINT,
            json!({"action": "set", "function": "F", "log_message": "x"}),
        )
        .unwrap_err()
        .to_string();
        assert!(e.contains("log_message"), "{e}");

        // Exception types.
        assert_eq!(
            parse(
                EXCEPTION_SETTINGS,
                json!({"types": [{"type": "System.InvalidOperationException"},
                                 {"type": "System.FormatException", "break_when_thrown": false}],
                       "remove": "System.IO.IOException", "clear": true})
            )
            .unwrap(),
            DebugRequest::ExceptionSettings {
                break_when_thrown: None,
                break_when_user_unhandled: None,
                break_on_rust_panic: None,
                types: vec![
                    ExceptionTypeRow {
                        type_name: "System.InvalidOperationException".into(),
                        break_when_thrown: true,
                        break_when_user_unhandled: true,
                    },
                    ExceptionTypeRow {
                        type_name: "System.FormatException".into(),
                        break_when_thrown: false,
                        break_when_user_unhandled: true,
                    },
                ],
                remove: Some("System.IO.IOException".into()),
                clear: true,
            }
        );
        for bad in [
            json!({"types": [{"type": ""}]}),
            json!({"types": [{"type": "A, B"}]}),
            json!({"types": [{"type": "A B"}]}),
            json!({"types": [{"name": "A"}]}),
            json!({"types": [{"type": "A", "break": true}]}),
            json!({"remove": " "}),
            json!({"types": (0..101).map(|i| json!({"type": format!("T{i}")})).collect::<Vec<_>>()}),
        ] {
            assert!(parse(EXCEPTION_SETTINGS, bad.clone()).is_err(), "{bad}");
        }

        // run_until: 1 to 50 points, remove_after defaulting to true, the budget.
        let r = parse(
            RUN_UNTIL,
            json!({"points": [{"path": "A.cs", "line": 4}, {"path": "B.cs", "line": 9, "condition": "i == 3"}],
                   "wait_ms": 2000, "stop": 3, "depth": 2}),
        )
        .unwrap();
        match r {
            DebugRequest::RunUntil {
                points,
                remove_after,
                stop,
                wait_ms,
                budget,
            } => {
                assert_eq!(points.len(), 2);
                assert_eq!(points[1].condition.as_deref(), Some("i == 3"));
                assert!(remove_after);
                assert_eq!((stop, wait_ms, budget.depth), (Some(3), Some(2000), 2));
            }
            other => panic!("{other:?}"),
        }
        assert!(matches!(
            parse(
                RUN_UNTIL,
                json!({"points": [{"path": "A.cs", "line": 4}], "remove_after": false})
            )
            .unwrap(),
            DebugRequest::RunUntil {
                remove_after: false,
                ..
            }
        ));
        let many: Vec<Value> = (1..=51)
            .map(|l| json!({"path": "A.cs", "line": l}))
            .collect();
        for bad in [
            json!({}),
            json!({"points": []}),
            json!({"points": many}),
            json!({"points": [{"path": "A.cs", "line": 0}]}),
            json!({"points": [{"path": " ", "line": 1}]}),
            json!({"points": [{"path": "A.cs"}]}),
            json!({"points": [{"path": "A.cs", "line": 1, "message": "x"}]}),
            json!({"points": [{"path": "A.cs", "line": 1}], "wait_ms": 30001}),
        ] {
            assert!(parse(RUN_UNTIL, bad.clone()).is_err(), "{bad}");
        }

        // trace: defaults, until with count, run start with the start parameters.
        let point = json!({"path": "A.cs", "line": 4, "message": "i = {i}"});
        assert_eq!(
            parse(TRACE, json!({"points": [point.clone()]})).unwrap(),
            DebugRequest::Trace {
                points: vec![TracePoint {
                    path: "A.cs".into(),
                    line: 4,
                    message: "i = {i}".into(),
                    condition: None
                }],
                run: TraceRun::Continue,
                start: StartParams::default(),
                until: TraceUntil::Terminated,
                wait_ms: DEFAULT_TRACE_WAIT_MS,
                max_hits: DEFAULT_MAX_HITS,
                stop: None,
                budget: Budget::default(),
            }
        );
        assert!(matches!(
            parse(
                TRACE,
                json!({"points": [point.clone()], "until": "hits", "count": 5, "max_hits": 3})
            )
            .unwrap(),
            DebugRequest::Trace {
                until: TraceUntil::Hits(5),
                max_hits: 3,
                ..
            }
        ));
        assert!(matches!(
            parse(
                TRACE,
                json!({"points": [point.clone()], "until": "stopped"})
            )
            .unwrap(),
            DebugRequest::Trace {
                until: TraceUntil::Stopped,
                ..
            }
        ));
        assert!(matches!(
            parse(TRACE, json!({"points": [point.clone()], "run": "start", "project": "App", "build": false})).unwrap(),
            DebugRequest::Trace { run: TraceRun::Start, start: StartParams { project: Some(ref p), build: Some(false), .. }, .. }
                if p == "App"
        ));
        for bad in [
            json!({"points": [point.clone()], "until": "hits"}),
            json!({"points": [point.clone()], "until": "hits", "count": 0}),
            json!({"points": [point.clone()], "until": "hits", "count": 10001}),
            json!({"points": [point.clone()], "count": 5}),
            json!({"points": [point.clone()], "until": "stopped", "count": 5}),
            json!({"points": [point.clone()], "max_hits": 0}),
            json!({"points": [point.clone()], "max_hits": 10001}),
            json!({"points": [point.clone()], "project": "App"}),
            json!({"points": [point.clone()], "run": "start", "stop": 2}),
            json!({"points": [point.clone()], "until": "forever"}),
            json!({"points": [{"path": "A.cs", "line": 4}]}),
            json!({"points": [{"path": "A.cs", "line": 4, "message": " "}]}),
            json!({"points": []}),
            json!({"points": [point.clone()], "wait_ms": 40000}),
        ] {
            assert!(parse(TRACE, bad.clone()).is_err(), "{bad}");
        }

        // set_variable: by name in a frame, or a member by reference.
        assert_eq!(
            parse(
                SET_VARIABLE,
                json!({"name": "x", "value": "42", "frame": 1, "stop": 2})
            )
            .unwrap(),
            DebugRequest::SetVariable {
                target: SetTarget::Frame {
                    thread: None,
                    frame: Some(1)
                },
                name: "x".into(),
                value: "42".into(),
                stop: Some(2),
            }
        );
        assert!(matches!(
            parse(
                SET_VARIABLE,
                json!({"name": "Name", "value": "\"B\"", "reference": 7})
            )
            .unwrap(),
            DebugRequest::SetVariable {
                target: SetTarget::Reference(7),
                ..
            }
        ));
        for bad in [
            json!({"value": "1"}),
            json!({"name": "x"}),
            json!({"name": "x", "value": " "}),
            json!({"name": "x", "value": "1", "reference": 0}),
            json!({"name": "x", "value": "1", "reference": 3, "frame": 0}),
            json!({"name": "x", "value": "1", "expression": "y"}),
        ] {
            assert!(parse(SET_VARIABLE, bad.clone()).is_err(), "{bad}");
        }

        // set_next_statement.
        assert!(matches!(
            parse(
                SET_NEXT_STATEMENT,
                json!({"path": "A.cs", "line": 9, "thread": 1})
            )
            .unwrap(),
            DebugRequest::SetNextStatement {
                line: Some(9),
                thread: Some(1),
                ..
            }
        ));
        assert!(parse(SET_NEXT_STATEMENT, json!({"line": 0})).is_err());

        // Classes (proposal 0001 rule 7), waits and budgets.
        for id in [
            RUN_UNTIL,
            TRACE,
            SET_VARIABLE,
            SET_NEXT_STATEMENT,
            TOGGLE_BREAKPOINT,
        ] {
            assert_eq!(spec(id).permission, PermissionClass::Execute, "{id}");
        }
        let t = parse(TRACE, json!({"points": [point]})).unwrap();
        assert_eq!(t.wait_ms(), Some(DEFAULT_TRACE_WAIT_MS));
        assert!(t.resumes() && t.budget().is_some());
        let v = parse(SET_VARIABLE, json!({"name": "x", "value": "1"})).unwrap();
        assert!(!v.resumes() && v.budget().is_none());
    }

    #[test]
    fn the_run_control_outputs_follow_their_schemas() {
        let trace = DebugOutput::Trace(Box::new(TraceOutput {
            lines: vec![TraceLine {
                seq: 0,
                path: "/s/Program.cs".into(),
                line: 12,
                hit: 1,
                time_ms: 3.5,
                text: "i = 0".into(),
            }],
            hits: 1,
            truncated: false,
            stopped_by: "stopped".into(),
            summary: Some(Box::new(StopSummary {
                mode: "break".into(),
                generation: 1,
                stop: 2,
                ..Default::default()
            })),
            exit_code: Some(0),
            points: vec![TracePointRow {
                path: "/s/Program.cs".into(),
                line: 12,
                hits: 1,
                verified: true,
                message: None,
            }],
            points_failed: Vec::new(),
            overhead_ms_per_hit: Some(4.2),
            emulated: true,
            generation: Some(1),
        }))
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-trace.output.json"),
            &trace,
        );
        conforms(SUMMARY_OUTPUT, &trace["summary"]);
        let set = DebugOutput::SetVariable(SetVariableOutput {
            name: "x".into(),
            value: "42".into(),
            type_name: Some("int".into()),
            reference: 0,
            request: Some("setExpression".into()),
            pending: false,
            stop: 3,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-set-variable.output.json"),
            &set,
        );
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "design".into(),
            breakpoints: vec![
                BreakpointRow {
                    kind: BreakpointKind::Tracepoint,
                    path: Some("/s/Program.cs".into()),
                    line: Some(12),
                    enabled: true,
                    log_message: Some("i = {i}".into()),
                    remove_after: true,
                    temporary: true,
                    ..Default::default()
                },
                BreakpointRow {
                    kind: BreakpointKind::Function,
                    function: Some("App.Calc.Add".into()),
                    enabled: true,
                    verified: true,
                    hits: 1,
                    ..Default::default()
                },
            ],
            exceptions: ExceptionSettingsRow {
                types: vec![ExceptionTypeRow {
                    type_name: "System.InvalidOperationException".into(),
                    break_when_thrown: true,
                    break_when_user_unhandled: false,
                }],
                ..Default::default()
            },
            ..Default::default()
        }))
        .to_json();
        conforms(STATE_OUTPUT, &state);
        assert_eq!(state["breakpoints"][1]["kind"], "function");
        assert!(state["breakpoints"][1].get("path").is_none());
        let rows: Vec<BreakpointRow> =
            serde_json::from_value(state["breakpoints"].clone()).unwrap();
        assert_eq!(rows[0].label(), "Program.cs, line 12");
        assert_eq!(rows[1].label(), "App.Calc.Add");
        // A state row of brief 0025 (no kind) is a line breakpoint.
        let old: BreakpointRow = serde_json::from_value(
            json!({"path": "/s/A.cs", "line": 3, "enabled": true, "verified": false, "hits": 0}),
        )
        .unwrap();
        assert_eq!(old.kind, BreakpointKind::Line);
    }

    #[test]
    fn the_attach_and_policy_commands_parse_and_validate() {
        let b = Budget::default();
        assert_eq!(
            parse(ATTACH, json!({"pid": 42})).unwrap(),
            DebugRequest::Attach {
                target: AttachTarget::Pid(42),
                adapter: None,
                transport: None,
                mono: None,
                web_root: None,
                wait_ms: None,
                budget: b
            }
        );
        assert_eq!(
            parse(
                ATTACH,
                json!({"process_name": " App ", "adapter": "coreclr", "wait_ms": 100, "max_frames": 3,
                       "transport": {"kind": "tcp", "host": "winbox", "port": 4711}})
            )
            .unwrap(),
            DebugRequest::Attach {
                target: AttachTarget::Name("App".into()),
                adapter: Some("coreclr".into()),
                transport: Some(("winbox".into(), 4711)),
                mono: None,
                web_root: None,
                wait_ms: Some(100),
                budget: Budget {
                    max_frames: 3,
                    ..b
                }
            }
        );
        // `mono` implies the Mono adapter; its address defaults to the loopback.
        assert_eq!(
            parse(ATTACH, json!({"pid": 7, "mono": {"port": 55555}})).unwrap(),
            DebugRequest::Attach {
                target: AttachTarget::Pid(7),
                adapter: Some("mono".into()),
                transport: None,
                mono: Some(("127.0.0.1".into(), 55555)),
                web_root: None,
                wait_ms: None,
                budget: b
            }
        );
        assert!(matches!(
            parse(ATTACH, json!({})).unwrap(),
            DebugRequest::Attach {
                target: AttachTarget::Dialog,
                ..
            }
        ));
        for bad in [
            json!({"pid": 0}),
            json!({"process_name": " "}),
            json!({"pid": 1, "adapter": "gdb"}),
            json!({"pid": 1, "adapter": "coreclr", "mono": {"port": 1}}),
            json!({"pid": 1, "mono": {"port": 0}}),
            json!({"pid": 1, "transport": {"kind": "ssh", "host": "h", "port": 1}}),
            json!({"pid": 1, "transport": {"kind": "tcp", "host": "", "port": 1}}),
            json!({"pid": 1, "wait_ms": 40000}),
            json!({"pid": 1, "bogus": true}),
        ] {
            assert!(parse(ATTACH, bad.clone()).is_err(), "{bad}");
        }
        assert_eq!(
            parse(PROCESSES, json!({})).unwrap(),
            DebugRequest::Processes { filter: None }
        );
        assert_eq!(
            parse(PROCESSES, json!({"filter": "dotnet"})).unwrap(),
            DebugRequest::Processes {
                filter: Some("dotnet".into())
            }
        );
        assert!(parse(PROCESSES, json!({"filter": ""})).is_err());
        assert!(parse(PROCESSES, json!({"filter": "x".repeat(201)})).is_err());
        assert_eq!(
            parse(RESTART, json!({"wait_ms": 0, "depth": 2})).unwrap(),
            DebugRequest::Restart {
                wait_ms: Some(0),
                budget: Budget { depth: 2, ..b }
            }
        );
        assert!(parse(RESTART, json!({"stop": 1})).is_err());
        assert_eq!(
            parse(ALLOW_AGENTS, json!({"enabled": false})).unwrap(),
            DebugRequest::AllowAgents { enabled: false }
        );
        assert!(parse(ALLOW_AGENTS, json!({})).is_err());
        assert!(parse(ALLOW_AGENTS, Value::Null).is_err());
        // Classes: attach, restart and the toggle execute, processes reads; attach may escalate.
        for (id, class) in [
            (ATTACH, PermissionClass::Execute),
            (RESTART, PermissionClass::Execute),
            (ALLOW_AGENTS, PermissionClass::Execute),
            (PROCESSES, PermissionClass::Read),
        ] {
            assert_eq!(spec(id).permission, class, "{id}");
            assert!(spec(id).agent_visible);
        }
        assert!(
            spec(ATTACH)
                .escalates()
                .unwrap()
                .contains("Eludite started")
        );
        assert!(spec(PROCESSES).escalates().is_none());
        // Which requests the toggle governs.
        let p = |id: &str, v: Value| parse(id, v).unwrap();
        assert!(p(CONTINUE, json!({})).drives());
        assert!(p(SET_VARIABLE, json!({"name": "x", "value": "1"})).drives());
        assert!(p(PAUSE, json!({})).drives());
        assert!(p(ATTACH, json!({"pid": 3})).drives());
        assert!(p(RESTART, json!({})).drives());
        assert!(
            p(
                TOGGLE_BREAKPOINT,
                json!({"path": "a", "line": 1, "action": "set", "log_message": "x={x}"})
            )
            .drives()
        );
        assert!(
            !p(
                TOGGLE_BREAKPOINT,
                json!({"path": "a", "line": 1, "action": "set", "log_message": "{{x}}"})
            )
            .drives()
        );
        assert!(!p(TOGGLE_BREAKPOINT, json!({"path": "a", "line": 1})).drives());
        assert!(!p(ATTACH, json!({})).drives());
        for read in [SNAPSHOT, STATE, OUTPUT, PROCESSES, WAIT, EVALUATE] {
            let v = if read == EVALUATE {
                json!({"expression": "x"})
            } else {
                json!({})
            };
            assert!(!p(read, v).drives(), "{read}");
        }
        assert!(has_expression("a {b} c") && !has_expression("a {{b}} c") && !has_expression("x"));
    }

    #[test]
    fn the_attach_and_policy_outputs_follow_their_schemas() {
        let out = DebugOutput::Processes(ProcessesOutput {
            processes: vec![
                ProcessRow {
                    pid: 4242,
                    parent: Some(1),
                    name: "dotnet".into(),
                    command_line: "/usr/bin/dotnet /s/App.dll".into(),
                    runtime: "dotnet".into(),
                    launched_by_eludite: true,
                    debugger_agent: None,
                },
                ProcessRow {
                    pid: 7,
                    name: "mono".into(),
                    command_line: "mono --debugger-agent=server=y,address=127.0.0.1:1 a.exe".into(),
                    runtime: "mono".into(),
                    debugger_agent: Some("127.0.0.1:1".into()),
                    ..Default::default()
                },
            ],
            total: 2,
            truncated: false,
            // Brief 0038: the browser's tabs.
            tabs: Some(vec![
                AttachTabRow {
                    id: "t1".into(),
                    title: "Minimal API".into(),
                    url: "http://localhost:5180/".into(),
                    session: Some(2),
                },
                AttachTabRow {
                    id: "t2".into(),
                    title: "".into(),
                    url: "about:blank".into(),
                    session: None,
                },
            ]),
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-processes.output.json"),
            &out,
        );
        let a = DebugOutput::AllowAgents(AllowAgentsOutput {
            agents_allowed: false,
            default: true,
            mode: "break".into(),
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-allow-agents.output.json"),
            &a,
        );
        let s = DebugOutput::Summary(Box::new(StopSummary {
            mode: "running".into(),
            interrupted_by: Some("user".into()),
            ..Default::default()
        }))
        .to_json();
        conforms(SUMMARY_OUTPUT, &s);
        assert_eq!(s["interrupted_by"], "user");
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "running".into(),
            session: Some(SessionRow {
                project: "dotnet".into(),
                program: "/usr/bin/dotnet".into(),
                attached: true,
                process_id: Some(4242),
                ..Default::default()
            }),
            agents_allowed: false,
            ..Default::default()
        }))
        .to_json();
        conforms(STATE_OUTPUT, &state);
        assert_eq!(state["session"]["attached"], true);
        assert_eq!(state["agents_allowed"], false);
        // A state of brief 0026 (no agents_allowed) reads as allowed.
        let old: DebugState =
            serde_json::from_value(json!({"mode": "design", "generation": 0, "stop": 0,
            "threads": [], "frames": [], "locals": [], "watches": [], "breakpoints": [],
            "exceptions": {"break_when_thrown": false, "break_when_user_unhandled": true},
            "console": {"lines": 0, "tail": []}}))
            .unwrap();
        assert!(old.agents_allowed);
        let trace = DebugOutput::Trace(Box::new(TraceOutput {
            stopped_by: "interrupted".into(),
            ..Default::default()
        }))
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-trace.output.json"),
            &trace,
        );
    }

    #[test]
    fn hooks_apply_the_debug_policy() {
        use crate::Escalation;
        use crate::policy::{AlwaysAllow, DebugKnob, LaunchedProcesses, PolicySnapshot};
        let view = |policy: Value| {
            PolicyView::of(PolicySnapshot {
                policy: serde_json::from_value(policy).unwrap(),
                launched: LaunchedProcesses::new(|pid, name| {
                    pid == Some(100) || (pid.is_none() && name == Some("App"))
                }),
                ..Default::default()
            })
        };
        let hook = |id: &'static str, input: Value, v: &PolicyView| {
            escalation(id).and_then(|h| h(&input, v))
        };
        let defaults = view(json!({"version": 1}));
        // Defaults: driving and evaluating keep their class; an attach to a process Eludite did not start is
        // dangerous and allowed once; to one it started (by pid or name) it is not.
        assert_eq!(hook(CONTINUE, json!({}), &defaults), None);
        assert_eq!(hook(EVALUATE, json!({"expression": "x"}), &defaults), None);
        assert_eq!(hook(ATTACH, json!({"pid": 100}), &defaults), None);
        assert_eq!(
            hook(ATTACH, json!({"process_name": "App"}), &defaults),
            None
        );
        assert_eq!(
            hook(ATTACH, json!({"pid": 31337}), &defaults),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "attach to a process Eludite did not start".into(),
                always_allow: AlwaysAllow::Never,
            })
        );
        assert!(hook(ATTACH, json!({"process_name": "Other"}), &defaults).is_some());
        // A process on another machine is never Eludite's.
        assert!(
            hook(
                ATTACH,
                json!({"pid": 100, "transport": {"kind": "tcp", "host": "h", "port": 1}}),
                &defaults
            )
            .is_some()
        );
        // The dialog attaches nothing.
        assert_eq!(hook(ATTACH, json!({}), &defaults), None);
        // drive: deny refuses every driving command, not reads; attach: deny refuses every attach.
        let deny = view(json!({"version": 1, "debug": {"drive": "deny"}}));
        for id in [
            START, CONTINUE, STEP_OVER, RUN_UNTIL, TRACE, PAUSE, STOP, RESTART, ATTACH,
        ] {
            let input = if id == ATTACH {
                json!({"pid": 100})
            } else {
                json!({})
            };
            assert_eq!(
                hook(id, input, &deny),
                Some(Escalation::Refuse(
                    "the solution's policy sets debug.drive to deny".into()
                )),
                "{id}"
            );
        }
        assert_eq!(hook(EVALUATE, json!({"expression": "x"}), &deny), None);
        assert!(escalation(SNAPSHOT).is_none() && escalation(PROCESSES).is_none());
        assert!(escalation(ALLOW_AGENTS).is_none() && escalation(WATCH).is_none());
        let no_attach = view(json!({"version": 1, "debug": {"attach": "deny"}}));
        assert!(matches!(
            hook(ATTACH, json!({"pid": 100}), &no_attach),
            Some(Escalation::Refuse(m)) if m.contains("debug.attach")
        ));
        // prompt raises to dangerous; Always Allow writes `allow`.
        let prompt =
            view(json!({"version": 1, "debug": {"drive": "prompt", "evaluate": "prompt"}}));
        assert_eq!(
            hook(STEP_OVER, json!({}), &prompt),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "the solution's policy asks before an agent drives the debugger (debug.drive: prompt)"
                    .into(),
                always_allow: AlwaysAllow::Debug(vec![DebugKnob::Drive]),
            })
        );
        assert!(matches!(
            hook(EVALUATE, json!({"expression": "x"}), &prompt),
            Some(Escalation::Raise { always_allow: AlwaysAllow::Debug(k), .. }) if k == vec![DebugKnob::Evaluate]
        ));
        assert!(matches!(
            hook(SET_VARIABLE, json!({"name": "x", "value": "1"}), &prompt),
            Some(Escalation::Raise { always_allow: AlwaysAllow::Debug(k), .. })
                if k == vec![DebugKnob::Drive, DebugKnob::Evaluate]
        ));
        // Expressions in tracepoints count as evaluating; plain breakpoints are neither.
        let eval_deny = view(json!({"version": 1, "debug": {"evaluate": "deny"}}));
        assert!(matches!(
            hook(TRACE, json!({"points": [{"path": "a", "line": 1, "message": "i={i}"}]}), &eval_deny),
            Some(Escalation::Refuse(m)) if m.contains("debug.evaluate")
        ));
        assert_eq!(
            hook(
                TRACE,
                json!({"points": [{"path": "a", "line": 1, "message": "here"}]}),
                &eval_deny
            ),
            None
        );
        assert!(
            hook(
                TOGGLE_BREAKPOINT,
                json!({"path": "a", "line": 1, "log_message": "{x}"}),
                &eval_deny
            )
            .is_some()
        );
        assert_eq!(
            hook(
                TOGGLE_BREAKPOINT,
                json!({"path": "a", "line": 1}),
                &eval_deny
            ),
            None
        );
        // Tool rules are checked first: with a rule for the tool, the policy's verdict becomes a raise the rules decide.
        let ruled = view(json!({"version": 1, "debug": {"drive": "deny"},
            "rules": [{"tool": "eludite-debug-step_over", "decision": "allow"}]}));
        assert_eq!(
            hook(STEP_OVER, json!({}), &ruled),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                reason: "the solution's policy sets debug.drive to deny".into(),
                always_allow: AlwaysAllow::Rule,
            })
        );
        assert!(matches!(
            hook(CONTINUE, json!({}), &ruled),
            Some(Escalation::Refuse(_))
        ));
        // Through the registry: the call's class and the audit.
        struct Nothing;
        impl DebugTarget for Nothing {
            fn apply(&self, _: Option<u32>, _: DebugRequest) -> Result<DebugOutput, CommandError> {
                Ok(DebugOutput::AllowAgents(AllowAgentsOutput::default()))
            }
        }
        let r = CommandRegistry::new();
        register(&r, Arc::new(Nothing));
        assert!(r.has_escalation(ATTACH) && r.has_escalation(EVALUATE));
        assert!(!r.has_escalation(SNAPSHOT));
        let c = r.classify(ATTACH, &json!({"pid": 31337})).unwrap();
        assert_eq!(c.class, PermissionClass::Dangerous);
        r.set_policy_source(Arc::new(|| PolicySnapshot {
            policy: serde_json::from_value(json!({"version": 1, "debug": {"evaluate": "prompt"}}))
                .unwrap(),
            ..Default::default()
        }));
        let c = r.classify(EVALUATE, &json!({"expression": "x"})).unwrap();
        assert_eq!(c.class, PermissionClass::Dangerous);
        assert!(c.reason.unwrap().contains("debug.evaluate: prompt"));
        assert_eq!(
            r.classify(CONTINUE, &json!({})).unwrap(),
            crate::CallClass::declared(PermissionClass::Execute)
        );
    }

    /// Brief 0028: `session` on every command that acts on a session, `sessions`, `compound`, and the outputs.
    #[test]
    fn the_multi_session_commands_parse_and_follow_their_schemas() {
        // `session` parses on every command that acts on a session, and each schema declares it.
        let minimal = |id: &str| match id {
            EVALUATE => json!({"expression": "x"}),
            WATCH => json!({"add": "x"}),
            RUN_UNTIL => json!({"points": [{"path": "a.cs", "line": 1}]}),
            TRACE => json!({"points": [{"path": "a.cs", "line": 1, "message": "m"}]}),
            SET_VARIABLE => json!({"name": "x", "value": "1"}),
            ALLOW_AGENTS => json!({"enabled": false}),
            _ => json!({}),
        };
        for id in SESSION_COMMANDS {
            let mut v = minimal(id);
            v["session"] = json!(2);
            let (session, request) = parse_with_session(id, v.clone()).unwrap();
            assert_eq!(session, Some(2), "{id}");
            assert_eq!(request.command(), id);
            // Without it: the active session.
            assert_eq!(parse_with_session(id, minimal(id)).unwrap().0, None, "{id}");
            for bad in [json!(0), json!(-1), json!("2"), json!(1.5)] {
                let mut v = minimal(id);
                v["session"] = bad.clone();
                assert!(parse_with_session(id, v).is_err(), "{id} {bad}");
            }
            let schema = spec(id).input_schema;
            assert_eq!(schema["properties"]["session"]["type"], "integer", "{id}");
            assert_eq!(schema["properties"]["session"]["minimum"], 1, "{id}");
            assert!(
                schema["properties"]["session"]["description"]
                    .as_str()
                    .unwrap()
                    .contains("live sessions"),
                "{id}"
            );
        }
        // The commands that change every session, add one or act on none refuse it, and their schemas lack it.
        for id in ALL.iter().filter(|id| !SESSION_COMMANDS.contains(id)) {
            let mut v = match *id {
                TOGGLE_BREAKPOINT => json!({"path": "a.cs", "line": 1}),
                ATTACH => json!({"pid": 1}),
                _ => json!({}),
            };
            v["session"] = json!(1);
            assert!(parse_with_session(id, v).is_err(), "{id}");
            assert!(
                spec(id).input_schema["properties"].get("session").is_none(),
                "{id}"
            );
        }
        // `parse` keeps answering the request alone.
        assert_eq!(
            parse(CONTINUE, json!({"session": 3, "stop": 2})).unwrap(),
            DebugRequest::Continue {
                stop: Some(2),
                wait_ms: None,
                budget: Budget::default()
            }
        );
        // `sessions` reads.
        assert_eq!(parse(SESSIONS, json!({})).unwrap(), DebugRequest::Sessions);
        assert!(parse(SESSIONS, json!({"x": 1})).is_err());
        assert_eq!(spec(SESSIONS).permission, PermissionClass::Read);
        assert!(spec(SESSIONS).agent_visible);
        assert!(escalation(SESSIONS).is_none());
        assert!(!DebugRequest::Sessions.drives() && !DebugRequest::Sessions.resumes());
        // `compound`: the startup projects, or a list.
        let start = |v: Value| match parse(START, v).unwrap() {
            DebugRequest::Start { compound, .. } => compound,
            other => panic!("{other:?}"),
        };
        assert_eq!(start(json!({})), None);
        assert_eq!(
            start(json!({"compound": "startup"})),
            Some(Compound::Startup)
        );
        assert_eq!(
            start(json!({"compound": "startup", "debug": false, "wait_ms": 100})),
            Some(Compound::Startup)
        );
        assert_eq!(
            start(json!({"compound": [
                {"project": "App"},
                {"project": "src/Web/Web.csproj", "debug": false, "profile": "https"}
            ]})),
            Some(Compound::Projects(vec![
                CompoundEntry {
                    project: "App".into(),
                    debug: true,
                    profile: None
                },
                CompoundEntry {
                    project: "src/Web/Web.csproj".into(),
                    debug: false,
                    profile: Some("https".into())
                },
            ]))
        );
        for bad in [
            json!({"compound": "all"}),
            json!({"compound": []}),
            json!({"compound": [{"project": ""}]}),
            json!({"compound": [{"project": "A", "bogus": 1}]}),
            json!({"compound": [{"project": "A", "profile": " "}]}),
            json!({"compound": 3}),
            json!({"compound": "startup", "project": "A"}),
            json!({"compound": "startup", "profile": "p"}),
            json!({"compound": "startup", "test": true}),
            json!({"compound": (0..21).map(|i| json!({"project": format!("P{i}")})).collect::<Vec<_>>()}),
        ] {
            assert!(parse(START, bad.clone()).is_err(), "{bad}");
        }
        let schema = spec(START).input_schema;
        assert_eq!(
            schema["properties"]["compound"]["oneOf"][0]["const"],
            "startup"
        );
        assert!(
            spec(START).input_schema["properties"]
                .get("session")
                .is_none()
        );

        // Outputs: `sessions`, the state's `sessions`, `session.id` and per-session bindings, the summary's
        // `session` and `sessions`.
        let rows = vec![
            SessionInfo {
                id: 1,
                name: "App".into(),
                mode: "break".into(),
                active: true,
                generation: 3,
                stop: 2,
                runtime: Some("coreclr".into()),
                adapter: Some("fake (stdio)".into()),
                adapter_version: None,
                process_id: Some(4242),
                project: Some("/s/App/App.csproj".into()),
                attached: false,
                parent: None,
                tab: None,
                url: None,
                agents_allowed: true,
                stopped: Some("breakpoint".into()),
            },
            SessionInfo {
                id: 2,
                name: "Web".into(),
                mode: "running".into(),
                active: false,
                generation: 4,
                stop: 0,
                agents_allowed: false,
                ..Default::default()
            },
        ];
        let out = DebugOutput::Sessions(SessionsOutput {
            sessions: rows.clone(),
            active: Some(1),
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-sessions.output.json"),
            &out,
        );
        assert_eq!(out["sessions"][1]["agents_allowed"], false);
        conforms(
            include_str!("../../../protocol/schemas/debug-sessions.output.json"),
            &DebugOutput::Sessions(SessionsOutput::default()).to_json(),
        );
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "break".into(),
            generation: 3,
            stop: 2,
            session: Some(SessionRow {
                id: Some(1),
                project: "/s/App/App.csproj".into(),
                program: "/s/App/bin/App.dll".into(),
                ..Default::default()
            }),
            breakpoints: vec![BreakpointRow {
                path: Some("/s/App/Program.cs".into()),
                line: Some(6),
                enabled: true,
                verified: true,
                hits: 2,
                sessions: vec![
                    BreakpointSessionRow {
                        session: 1,
                        verified: true,
                        hits: 1,
                        message: None,
                    },
                    BreakpointSessionRow {
                        session: 2,
                        verified: false,
                        hits: 1,
                        message: Some("No code at this line".into()),
                    },
                ],
                ..Default::default()
            }],
            sessions: rows,
            ..Default::default()
        }))
        .to_json();
        conforms(STATE_OUTPUT, &state);
        assert_eq!(state["session"]["id"], 1);
        assert_eq!(state["breakpoints"][0]["sessions"][1]["verified"], false);
        let back: DebugState = serde_json::from_value(state).unwrap();
        assert_eq!(back.sessions.len(), 2);
        let summary = DebugOutput::Summary(Box::new(StopSummary {
            session: Some(2),
            mode: "running".into(),
            timed_out: Some(true),
            sessions: vec![
                CompoundSessionRow {
                    id: 1,
                    name: "App".into(),
                    mode: "running".into(),
                },
                CompoundSessionRow {
                    id: 2,
                    name: "Web".into(),
                    mode: "running_without_debugging".into(),
                },
            ],
            ..Default::default()
        }))
        .to_json();
        conforms(SUMMARY_OUTPUT, &summary);
        assert_eq!(summary["session"], 2);
        // A summary of brief 0027 (no `session`) still reads.
        let old: StopSummary = serde_json::from_value(json!({"mode": "design", "generation": 0,
            "stop": 0, "output": {"lines": [], "next": 0, "dropped": 0, "total": 0, "truncated": false},
            "agent_driving": false, "truncated": false}))
        .unwrap();
        assert_eq!(old.session, None);
    }

    /// Brief 0038: `attach` by `tab` or `url` (vscode-js-debug), the compound's `browser` entries, the permission
    /// class of each, and the outputs' new members against their schemas.
    #[test]
    fn browser_attach_compound_entries_and_outputs() {
        use crate::Escalation;
        use crate::policy::PolicySnapshot;
        let b = Budget::default();
        assert_eq!(
            parse(
                ATTACH,
                json!({"tab": "t1", "web_root": "/w/wwwroot", "wait_ms": 100})
            )
            .unwrap(),
            DebugRequest::Attach {
                target: AttachTarget::Tab("t1".into()),
                adapter: Some("javascript".into()),
                transport: None,
                mono: None,
                web_root: Some("/w/wwwroot".into()),
                wait_ms: Some(100),
                budget: b
            }
        );
        assert!(matches!(
            parse(ATTACH, json!({"url": "ws://127.0.0.1:9222/devtools/page/AB", "adapter": "javascript"})).unwrap(),
            DebugRequest::Attach { target: AttachTarget::Url(u), adapter: Some(a), .. }
                if u == "ws://127.0.0.1:9222/devtools/page/AB" && a == "javascript"
        ));
        // `adapter: javascript` alone is the dialog filtered to the tabs (from the UI).
        assert!(matches!(
            parse(ATTACH, json!({"adapter": "javascript"})).unwrap(),
            DebugRequest::Attach { target: AttachTarget::Dialog, adapter: Some(a), .. } if a == "javascript"
        ));
        let refused = |v: Value| {
            parse(ATTACH, v.clone())
                .err()
                .map(|e| e.to_string())
                .unwrap_or_else(|| panic!("{v}"))
        };
        assert!(refused(json!({"pid": 42, "adapter": "javascript"})).contains("by tab or url"));
        for bad in [
            json!({"tab": "1"}),
            json!({"tab": "t"}),
            json!({"tab": "t1", "pid": 4}),
            json!({"tab": "t1", "url": "http://x/"}),
            json!({"url": ""}),
            json!({"url": "x".repeat(2049)}),
            json!({"tab": "t1", "adapter": "coreclr"}),
            json!({"tab": "t1", "mono": {"port": 1}}),
            json!({"tab": "t1", "transport": {"kind": "tcp", "host": "h", "port": 1}}),
            json!({"pid": 4, "web_root": "/w"}),
        ] {
            assert!(parse(ATTACH, bad.clone()).is_err(), "{bad}");
        }
        // The compound's browser entries.
        match parse(
            START,
            json!({"compound": [{"project": "Web"}, {"browser": {"project": "Web", "web_root": "/w/Web/wwwroot"}},
                                {"browser": {"tab": "t3"}}]}),
        )
        .unwrap()
        {
            DebugRequest::Start { compound, browsers, .. } => {
                assert_eq!(
                    compound,
                    Some(Compound::Projects(vec![CompoundEntry {
                        project: "Web".into(),
                        debug: true,
                        profile: None
                    }]))
                );
                assert_eq!(
                    browsers,
                    vec![
                        BrowserEntry {
                            project: Some("Web".into()),
                            web_root: Some("/w/Web/wwwroot".into()),
                            ..Default::default()
                        },
                        BrowserEntry { tab: Some("t3".into()), ..Default::default() },
                    ]
                );
            }
            other => panic!("{other:?}"),
        }
        for bad in [
            json!({"compound": [{"browser": {}}]}),
            json!({"compound": [{"project": "Web"}, {"browser": {"tab": "x1"}}]}),
            json!({"compound": [{"project": "Web"}, {"browser": {"bogus": 1}}]}),
            json!({"compound": [{"project": "Web"}, {"browser": {"url": ""}}]}),
            json!({"compound": [{"project": "Web", "browser": {}}]}),
        ] {
            assert!(parse(START, bad.clone()).is_err(), "{bad}");
        }
        // A tab of Eludite's browser is Eludite's; a url may be any browser's.
        let defaults = PolicyView::of(PolicySnapshot {
            policy: serde_json::from_value(json!({"version": 1})).unwrap(),
            ..Default::default()
        });
        let hook = |input: Value| escalation(ATTACH).and_then(|h| h(&input, &defaults));
        assert_eq!(hook(json!({"tab": "t1"})), None);
        assert!(matches!(
            hook(json!({"url": "ws://127.0.0.1:9222/devtools/page/AB"})),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        // Outputs.
        let source = FrameSourceRow {
            original: "/w/wwwroot/app.ts".into(),
            generated: Some("/w/wwwroot/app.js".into()),
            generated_line: Some(18),
            generated_column: Some(5),
        };
        let frame = StackFrameRow {
            index: 0,
            name: "button#add.onAdd".into(),
            path: Some("/w/wwwroot/app.ts".into()),
            line: Some(25),
            column: Some(3),
            external: false,
            source: Some(source.clone()),
            ..Default::default()
        };
        let stack = DebugOutput::Stack(StackOutput {
            threads: vec![StackThread {
                id: 0,
                name: "Minimal API".into(),
                frames: vec![frame.clone()],
                total: 1,
                truncated: false,
                next: None,
            }],
            stop: 1,
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-stack.output.json"),
            &stack,
        );
        let summary = DebugOutput::Summary(Box::new(StopSummary {
            session: Some(3),
            mode: "break".into(),
            frames: Some(FramesBlock {
                thread: 0,
                rows: vec![frame],
                total: 1,
                truncated: false,
            }),
            ..Default::default()
        }))
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-stop-summary.output.json"),
            &summary,
        );
        let browser = SessionInfo {
            id: 2,
            name: "Minimal API".into(),
            mode: "running".into(),
            generation: 5,
            runtime: Some("javascript".into()),
            adapter: Some(
                "vscode-js-debug 1.140.0 under node v22.12.0 (tcp 127.0.0.1:41234)".into(),
            ),
            adapter_version: Some("vscode-js-debug 1.140.0, node v22.12.0".into()),
            project: Some("Minimal API".into()),
            attached: true,
            tab: Some("t1".into()),
            url: Some("http://localhost:5180/".into()),
            agents_allowed: true,
            ..Default::default()
        };
        let child = SessionInfo {
            id: 3,
            parent: Some(2),
            mode: "break".into(),
            stopped: Some("breakpoint".into()),
            ..browser.clone()
        };
        let sessions = DebugOutput::Sessions(SessionsOutput {
            sessions: vec![browser.clone(), child.clone()],
            active: Some(3),
        })
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-sessions.output.json"),
            &sessions,
        );
        let state = DebugOutput::State(Box::new(DebugState {
            mode: "break".into(),
            session: Some(SessionRow {
                id: Some(3),
                project: "Minimal API".into(),
                program: "http://localhost:5180/".into(),
                debug: true,
                runtime: Some("javascript".into()),
                attached: true,
                parent: Some(2),
                tab: Some("t1".into()),
                url: Some("http://localhost:5180/".into()),
                ..Default::default()
            }),
            frames: vec![FrameRow {
                index: 0,
                name: "total".into(),
                path: Some("/w/wwwroot/app.ts".into()),
                line: Some(15),
                source: Some(source),
                ..Default::default()
            }],
            sessions: vec![browser, child],
            ..Default::default()
        }))
        .to_json();
        conforms(
            include_str!("../../../protocol/schemas/debug-state.output.json"),
            &state,
        );
        assert_eq!(state["sessions"][1]["parent"], 2);
        assert_eq!(state["frames"][0]["source"]["generated_line"], 18);
    }
}
