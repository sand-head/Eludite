//! Run and debug (brief 0018): F5 runs the startup project under its debug adapter through `eludite-dap` (netcoredbg
//! for .NET, `eludite-dbg-mono` under the located Mono for .NET Framework on Linux and macOS: brief 0022), Ctrl+F5
//! without the debugger, with Visual Studio's debugger windows, breakpoints in the margin, the execution point, data tips,
//! the Debug menu and keys and a status bar slot. Every action is an `eludite.debug.*` command, and agents drive a
//! session through the same commands and read the same state the windows render ([`state`], whose module docs give
//! the two-driver rules).
//!
//! - **Never waiting on the adapter.** The launch (project resolution, the launch configuration, the adapter's choice
//!   by target framework and platform, finding Mono and reading `mono --version`, finding and starting the adapter,
//!   DAP's handshake) runs on a `debug-launch` thread; nothing of Mono is touched before F5. Every later request is sent with
//!   [`DapClient::request`], which only queues it; answers and events come back through one channel and are applied
//!   on the UI thread in batches, tagged with the session generation so an old session's never land.
//! - **Hit counts.** netcoredbg ignores `hitCondition`, so the shell counts hits itself: a stop at a breakpoint whose
//!   hit condition is not met resumes at once and is never shown. An adapter that supports hit conditions gets them.
//! - **Persistence.** Breakpoints, exception settings and watch expressions are saved per solution under
//!   `<config dir>/eludite/breakpoints/solutions/`, written off the UI thread.
//! - **Build before run** (brief 0020). With the setting `build.beforeRun` (on by default), F5 and Ctrl+F5 first run
//!   `eludite.build.project` for the project to start, through the bus like the Build menu, streaming into the Output
//!   window; the session is in mode `building` meanwhile. The launch starts when that build succeeds; a failed,
//!   refused or canceled build ends the start with the reason in the status bar, and a failed one brings the Error
//!   List forward. Shift+F5 during the build cancels both. MSBuild's incremental build makes an up-to-date project
//!   cost a check, so the build always runs rather than the shell guessing whether the project is stale.
//! - **Inspection for agents** (brief 0025). `snapshot`, `stack`, `variables`, `exception_info`, `output`, `wait` and
//!   `pause`, and the stop summary every command that runs the debuggee answers with. An agent's command is applied on
//!   the UI thread like any other; what it then waits for (the debuggee to settle, a condition of `wait`, the adapter's
//!   answers to the requests a read needs) is awaited in a task of its own, never on the UI thread. Those requests are
//!   tagged with the session generation and the stop ([`Pending::Agent`]): an answer for an older stop is a stale
//!   error, never a value (rule 4). Reads take the thread and frame as parameters and never move the windows'
//!   selection, the Locals window or the execution point (proposal 0001 rule 3). The UI thread cannot wait, so from
//!   it the summary is the model's ([`state::DebugModel::summary`]) and reads that need the adapter are refused.
//! - **Output by source.** The program's lines (stdout, stderr), the debugger's own messages and the adapter's
//!   (stderr, console) go to three rings of 10,000 lines per session, read by cursor (`eludite.debug.output`); the
//!   Output window's Debug source still shows the program's output and the debugger's messages together.

pub mod state;
#[cfg(test)]
mod tests;
pub mod windows;

use std::cell::RefCell;
use std::collections::HashMap;
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::debug::{
    self as cmds, BreakpointAction, Budget, CapabilitiesRow, DebugOutput, DebugRequest,
    EvalContext, EvaluateOutput, ExceptionDetailsRow, ExceptionInfoOutput, FramesBlock,
    LocalsBlock, OutputKind, OutputPage, ScopeKind, SessionRow, StackFrameRow, StackOutput,
    StackThread, StopSummary, StoppedRow, ThreadRow, VarRow, VariableRow, VariablesOutput,
    VariablesTarget, WaitUntil,
};
use eludite_commands::view::{DockEdge, DockTarget, ViewRequest, ViewTarget as _};
use eludite_commands::{Caller, CommandError};
use eludite_dap::discovery::{AdapterSearch, MonoAdapterSearch, MonoSearch};
use eludite_dap::launch::{AdapterKind, FrameworkKind, Platform};
use eludite_dap::session::{self as dap_session, StartKind, StartPlan, Started};
use eludite_dap::types::{
    Capabilities, EvaluateResponse, Event, ExceptionDetails, ExceptionInfoResponse, ScopesResponse,
    SetBreakpointsResponse, StackTraceResponse, StoppedEvent, ThreadsResponse, Variable,
    VariablesResponse,
};
use eludite_dap::{ClientEvent, Connection, DapClient, launch, transport};
use eludite_docking::ids;
use eludite_editor::{EditorEvent, ExecutionKind};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui::{AppContext as _, AsyncWindowContext, Context, WeakEntity, Window};
use serde_json::{Value, json};

use self::state::{
    DebugModel, Frame, Mode, Persisted, VarNode, exception_filters, flatten, node_mut, row_at_mut,
};
use self::windows::{DebugWindows, StackRow, ThreadLine};
use super::Shell;
use super::documents::{normalize_path, trace};

/// Status bar slot: the debugger's state (left, after the solution's).
pub const DEBUG_SLOT: &str = "debug";
/// How long an agent's resuming command waits for the debuggee to settle, and its evaluate for the answer, by
/// default.
pub const AGENT_WAIT: Duration = Duration::from_secs(5);
/// How long the launch handshake may take.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(30);
/// How long Stop waits for the adapter to end the session before killing it.
const STOP_TIMEOUT: Duration = Duration::from_secs(3);
/// Frames fetched per stop.
const STACK_LEVELS: usize = 200;

/// Opens a connection to the adapter (tests: the fake adapter).
pub type Connector = Arc<dyn Fn() -> std::io::Result<Connection> + Send + Sync>;

/// How sessions reach their adapter and where breakpoints persist.
#[derive(Clone)]
pub struct DebugSetup {
    /// Connect here instead of locating and starting the adapter (tests). The launch plan is computed as usual.
    pub connect: Option<Connector>,
    /// netcoredbg (.NET).
    pub search: AdapterSearch,
    /// Mono, which runs .NET Framework programs and `eludite-dbg-mono` off Windows (brief 0022).
    pub mono: MonoSearch,
    /// `eludite-dbg-mono.exe`.
    pub mono_adapter: MonoAdapterSearch,
    /// The platform adapter selection assumes (the current one; tests choose).
    pub platform: Platform,
    /// The directory breakpoints persist in (`None`: `<config dir>/eludite/breakpoints`).
    pub store_dir: Option<PathBuf>,
    /// The `dotnet` Start Without Debugging runs (`dotnet` on `PATH`).
    pub dotnet: String,
}

impl DebugSetup {
    pub fn from_env() -> Self {
        Self {
            connect: None,
            // The configured paths come from the settings store (`set_adapter_path`, `set_mono_prefix`,
            // `set_mono_adapter_path`), which resolves the variables.
            search: AdapterSearch {
                env: None,
                ..AdapterSearch::from_env()
            },
            mono: MonoSearch::from_env(),
            mono_adapter: MonoAdapterSearch::from_env(),
            platform: Platform::current(),
            store_dir: eludite_docking::eludite_config_dir().map(|d| d.join("breakpoints")),
            dotnet: "dotnet".into(),
        }
    }
}

pub type Outcome = Result<DebugOutput, CommandError>;

/// `eludite.debug.*` from another thread (an agent), for the UI thread to apply.
pub struct DebugJob {
    pub request: DebugRequest,
    pub reply: mpsc::SyncSender<Outcome>,
    pub caller: Caller,
}

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// The shell's `DebugTarget`: on the UI thread the shell has applied the request already (keys, menus, windows);
/// from another thread the request is posted to the UI and the caller waits for its answer.
pub struct DebugBus {
    pub ui_thread: std::thread::ThreadId,
    pub jobs: UnboundedSender<DebugJob>,
}

impl cmds::DebugTarget for DebugBus {
    fn apply(&self, request: DebugRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let wait = request
            .wait_ms()
            .map(Duration::from_millis)
            .unwrap_or(AGENT_WAIT);
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(DebugJob {
                request,
                reply,
                caller: eludite_commands::current_caller(),
            })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(wait + Duration::from_secs(30))
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
    }
}

/// Register the debug commands; the shell drains the returned queue.
pub fn register(commands: &eludite_commands::CommandRegistry) -> UnboundedReceiver<DebugJob> {
    let (tx, rx) = unbounded();
    cmds::register(
        commands,
        Arc::new(DebugBus {
            ui_thread: std::thread::current().id(),
            jobs: tx,
        }),
    );
    rx
}

/// A program run without debugging.
type RunHandle = Arc<Mutex<Option<Child>>>;

/// What the launch thread, the adapter and a program run report.
pub enum DebugMsg {
    /// The launch configuration is known.
    Launched {
        generation: u64,
        session: SessionRow,
        run: Option<RunHandle>,
    },
    Connected {
        generation: u64,
        client: DapClient,
    },
    Started {
        generation: u64,
        result: Result<Started, String>,
        /// `initialize`'s `adapterID` (`coreclr`, `mono`), or `fake` for the tests' fake adapter.
        adapter_id: String,
    },
    LaunchFailed {
        generation: u64,
        message: String,
    },
    Client {
        generation: u64,
        event: ClientEvent,
    },
    /// A program run without debugging wrote a line: on `stdout` or `stderr`.
    Output {
        generation: u64,
        text: String,
        stream: &'static str,
    },
    ProgramExited {
        generation: u64,
        code: Option<i32>,
    },
    /// A solution's persisted breakpoints were read.
    Loaded {
        solution: PathBuf,
        persisted: Option<Persisted>,
    },
    StopTimeout {
        generation: u64,
    },
}

/// What an agent's command waits for after it was applied, off the UI thread (brief 0025).
pub enum Followup {
    /// `evaluate`'s answer.
    Eval(oneshot::Receiver<EvaluateOutput>),
    /// A command that runs the debuggee: wait until it settles (a start: until it runs), then answer the summary.
    /// `pause`: Break All of that generation, which fails when the adapter refuses it.
    Settle {
        start: bool,
        pause: Option<u64>,
        wait: Duration,
        budget: Budget,
    },
    /// `stop`: until the session ended, then the state (as before brief 0025).
    Ended {
        wait: Duration,
    },
    /// `wait`: until the condition holds (`baseline`: the program output's cursor it waits past).
    Wait {
        until: WaitUntil,
        stop: Option<u64>,
        baseline: u64,
        wait: Duration,
        budget: Budget,
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
    },
    Variables {
        target: VariablesTarget,
        start: usize,
        count: usize,
        depth: usize,
        filter: Option<String>,
        max_value_chars: usize,
    },
    ExceptionInfo {
        thread: i64,
    },
}

enum VarTarget {
    Locals(Vec<usize>),
    Watch(Vec<usize>),
    Agent {
        reply: oneshot::Sender<EvaluateOutput>,
        out: EvaluateOutput,
    },
}

enum EvalTarget {
    Watch(usize),
    Console,
    Hover {
        doc: String,
        id: u64,
        expression: String,
    },
    Agent {
        reply: oneshot::Sender<EvaluateOutput>,
        expression: String,
        expand: bool,
    },
}

enum Pending {
    Threads {
        generation: u64,
    },
    /// The stack of a stop being examined (it is not shown before its breakpoint's hit condition is checked).
    StopStack {
        generation: u64,
        stopped: StoppedEvent,
        thread: i64,
    },
    /// Another thread's stack (the Threads window).
    ThreadStack {
        generation: u64,
        stop: u64,
        thread: i64,
        frame: usize,
    },
    Scopes {
        generation: u64,
        stop: u64,
    },
    Vars {
        generation: u64,
        stop: u64,
        target: VarTarget,
    },
    Eval {
        generation: u64,
        stop: u64,
        target: EvalTarget,
    },
    ExceptionInfo {
        generation: u64,
        stop: u64,
    },
    SetBreakpoints {
        generation: u64,
        path: String,
        lines: Vec<u32>,
    },
    Resume {
        generation: u64,
        stop: u64,
    },
    /// Break All was sent.
    Pause {
        generation: u64,
    },
    /// A request an agent's read waits for (brief 0025): its answer goes to `reply` when it is for the generation and
    /// the stop it was asked in, else the read hears that it is stale.
    Agent {
        generation: u64,
        stop: u64,
        reply: oneshot::Sender<Result<Value, String>>,
    },
    Other,
}

/// When things happened (the benchmarks and the report).
#[derive(Debug, Clone, Default)]
pub struct DebugTimings {
    /// `eludite.debug.start` applied.
    #[cfg_attr(not(test), allow(dead_code))]
    pub start: Option<Instant>,
    /// The first break of the session was shown (locals loaded).
    pub first_break: Option<Instant>,
    /// A step was sent and not yet shown.
    pub step_sent: Option<Instant>,
    /// When each agent's read held the UI thread, and for how long (brief 0025's frame cost while an agent polls).
    pub agent_ui: Vec<(Instant, Duration)>,
    /// The last `wait` answered.
    pub wait_answered: Option<Instant>,
    /// Step sent to its break shown (locals loaded and the windows given them).
    pub steps: Vec<Duration>,
    /// The last time the windows were given a break's locals.
    pub locals_shown: Option<Instant>,
    /// Build before run (brief 0020): the build was requested, its result arrived, the launch thread started.
    pub build_requested: Option<Instant>,
    pub build_finished: Option<Instant>,
    pub launched: Option<Instant>,
}

/// A start waiting for its build (brief 0020).
#[derive(Debug, Clone)]
pub struct PendingLaunch {
    /// The session generation the start began.
    pub generation: u64,
    /// The build's ticket, once it started.
    pub ticket: Option<u64>,
    project: Option<String>,
    debug: bool,
    profile: Option<String>,
    driver: String,
}

/// How the build before a launch ended.
#[derive(Debug, Clone, PartialEq)]
pub enum PrelaunchBuild {
    Succeeded,
    Failed {
        errors: u32,
        warnings: u32,
    },
    /// Canceled, refused or lost with its host.
    Ended(String),
}

/// Where the execution point is: a document id, a 1-based statement range, which arrow.
#[derive(Debug, Clone, PartialEq, Eq)]
struct ExecPoint {
    path: String,
    line: u32,
    column: u32,
    end: Option<(u32, u32)>,
    kind: ExecutionKind,
}

/// The debugger: the model, the windows and the session's plumbing.
pub struct Debugger {
    pub model: DebugModel,
    pub windows: DebugWindows,
    setup: DebugSetup,
    tx: UnboundedSender<DebugMsg>,
    client: Option<DapClient>,
    run: Option<RunHandle>,
    caps: Capabilities,
    pending: HashMap<i64, Pending>,
    run_to_cursor: Option<(String, u32)>,
    exec: Option<ExecPoint>,
    waiters: Vec<oneshot::Sender<()>>,
    /// A data tip about to evaluate: (document, popup id).
    hover: Option<(String, u64)>,
    solution: Option<PathBuf>,
    console_partial: String,
    /// Text for the Output window's Debug source not given to it yet (brief 0020), and whether to clear it first.
    output_queue: String,
    output_clear: bool,
    /// `breakpoint` events for ids not known yet: netcoredbg binds breakpoints (events) before the handshake's
    /// `setBreakpoints` answers reach the shell.
    early_breakpoints: Vec<eludite_dap::types::Breakpoint>,
    pub timings: DebugTimings,
    /// F5's build, while it runs (brief 0020).
    pub pending_launch: Option<PendingLaunch>,
    /// Break All failed in this generation: the adapter's message.
    pause_error: Option<(u64, String)>,
    /// How many members each variables reference of the current stop has, as the adapter said (`indexedVariables`,
    /// `namedVariables`): the `total` of `eludite.debug.variables` by reference.
    counts: HashMap<i64, usize>,
}

