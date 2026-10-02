//! The debugger's commands (brief 0018): `eludite.debug.start`, `stop`, `continue`, `step_over`, `step_into`,
//! `step_out`, `run_to_cursor`, `toggle_breakpoint`, `evaluate` and `state`, plus `select_frame` (the Call Stack and
//! Threads windows' clicks), `watch` (the Watch window's rows) and `exception_settings` (the Exception Settings
//! window), which every user-visible action needs to be a command (CLAUDE.md invariant 3).
//!
//! The keys, the Debug menu, the margin, the debugger windows and agents all run these, against one state machine
//! in the shell (PLAN.md 5.5): see [`DebugTarget`]. Every command but `evaluate` answers with the debugger's state
//! ([`DebugState`], `debug-state.output.json`), which is what the windows show.
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

pub const ALL: [&str; 13] = [
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
];

/// The longest an agent's command may wait for the debuggee (`wait_ms`).
pub const MAX_WAIT_MS: u64 = 30_000;

const STATE_OUTPUT: &str = include_str!("../../../protocol/schemas/debug-state.output.json");

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
            STATE_OUTPUT,
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
            STATE_OUTPUT,
            Execute,
        ),
        STEP_OVER => (
            "Debug: Step Over",
            input!("debug-step-over.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        STEP_INTO => (
            "Debug: Step Into",
            input!("debug-step-into.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        STEP_OUT => (
            "Debug: Step Out",
            input!("debug-step-out.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        RUN_TO_CURSOR => (
            "Debug: Run To Cursor",
            input!("debug-run-to-cursor.input.json"),
            STATE_OUTPUT,
            Execute,
        ),
        EVALUATE => (
            "Debug: Evaluate Expression",
            input!("debug-evaluate.input.json"),
            input!("debug-evaluate.output.json"),
            Execute,
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
    },
    Stop,
    Continue {
        stop: Option<u64>,
        wait_ms: Option<u64>,
    },
    Step {
        kind: StepKind,
        thread: Option<i64>,
        stop: Option<u64>,
        wait_ms: Option<u64>,
    },
    RunToCursor {
        path: Option<String>,
        line: Option<u32>,
        stop: Option<u64>,
        wait_ms: Option<u64>,
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
        }
    }

    /// How long an agent asked to wait for the debuggee.
    pub fn wait_ms(&self) -> Option<u64> {
        match self {
            DebugRequest::Start { wait_ms, .. }
            | DebugRequest::Continue { wait_ms, .. }
            | DebugRequest::Step { wait_ms, .. }
            | DebugRequest::RunToCursor { wait_ms, .. } => *wait_ms,
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
}

impl DebugOutput {
    pub fn to_json(&self) -> Value {
        match self {
            DebugOutput::State(s) => serde_json::to_value(s),
            DebugOutput::Evaluate(e) => serde_json::to_value(e),
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
pub fn parse(id: &str, value: Value) -> Result<DebugRequest, CommandError> {
    Ok(match id {
        START => {
            let i: StartIn = input(value)?;
            DebugRequest::Start {
                project: non_empty("project", i.project)?,
                debug: i.debug.unwrap_or(true),
                profile: non_empty("profile", i.profile)?,
                build: i.build,
                wait_ms: check_wait(i.wait_ms)?,
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
            }
        }
        RUN_TO_CURSOR => {
            let i: CursorIn = input(value)?;
            DebugRequest::RunToCursor {
                path: non_empty("path", i.path)?,
                line: check_line(i.line)?,
                stop: check_stop(i.stop)?,
                wait_ms: check_wait(i.wait_ms)?,
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

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
                wait_ms: None
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
                wait_ms: None
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
                wait_ms: Some(100)
            }
        );
        assert!(parse(STEP_OUT, json!({"stop": 0})).is_err());
        assert_eq!(
            parse(CONTINUE, json!({})).unwrap(),
            DebugRequest::Continue {
                stop: None,
                wait_ms: None
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
            },
            last_driver: Some("agent:Claude Code".into()),
            message: None,
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
}