impl Debugger {
    pub fn new<T>(
        setup: DebugSetup,
        theme: eludite_ui::Theme,
        cx: &mut Context<T>,
    ) -> (Self, UnboundedReceiver<DebugMsg>) {
        let (tx, rx) = unbounded();
        (
            Self {
                model: DebugModel::default(),
                windows: DebugWindows::new(theme, cx),
                setup,
                tx,
                client: None,
                run: None,
                caps: Capabilities::default(),
                pending: HashMap::new(),
                run_to_cursor: None,
                exec: None,
                waiters: Vec::new(),
                hover: None,
                solution: None,
                console_partial: String::new(),
                output_queue: String::new(),
                output_clear: false,
                early_breakpoints: Vec::new(),
                timings: DebugTimings::default(),
                pending_launch: None,
                pause_error: None,
                counts: HashMap::new(),
            },
            rx,
        )
    }

    /// Send a request to the adapter without waiting; its answer comes back as a message.
    fn send(&mut self, command: &str, args: Value, pending: Pending) -> Result<i64, CommandError> {
        let client = self
            .client
            .as_ref()
            .ok_or_else(|| CommandError::Failed("the debug adapter is not connected yet".into()))?;
        let seq = client
            .request(command, args)
            .map_err(|e| CommandError::Failed(e.to_string()))?;
        self.pending.insert(seq, pending);
        Ok(seq)
    }

    /// The program wrote `text` (`stdout` or `stderr`): its ring, and the Output window's Debug source.
    fn program_output(&mut self, text: &str, stream: &'static str) {
        self.model
            .output_mut(OutputKind::Program)
            .push_text(text, Some(stream));
        self.console(text);
    }

    /// A request for an agent's read, tagged with the generation and the stop it reads (rule 4 of brief 0018).
    fn agent_request(
        &mut self,
        generation: u64,
        stop: u64,
        command: &str,
        args: Value,
    ) -> Result<oneshot::Receiver<Result<Value, String>>, String> {
        let m = &self.model;
        if m.generation != generation || m.stop != stop || m.mode != Mode::Break {
            return Err(format!(
                "stale: the debuggee moved on (now {}, stop {}, generation {}) before the read finished; read it \
                 again",
                m.mode.as_str(),
                m.stop,
                m.generation
            ));
        }
        let (tx, rx) = oneshot::channel();
        self.send(
            command,
            args,
            Pending::Agent {
                generation,
                stop,
                reply: tx,
            },
        )
        .map_err(|e| e.to_string())?;
        Ok(rx)
    }

    fn console(&mut self, text: &str) {
        let text = text.replace('\r', "");
        self.output_queue.push_str(&text);
        let mut buf = std::mem::take(&mut self.console_partial);
        buf.push_str(&text);
        let mut parts: Vec<&str> = buf.split('\n').collect();
        let rest = parts.pop().unwrap_or_default().to_owned();
        for p in parts {
            self.model.push_console(p);
        }
        self.console_partial = rest;
    }

    fn console_line(&mut self, line: impl Into<String>) {
        if !self.console_partial.is_empty() {
            let p = std::mem::take(&mut self.console_partial);
            self.model.push_console(p);
            self.output_queue.push('\n');
        }
        let line = line.into();
        self.model
            .output_mut(OutputKind::Debug)
            .push_line(line.clone(), None);
        self.output_queue.push_str(&line);
        self.output_queue.push('\n');
        self.model.push_console(line);
    }

    fn generation(&self) -> u64 {
        self.model.generation
    }

    /// The setting `debugger.netcoredbgPath` (or `ELUDITE_NETCOREDBG`): searched where `ELUDITE_NETCOREDBG` was
    /// (brief 0020). `None` leaves the search to the executable's folder and `PATH`.
    pub fn set_adapter_path(&mut self, path: Option<PathBuf>) {
        self.setup.search.env = path.map(PathBuf::into_os_string);
    }

    /// The setting `debugger.monoPrefix` (or `ELUDITE_MONO_PREFIX`): the first place the next session looks for Mono
    /// (brief 0022). `None` leaves the search to `PATH` and the usual prefixes.
    pub fn set_mono_prefix(&mut self, prefix: Option<PathBuf>) {
        self.setup.mono.configured = prefix;
    }

    /// The setting `debugger.monoAdapterPath` (or `ELUDITE_DBG_MONO`): where the next session looks for
    /// `eludite-dbg-mono.exe` after the executable's folder.
    pub fn set_mono_adapter_path(&mut self, path: Option<PathBuf>) {
        self.setup.mono_adapter.configured = path;
    }

    /// The searches the next session uses (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn setup(&self) -> &DebugSetup {
        &self.setup
    }

    fn store_path(&self, solution: &Path) -> Option<PathBuf> {
        self.setup
            .store_dir
            .as_ref()
            .map(|d| eludite_docking::LayoutStore::new(d.clone()).solution_path(solution))
    }
}

fn driver_of(caller: &Caller) -> String {
    match caller {
        Caller::User => "user".into(),
        Caller::Agent { agent, .. } => format!("agent:{agent}"),
    }
}

fn file_name(path: &str) -> String {
    Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
}

fn norm(path: &str) -> String {
    normalize_path(Path::new(path))
        .to_string_lossy()
        .into_owned()
}

fn eval_failed(expression: &str, stop: u64, message: impl Into<String>) -> EvaluateOutput {
    EvaluateOutput {
        expression: expression.to_owned(),
        state: "failed".into(),
        message: Some(message.into()),
        stop,
        ..Default::default()
    }
}

/// Resolve the project to run and its launch configuration (on the launch thread: it reads files).
fn resolve_launch(
    hint: Option<&str>,
    profile: Option<&str>,
    projects: &[PathBuf],
    solution_dir: Option<&Path>,
    startup: Option<&Path>,
) -> Result<launch::LaunchConfig, String> {
    let project = resolve_project(hint, projects, solution_dir, startup)?;
    launch::launch_config(&project, profile)
}

/// The project file to run: the hint (a path or a project name), else the startup project (Set as Startup
/// Project's, else the solution's first executable project). Reads project files: off the UI thread.
pub(super) fn resolve_project(
    hint: Option<&str>,
    projects: &[PathBuf],
    solution_dir: Option<&Path>,
    startup: Option<&Path>,
) -> Result<PathBuf, String> {
    if hint.is_none()
        && let Some(s) = startup.filter(|s| {
            projects
                .iter()
                .any(|p| normalize_path(p) == normalize_path(s))
        })
    {
        return Ok(s.to_path_buf());
    }
    Ok(match hint {
        Some(h) => {
            let p = Path::new(h);
            let as_path = if p.is_absolute() {
                Some(p.to_path_buf())
            } else {
                solution_dir.map(|d| d.join(p))
            };
            match as_path.filter(|p| p.is_file()) {
                Some(p) => p,
                None => projects
                    .iter()
                    .find(|p| {
                        p.file_stem()
                            .is_some_and(|s| s.to_string_lossy().eq_ignore_ascii_case(h))
                    })
                    .cloned()
                    .ok_or_else(|| format!("no project `{h}` in the solution"))?,
            }
        }
        None => launch::startup_project(projects).ok_or_else(|| {
            if projects.is_empty() {
                "open a .NET solution first: there is no startup project".to_owned()
            } else {
                "the .NET solution has no executable project to start".to_owned()
            }
        })?,
    })
}

/// What the launch thread needs.
struct LaunchJob {
    generation: u64,
    debug: bool,
    hint: Option<String>,
    profile: Option<String>,
    projects: Vec<PathBuf>,
    solution_dir: Option<PathBuf>,
    startup: Option<PathBuf>,
    breakpoints: Vec<(String, Vec<eludite_dap::types::SourceBreakpoint>)>,
    filters: Vec<String>,
    setup: DebugSetup,
    tx: UnboundedSender<DebugMsg>,
}

fn launch_thread(job: LaunchJob) {
    let LaunchJob {
        generation,
        debug,
        hint,
        profile,
        projects,
        solution_dir,
        startup,
        breakpoints,
        filters,
        setup,
        tx,
    } = job;
    let fail = |message: String| {
        let _ = tx.unbounded_send(DebugMsg::LaunchFailed {
            generation,
            message,
        });
    };
    let config = match resolve_launch(
        hint.as_deref(),
        profile.as_deref(),
        &projects,
        solution_dir.as_deref(),
        startup.as_deref(),
    ) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let platform = setup.platform;
    let mut session = SessionRow {
        project: config.project.to_string_lossy().into_owned(),
        program: config.program.to_string_lossy().into_owned(),
        args: config.args.clone(),
        cwd: config.cwd.to_string_lossy().into_owned(),
        profile: config.profile.clone(),
        debug,
        adapter: None,
        runtime: Some(launch::runtime_name(config.kind, platform).to_owned()),
        process_id: None,
    };
    // A .NET Framework program off Windows runs under the located Mono, with or without the debugger.
    let needs_mono = config.kind == FrameworkKind::NetFramework && platform != Platform::Windows;
    let mono = if needs_mono {
        match setup.mono.find_mono() {
            Ok(m) => Some(m),
            Err(e) => return fail(e),
        }
    } else {
        None
    };
    if !debug {
        let (cmd, args) = match config.run_command(
            platform,
            &setup.dotnet,
            mono.as_ref().map(|m| m.mono.as_path()),
        ) {
            Ok(c) => c,
            Err(e) => return fail(e),
        };
        let mono_env = mono.as_ref().map(|m| m.env.clone()).unwrap_or_default();
        let child = Command::new(&cmd)
            .args(&args)
            .current_dir(&config.cwd)
            .envs(mono_env)
            .envs(&config.env)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn();
        let mut child = match child {
            Ok(c) => c,
            Err(e) => return fail(format!("{cmd}: {e}")),
        };
        session.process_id = Some(i64::from(child.id()));
        let streams = [
            child
                .stdout
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
            child
                .stderr
                .take()
                .map(|s| Box::new(s) as Box<dyn std::io::Read + Send>),
        ];
        let handle: RunHandle = Arc::new(Mutex::new(Some(child)));
        let _ = tx.unbounded_send(DebugMsg::Launched {
            generation,
            session,
            run: Some(handle.clone()),
        });
        let mut readers = Vec::new();
        for (s, stream) in streams.into_iter().zip(["stdout", "stderr"]) {
            let Some(s) = s else { continue };
            let tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(s).lines() {
                    let Ok(line) = line else { break };
                    let _ = tx.unbounded_send(DebugMsg::Output {
                        generation,
                        text: format!("{line}\n"),
                        stream,
                    });
                }
            }));
        }
        for r in readers {
            let _ = r.join();
        }
        let code = loop {
            let status = handle
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .as_mut()
                .and_then(|c| c.try_wait().ok().flatten());
            if let Some(s) = status {
                break s.code();
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        let _ = tx.unbounded_send(DebugMsg::ProgramExited { generation, code });
        return;
    }
    let kind = match launch::select_adapter(config.kind, platform) {
        Ok(k) => k,
        Err(e) => return fail(e),
    };
    let (adapter_id, arguments) = match (kind, &mono) {
        (AdapterKind::Mono, Some(m)) => ("mono", config.mono_arguments(&m.mono)),
        _ => ("coreclr", config.netcoredbg_arguments()),
    };
    // `mono --version`, read once here: the adapter's description names the Mono that runs it.
    let mono_version = mono
        .as_ref()
        .map(|m| m.version().unwrap_or_else(|| "(unknown version)".into()));
    let (connection, adapter) = match &setup.connect {
        Some(connect) => match connect() {
            Ok(c) => {
                let d = match &mono_version {
                    Some(v) => format!("eludite-dbg-mono under mono {v} ({})", c.description),
                    None => c.description.clone(),
                };
                (c, d)
            }
            Err(e) => return fail(format!("cannot reach the debug adapter: {e}")),
        },
        None => match (kind, &mono) {
            (AdapterKind::Mono, Some(m)) => {
                let exe = match setup.mono_adapter.find() {
                    Ok(p) => p,
                    Err(e) => return fail(e),
                };
                let t = m.adapter_transport(&exe);
                match transport::connect_with_env(&t, &m.env) {
                    Ok(c) => (
                        c,
                        format!(
                            "eludite-dbg-mono under mono {} (stdio)",
                            mono_version.as_deref().unwrap_or_default()
                        ),
                    ),
                    Err(e) => return fail(format!("cannot start eludite-dbg-mono: {e}")),
                }
            }
            _ => {
                let found = match setup.search.find_netcoredbg() {
                    Ok(f) => f,
                    Err(e) => return fail(e),
                };
                let t = found.transport();
                match transport::connect(&t) {
                    Ok(c) => (c, transport::describe(&t)),
                    Err(e) => return fail(format!("cannot start netcoredbg: {e}")),
                }
            }
        },
    };
    session.adapter = Some(adapter);
    let _ = tx.unbounded_send(DebugMsg::Launched {
        generation,
        session,
        run: None,
    });
    let sink_tx = tx.clone();
    let client = DapClient::start(
        connection,
        Arc::new(move |event| {
            let _ = sink_tx.unbounded_send(DebugMsg::Client { generation, event });
        }),
    );
    let _ = tx.unbounded_send(DebugMsg::Connected {
        generation,
        client: client.clone(),
    });
    let plan = StartPlan {
        adapter_id: adapter_id.into(),
        kind: StartKind::Launch,
        arguments,
        breakpoints,
        function_breakpoints: Vec::new(),
        exception_filters: filters,
    };
    let result = dap_session::start(&client, &plan, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    // The tests' fake adapter says so in its connection's description.
    let adapter_id = if client.description().starts_with("fake adapter") {
        "fake".to_owned()
    } else {
        plan.adapter_id.clone()
    };
    let _ = tx.unbounded_send(DebugMsg::Started {
        generation,
        result,
        adapter_id,
    });
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn debugger(&self) -> &Debugger {
        &self.debug
    }

    /// `eludite.debug.state`'s output now.
    pub fn debug_state(&self) -> DebugOutput {
        DebugOutput::State(Box::new(self.debug.model.state()))
    }

    /// A receiver woken at the next change of the debugger's state.
    pub fn debug_waiter(&mut self) -> oneshot::Receiver<()> {
        let (tx, rx) = oneshot::channel();
        self.debug.waiters.push(tx);
        rx
    }

    /// Whether an agent's command can answer now: a start once the program runs (or breaks with its locals
    /// loaded), a resume once the debuggee breaks with its locals loaded or the session ends.
    pub fn debug_settled(&self, start: bool) -> bool {
        (start && self.debug.model.mode == Mode::Running) || self.debug.model.settled()
    }

    /// The tasks that apply the debugger's messages (in batches: a burst of events costs one frame) and agents'
    /// commands.
    pub(super) fn debug_tasks(
        mut msgs: UnboundedReceiver<DebugMsg>,
        mut jobs: UnboundedReceiver<DebugJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (gpui::Task<()>, gpui::Task<()>) {
        use futures::StreamExt as _;
        let msg_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = msgs.next().await {
                let mut batch = vec![first];
                while let Ok(more) = msgs.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.on_debug_msgs(batch, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let job_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let DebugJob {
                    request,
                    reply,
                    caller,
                } = job;
                // One queue (rule 1): the command is applied here, in order; what it then waits for runs in a task of
                // its own, so a long `wait` never holds the next agent's command.
                let applied = this.update_in(cx, |shell, window, cx| {
                    let t = Instant::now();
                    let r = eludite_commands::with_caller(caller.clone(), || {
                        shell.apply_debug(request, &caller, true, window, cx)
                    });
                    shell.debug.timings.agent_ui.push((t, t.elapsed()));
                    r
                });
                match applied {
                    Err(_) => {
                        let _ =
                            reply.send(Err(CommandError::Failed("the window is closed".into())));
                    }
                    Ok(Err(e)) => {
                        let _ = reply.send(Err(e));
                    }
                    Ok(Ok((out, None))) => {
                        let _ = reply.send(Ok(out));
                    }
                    Ok(Ok((out, Some(follow)))) => {
                        let this = this.clone();
                        cx.spawn(async move |cx| {
                            let outcome = follow_up(this, cx, out, follow).await;
                            let _ = reply.send(outcome);
                        })
                        .detach();
                    }
                }
            }
        });
        (msg_task, job_task)
    }

    /// Apply a debug command (the UI-thread half of [`DebugBus`]). For an agent (`agent`: the caller waits off the
    /// UI thread) a command may also return what to wait for before answering ([`Followup`]); the UI thread gets its
    /// answer at once.
    pub fn apply_debug(
        &mut self,
        request: DebugRequest,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<Followup>), CommandError> {
        self.debug.model.check(&request)?;
        let driver = driver_of(caller);
        let budget = request.budget().unwrap_or_default();
        let wait = request
            .wait_ms()
            .map(Duration::from_millis)
            .unwrap_or(AGENT_WAIT);
        // Commands that run the debuggee answer with the summary once it settles (an agent waits for that).
        let settle = |start: bool, pause: Option<u64>| {
            (agent && !wait.is_zero()).then_some(Followup::Settle {
                start,
                pause,
                wait,
                budget,
            })
        };
        let follow = match request {
            DebugRequest::State => {
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::Start {
                project,
                debug,
                profile,
                build,
                ..
            } => {
                self.debug_start(project, debug, profile, build, &driver, window, cx);
                settle(true, None)
            }
            DebugRequest::Stop => {
                self.debug_stop(&driver, window, cx);
                self.refresh_debug(cx);
                let follow = (agent && !wait.is_zero()).then_some(Followup::Ended { wait });
                return Ok((self.debug_state(), follow));
            }
            DebugRequest::Continue { .. } => {
                self.debug_resume("continue", None, &driver)?;
                settle(false, None)
            }
            DebugRequest::Step { kind, thread, .. } => {
                self.debug.timings.step_sent = Some(Instant::now());
                self.debug_resume(kind.dap_command(), thread, &driver)?;
                settle(false, None)
            }
            DebugRequest::RunToCursor { path, line, .. } => {
                let (path, line) = self.debug_location(path.as_deref(), line, cx)?;
                self.debug.run_to_cursor = Some((path.clone(), line));
                self.debug_send_breakpoints(&path);
                self.debug_resume("continue", None, &driver)?;
                settle(false, None)
            }
            DebugRequest::Pause { thread, .. } => {
                let generation = self.debug_pause(thread, &driver)?;
                settle(false, Some(generation))
            }
            DebugRequest::Breakpoint {
                path,
                line,
                action,
                enabled,
                condition,
                hit_condition,
            } => {
                self.debug_breakpoint(path, line, action, enabled, condition, hit_condition, cx)?;
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::Evaluate {
                expression,
                frame,
                context,
                expand,
                ..
            } => {
                let stop = self.debug.model.stop;
                let out = self.debug_evaluate(expression.clone(), frame, context, expand, agent)?;
                self.refresh_debug(cx);
                return Ok(match out {
                    Ok(rx) => (
                        DebugOutput::Evaluate(EvaluateOutput {
                            expression,
                            state: "pending".into(),
                            stop,
                            ..Default::default()
                        }),
                        Some(Followup::Eval(rx)),
                    ),
                    Err(pending) => (DebugOutput::Evaluate(pending), None),
                });
            }
            DebugRequest::SelectFrame { thread, frame, .. } => {
                self.debug_select(thread, frame, window, cx)?;
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::AddWatch(expression) => {
                self.debug.model.watches.push(VarNode::watch(&expression));
                let ix = self.debug.model.watches.len() - 1;
                self.debug_eval_watch(ix);
                self.debug_persist(cx);
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::RemoveWatch(ix) => {
                if ix >= self.debug.model.watches.len() {
                    return Err(CommandError::InvalidInput(format!(
                        "there is no watch {ix} (there are {})",
                        self.debug.model.watches.len()
                    )));
                }
                self.debug.model.watches.remove(ix);
                self.debug_persist(cx);
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::ExceptionSettings {
                break_when_thrown,
                break_when_user_unhandled,
            } => {
                let e = &mut self.debug.model.exceptions;
                if let Some(v) = break_when_thrown {
                    e.break_when_thrown = v;
                }
                if let Some(v) = break_when_user_unhandled {
                    e.break_when_user_unhandled = v;
                }
                if self.debug.client.is_some() {
                    let filters = exception_filters(&self.debug.model.exceptions);
                    let _ = self.debug.send(
                        "setExceptionBreakpoints",
                        json!({ "filters": filters }),
                        Pending::Other,
                    );
                }
                self.debug_persist(cx);
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::Output {
                source,
                since,
                max_lines,
                pattern,
            } => {
                let (lines, next, dropped, total, truncated) = self
                    .debug
                    .model
                    .output(source)
                    .read(since, max_lines, pattern.as_ref());
                return Ok((
                    DebugOutput::Output(OutputPage {
                        source,
                        lines,
                        next,
                        dropped,
                        total,
                        truncated,
                    }),
                    None,
                ));
            }
            DebugRequest::Snapshot { thread, frame, .. } => {
                let m = &self.debug.model;
                let selected = thread.is_none_or(|t| Some(t) == m.thread)
                    && frame.is_none_or(|f| f == m.frame);
                if agent && m.mode == Mode::Break {
                    Some(Followup::Snapshot {
                        thread,
                        frame,
                        budget,
                    })
                } else if m.mode != Mode::Break || selected {
                    None
                } else {
                    return Err(ui_thread_refusal(cmds::SNAPSHOT));
                }
            }
            DebugRequest::Stack {
                thread,
                start,
                count,
                all_threads,
                ..
            } => {
                if agent {
                    Some(Followup::Stack {
                        thread,
                        start,
                        count,
                        all_threads,
                    })
                } else {
                    return self
                        .debug
                        .stack_from_model(thread, start, count, all_threads)
                        .map(|o| (DebugOutput::Stack(o), None))
                        .ok_or_else(|| ui_thread_refusal(cmds::STACK));
                }
            }
            DebugRequest::Variables {
                target,
                start,
                count,
                depth,
                filter,
                max_value_chars,
                ..
            } => {
                if !agent {
                    return Err(ui_thread_refusal(cmds::VARIABLES));
                }
                Some(Followup::Variables {
                    target,
                    start,
                    count,
                    depth,
                    filter,
                    max_value_chars,
                })
            }
            DebugRequest::ExceptionInfo { thread, .. } => {
                let m = &self.debug.model;
                let stopped = m.stopped.as_ref().map(|s| s.thread).unwrap_or_default();
                let thread = thread.unwrap_or(stopped);
                if thread == stopped {
                    return Ok((
                        DebugOutput::ExceptionInfo(self.debug.exception_from_model(thread)),
                        None,
                    ));
                }
                if !agent {
                    return Err(ui_thread_refusal(cmds::EXCEPTION_INFO));
                }
                Some(Followup::ExceptionInfo { thread })
            }
            DebugRequest::Wait { until, stop, .. } => {
                let m = &self.debug.model;
                let baseline = budget
                    .output_since
                    .unwrap_or_else(|| m.output(OutputKind::Program).next());
                if agent {
                    Some(Followup::Wait {
                        until,
                        stop,
                        baseline,
                        wait,
                        budget,
                    })
                } else {
                    // The UI thread cannot wait: what holds now.
                    let mut out = m.summary(&budget);
                    match wait_satisfied(m, until, stop, baseline) {
                        Some(why) => out.satisfied = Some(why.into()),
                        None => out.timed_out = Some(true),
                    }
                    return Ok((DebugOutput::Summary(Box::new(out)), None));
                }
            }
        };
        self.refresh_debug(cx);
        let out = DebugOutput::Summary(Box::new(self.debug.model.summary(&budget)));
        Ok((out, follow))
    }

    #[allow(clippy::too_many_arguments)]
    fn debug_start(
        &mut self,
        project: Option<String>,
        debug: bool,
        profile: Option<String>,
        build: Option<bool>,
        driver: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // Build first (brief 0020): only a .NET solution's projects are built and run here.
        let build = build.unwrap_or(self.builds.build_before_run) && self.solution.is_some();
        if !build {
            self.debug_launch(project, debug, profile, driver, false, cx);
            return;
        }
        let what = project
            .clone()
            .unwrap_or_else(|| "the startup project".into());
        let d = &mut self.debug;
        d.model.begin(Mode::Building, driver);
        d.timings = DebugTimings {
            start: Some(Instant::now()),
            ..DebugTimings::default()
        };
        d.model.console.clear();
        d.console_partial.clear();
        d.output_queue.clear();
        d.output_clear = true;
        d.console_line(format!("Building {what} before starting\u{2026}"));
        let generation = d.model.generation;
        d.pending_launch = Some(PendingLaunch {
            generation,
            ticket: None,
            project: project.clone(),
            debug,
            profile,
            driver: driver.to_owned(),
        });
        trace(format_args!("debug start: building {what} first"));
        // Which project to build is read from the project files: off the UI thread.
        let projects = self.solution_projects();
        let solution_dir = self.solution_dir();
        let startup = self.debug.model.startup_project.clone().map(PathBuf::from);
        let resolve = cx.background_spawn(async move {
            resolve_project(
                project.as_deref(),
                &projects,
                solution_dir.as_deref(),
                startup.as_deref(),
            )
        });
        cx.spawn_in(window, async move |this, cx| {
            let resolved = resolve.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.prelaunch_build(generation, resolved, window, cx)
            });
        })
        .detach();
        self.refresh_debug(cx);
    }

    /// The solution's project files, as the Workspace tree lists them.
    fn solution_projects(&self) -> Vec<PathBuf> {
        self.tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| PathBuf::from(&p.path))
            .collect()
    }

    /// The project to start is known: build it through the bus, as Build > Build Project does.
    fn prelaunch_build(
        &mut self,
        generation: u64,
        project: Result<PathBuf, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.debug.model.mode != Mode::Building
            || self.debug.pending_launch.as_ref().map(|p| p.generation) != Some(generation)
        {
            return;
        }
        let project = match project {
            Ok(p) => p,
            Err(e) => return self.prelaunch_failed(format!("Cannot start: {e}"), false, cx),
        };
        let started = self.invoke(
            eludite_commands::build::PROJECT,
            json!({ "project": project.to_string_lossy() }),
            window,
            cx,
        );
        match started {
            Ok(_) => {
                let ticket = self.builds.current.as_ref().map(|b| b.ticket);
                if let Some(p) = self.debug.pending_launch.as_mut() {
                    p.ticket = ticket;
                }
                self.debug.timings.build_requested = Some(Instant::now());
                trace(format_args!(
                    "debug start: build of {} requested",
                    project.display()
                ));
            }
            Err(e) => self.prelaunch_failed(
                format!("Cannot start: the build did not start: {e}"),
                false,
                cx,
            ),
        }
    }

    /// The build before a launch ended (`build`'s handlers call this for every build).
    pub(super) fn prelaunch_build_done(
        &mut self,
        ticket: u64,
        outcome: PrelaunchBuild,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(p) = self
            .debug
            .pending_launch
            .take_if(|p| p.ticket == Some(ticket))
        else {
            return;
        };
        if self.debug.model.mode != Mode::Building || self.debug.model.generation != p.generation {
            return;
        }
        let now = Instant::now();
        self.debug.timings.build_finished = Some(now);
        let plural = |n: u32, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
        match outcome {
            PrelaunchBuild::Succeeded => {
                trace(format_args!("debug start: build succeeded; launching"));
                self.debug_launch(p.project, p.debug, p.profile, &p.driver, true, cx);
            }
            PrelaunchBuild::Failed { errors, warnings } => self.prelaunch_failed(
                format!(
                    "Not started: the build failed ({}, {})",
                    plural(errors, "error"),
                    plural(warnings, "warning")
                ),
                true,
                cx,
            ),
            PrelaunchBuild::Ended(why) => {
                self.prelaunch_failed(format!("Not started: the build ended ({why})"), false, cx)
            }
        }
        let _ = window;
    }

    /// The start ends before launching: the status bar says why; a failed build brings the Error List forward.
    fn prelaunch_failed(&mut self, message: String, show_errors: bool, cx: &mut Context<Self>) {
        trace(format_args!("debug start: {message}"));
        self.debug.pending_launch = None;
        self.debug.console_line(message.clone());
        if show_errors {
            let _ = self.controller.apply(ViewRequest::Show {
                id: ids::ERROR_LIST.into(),
            });
        }
        self.end_session(Some(message), cx);
        self.refresh_debug(cx);
    }

    /// Launch the program (after its build, or at once without one).
    fn debug_launch(
        &mut self,
        project: Option<String>,
        debug: bool,
        profile: Option<String>,
        driver: &str,
        after_build: bool,
        cx: &mut Context<Self>,
    ) {
        let solution_dir = self.solution_dir();
        let d = &mut self.debug;
        if after_build {
            // The session began with the build: same generation, its console lines kept.
            d.model.mode = Mode::Launching;
        } else {
            d.model.begin(Mode::Launching, driver);
            d.timings = DebugTimings {
                start: Some(Instant::now()),
                ..DebugTimings::default()
            };
            d.model.console.clear();
            d.console_partial.clear();
            d.output_queue.clear();
            d.output_clear = true;
        }
        let launched = Instant::now();
        d.timings.launched = Some(launched);
        if let (Some(start), Some(req), Some(done)) = (
            d.timings.start,
            d.timings.build_requested,
            d.timings.build_finished,
        ) {
            // What F5 adds to the build: the budget is the build time plus 50 ms.
            let total = launched - start;
            let build = done - req;
            trace(format_args!(
                "debug start: launched {:.2} ms after F5; build {:.2} ms; added {:.2} ms",
                total.as_secs_f64() * 1e3,
                build.as_secs_f64() * 1e3,
                total.saturating_sub(build).as_secs_f64() * 1e3
            ));
        }
        d.pending.clear();
        d.caps = Capabilities::default();
        d.run_to_cursor = None;
        d.early_breakpoints.clear();
        d.console_line(format!(
            "{} {}\u{2026}",
            if debug {
                "Starting debugging"
            } else {
                "Starting without debugging"
            },
            project.as_deref().unwrap_or("the startup project")
        ));
        let breakpoints = d
            .model
            .breakpoints
            .files()
            .into_iter()
            .map(|f| {
                let (_, sbps) = d.model.breakpoints.source_breakpoints(&f, false, None);
                (f, sbps)
            })
            .collect();
        let projects: Vec<PathBuf> = self
            .tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| PathBuf::from(&p.path))
            .collect();
        let job = LaunchJob {
            generation: d.model.generation,
            debug,
            hint: project,
            profile,
            projects,
            solution_dir,
            startup: d.model.startup_project.clone().map(PathBuf::from),
            breakpoints,
            filters: exception_filters(&d.model.exceptions),
            setup: d.setup.clone(),
            tx: d.tx.clone(),
        };
        std::thread::Builder::new()
            .name("debug-launch".into())
            .spawn(move || launch_thread(job))
            .expect("spawn debug-launch");
        if debug {
            self.show_debug_windows();
        } else {
            // Start Without Debugging: the program's output is all there is to see.
            let _ = self.controller.apply(ViewRequest::Show {
                id: ids::OUTPUT.into(),
            });
        }
        self.output
            .update(cx, |o, cx| o.select(OutputSource::Debug, cx));
        self.refresh_glyphs(cx);
    }

    /// Visual Studio's Debug layout the first time: Locals and Watch beside the Error List and Output, Call Stack and
    /// Breakpoints in a second group at the bottom. A layout the user changed is left alone.
    fn show_debug_windows(&mut self) {
        let c = &self.controller;
        let hidden = |id: &str| {
            c.all_states()
                .iter()
                .find(|s| s.id == id)
                .is_none_or(|s| s.state == eludite_commands::view::WindowState::Hidden)
        };
        if hidden(ids::CALL_STACK) {
            let _ = c.apply(ViewRequest::Dock {
                id: Some(ids::CALL_STACK.into()),
                target: DockTarget::Side(DockEdge::Bottom),
            });
            for id in [ids::BREAKPOINTS] {
                if hidden(id) {
                    let _ = c.apply(ViewRequest::Dock {
                        id: Some(id.into()),
                        target: DockTarget::TabWith(ids::CALL_STACK.into()),
                    });
                }
            }
        }
        for id in [ids::WATCH, ids::LOCALS, ids::CALL_STACK] {
            let _ = c.apply(ViewRequest::Show { id: id.into() });
        }
    }

    fn debug_stop(&mut self, driver: &str, window: &mut Window, cx: &mut Context<Self>) {
        // Stopping during the build before a launch cancels that build and the launch.
        if self.debug.model.mode == Mode::Building {
            self.debug.model.last_driver = Some(driver.to_owned());
            let ticket = self.debug.pending_launch.take().and_then(|p| p.ticket);
            if ticket.is_some() && self.builds.current.as_ref().map(|b| b.ticket) == ticket {
                let _ = self.invoke(eludite_commands::build::CANCEL, json!({}), window, cx);
            }
            self.debug.console_line("Start canceled.");
            self.end_session(Some("Start canceled".into()), cx);
            return;
        }
        let generation = self.debug.generation();
        let d = &mut self.debug;
        d.model.last_driver = Some(driver.to_owned());
        let was = d.model.mode;
        d.model.mode = Mode::Stopping;
        if let Some(run) = d.run.clone() {
            if let Some(c) = run.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                let _ = c.kill();
            }
            return;
        }
        if d.client.is_some() {
            let _ = d.send(
                "disconnect",
                json!({ "terminateDebuggee": true }),
                Pending::Other,
            );
        } else if was == Mode::Launching {
            // Connected or LaunchFailed will see Stopping and end the session.
        }
        let tx = d.tx.clone();
        let timer = cx.background_executor().timer(STOP_TIMEOUT);
        cx.background_spawn(async move {
            timer.await;
            let _ = tx.unbounded_send(DebugMsg::StopTimeout { generation });
        })
        .detach();
    }

    /// Continue or step `thread` (default: the selected one).
    fn debug_resume(
        &mut self,
        command: &str,
        thread: Option<i64>,
        driver: &str,
    ) -> Result<(), CommandError> {
        let d = &mut self.debug;
        let tid = thread
            .or(d.model.thread)
            .or_else(|| d.model.threads.first().map(|t| t.id))
            .ok_or_else(|| CommandError::Failed("no thread to resume".into()))?;
        let stop = d.model.stop;
        let generation = d.generation();
        d.send(
            command,
            json!({ "threadId": tid }),
            Pending::Resume { generation, stop },
        )?;
        d.model.resume(driver);
        d.exec = None;
        Ok(())
    }

    /// Break All: ask the adapter to pause `thread` (default: the last one that stopped, else 0, all threads). Nothing
    /// moves until its `stopped` event (reason `pause`) arrives; then it is shown as any break. Returns the session
    /// generation the pause belongs to.
    fn debug_pause(&mut self, thread: Option<i64>, driver: &str) -> Result<u64, CommandError> {
        let d = &mut self.debug;
        let tid = thread.or(d.model.thread).unwrap_or(0);
        let generation = d.generation();
        d.send(
            "pause",
            json!({ "threadId": tid }),
            Pending::Pause { generation },
        )?;
        d.pause_error = None;
        // Who stopped it drives, as for a resume (the status bar's `agent driving`).
        d.model.last_driver = Some(driver.to_owned());
        trace(format_args!("debug pause thread {tid} by {driver}"));
        Ok(generation)
    }

    /// The path (normalized) and line a command names, defaulting to the active document and its caret.
    fn debug_location(
        &self,
        path: Option<&str>,
        line: Option<u32>,
        cx: &Context<Self>,
    ) -> Result<(String, u32), CommandError> {
        let id = match path {
            Some(p) => self.resolve_file(p).to_string_lossy().into_owned(),
            None => self
                .active_document()
                .filter(|id| self.documents.contains_key(id))
                .ok_or_else(|| {
                    CommandError::InvalidInput("no `path` given and no document is active".into())
                })?,
        };
        let line = match line {
            Some(l) => l,
            None => {
                let view = self
                    .documents
                    .get(&id)
                    .map(|d| d.view.clone())
                    .ok_or_else(|| {
                        CommandError::InvalidInput(format!(
                            "no `line` given and {id} is not open in an editor"
                        ))
                    })?;
                view.read(cx).editor().primary_head().row + 1
            }
        };
        Ok((id, line))
    }

    #[allow(clippy::too_many_arguments)]
    fn debug_breakpoint(
        &mut self,
        path: Option<String>,
        line: Option<u32>,
        action: BreakpointAction,
        enabled: Option<bool>,
        condition: Option<String>,
        hit_condition: Option<Option<cmds::HitCondition>>,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        let files = if action == BreakpointAction::DeleteAll {
            self.debug.model.breakpoints.delete_all()
        } else {
            let (path, line) = self.debug_location(path.as_deref(), line, cx)?;
            let b = &mut self.debug.model.breakpoints;
            match action {
                BreakpointAction::Toggle => {
                    b.toggle(&path, line);
                }
                BreakpointAction::Delete => {
                    if !b.delete(&path, line) {
                        return Err(CommandError::InvalidInput(format!(
                            "there is no breakpoint at {}, line {line}",
                            file_name(&path)
                        )));
                    }
                }
                BreakpointAction::Set => {
                    let bp = b.ensure(&path, line);
                    if let Some(e) = enabled {
                        bp.enabled = e;
                    }
                    if let Some(c) = condition {
                        bp.condition = (!c.trim().is_empty()).then(|| c.trim().to_owned());
                    }
                    if let Some(h) = hit_condition {
                        bp.hit_condition = h;
                    }
                }
                BreakpointAction::DeleteAll => unreachable!(),
            }
            vec![path]
        };
        for f in &files {
            self.debug_send_breakpoints(f);
        }
        self.refresh_glyphs(cx);
        self.debug_persist(cx);
        Ok(())
    }

    /// Send `path`'s breakpoints (and Run To Cursor's one-shot line) to a running session.
    fn debug_send_breakpoints(&mut self, path: &str) {
        let d = &mut self.debug;
        if d.client.is_none() {
            return;
        }
        let extra = d
            .run_to_cursor
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, l)| *l);
        let (lines, sbps) = d.model.breakpoints.source_breakpoints(
            path,
            d.caps.supports_hit_conditional_breakpoints,
            extra,
        );
        let generation = d.generation();
        let _ = d.send(
            "setBreakpoints",
            dap_session::set_breakpoints_arguments(path, &sbps),
            Pending::SetBreakpoints {
                generation,
                path: path.to_owned(),
                lines,
            },
        );
    }

    /// Evaluate in the selected (or given) frame. `Ok` carries an agent's answer to come; `Err` the immediate
    /// output (pending, for the UI).
    fn debug_evaluate(
        &mut self,
        expression: String,
        frame: Option<usize>,
        context: EvalContext,
        expand: bool,
        agent: bool,
    ) -> Result<Result<oneshot::Receiver<EvaluateOutput>, EvaluateOutput>, CommandError> {
        let d = &mut self.debug;
        let ix = frame.unwrap_or(d.model.frame);
        let frame_id = d.model.frames.get(ix).map(|f| f.id).ok_or_else(|| {
            CommandError::InvalidInput(format!(
                "there is no frame {ix} (the call stack has {})",
                d.model.frames.len()
            ))
        })?;
        let (generation, stop) = (d.generation(), d.model.stop);
        let hover = d.hover.take();
        let (target, rx) = if agent {
            let (tx, rx) = oneshot::channel();
            (
                EvalTarget::Agent {
                    reply: tx,
                    expression: expression.clone(),
                    expand,
                },
                Some(rx),
            )
        } else {
            match (context, hover) {
                (EvalContext::Hover, Some((doc, id))) => (
                    EvalTarget::Hover {
                        doc,
                        id,
                        expression: expression.clone(),
                    },
                    None,
                ),
                _ => {
                    d.console_line(format!("> {expression}"));
                    (EvalTarget::Console, None)
                }
            }
        };
        d.send(
            "evaluate",
            json!({"expression": expression, "frameId": frame_id, "context": context.as_str()}),
            Pending::Eval {
                generation,
                stop,
                target,
            },
        )?;
        Ok(match rx {
            Some(rx) => Ok(rx),
            None => Err(EvaluateOutput {
                expression,
                state: "pending".into(),
                stop,
                ..Default::default()
            }),
        })
    }

    fn debug_eval_watch(&mut self, ix: usize) {
        let d = &mut self.debug;
        if d.model.mode != Mode::Break {
            return;
        }
        let Some(frame) = d.model.frames.get(d.model.frame).map(|f| f.id) else {
            return;
        };
        let Some(w) = d.model.watches.get_mut(ix) else {
            return;
        };
        *w = VarNode::watch(&w.name);
        w.loading = true;
        let expression = w.name.clone();
        let (generation, stop) = (d.generation(), d.model.stop);
        let _ = d.send(
            "evaluate",
            json!({"expression": expression, "frameId": frame, "context": "watch"}),
            Pending::Eval {
                generation,
                stop,
                target: EvalTarget::Watch(ix),
            },
        );
    }

    /// Select a thread or a frame (Call Stack, Threads).
    fn debug_select(
        &mut self,
        thread: Option<i64>,
        frame: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        let d = &mut self.debug;
        let frame = frame.unwrap_or(0);
        if let Some(t) = thread.filter(|t| Some(*t) != d.model.thread) {
            if !d.model.threads.iter().any(|x| x.id == t) {
                return Err(CommandError::InvalidInput(format!(
                    "there is no thread {t}"
                )));
            }
            let (generation, stop) = (d.generation(), d.model.stop);
            d.send(
                "stackTrace",
                json!({"threadId": t, "startFrame": 0, "levels": STACK_LEVELS}),
                Pending::ThreadStack {
                    generation,
                    stop,
                    thread: t,
                    frame,
                },
            )?;
            return Ok(());
        }
        if frame >= d.model.frames.len() {
            return Err(CommandError::InvalidInput(format!(
                "there is no frame {frame} (the call stack has {})",
                d.model.frames.len()
            )));
        }
        self.debug_show_frame(frame, window, cx);
        Ok(())
    }

    /// Show frame `ix` of the current stack: the execution point, its locals and the watches.
    fn debug_show_frame(&mut self, ix: usize, window: &mut Window, cx: &mut Context<Self>) {
        let d = &mut self.debug;
        d.model.frame = ix;
        d.model.locals.clear();
        d.model.locals_reference = 0;
        d.model.locals_total = 0;
        let Some(f) = d.model.frames.get(ix).cloned() else {
            d.model.locals_loading = false;
            return;
        };
        d.model.locals_loading = true;
        let (generation, stop) = (d.generation(), d.model.stop);
        if d.send(
            "scopes",
            json!({ "frameId": f.id }),
            Pending::Scopes { generation, stop },
        )
        .is_err()
        {
            d.model.locals_loading = false;
        }
        for i in 0..d.model.watches.len() {
            self.debug_eval_watch(i);
        }
        let exec = match (&f.row.path, f.row.line) {
            (Some(p), Some(line)) => Some(ExecPoint {
                path: norm(p),
                line,
                column: f.row.column.unwrap_or(1),
                end: f.row.end_line.zip(f.row.end_column),
                kind: if ix == 0 {
                    ExecutionKind::Current
                } else {
                    ExecutionKind::Frame
                },
            }),
            _ => None,
        };
        self.debug.exec = exec.clone();
        if let Some(e) = exec {
            // Visual Studio brings the statement's document forward (opening it if needed) without moving the
            // caret; `apply_exec` scrolls to the statement.
            let _ = self.open_file(&e.path, None, window, cx);
        }
        self.apply_exec(cx);
    }

    /// Draw the execution point in its document, and nowhere else.
    fn apply_exec(&mut self, cx: &mut Context<Self>) {
        let exec = self.debug.exec.clone();
        for (id, doc) in &self.documents {
            let at = exec.as_ref().filter(|e| &e.path == id).map(|e| {
                let b = doc.view.read(cx).editor().buffer();
                let last = b.line_count().saturating_sub(1);
                let offset = |line: u32, col: u32| {
                    let row = line.saturating_sub(1).min(last);
                    let len = b.line(row).len() as u32;
                    b.point_to_offset(eludite_editor::text::Point::new(
                        row,
                        col.saturating_sub(1).min(len),
                    ))
                };
                let start = offset(e.line, e.column);
                let end = match e.end {
                    Some((l, c)) if (l, c) > (e.line, e.column) => offset(l, c),
                    _ => offset(e.line, u32::MAX),
                };
                (start..end, e.kind)
            });
            let current = doc.view.read(cx).execution_point();
            if at.is_some() || current.is_some() {
                doc.view
                    .update(cx, |v, cx| v.set_execution_point(at.clone(), cx));
            }
            if let Some((range, _)) = &at {
                let start = range.start;
                doc.view.update(cx, |v, cx| {
                    let row = v.editor().buffer().offset_to_point(start).row;
                    let rows = v.visible_rows();
                    if !rows.contains(&row) {
                        v.scroll_to_row(row.saturating_sub(5), cx);
                    }
                });
            }
        }
    }

    /// The margin glyphs of every open document.
    pub(super) fn refresh_glyphs(&mut self, cx: &mut Context<Self>) {
        let in_session = self.debug.client.is_some();
        for (id, doc) in &self.documents {
            let rows = self.debug.model.breakpoints.glyphs(id, in_session);
            if doc.view.read(cx).breakpoint_glyphs() != rows {
                doc.view
                    .update(cx, |v, cx| v.set_breakpoint_glyphs(rows, cx));
            }
        }
    }

    /// A document opened: its breakpoints and the execution point.
    pub(super) fn debug_document_opened(&mut self, cx: &mut Context<Self>) {
        self.refresh_glyphs(cx);
        self.apply_exec(cx);
    }

    /// A document changed: breakpoints follow their lines as text is inserted or deleted above them.
    pub(super) fn debug_sync_lines(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let rows = doc.view.read(cx).breakpoint_glyphs();
        if rows.is_empty() {
            return;
        }
        let lines: Vec<u32> = rows.iter().map(|(r, _)| r + 1).collect();
        if self.debug.model.breakpoints.moved(id, &lines) {
            self.debug_send_breakpoints(id);
            self.debug_persist(cx);
            self.refresh_debug(cx);
        }
    }

    /// The editor's debugger events: the margin toggles a breakpoint; a data tip replaces Quick Info in break mode.
    /// True when handled.
    pub(super) fn debug_on_editor_event(
        &mut self,
        id: &str,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        match event {
            EditorEvent::BreakpointMarginClicked { row } => {
                self.run(
                    cmds::TOGGLE_BREAKPOINT,
                    json!({ "path": id, "line": row + 1 }),
                    window,
                    cx,
                );
                true
            }
            EditorEvent::HoverTriggered { offset } if self.debug.model.mode == Mode::Break => {
                let Some(doc) = self.documents.get(id) else {
                    return false;
                };
                let view = doc.view.clone();
                let Some((range, expression)) = view.read(cx).expression_at(*offset) else {
                    return true;
                };
                let tip = view.update(cx, |v, cx| v.open_data_tip(range, cx));
                self.debug.hover = Some((id.to_owned(), tip));
                if let Err(e) = self.invoke(
                    cmds::EVALUATE,
                    json!({"expression": expression, "context": "hover"}),
                    window,
                    cx,
                ) {
                    self.debug.hover = None;
                    view.update(cx, |v, cx| v.set_hover(tip, Some(&e.to_string()), None, cx));
                }
                true
            }
            _ => false,
        }
    }

    /// The Locals or Watch window asks to expand or collapse a variable.
    pub(super) fn debug_toggle_variable(
        &mut self,
        watch: bool,
        path: &[usize],
        cx: &mut Context<Self>,
    ) {
        let d = &mut self.debug;
        let nodes = if watch {
            &mut d.model.watches
        } else {
            &mut d.model.locals
        };
        let Some(node) = node_mut(nodes, path) else {
            return;
        };
        if node.expanded {
            node.expanded = false;
        } else {
            node.expanded = true;
            if node.children.is_none() && node.reference > 0 && !node.loading {
                node.loading = true;
                let reference = node.reference;
                let (generation, stop) = (d.generation(), d.model.stop);
                let target = if watch {
                    VarTarget::Watch(path.to_vec())
                } else {
                    VarTarget::Locals(path.to_vec())
                };
                let _ = d.send(
                    "variables",
                    json!({ "variablesReference": reference }),
                    Pending::Vars {
                        generation,
                        stop,
                        target,
                    },
                );
            }
        }
        self.refresh_debug(cx);
    }

    /// A solution opened: load its breakpoints off the UI thread.
    pub(super) fn debug_solution_opened(&mut self, solution: &Path, cx: &mut Context<Self>) {
        if self.debug.solution.as_deref() == Some(solution) {
            return;
        }
        self.debug.solution = Some(solution.to_path_buf());
        // Another solution's startup project is not this one's (brief 0020).
        self.debug.model.startup_project = None;
        let Some(file) = self.debug.store_path(solution) else {
            return;
        };
        let tx = self.debug.tx.clone();
        let solution = solution.to_path_buf();
        cx.background_spawn(async move {
            let persisted = std::fs::read_to_string(&file)
                .ok()
                .and_then(|t| serde_json::from_str::<Persisted>(&t).ok());
            let _ = tx.unbounded_send(DebugMsg::Loaded {
                solution,
                persisted,
            });
        })
        .detach();
    }

    /// Save what persists, off the UI thread.
    pub(super) fn debug_persist(&mut self, cx: &mut Context<Self>) {
        let Some(file) = self
            .debug
            .solution
            .clone()
            .and_then(|s| self.debug.store_path(&s))
        else {
            return;
        };
        let Ok(text) = serde_json::to_string_pretty(&self.debug.model.persisted()) else {
            return;
        };
        cx.background_spawn(async move {
            if let Err(e) = eludite_docking::persist::write_atomic(&file, &text) {
                eprintln!(
                    "eludite: cannot save breakpoints to {}: {e}",
                    file.display()
                );
            }
        })
        .detach();
    }

    /// Push the model into the windows, the margin and the status bar, and wake waiting agents.
    pub(super) fn refresh_debug(&mut self, cx: &mut Context<Self>) {
        let d = &mut self.debug;
        let m = &d.model;
        let note = |what: &str| match m.mode {
            Mode::Break => None,
            Mode::Design => Some(format!("{what} are shown when debugging breaks.")),
            _ => Some(format!("{what} are shown when the debuggee breaks.")),
        };
        let locals_note = if m.mode == Mode::Break && m.locals_loading && m.locals.is_empty() {
            Some("Loading\u{2026}".to_owned())
        } else {
            note("Locals")
        };
        let locals = flatten(&m.locals);
        let watches = flatten(&m.watches);
        let frames: Vec<StackRow> = m
            .frames
            .iter()
            .map(|f| StackRow {
                name: f.row.name.clone(),
                location: match (&f.row.path, f.row.line) {
                    (Some(p), Some(l)) => format!("{}, line {l}", file_name(p)),
                    _ => String::new(),
                },
                selected: f.row.index == m.frame,
            })
            .collect();
        let top = m
            .frames
            .first()
            .map(|f| f.row.name.clone())
            .unwrap_or_default();
        let threads: Vec<ThreadLine> = m
            .threads
            .iter()
            .map(|t| ThreadLine {
                id: t.id,
                name: t.name.clone(),
                location: if Some(t.id) == m.thread {
                    top.clone()
                } else {
                    String::new()
                },
                current: Some(t.id) == m.thread,
            })
            .collect();
        let breakpoints = m.breakpoints.rows();
        let exceptions = m.exceptions;
        let w = d.windows.clone();
        w.locals
            .update(cx, |v, cx| v.set_rows(locals, locals_note, cx));
        w.watch.update(cx, |v, cx| v.set_rows(watches, None, cx));
        w.call_stack.update(cx, |v, cx| v.set_rows(frames, cx));
        w.threads.update(cx, |v, cx| v.set_rows(threads, cx));
        w.breakpoints
            .update(cx, |v, cx| v.set_rows(breakpoints, cx));
        w.exceptions.update(cx, |v, cx| v.set(exceptions, cx));
        // The program's output and the debugger's messages: the Output window's Debug source (brief 0020).
        let out = std::mem::take(&mut d.output_queue);
        let clear = std::mem::take(&mut d.output_clear);
        if clear || !out.is_empty() {
            self.output.update(cx, |o, cx| {
                if clear {
                    o.clear(OutputSource::Debug, cx);
                }
                o.append(OutputSource::Debug, &out, cx);
            });
        }
        let name = m
            .session
            .as_ref()
            .map(|s| file_name(&s.project))
            .map(|n| n.trim_end_matches(".csproj").to_owned())
            .unwrap_or_default();
        // While an agent's command drove the debuggee last, the slot says so (proposal 0001 section 7).
        let driving = if m.agent_driving() {
            ", agent driving"
        } else {
            ""
        };
        let status = match m.mode {
            Mode::Design => m.message.clone().unwrap_or_default(),
            Mode::Building => "Debugging: building before starting\u{2026}".to_owned(),
            Mode::Launching => format!("Debugging: starting {name}\u{2026}"),
            Mode::Running => format!("Debugging: {name} (running{driving})"),
            Mode::Break => {
                let at = m
                    .frames
                    .first()
                    .and_then(|f| {
                        Some(format!(
                            ", {} line {}",
                            file_name(f.row.path.as_deref()?),
                            f.row.line?
                        ))
                    })
                    .unwrap_or_default();
                format!(
                    "Debugging: {name} (break: {}{at}{driving})",
                    m.stopped.as_ref().map_or("", |s| s.reason.as_str())
                )
            }
            Mode::Stopping => format!("Debugging: stopping {name}\u{2026}"),
            Mode::RunningWithoutDebugging => format!("Running: {name} (without debugging)"),
        };
        self.status.set(DEBUG_SLOT, status);
        // Waiting agents re-check whether the state settled, and wait again if not.
        for w in self.debug.waiters.drain(..) {
            let _ = w.send(());
        }
        cx.notify();
    }

    fn end_session(&mut self, message: Option<String>, cx: &mut Context<Self>) {
        let d = &mut self.debug;
        if let Some(c) = d.client.take() {
            c.kill();
        }
        if let Some(run) = d.run.take()
            && let Some(c) = run.lock().unwrap_or_else(|e| e.into_inner()).as_mut()
        {
            let _ = c.kill();
        }
        if !d.console_partial.is_empty() {
            let p = std::mem::take(&mut d.console_partial);
            d.model.push_console(p);
        }
        d.pending.clear();
        d.run_to_cursor = None;
        d.exec = None;
        trace(format_args!("debug ended {message:?}"));
        d.model.end();
        d.model.message = message;
        self.apply_exec(cx);
        self.refresh_glyphs(cx);
    }

    /// Apply a batch of messages from the launch thread, the adapter and the program.
    pub(super) fn on_debug_msgs(
        &mut self,
        batch: Vec<DebugMsg>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for msg in batch {
            self.on_debug_msg(msg, window, cx);
        }
        self.refresh_debug(cx);
    }

    fn on_debug_msg(&mut self, msg: DebugMsg, window: &mut Window, cx: &mut Context<Self>) {
        let current = self.debug.generation();
        match msg {
            DebugMsg::Loaded {
                solution,
                persisted,
            } => {
                if self.debug.solution.as_deref() == Some(solution.as_path()) {
                    if let Some(p) = persisted {
                        self.debug.model.restore(&p);
                    }
                    self.refresh_glyphs(cx);
                    self.refresh_startup(cx);
                }
            }
            DebugMsg::Launched {
                generation,
                session,
                run,
            } if generation == current => {
                let program = file_name(&session.program);
                trace(format_args!(
                    "debug launched {} debug={} adapter={:?}",
                    session.program, session.debug, session.adapter
                ));
                self.debug.console_line(format!(
                    "{} {program} {}",
                    if session.debug {
                        "Debugging"
                    } else {
                        "Running"
                    },
                    session.args.join(" ")
                ));
                self.debug.model.session = Some(session);
                if let Some(run) = run {
                    self.debug.run = Some(run.clone());
                    if self.debug.model.mode == Mode::Stopping {
                        if let Some(c) = run.lock().unwrap_or_else(|e| e.into_inner()).as_mut() {
                            let _ = c.kill();
                        }
                    } else {
                        self.debug.model.mode = Mode::RunningWithoutDebugging;
                    }
                }
            }
            DebugMsg::Connected { generation, client } => {
                if generation != current || self.debug.model.mode == Mode::Stopping {
                    client.kill();
                    return;
                }
                self.debug.client = Some(client);
            }
            DebugMsg::Started {
                generation,
                result,
                adapter_id,
            } if generation == current => match result {
                Ok(started) => {
                    self.debug.caps = started.capabilities.clone();
                    self.debug.model.capabilities =
                        Some(capabilities_row(&started.capabilities, &adapter_id));
                    for (path, answer) in &started.breakpoints {
                        let (lines, _) = self
                            .debug
                            .model
                            .breakpoints
                            .source_breakpoints(path, false, None);
                        self.debug
                            .model
                            .breakpoints
                            .apply_answer(path, &lines, answer);
                    }
                    for b in std::mem::take(&mut self.debug.early_breakpoints) {
                        self.debug.model.breakpoints.apply_event(&b);
                    }
                    if self.debug.model.mode == Mode::Launching {
                        self.debug.model.mode = Mode::Running;
                    }
                    trace(format_args!("debug running generation {current}"));
                    if self.debug.caps.supports_hit_conditional_breakpoints {
                        for f in self.debug.model.breakpoints.files() {
                            self.debug_send_breakpoints(&f);
                        }
                    }
                    self.refresh_glyphs(cx);
                }
                Err(e) => {
                    if self.debug.model.mode != Mode::Design {
                        self.debug
                            .console_line(format!("Cannot start debugging: {e}"));
                        self.end_session(Some(format!("Cannot start debugging: {e}")), cx);
                    }
                }
            },
            DebugMsg::LaunchFailed {
                generation,
                message,
            } if generation == current => {
                self.debug.console_line(message.clone());
                self.end_session(Some(message), cx);
            }
            DebugMsg::Output {
                generation,
                text,
                stream,
            } if generation == current => {
                self.debug.program_output(&text, stream);
            }
            DebugMsg::ProgramExited { generation, code } if generation == current => {
                self.debug.model.exit_code = code.map(i64::from);
                self.debug.console_line(format!(
                    "The program has exited with code {}.",
                    code.map_or("unknown".to_owned(), |c| c.to_string())
                ));
                self.end_session(None, cx);
            }
            DebugMsg::StopTimeout { generation }
                if generation == current && self.debug.model.mode == Mode::Stopping =>
            {
                self.debug
                    .console_line("The debug adapter did not end the session; it was stopped.");
                self.end_session(None, cx);
            }
            DebugMsg::Client { generation, event } if generation == current => {
                self.on_client_event(event, window, cx)
            }
            _ => {}
        }
    }

    fn on_client_event(&mut self, event: ClientEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            ClientEvent::Event(e) => self.on_dap_event(e, cx),
            ClientEvent::Stderr(line) => {
                self.debug
                    .model
                    .output_mut(OutputKind::Adapter)
                    .push_line(line, Some("stderr"));
            }
            ClientEvent::Response {
                request_seq,
                result,
                command,
            } => {
                if let Some(p) = self.debug.pending.remove(&request_seq) {
                    self.on_response(p, &command, result, window, cx);
                }
            }
            ClientEvent::Closed {
                exit_code,
                terminated,
                stderr_tail,
            } => {
                if self.debug.model.mode == Mode::Design {
                    return;
                }
                let message = if terminated || self.debug.model.mode == Mode::Stopping {
                    None
                } else {
                    for l in stderr_tail.iter().rev().take(5).rev() {
                        self.debug.console_line(format!("netcoredbg: {l}"));
                    }
                    let m = format!(
                        "The debug adapter exited unexpectedly{}.",
                        exit_code
                            .map(|c| format!(" (exit code {c})"))
                            .unwrap_or_default()
                    );
                    self.debug.console_line(m.clone());
                    Some(m)
                };
                self.end_session(message, cx);
            }
        }
    }

    fn on_dap_event(&mut self, event: Event, cx: &mut Context<Self>) {
        let d = &mut self.debug;
        match event {
            Event::Output(o) => match o.category.as_deref() {
                Some("telemetry") => {}
                // The program's own output.
                Some(stream @ ("stdout" | "stderr")) => {
                    let stream = if stream == "stdout" {
                        "stdout"
                    } else {
                        "stderr"
                    };
                    d.program_output(&o.output, stream);
                }
                // `console` (DAP's default), `important` and the rest: the adapter's messages.
                _ => {
                    d.model
                        .output_mut(OutputKind::Adapter)
                        .push_text(&o.output, None);
                    d.console(&o.output);
                }
            },
            Event::Process(p) => {
                trace(format_args!("debug process {:?}", p.system_process_id));
                if let Some(s) = d.model.session.as_mut() {
                    s.process_id = p.system_process_id;
                }
            }
            Event::Breakpoint(b) => {
                trace(format_args!(
                    "debug breakpoint {} verified={} line={:?}",
                    b.reason, b.breakpoint.verified, b.breakpoint.line
                ));
                if d.model.breakpoints.apply_event(&b.breakpoint) {
                    self.refresh_glyphs(cx);
                } else {
                    d.early_breakpoints.push(b.breakpoint);
                }
            }
            Event::Capabilities(c) => {
                if !c.exception_breakpoint_filters.is_empty() {
                    d.caps.exception_breakpoint_filters = c.exception_breakpoint_filters;
                }
                // A change after the handshake: what the client merged.
                if let (Some(row), Some(client)) = (d.model.capabilities.as_mut(), &d.client) {
                    let adapter = std::mem::take(&mut row.adapter);
                    let merged = client.capabilities();
                    d.caps.supports_delayed_stack_trace_loading =
                        merged.supports_delayed_stack_trace_loading;
                    *row = capabilities_row(&merged, &adapter);
                }
            }
            Event::Exited(e) => {
                d.model.exit_code = Some(e.exit_code);
                let pid = d
                    .model
                    .session
                    .as_ref()
                    .and_then(|s| s.process_id)
                    .map(|p| format!("[{p}] "))
                    .unwrap_or_default();
                let program = d
                    .model
                    .session
                    .as_ref()
                    .map(|s| file_name(&s.program))
                    .unwrap_or_default();
                d.console_line(format!(
                    "The program '{pid}{program}' has exited with code {} (0x{:x}).",
                    e.exit_code, e.exit_code
                ));
            }
            Event::Terminated => {
                if d.model.mode != Mode::Stopping {
                    let _ = d.send("disconnect", json!({}), Pending::Other);
                }
                d.model.mode = Mode::Stopping;
            }
            Event::Stopped(s) => {
                if !matches!(d.model.mode, Mode::Running | Mode::Launching | Mode::Break) {
                    return;
                }
                let thread = s
                    .thread_id
                    .or(d.model.thread)
                    .or_else(|| d.model.threads.first().map(|t| t.id))
                    .unwrap_or(0);
                let generation = d.generation();
                let _ = d.send("threads", Value::Null, Pending::Threads { generation });
                let _ = d.send(
                    "stackTrace",
                    json!({"threadId": thread, "startFrame": 0, "levels": STACK_LEVELS}),
                    Pending::StopStack {
                        generation,
                        stopped: s,
                        thread,
                    },
                );
            }
            Event::Continued(_) | Event::Thread(_) | Event::Initialized | Event::Other { .. } => {}
        }
    }

    fn on_response(
        &mut self,
        pending: Pending,
        command: &str,
        result: Result<Value, String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let current = self.debug.generation();
        let stop_now = self.debug.model.stop;
        match pending {
            Pending::Threads { generation } if generation == current => {
                if let Ok(b) = result {
                    let t: ThreadsResponse = serde_json::from_value(b).unwrap_or_default();
                    self.debug.model.threads = t
                        .threads
                        .into_iter()
                        .map(|t| ThreadRow {
                            id: t.id,
                            name: t.name,
                        })
                        .collect();
                }
            }
            Pending::StopStack {
                generation,
                stopped,
                thread,
            } if generation == current => {
                let st: StackTraceResponse = result
                    .ok()
                    .and_then(|b| serde_json::from_value(b).ok())
                    .unwrap_or_default();
                self.on_stop(stopped, thread, st, window, cx);
            }
            Pending::ThreadStack {
                generation,
                stop,
                thread,
                frame,
            } if generation == current && stop == stop_now => {
                if let Ok(b) = result {
                    let st: StackTraceResponse = serde_json::from_value(b).unwrap_or_default();
                    self.debug.model.frames_total = st
                        .total_frames
                        .and_then(|t| usize::try_from(t).ok())
                        .unwrap_or(0)
                        .max(st.stack_frames.len());
                    self.debug.model.thread = Some(thread);
                    self.debug.model.frames = st
                        .stack_frames
                        .iter()
                        .enumerate()
                        .map(|(i, f)| Frame::from_dap(i, f))
                        .collect();
                    let ix = frame.min(self.debug.model.frames.len().saturating_sub(1));
                    self.debug_show_frame(ix, window, cx);
                }
            }
            Pending::Scopes { generation, stop } if generation == current && stop == stop_now => {
                let scopes: ScopesResponse = result
                    .ok()
                    .and_then(|b| serde_json::from_value(b).ok())
                    .unwrap_or_default();
                let scope = scopes.scopes.iter().find(|s| !s.expensive);
                match scope {
                    Some(s) => {
                        self.debug.model.locals_reference = s.variables_reference;
                        let _ = self.debug.send(
                            "variables",
                            json!({ "variablesReference": s.variables_reference }),
                            Pending::Vars {
                                generation,
                                stop,
                                target: VarTarget::Locals(Vec::new()),
                            },
                        );
                    }
                    None => self.locals_done(Vec::new()),
                }
            }
            Pending::Vars {
                generation,
                stop,
                target,
            } if generation == current && stop == stop_now => {
                let all = match &result {
                    Ok(b) => {
                        serde_json::from_value::<VariablesResponse>(b.clone())
                            .unwrap_or_default()
                            .variables
                    }
                    Err(_) => Vec::new(),
                };
                let count = all.len();
                let vars: Vec<VarNode> = all
                    .iter()
                    .take(state::MAX_VARIABLES)
                    .map(VarNode::from_dap)
                    .collect();
                for v in &vars {
                    if v.reference > 0 && (v.indexed.is_some() || v.named.is_some()) {
                        let n = v.indexed.unwrap_or(0) + v.named.unwrap_or(0);
                        self.debug
                            .counts
                            .insert(v.reference, usize::try_from(n).unwrap_or(0));
                    }
                }
                match target {
                    VarTarget::Locals(path) if path.is_empty() => {
                        self.debug.model.locals_total = count;
                        self.locals_done(vars)
                    }
                    VarTarget::Locals(path) => {
                        set_children(&mut self.debug.model.locals, &path, vars)
                    }
                    VarTarget::Watch(path) => {
                        set_children(&mut self.debug.model.watches, &path, vars)
                    }
                    VarTarget::Agent { reply, mut out } => {
                        out.children = Some(
                            vars.iter()
                                .map(|v| VariableRow {
                                    name: v.name.clone(),
                                    value: v.value.clone(),
                                    type_name: v.type_name.clone(),
                                    reference: v.reference,
                                    evaluate_name: v.evaluate_name.clone(),
                                })
                                .collect(),
                        );
                        let _ = reply.send(out);
                    }
                }
            }
            Pending::Eval {
                generation,
                stop,
                target,
            } if generation == current && stop == stop_now => {
                self.on_eval(stop, target, result, cx);
            }
            Pending::ExceptionInfo { generation, stop }
                if generation == current && stop == stop_now =>
            {
                self.debug.model.exception_loading = false;
                if let Ok(b) = result {
                    let e: ExceptionInfoResponse = serde_json::from_value(b).unwrap_or_default();
                    self.debug.model.exception_info = Some(e.clone());
                    if let Some(s) = self.debug.model.stopped.as_mut() {
                        s.exception = Some(cmds::ExceptionRow {
                            id: Some(e.exception_id.clone()).filter(|x| !x.is_empty()),
                            description: e.description.clone(),
                            break_mode: Some(e.break_mode.clone()).filter(|x| !x.is_empty()),
                        });
                    }
                    self.debug.console_line(format!(
                        "Exception thrown: '{}'{}",
                        e.exception_id,
                        e.description.map(|d| format!(": {d}")).unwrap_or_default()
                    ));
                }
            }
            Pending::SetBreakpoints {
                generation,
                path,
                lines,
            } if generation == current => {
                if let Ok(b) = result {
                    let r: SetBreakpointsResponse = serde_json::from_value(b).unwrap_or_default();
                    self.debug
                        .model
                        .breakpoints
                        .apply_answer(&path, &lines, &r.breakpoints);
                    self.refresh_glyphs(cx);
                }
            }
            Pending::Pause { generation } if generation == current => {
                if let Err(e) = result {
                    // The adapter refused: the debuggee runs on.
                    trace(format_args!("debug pause failed: {e}"));
                    self.debug.model.message = Some(format!("Break All: {e}"));
                    self.debug.pause_error = Some((generation, e));
                }
            }
            Pending::Agent {
                generation,
                stop,
                reply,
            } => {
                let fresh = generation == current
                    && stop == stop_now
                    && self.debug.model.mode == Mode::Break;
                let _ = reply.send(if fresh {
                    result
                } else {
                    Err(format!(
                        "stale: the debuggee moved on (now stop {stop_now}, generation {current}) before the \
                         answer arrived; read it again"
                    ))
                });
            }
            Pending::Resume { generation, stop } if generation == current => {
                if let Err(e) = result {
                    // The adapter refused: the debuggee did not move.
                    if self.debug.model.mode == Mode::Running && self.debug.model.stop == stop {
                        self.debug.model.message = Some(format!("{command}: {e}"));
                        self.debug.model.mode = Mode::Break;
                    }
                }
            }
            // An answer for an older session or stop: dropped (CLAUDE.md invariant 12). An agent waiting on it hears
            // that it is stale.
            Pending::Eval {
                target:
                    EvalTarget::Agent {
                        reply, expression, ..
                    },
                stop,
                ..
            } => {
                let _ = reply.send(eval_failed(
                    &expression,
                    stop,
                    "stale: the debuggee moved on before the answer arrived",
                ));
            }
            _ => {}
        }
    }

    /// The stack of a stop arrived: count the breakpoint's hit, resume at once if its hit condition is not met, else
    /// show the break.
    fn on_stop(
        &mut self,
        s: StoppedEvent,
        thread: i64,
        st: StackTraceResponse,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let frames_total = st
            .total_frames
            .and_then(|t| usize::try_from(t).ok())
            .unwrap_or(st.stack_frames.len())
            .max(st.stack_frames.len());
        let d = &mut self.debug;
        if !matches!(d.model.mode, Mode::Running | Mode::Launching | Mode::Break) {
            return;
        }
        let frames: Vec<Frame> = st
            .stack_frames
            .iter()
            .enumerate()
            .map(|(i, f)| Frame::from_dap(i, f))
            .collect();
        let top = frames
            .first()
            .and_then(|f| Some((norm(f.row.path.as_deref()?), f.row.line?)));
        // Run To Cursor's one-shot breakpoint ends at the next break, whatever it is.
        let cursor = d.run_to_cursor.take();
        if s.reason == "breakpoint"
            && let Some((path, line)) = &top
        {
            let at_cursor = cursor
                .as_ref()
                .is_some_and(|c| (&c.0, c.1) == (path, *line));
            if let Some(bp) = d.model.breakpoints.at_mut(path, *line) {
                bp.hits += 1;
                let hits = bp.hits;
                let skip = !d.caps.supports_hit_conditional_breakpoints
                    && !at_cursor
                    && bp.hit_condition.is_some_and(|h| !h.breaks_on(hits));
                if skip {
                    d.run_to_cursor = cursor;
                    let (generation, stop) = (d.generation(), d.model.stop);
                    let _ = d.send(
                        "continue",
                        json!({ "threadId": thread }),
                        Pending::Resume { generation, stop },
                    );
                    return;
                }
            }
        }
        if let Some((path, _)) = cursor {
            self.debug_send_breakpoints(&path);
        }
        let d = &mut self.debug;
        d.model.mode = Mode::Break;
        d.model.stop += 1;
        d.model.message = None;
        d.model.stopped = Some(StoppedRow {
            reason: s.reason.clone(),
            thread,
            description: s.description.clone().or(s.text.clone()),
            exception: None,
            driver: d.model.last_driver.clone(),
        });
        d.model.thread = Some(thread);
        d.model.frames = frames;
        d.model.frames_total = frames_total;
        d.counts.clear();
        trace(format_args!(
            "debug break stop {} {} at {:?}",
            d.model.stop,
            s.reason,
            top.as_ref().map(|(p, l)| format!("{}:{l}", file_name(p)))
        ));
        if s.reason == "exception" && d.caps.supports_exception_info_request {
            let (generation, stop) = (d.generation(), d.model.stop);
            d.model.exception_loading = d
                .send(
                    "exceptionInfo",
                    json!({ "threadId": thread }),
                    Pending::ExceptionInfo { generation, stop },
                )
                .is_ok();
        }
        self.debug_show_frame(0, window, cx);
    }

    /// The selected frame's locals arrived.
    fn locals_done(&mut self, vars: Vec<VarNode>) {
        let d = &mut self.debug;
        trace(format_args!(
            "debug locals stop {} {}",
            d.model.stop,
            vars.iter()
                .map(|v| format!("{}={}", v.name, v.value))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        d.model.locals = vars;
        d.model.locals_loading = false;
        let now = Instant::now();
        d.timings.locals_shown = Some(now);
        d.timings.first_break.get_or_insert(now);
        if let Some(t) = d.timings.step_sent.take() {
            d.timings.steps.push(now - t);
        }
    }

    fn on_eval(
        &mut self,
        stop: u64,
        target: EvalTarget,
        result: Result<Value, String>,
        cx: &mut Context<Self>,
    ) {
        let r: Result<EvaluateResponse, String> =
            result.and_then(|b| serde_json::from_value(b).map_err(|e| e.to_string()));
        match target {
            EvalTarget::Watch(ix) => {
                if let Some(w) = self.debug.model.watches.get_mut(ix) {
                    match r {
                        Ok(e) => {
                            w.value = e.result;
                            w.type_name = e.type_name.filter(|t| !t.is_empty());
                            w.reference = e.variables_reference;
                            w.error = false;
                        }
                        Err(m) => {
                            w.value = m;
                            w.reference = 0;
                            w.error = true;
                        }
                    }
                    w.loading = false;
                    w.children = None;
                    w.expanded = false;
                }
            }
            EvalTarget::Console => {
                let line = match r {
                    Ok(e) => e.result,
                    Err(m) => m,
                };
                self.debug.console_line(line);
            }
            EvalTarget::Hover {
                doc,
                id,
                expression,
            } => {
                if let Some(d) = self.documents.get(&doc) {
                    let text = match r {
                        Ok(e) => format!(
                            "{expression} = {}{}",
                            e.result,
                            e.type_name
                                .filter(|t| !t.is_empty())
                                .map(|t| format!("  ({t})"))
                                .unwrap_or_default()
                        ),
                        Err(m) => m,
                    };
                    d.view
                        .update(cx, |v, cx| v.set_hover(id, Some(&text), None, cx));
                }
            }
            EvalTarget::Agent {
                reply,
                expression,
                expand,
            } => match r {
                Ok(e) => {
                    let out = EvaluateOutput {
                        expression: expression.clone(),
                        state: "done".into(),
                        result: Some(e.result),
                        type_name: e.type_name.filter(|t| !t.is_empty()),
                        reference: Some(e.variables_reference),
                        children: None,
                        message: None,
                        stop,
                    };
                    if expand && e.variables_reference > 0 {
                        let generation = self.debug.generation();
                        let pending = Pending::Vars {
                            generation,
                            stop,
                            target: VarTarget::Agent {
                                reply,
                                out: out.clone(),
                            },
                        };
                        if let Some(client) = self.debug.client.clone()
                            && let Ok(seq) = client.request(
                                "variables",
                                json!({ "variablesReference": e.variables_reference }),
                            )
                        {
                            self.debug.pending.insert(seq, pending);
                        }
                    } else {
                        let _ = reply.send(out);
                    }
                }
                Err(m) => {
                    let _ = reply.send(eval_failed(&expression, stop, m));
                }
            },
        }
    }
}

/// A timer in real time for agents' waits: one thread for every pending deadline. (The executor's timers follow the
/// test executor's simulated clock in headless tests, which a `wait` timing out has to see pass.)
fn real_timer(after: Duration) -> oneshot::Receiver<()> {
    use std::sync::{Condvar, OnceLock};
    type Queue = (Mutex<Vec<(Instant, oneshot::Sender<()>)>>, Condvar);
    static QUEUE: OnceLock<Arc<Queue>> = OnceLock::new();
    let queue = QUEUE.get_or_init(|| {
        let q: Arc<Queue> = Arc::default();
        let worker = q.clone();
        std::thread::Builder::new()
            .name("debug-agent-timer".into())
            .spawn(move || {
                let (m, cv) = &*worker;
                let mut due = m.lock().unwrap_or_else(|e| e.into_inner());
                loop {
                    let now = Instant::now();
                    let (fire, keep): (Vec<_>, Vec<_>) = std::mem::take(&mut *due)
                        .into_iter()
                        .partition(|(at, _)| *at <= now);
                    *due = keep;
                    for (_, tx) in fire {
                        let _ = tx.send(());
                    }
                    let next = due.iter().map(|(at, _)| *at).min();
                    due = match next {
                        Some(at) => {
                            cv.wait_timeout(due, at.saturating_duration_since(now))
                                .unwrap_or_else(|e| e.into_inner())
                                .0
                        }
                        None => cv.wait(due).unwrap_or_else(|e| e.into_inner()),
                    };
                }
            })
            .expect("spawn debug-agent-timer");
        q
    });
    let (tx, rx) = oneshot::channel();
    let (m, cv) = &**queue;
    m.lock()
        .unwrap_or_else(|e| e.into_inner())
        .push((Instant::now() + after, tx));
    cv.notify_one();
    rx
}

// ----- Agents' reads (brief 0025): run off the UI thread, reading the adapter through tagged requests. -----

/// An agent's read of one stop: the requests it sends carry the session generation and the stop, and an answer for an
/// older one is a stale error (rule 4 of brief 0018).
struct Reader {
    this: WeakEntity<Shell>,
    generation: u64,
    stop: u64,
}

const WINDOW_CLOSED: &str = "the window is closed";

impl Reader {
    fn new(this: &WeakEntity<Shell>, cx: &mut AsyncWindowContext) -> Result<Self, String> {
        let (generation, stop) = this
            .update(cx, |s, _| (s.debug.model.generation, s.debug.model.stop))
            .map_err(|_| WINDOW_CLOSED.to_owned())?;
        Ok(Self {
            this: this.clone(),
            generation,
            stop,
        })
    }

    fn model<R>(
        &self,
        cx: &mut AsyncWindowContext,
        f: impl FnOnce(&Debugger) -> R,
    ) -> Result<R, String> {
        self.this
            .update(cx, |s, _| f(&s.debug))
            .map_err(|_| WINDOW_CLOSED.to_owned())
    }

    /// Send `requests` together and wait for every answer (at most [`AGENT_WAIT`]); the UI thread only sends them.
    async fn dap(
        &self,
        cx: &mut AsyncWindowContext,
        requests: Vec<(&'static str, Value)>,
    ) -> Result<Vec<Result<Value, String>>, String> {
        let (g, s) = (self.generation, self.stop);
        let receivers = self
            .this
            .update(cx, |shell, _| {
                let t = Instant::now();
                let r: Result<Vec<_>, String> = requests
                    .into_iter()
                    .map(|(c, a)| shell.debug.agent_request(g, s, c, a))
                    .collect();
                shell.debug.timings.agent_ui.push((t, t.elapsed()));
                r
            })
            .map_err(|_| WINDOW_CLOSED.to_owned())??;
        let timer = real_timer(AGENT_WAIT);
        match futures::future::select(futures::future::join_all(receivers), timer).await {
            futures::future::Either::Left((answers, _)) => Ok(answers
                .into_iter()
                .map(|a| {
                    a.unwrap_or_else(|_| Err("the session ended before the answer arrived".into()))
                })
                .collect()),
            futures::future::Either::Right(_) => {
                Err("the debug adapter did not answer in time".into())
            }
        }
    }

    async fn one(
        &self,
        cx: &mut AsyncWindowContext,
        command: &'static str,
        args: Value,
    ) -> Result<Value, String> {
        self.dap(cx, vec![(command, args)])
            .await?
            .pop()
            .expect("one answer")
    }

    /// Frames `start..start + count` of `thread`: (the adapter's frame id and the row, the stack's total when known).
    /// From what the Call Stack window has when that is enough; else `stackTrace` with `startFrame` and `levels` when
    /// the adapter loads stacks lazily, or the whole stack paged here.
    async fn frames(
        &self,
        cx: &mut AsyncWindowContext,
        thread: i64,
        start: usize,
        count: usize,
    ) -> Result<(Vec<(i64, StackFrameRow)>, Option<usize>), String> {
        let (cached, delayed) = self.model(cx, |d| {
            let m = &d.model;
            let complete = m.frames.len() >= m.frames_total || start + count <= m.frames.len();
            let cached =
                (m.thread == Some(thread) && !m.frames.is_empty() && complete).then(|| {
                    let rows = m
                        .frames
                        .iter()
                        .skip(start)
                        .take(count)
                        .map(|f| (f.id, f.stack_row()))
                        .collect::<Vec<_>>();
                    (rows, m.frames_total.max(m.frames.len()))
                });
            (cached, d.caps.supports_delayed_stack_trace_loading)
        })?;
        if let Some((rows, total)) = cached {
            return Ok((rows, Some(total)));
        }
        let args = if delayed {
            json!({"threadId": thread, "startFrame": start, "levels": count})
        } else {
            json!({ "threadId": thread })
        };
        let st: StackTraceResponse =
            serde_json::from_value(self.one(cx, "stackTrace", args).await?)
                .map_err(|e| format!("stackTrace: {e}"))?;
        let total = st.total_frames.and_then(|t| usize::try_from(t).ok());
        let skip = if delayed { 0 } else { start };
        let rows: Vec<(i64, StackFrameRow)> = st
            .stack_frames
            .iter()
            .skip(skip)
            .take(count)
            .enumerate()
            .map(|(i, f)| (f.id, Frame::from_dap(start + i, f).stack_row()))
            .collect();
        let total = if delayed {
            total
        } else {
            Some(total.unwrap_or(st.stack_frames.len()))
        };
        Ok((rows, total))
    }

    /// Whether the adapter pages `variables` by `start` and `count`.
    fn paging(&self, cx: &mut AsyncWindowContext) -> Result<bool, String> {
        self.model(cx, |d| {
            d.model
                .capabilities
                .as_ref()
                .is_some_and(|c| c.variable_paging)
        })
    }

    /// Every member of `reference`.
    async fn all_vars(
        &self,
        cx: &mut AsyncWindowContext,
        reference: i64,
    ) -> Result<Vec<Variable>, String> {
        let r: VariablesResponse = serde_json::from_value(
            self.one(cx, "variables", json!({ "variablesReference": reference }))
                .await?,
        )
        .map_err(|e| format!("variables: {e}"))?;
        Ok(r.variables)
    }

    /// A page of `reference`'s members: (rows, total, more follow). Through the adapter's paging when it has it
    /// (asking one row more to learn whether more follow when it gave no count), else all of them paged here.
    async fn page(
        &self,
        cx: &mut AsyncWindowContext,
        reference: i64,
        start: usize,
        count: usize,
        known: Option<usize>,
    ) -> Result<(Vec<Variable>, usize, bool), String> {
        let known = match known {
            Some(k) => Some(k),
            None => self.model(cx, |d| d.counts.get(&reference).copied())?,
        };
        if !self.paging(cx)? {
            let all = self.all_vars(cx, reference).await?;
            let total = all.len();
            let rows: Vec<Variable> = all.into_iter().skip(start).take(count).collect();
            let more = start + rows.len() < total;
            return Ok((rows, total, more));
        }
        let ask = if known.is_some() { count } else { count + 1 };
        let r: VariablesResponse = serde_json::from_value(
            self.one(
                cx,
                "variables",
                json!({"variablesReference": reference, "start": start, "count": ask}),
            )
            .await?,
        )
        .map_err(|e| format!("variables: {e}"))?;
        let mut rows = r.variables;
        let more = rows.len() > count || known.is_some_and(|k| start + count < k);
        rows.truncate(count);
        let total = known.unwrap_or(start + rows.len() + usize::from(more));
        Ok((rows, total, more))
    }

    /// The top-level rows of a frame's `scope` from `start` (at most `count`, those named with the prefix `filter`):
    /// (rows, total, more follow). The Locals window's rows serve when they are that frame's.
    #[allow(clippy::too_many_arguments)]
    async fn frame_rows(
        &self,
        cx: &mut AsyncWindowContext,
        thread: i64,
        frame: usize,
        scope: ScopeKind,
        start: usize,
        count: usize,
        filter: Option<&str>,
        max_chars: usize,
    ) -> Result<(Vec<VarRow>, usize, bool), String> {
        let plain = scope == ScopeKind::Locals && filter.is_none();
        let cached = self.model(cx, |d| {
            let m = &d.model;
            let shown = m.mode == Mode::Break
                && m.thread == Some(thread)
                && m.frame == frame
                && !m.locals_loading
                && m.locals_reference > 0;
            let complete = m.locals.len() >= m.locals_total || start + count <= m.locals.len();
            (plain && shown && complete).then(|| {
                let total = m.locals_total.max(m.locals.len());
                let rows: Vec<VarRow> = m
                    .locals
                    .iter()
                    .skip(start)
                    .take(count)
                    .map(|v| v.var_row(max_chars))
                    .collect();
                let more = start + rows.len() < total;
                (rows, total, more)
            })
        })?;
        if let Some(c) = cached {
            return Ok(c);
        }
        let (ids, total) = self.frames(cx, thread, frame, 1).await?;
        let Some((frame_id, _)) = ids.first().cloned() else {
            return Err(format!(
                "there is no frame {frame} (the call stack of thread {thread} has {})",
                total.map_or("fewer".to_owned(), |t| t.to_string())
            ));
        };
        let scopes: ScopesResponse = serde_json::from_value(
            self.one(cx, "scopes", json!({ "frameId": frame_id }))
                .await?,
        )
        .map_err(|e| format!("scopes: {e}"))?;
        let named = |s: &eludite_dap::types::Scope, hint: &str, names: &[&str]| {
            s.presentation_hint.as_deref() == Some(hint)
                || names.iter().any(|n| s.name.eq_ignore_ascii_case(n))
        };
        let locals = scopes
            .scopes
            .iter()
            .find(|s| named(s, "locals", &["locals"]))
            .or_else(|| scopes.scopes.iter().find(|s| !s.expensive))
            .or(scopes.scopes.first())
            .cloned();
        let own = match scope {
            ScopeKind::Locals => locals.clone(),
            ScopeKind::Arguments => scopes
                .scopes
                .iter()
                .find(|s| named(s, "arguments", &["arguments", "parameters"]))
                .cloned(),
            ScopeKind::This => scopes
                .scopes
                .iter()
                .find(|s| s.name.eq_ignore_ascii_case("this"))
                .cloned(),
        };
        let count_of =
            |s: &eludite_dap::types::Scope| match (s.named_variables, s.indexed_variables) {
                (None, None) => None,
                (n, i) => usize::try_from(n.unwrap_or(0) + i.unwrap_or(0)).ok(),
            };
        if let Some(s) = own.as_ref().filter(|_| filter.is_none()) {
            let (rows, total, more) = self
                .page(cx, s.variables_reference, start, count, count_of(s))
                .await?;
            self.record(cx, &rows)?;
            return Ok((
                rows.iter().map(|v| dap_row(v, max_chars)).collect(),
                total,
                more,
            ));
        }
        // Filtered: read the whole scope (or the locals, for a scope the adapter does not have) and page here.
        let Some(source) = own.or(locals) else {
            return Ok((Vec::new(), 0, false));
        };
        let mut all = self.all_vars(cx, source.variables_reference).await?;
        if !scopes.scopes.iter().any(|s| {
            matches!(scope, ScopeKind::Arguments)
                && named(s, "arguments", &["arguments", "parameters"])
                || matches!(scope, ScopeKind::This) && s.name.eq_ignore_ascii_case("this")
                || matches!(scope, ScopeKind::Locals)
        }) {
            match scope {
                ScopeKind::Arguments => {
                    let hinted = all.iter().any(|v| v.presentation_hint.is_some());
                    if !hinted {
                        let adapter = self.model(cx, |d| {
                            d.model
                                .capabilities
                                .as_ref()
                                .map(|c| c.adapter.clone())
                                .unwrap_or_default()
                        })?;
                        return Err(format!(
                            "the debug adapter (`{adapter}`) keeps arguments and locals in one scope and does not \
                             mark which is which: read `scope: \"locals\"` (arguments come first)"
                        ));
                    }
                    all.retain(|v| {
                        v.presentation_hint
                            .as_ref()
                            .is_some_and(|h| h.kind.as_deref() == Some("parameter"))
                    });
                }
                ScopeKind::This => all.retain(|v| v.name == "this"),
                ScopeKind::Locals => {}
            }
        }
        if let Some(f) = filter {
            let f = f.to_lowercase();
            all.retain(|v| v.name.to_lowercase().starts_with(&f));
        }
        self.record(cx, &all)?;
        let total = all.len();
        let rows: Vec<VarRow> = all
            .iter()
            .skip(start)
            .take(count)
            .map(|v| dap_row(v, max_chars))
            .collect();
        let more = start + rows.len() < total;
        Ok((rows, total, more))
    }

    /// Remember the member counts the adapter gave for these rows (the `total` of a later read by reference).
    fn record(&self, cx: &mut AsyncWindowContext, rows: &[Variable]) -> Result<(), String> {
        let counts: Vec<(i64, usize)> = rows
            .iter()
            .filter(|v| v.variables_reference > 0)
            .filter_map(|v| {
                let n = v.indexed_variables.unwrap_or(0) + v.named_variables.unwrap_or(0);
                (v.indexed_variables.is_some() || v.named_variables.is_some())
                    .then(|| (v.variables_reference, usize::try_from(n).unwrap_or(0)))
            })
            .collect();
        let stop = self.stop;
        self.this
            .update(cx, |s, _| {
                if s.debug.model.stop == stop {
                    s.debug.counts.extend(counts);
                }
            })
            .map_err(|_| WINDOW_CLOSED.to_owned())
    }

    /// Expand `rows` breadth-first to `depth` levels within `left` more rows (counted down); `cut` is set when the
    /// budget left members out. Each level's members are asked for together.
    async fn expand(
        &self,
        cx: &mut AsyncWindowContext,
        rows: &mut [VarRow],
        depth: usize,
        left: &mut usize,
        max_chars: usize,
        cut: &mut bool,
    ) -> Result<(), String> {
        let mut frontier: Vec<Vec<usize>> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.reference > 0)
            .map(|(i, _)| vec![i])
            .collect();
        let paging = self.paging(cx)?;
        for _ in 1..depth {
            if frontier.is_empty() {
                break;
            }
            if *left == 0 {
                for p in &frontier {
                    if let Some(r) = row_at_mut(rows, p) {
                        r.truncated = true;
                    }
                }
                *cut = true;
                break;
            }
            let requests: Vec<(&'static str, Value)> = frontier
                .iter()
                .map(|p| {
                    let reference = row_at_mut(rows, p).map_or(0, |r| r.reference);
                    let args = if paging {
                        json!({"variablesReference": reference, "start": 0, "count": *left + 1})
                    } else {
                        json!({ "variablesReference": reference })
                    };
                    ("variables", args)
                })
                .collect();
            let answers = self.dap(cx, requests).await?;
            let mut next = Vec::new();
            for (path, answer) in frontier.iter().zip(answers) {
                let Ok(body) = answer else { continue };
                let vars = serde_json::from_value::<VariablesResponse>(body)
                    .unwrap_or_default()
                    .variables;
                self.record(cx, &vars)?;
                let Some(row) = row_at_mut(rows, path) else {
                    continue;
                };
                let n = vars.len().min(*left);
                *left -= n;
                if n < vars.len() {
                    row.truncated = true;
                    *cut = true;
                }
                if n == 0 && !vars.is_empty() {
                    continue;
                }
                row.children = Some(vars[..n].iter().map(|v| dap_row(v, max_chars)).collect());
                for (i, v) in vars[..n].iter().enumerate() {
                    if v.variables_reference > 0 {
                        let mut p = path.clone();
                        p.push(i);
                        next.push(p);
                    }
                }
            }
            frontier = next;
        }
        Ok(())
    }

    /// The stop summary for `thread` (default: the one that stopped) and `frame` (default 0), through the adapter
    /// where the model does not have it.
    async fn summary(
        &self,
        cx: &mut AsyncWindowContext,
        thread: Option<i64>,
        frame: Option<usize>,
        budget: &Budget,
    ) -> Result<StopSummary, String> {
        let (mut out, stopped, selected, threads) = self.model(cx, |d| {
            let m = &d.model;
            (
                m.summary_base(budget),
                (m.mode == Mode::Break)
                    .then(|| m.stopped.as_ref().map(|s| s.thread))
                    .flatten(),
                (m.thread, m.frame),
                m.threads.iter().map(|t| t.id).collect::<Vec<_>>(),
            )
        })?;
        let Some(stopped) = stopped else {
            return Ok(out);
        };
        let thread = thread.unwrap_or(stopped);
        if !threads.is_empty() && !threads.contains(&thread) {
            return Err(format!(
                "there is no thread {thread} (threads: {threads:?})"
            ));
        }
        let frame = frame.unwrap_or(0);
        let (rows, total) = self
            .frames(cx, thread, 0, budget.max_frames.max(frame + 1))
            .await?;
        if frame >= rows.len() {
            return Err(format!(
                "there is no frame {frame} (the call stack of thread {thread} has {})",
                total.unwrap_or(rows.len())
            ));
        }
        let top = if thread == stopped {
            rows.first().map(|(_, r)| r.clone())
        } else {
            self.frames(cx, stopped, 0, 1)
                .await?
                .0
                .first()
                .map(|(_, r)| r.clone())
        };
        out.stopped = self.model(cx, |d| d.model.summary_stopped(top.as_ref()))?;
        let shown: Vec<StackFrameRow> = rows
            .iter()
            .take(budget.max_frames)
            .map(|(_, r)| r.clone())
            .collect();
        let total = total.unwrap_or(rows.len() + usize::from(rows.len() > shown.len()));
        let frames = FramesBlock {
            thread,
            truncated: shown.len() < total,
            rows: shown,
            total,
        };
        let mut locals = LocalsBlock {
            thread,
            frame,
            ..Default::default()
        };
        match self
            .frame_rows(
                cx,
                thread,
                frame,
                ScopeKind::Locals,
                0,
                budget.max_variables,
                None,
                budget.max_value_chars,
            )
            .await
        {
            Ok((mut rows, total, more)) => {
                let mut left = budget.max_variables - rows.len();
                let mut cut = false;
                self.expand(
                    cx,
                    &mut rows,
                    budget.depth,
                    &mut left,
                    budget.max_value_chars,
                    &mut cut,
                )
                .await?;
                locals.next = more.then_some(rows.len());
                locals.truncated = more || cut;
                locals.total = total;
                locals.rows = rows;
            }
            // A frame without locals the adapter can read (external code): the summary says why.
            Err(e) if !e.starts_with("stale") => {
                out.message = Some(format!("locals of frame {frame}: {e}"));
            }
            Err(e) => return Err(e),
        }
        // The Watch window's values are the selected frame's: listed when that is the frame read (evaluating them in
        // another would run code, which a read does not).
        if (Some(thread), frame) == selected {
            out.watches =
                Some(self.model(cx, |d| d.model.summary_watches(budget.max_value_chars))?);
        }
        out.truncated |= frames.truncated || locals.truncated;
        out.frames = Some(frames);
        out.locals = Some(locals);
        Ok(out)
    }

    async fn stack(
        &self,
        cx: &mut AsyncWindowContext,
        thread: Option<i64>,
        start: usize,
        count: usize,
        all_threads: bool,
    ) -> Result<StackOutput, String> {
        let (stopped, mut threads) = self.model(cx, |d| {
            (
                d.model.stopped.as_ref().map(|s| s.thread),
                d.model
                    .threads
                    .iter()
                    .map(|t| (t.id, t.name.clone()))
                    .collect::<Vec<_>>(),
            )
        })?;
        if threads.is_empty() {
            let t: ThreadsResponse =
                serde_json::from_value(self.one(cx, "threads", Value::Null).await?)
                    .unwrap_or_default();
            threads = t.threads.into_iter().map(|t| (t.id, t.name)).collect();
        }
        let wanted: Vec<(i64, String)> = if all_threads {
            // The thread that stopped first, then the others in the adapter's order.
            let mut v = threads.clone();
            v.sort_by_key(|(id, _)| Some(*id) != stopped);
            v
        } else {
            let id = thread
                .or(stopped)
                .ok_or_else(|| "no thread has stopped".to_owned())?;
            let name = threads
                .iter()
                .find(|(t, _)| *t == id)
                .map(|(_, n)| n.clone())
                .ok_or_else(|| format!("there is no thread {id}"))?;
            vec![(id, name)]
        };
        let mut out = Vec::new();
        for (id, name) in wanted {
            let (rows, total) = self.frames(cx, id, start, count).await?;
            let end = start + rows.len();
            let total = total.unwrap_or(end + usize::from(rows.len() == count));
            out.push(StackThread {
                id,
                name,
                frames: rows.into_iter().map(|(_, r)| r).collect(),
                truncated: end < total,
                next: (end < total).then_some(end),
                total,
            });
        }
        Ok(StackOutput {
            threads: out,
            stop: self.stop,
        })
    }

    #[allow(clippy::too_many_arguments)]
    async fn variables(
        &self,
        cx: &mut AsyncWindowContext,
        target: VariablesTarget,
        start: usize,
        count: usize,
        depth: usize,
        filter: Option<String>,
        max_chars: usize,
    ) -> Result<VariablesOutput, String> {
        let (mut rows, total, more) = match target {
            VariablesTarget::Reference(reference) => match &filter {
                None => {
                    let (vars, total, more) = self.page(cx, reference, start, count, None).await?;
                    self.record(cx, &vars)?;
                    (
                        vars.iter().map(|v| dap_row(v, max_chars)).collect(),
                        total,
                        more,
                    )
                }
                Some(f) => {
                    let f = f.to_lowercase();
                    let mut all = self.all_vars(cx, reference).await?;
                    all.retain(|v| v.name.to_lowercase().starts_with(&f));
                    self.record(cx, &all)?;
                    let total = all.len();
                    let rows: Vec<VarRow> = all
                        .iter()
                        .skip(start)
                        .take(count)
                        .map(|v| dap_row(v, max_chars))
                        .collect();
                    let more = start + rows.len() < total;
                    (rows, total, more)
                }
            },
            VariablesTarget::Frame {
                thread,
                frame,
                scope,
            } => {
                let stopped = self.model(cx, |d| d.model.stopped.as_ref().map(|s| s.thread))?;
                let thread = thread
                    .or(stopped)
                    .ok_or_else(|| "no thread has stopped".to_owned())?;
                self.frame_rows(
                    cx,
                    thread,
                    frame.unwrap_or(0),
                    scope,
                    start,
                    count,
                    filter.as_deref(),
                    max_chars,
                )
                .await?
            }
        };
        let mut left = count - rows.len();
        let mut cut = false;
        self.expand(cx, &mut rows, depth, &mut left, max_chars, &mut cut)
            .await?;
        Ok(VariablesOutput {
            next: more.then_some(start + rows.len()),
            truncated: more,
            total,
            rows,
            stop: self.stop,
        })
    }
}

/// What an agent's command waited for, then its answer (brief 0025).
async fn follow_up(
    this: WeakEntity<Shell>,
    cx: &mut AsyncWindowContext,
    out: DebugOutput,
    follow: Followup,
) -> Outcome {
    let closed = || CommandError::Failed(WINDOW_CLOSED.into());
    match follow {
        Followup::Eval(rx) => {
            let timer = real_timer(AGENT_WAIT);
            Ok(match futures::future::select(rx, timer).await {
                futures::future::Either::Left((Ok(e), _)) => DebugOutput::Evaluate(e),
                futures::future::Either::Left((Err(_), _)) => match out {
                    DebugOutput::Evaluate(p) => DebugOutput::Evaluate(eval_failed(
                        &p.expression,
                        p.stop,
                        "the session ended before the answer arrived",
                    )),
                    other => other,
                },
                futures::future::Either::Right(_) => out,
            })
        }
        Followup::Settle {
            start,
            pause,
            wait,
            budget,
        } => {
            let deadline = Instant::now() + wait;
            let settled = loop {
                let (done, failed, waiter) = this
                    .update(cx, |s, _| {
                        let failed = pause.and_then(|g| {
                            s.debug
                                .pause_error
                                .as_ref()
                                .filter(|(pg, _)| *pg == g)
                                .map(|(_, m)| m.clone())
                        });
                        let done = s.debug_settled(start);
                        // A waiter only while waiting: none is left behind once the command answers.
                        let waiter = (!done && failed.is_none()).then(|| s.debug_waiter());
                        (done, failed, waiter)
                    })
                    .map_err(|_| closed())?;
                if let Some(m) = failed {
                    return Err(CommandError::Failed(format!("Break All failed: {m}")));
                }
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break done;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            };
            let mut summary = summarize(&this, cx, None, None, &budget).await?;
            if !settled {
                summary.timed_out = Some(true);
            }
            Ok(DebugOutput::Summary(Box::new(summary)))
        }
        Followup::Ended { wait } => {
            let deadline = Instant::now() + wait;
            loop {
                let waiter = this
                    .update(cx, |s, _| {
                        (!s.debug_settled(false)).then(|| s.debug_waiter())
                    })
                    .map_err(|_| closed())?;
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            }
            this.update(cx, |s, _| s.debug_state())
                .map_err(|_| closed())
        }
        Followup::Wait {
            until,
            stop,
            baseline,
            wait,
            budget,
        } => {
            let deadline = Instant::now() + wait;
            let satisfied = loop {
                let (holds, waiter) = this
                    .update(cx, |s, _| {
                        let holds = wait_satisfied(&s.debug.model, until, stop, baseline);
                        (holds, holds.is_none().then(|| s.debug_waiter()))
                    })
                    .map_err(|_| closed())?;
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break holds;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            };
            let mut summary = summarize(&this, cx, None, None, &budget).await?;
            match satisfied {
                Some(why) => summary.satisfied = Some(why.into()),
                None => summary.timed_out = Some(true),
            }
            this.update(cx, |s, _| {
                s.debug.timings.wait_answered = Some(Instant::now())
            })
            .map_err(|_| closed())?;
            Ok(DebugOutput::Summary(Box::new(summary)))
        }
        Followup::Snapshot {
            thread,
            frame,
            budget,
        } => {
            let r = Reader::new(&this, cx).map_err(CommandError::Failed)?;
            r.summary(cx, thread, frame, &budget)
                .await
                .map(|s| DebugOutput::Summary(Box::new(s)))
                .map_err(CommandError::Failed)
        }
        Followup::Stack {
            thread,
            start,
            count,
            all_threads,
        } => {
            let r = Reader::new(&this, cx).map_err(CommandError::Failed)?;
            r.stack(cx, thread, start, count, all_threads)
                .await
                .map(DebugOutput::Stack)
                .map_err(CommandError::Failed)
        }
        Followup::Variables {
            target,
            start,
            count,
            depth,
            filter,
            max_value_chars,
        } => {
            let r = Reader::new(&this, cx).map_err(CommandError::Failed)?;
            r.variables(cx, target, start, count, depth, filter, max_value_chars)
                .await
                .map(DebugOutput::Variables)
                .map_err(CommandError::Failed)
        }
        Followup::ExceptionInfo { thread } => {
            let r = Reader::new(&this, cx).map_err(CommandError::Failed)?;
            let body = r
                .one(cx, "exceptionInfo", json!({ "threadId": thread }))
                .await
                .map_err(CommandError::Failed)?;
            let e: ExceptionInfoResponse = serde_json::from_value(body)
                .map_err(|e| CommandError::Failed(format!("exceptionInfo: {e}")))?;
            Ok(DebugOutput::ExceptionInfo(exception_output(
                &e, thread, r.stop,
            )))
        }
    }
}

/// The stop summary once the debuggee settled: read through the adapter, again if the debuggee moved meanwhile
/// (another driver), and as the model has it if it keeps moving.
async fn summarize(
    this: &WeakEntity<Shell>,
    cx: &mut AsyncWindowContext,
    thread: Option<i64>,
    frame: Option<usize>,
    budget: &Budget,
) -> Result<StopSummary, CommandError> {
    for _ in 0..3 {
        let r = Reader::new(this, cx).map_err(CommandError::Failed)?;
        match r.summary(cx, thread, frame, budget).await {
            Ok(s) => return Ok(s),
            Err(e) if e.starts_with("stale") => continue,
            Err(e) => return Err(CommandError::Failed(e)),
        }
    }
    this.update(cx, |s, _| s.debug.model.summary(budget))
        .map_err(|_| CommandError::Failed(WINDOW_CLOSED.into()))
}

/// Reads that need the adapter cannot wait on the UI thread.
fn ui_thread_refusal(command: &str) -> CommandError {
    CommandError::Failed(format!(
        "{command} waits for the debug adapter, which the UI thread never does: call it from another thread (agents \
         and the MCP server do); from the UI thread only what the debugger windows show is answered"
    ))
}

/// Which of `wait`'s conditions holds now, if one does.
fn wait_satisfied(
    m: &DebugModel,
    until: WaitUntil,
    stop: Option<u64>,
    baseline: u64,
) -> Option<&'static str> {
    // Every condition ends when the session does.
    if m.mode == Mode::Design {
        return Some("terminated");
    }
    let stopped = m.mode == Mode::Break && m.settled() && stop.is_none_or(|s| m.stop > s);
    let printed = m.output(OutputKind::Program).next() > baseline;
    match until {
        WaitUntil::Terminated => None,
        WaitUntil::Stopped => stopped.then_some("stopped"),
        WaitUntil::Output => printed.then_some("output"),
        WaitUntil::Any if stopped => Some("stopped"),
        WaitUntil::Any => printed.then_some("output"),
    }
}

/// The adapter's exception details as `eludite.debug.exception_info` lists them, inner exceptions at most
/// [`cmds::MAX_INNER_EXCEPTIONS`] deep.
fn details_row(d: &ExceptionDetails, depth: usize) -> ExceptionDetailsRow {
    ExceptionDetailsRow {
        message: d.message.clone(),
        type_name: d.type_name.clone(),
        full_type_name: d.full_type_name.clone(),
        stack_trace: d.stack_trace.clone(),
        inner_exceptions: if depth < cmds::MAX_INNER_EXCEPTIONS {
            d.inner_exception
                .iter()
                .map(|i| details_row(i, depth + 1))
                .collect()
        } else {
            Vec::new()
        },
    }
}

fn exception_output(e: &ExceptionInfoResponse, thread: i64, stop: u64) -> ExceptionInfoOutput {
    ExceptionInfoOutput {
        supported: true,
        type_name: Some(e.exception_id.clone()).filter(|x| !x.is_empty()),
        message: e.description.clone(),
        break_mode: Some(e.break_mode.clone()).filter(|x| !x.is_empty()),
        details: e.details.as_ref().map(|d| details_row(d, 1)),
        thread,
        stop,
    }
}

/// What the session's adapter supports, for `capabilities` (brief 0025). `eludite-dbg-mono` pages variables by `start`
/// and `count` without advertising it (brief 0022 report, section 9).
fn capabilities_row(c: &Capabilities, adapter: &str) -> CapabilitiesRow {
    let by = |adapter: bool| if adapter { "adapter" } else { "shell" }.to_owned();
    CapabilitiesRow {
        adapter: adapter.to_owned(),
        pause: true,
        set_variable: c.supports_set_variable,
        exception_info: c.supports_exception_info_request,
        function_breakpoints: c.supports_function_breakpoints,
        log_points: by(c.supports_log_points),
        hit_conditions: by(c.supports_hit_conditional_breakpoints),
        exception_filter_options: c.supports_exception_filter_options,
        set_next_statement: c.supports_goto_targets_request,
        data_breakpoints: c.supports_data_breakpoints,
        step_back: c.supports_step_back,
        restart: c.supports_restart_request,
        terminate: c.supports_terminate_request,
        modules: c.supports_modules_request,
        memory: c.supports_read_memory_request,
        disassembly: c.supports_disassemble_request,
        delayed_stack_loading: c.supports_delayed_stack_trace_loading,
        variable_paging: c.supports_variable_paging || adapter == "mono",
    }
}

/// A variable as a row, its value cut at `max_chars`.
fn dap_row(v: &Variable, max_chars: usize) -> VarRow {
    VarNode::from_dap(v).var_row(max_chars)
}

impl Debugger {
    /// `eludite.debug.stack` from what the Call Stack window has, when that is enough (the UI thread).
    fn stack_from_model(
        &self,
        thread: Option<i64>,
        start: usize,
        count: usize,
        all_threads: bool,
    ) -> Option<StackOutput> {
        let m = &self.model;
        let id = thread.or(m.stopped.as_ref().map(|s| s.thread))?;
        let complete = m.frames.len() >= m.frames_total || start + count <= m.frames.len();
        if all_threads || Some(id) != m.thread || !complete {
            return None;
        }
        let total = m.frames_total.max(m.frames.len());
        let frames: Vec<StackFrameRow> = m
            .frames
            .iter()
            .skip(start)
            .take(count)
            .map(Frame::stack_row)
            .collect();
        let end = start + frames.len();
        Some(StackOutput {
            threads: vec![StackThread {
                id,
                name: thread_name(m, id),
                truncated: end < total,
                next: (end < total).then_some(end),
                frames,
                total,
            }],
            stop: m.stop,
        })
    }

    /// `eludite.debug.exception_info` for the thread that stopped: what the shell read at the stop.
    fn exception_from_model(&self, thread: i64) -> ExceptionInfoOutput {
        let m = &self.model;
        match &m.exception_info {
            Some(e) => exception_output(e, thread, m.stop),
            None => {
                let brief = m.summary_stopped(None).and_then(|s| s.exception);
                ExceptionInfoOutput {
                    supported: m.capabilities.as_ref().is_some_and(|c| c.exception_info),
                    type_name: brief.as_ref().and_then(|b| b.type_name.clone()),
                    message: brief.as_ref().and_then(|b| b.message.clone()),
                    break_mode: brief.and_then(|b| b.break_mode),
                    details: None,
                    thread,
                    stop: m.stop,
                }
            }
        }
    }
}

fn thread_name(m: &DebugModel, id: i64) -> String {
    m.threads
        .iter()
        .find(|t| t.id == id)
        .map(|t| t.name.clone())
        .unwrap_or_default()
}

fn set_children(nodes: &mut [VarNode], path: &[usize], vars: Vec<VarNode>) {
    if let Some(n) = node_mut(nodes, path) {
        n.children = Some(vars);
        n.loading = false;
    }
}
