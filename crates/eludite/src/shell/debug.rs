//! Run and debug (brief 0018): F5 runs the startup project under its debug adapter through `eludite-dap` (netcoredbg
//! for .NET, `eludite-dbg-mono` under the located Mono for .NET Framework on Linux and macOS: brief 0022; lldb-dap for
//! a Cargo package: brief 0029, [`native`]), Ctrl+F5
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
//! - **Run control** (brief 0026). Tracepoints print and continue: where the adapter has log points (`eludite-dbg-mono`)
//!   it gets `logMessage` and the lines it prints are told apart from its other console output by their message
//!   ([`state::message_matches`]); where it has none (netcoredbg) the breakpoint breaks and the shell, before showing
//!   anything, evaluates the message's `{expression}`s in the hit frame and resumes ([`Pending::TraceEval`],
//!   [`Pending::TraceResume`], tagged with the session generation), so the mode stays `running` and no stop is counted.
//!   Either way the line goes to the Output window's Debug source and the `debug` ring, and is recorded with its point
//!   and hit for `trace`. `run_until` adds temporary breakpoints removed at the next visible stop; `trace` installs
//!   temporary tracepoints and collects what they print in a task of its own (the job loop's), never on the UI thread.
//!   Function breakpoints go through `setFunctionBreakpoints` (in one list with a native session's Rust panics row,
//!   since the request replaces them all), exception types through `filterOptions`, values through `setVariable` (or
//!   `setExpression`), and Set Next Statement through `gotoTargets` and `goto`; each only where the adapter's
//!   capabilities have it (lldb-dap aborts on a request it does not know).
//! - **Attach, restart and who drives** (brief 0027). `attach` starts a session from a running process on a
//!   `debug-attach` thread (the process listing, the adapter by the process's runtime, its attach plan, the handshake
//!   with `attach`), with `session.attached`; Stop then detaches (`terminateDebuggee: false`) and the process keeps
//!   running. `processes` lists the candidates off the UI thread. `restart` sends DAP `restart` where the adapter has
//!   it, else stops and starts the last start's configuration again. Allow Agents to Drive
//!   ([`state::DebugModel::agents_allowed`]) refuses agents' driving commands while off; the person's resuming commands
//!   end an agent's waiting command at once with `interrupted_by: "user"` and make its next resuming command stale
//!   until it reads the state (proposal 0001 rule 5).
//! - **Several sessions** (brief 0028). Each start (a compound's projects each) and each attach is a session with an id
//!   (1, 2, ..., never reused while the shell runs), its own generation (unique across sessions), stop counter, mode,
//!   adapter, client, stack, locals, output rings, Allow Agents to Drive switch and interrupt count. The [`Debugger`]'s
//!   per-session fields hold the current session; the others wait in [`Slot`]s and [`Debugger::enter`] swaps one in.
//!   Between messages and commands the current session is the active one, which the windows show and commands
//!   without `session` address. Messages are routed by their generation, agents' follow-ups and reads by their
//!   session; breakpoints, exception settings and watch expressions stay in place when sessions swap (each
//!   breakpoint keeps its binding per session) and every change goes to every connected adapter. A stop takes the
//!   windows unless the person picked another session in the last two seconds or the active one is at its own break.
//!   Stop Debugging ends every session; `stop` with a `session` ends that one.
//! - **Output by source.** The program's lines (stdout, stderr), the debugger's own messages and the adapter's
//!   (stderr, console) go to three rings of 10,000 lines per session, read by cursor (`eludite.debug.output`); the
//!   Output window's Debug source still shows the program's output and the debugger's messages together.

#[cfg(test)]
mod conformance_tests;
pub mod native;
#[cfg(test)]
mod native_tests;
pub mod state;
#[cfg(test)]
mod tests;
pub mod windows;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::io::BufRead;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_commands::build::OutputSource;
use eludite_commands::debug::{
    self as cmds, BreakpointAction, Budget, CapabilitiesRow, DebugOutput, DebugRequest,
    EvalContext, EvaluateOutput, ExceptionDetailsRow, ExceptionInfoOutput, FramesBlock,
    LocalsBlock, OutputKind, OutputPage, ScopeKind, SessionRow, SetTarget, SetVariableOutput,
    StackFrameRow, StackOutput, StackThread, StopSummary, StoppedRow, ThreadRow, TraceLine,
    TraceOutput, TracePointRow, TraceRun, TraceUntil, VarRow, VariableRow, VariablesOutput,
    VariablesTarget, WaitUntil,
};
use eludite_commands::debug::{AllowAgentsOutput, AttachTarget, ProcessRow, ProcessesOutput};
use eludite_commands::project::StartupAction;
use eludite_commands::view::{DockEdge, DockTarget, ViewRequest, ViewTarget as _};
use eludite_commands::{Caller, CommandError, CommandRegistry};
use eludite_dap::attach::{AttachAdapter, attach_plan};
use eludite_dap::discovery::{
    AdapterSearch, JsDebugSearch, MonoAdapterSearch, MonoSearch, NodeSearch,
};
use eludite_dap::launch::{AdapterKind, FrameworkKind, Platform, Readiness, ServerWatch};
use eludite_dap::processes;
use eludite_dap::session::{self as dap_session, AdapterFamily, StartKind, StartPlan, Started};
use eludite_dap::transport::AdapterServer;
use eludite_dap::types::{
    Capabilities, EvaluateResponse, Event, ExceptionDetails, ExceptionInfoResponse,
    GotoTargetsResponse, ScopesResponse, SetBreakpointsResponse, SetVariableResponse,
    StackTraceResponse, StoppedEvent, ThreadsResponse, Variable, VariablesResponse,
};
use eludite_dap::{ClientEvent, Connection, DapClient, launch, transport};
use eludite_docking::ids;
use eludite_editor::{EditorEvent, ExecutionKind};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui::{AppContext as _, AsyncWindowContext, Context, WeakEntity, Window};
use serde_json::{Value, json};

use self::state::{
    Breakpoint, BrowserAttach, DebugModel, Frame, Mode, Persisted, Segment, VarNode,
    exception_plan, exception_plan_for, flatten, locals_scope, node_mut, parse_message, row_at_mut,
};
use self::windows::{DebugWindows, StackRow, ThreadLine};
use super::Shell;
use super::browser::BrowserBus;
use super::documents::{normalize_path, trace};

/// Status bar slot: the debugger's state (left, after the solution's).
pub const DEBUG_SLOT: &str = "debug";
/// The status bar's Allow Agents to Drive toggle (a debug selector; brief 0027).
pub const DEBUG_AGENTS_TOGGLE: &str = "debug-allow-agents";
/// How long an agent's resuming command waits for the debuggee to settle, and its evaluate for the answer, by
/// default.
pub const AGENT_WAIT: Duration = Duration::from_secs(5);
/// How long an agent's `toggle_breakpoint` waits for the live sessions' adapters to answer the change (brief 0036).
pub const BREAKPOINT_ANSWER_WAIT: Duration = Duration::from_millis(500);
/// A point's reason when the adapter gave none (brief 0036).
const NOT_BOUND: &str = "the debug adapter did not bind it";
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
    /// vscode-js-debug and the Node.js it runs on (brief 0038).
    pub js: JsSetup,
}

/// Starts vscode-js-debug for a browser session (brief 0038; tests: the fake js-debug): the server every connection
/// of the session's tree goes to, its description for `session.adapter`, and the versions for `adapter_version`.
pub type JsStarter =
    Arc<dyn Fn() -> Result<(Arc<dyn AdapterServer>, String, String), String> + Send + Sync>;

/// How browser sessions reach vscode-js-debug (brief 0038).
#[derive(Clone, Default)]
pub struct JsSetup {
    /// `dapDebugServer.js`: the setting `debugger.jsDebugPath`, the cache of `tools/js-debug/fetch.sh`, beside Eludite.
    pub search: JsDebugSearch,
    /// The Node.js it runs on: the setting `debugger.nodePath`, `PATH`, Volta, nvm, fnm.
    pub node: NodeSearch,
    /// Start this instead (tests): the search and Node are not looked at.
    pub start: Option<JsStarter>,
}

impl JsSetup {
    /// The machine's searches, without the configured paths (the settings store gives them).
    pub fn from_env() -> Self {
        Self {
            search: JsDebugSearch::from_env(),
            node: NodeSearch::from_env(),
            start: None,
        }
    }

    /// Locate vscode-js-debug and Node.js (its version checked) and start the DAP server on a loopback port. Blocks:
    /// call it on the attach thread. The error says what is missing and how to get it.
    pub fn start_server(&self) -> Result<(Arc<dyn AdapterServer>, String, String), String> {
        if let Some(start) = &self.start {
            return start();
        }
        let js = self.search.find()?;
        let (node, _) = self.node.find(&eludite_dap::discovery::JS_DEBUG_NODE)?;
        let node_version = eludite_dap::discovery::check_node_version(
            &node,
            eludite_dap::discovery::node_version_output(&node).as_deref(),
        )?;
        let args = vec![
            js.script.to_string_lossy().into_owned(),
            "0".to_owned(),
            "127.0.0.1".to_owned(),
        ];
        let server = eludite_dap::transport::start_tcp_server(
            &node,
            &args,
            eludite_dap::transport::TCP_SERVER_START_TIMEOUT,
        )
        .map_err(|e| format!("cannot start vscode-js-debug: {e}"))?;
        let description = format!(
            "vscode-js-debug {} under node {node_version} ({})",
            js.version,
            server.describe()
        );
        let version = format!("vscode-js-debug {}, node {node_version}", js.version);
        Ok((Arc::new(server), description, version))
    }
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
            js: JsSetup::from_env(),
        }
    }
}

pub type Outcome = Result<DebugOutput, CommandError>;

/// `eludite.debug.*` from another thread (an agent), for the UI thread to apply.
pub struct DebugJob {
    /// The session the call named (brief 0028).
    pub session: Option<u32>,
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
    fn apply(&self, session: Option<u32>, request: DebugRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let request = resolve_attach(request)?;
        let wait = request
            .wait_ms()
            .map(Duration::from_millis)
            .unwrap_or(AGENT_WAIT);
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(DebugJob {
                session,
                request,
                reply,
                caller: eludite_commands::current_caller(),
            })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(wait + Duration::from_secs(30))
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
    }
}

/// An attach by process name, off the UI thread (the caller's): the one process with that name, by id, or refused
/// listing the matches (brief 0027). A pid that no process has is refused here too; a process on another machine
/// (`transport`) is not looked up.
fn resolve_attach(request: DebugRequest) -> Result<DebugRequest, CommandError> {
    let DebugRequest::Attach {
        target,
        adapter,
        transport,
        mono,
        web_root,
        wait_ms,
        budget,
    } = request
    else {
        return Ok(request);
    };
    let target = match (&target, &transport) {
        (AttachTarget::Name(_) | AttachTarget::Pid(_), None) => {
            let all = processes::list().map_err(CommandError::Failed)?;
            AttachTarget::Pid(
                find_process(&all, &target)
                    .map_err(CommandError::Failed)?
                    .pid,
            )
        }
        _ => target,
    };
    Ok(DebugRequest::Attach {
        target,
        adapter,
        transport,
        mono,
        web_root,
        wait_ms,
        budget,
    })
}

/// The process `target` names among `all`: by id, or the one process with that name (case aside, `.exe` optional).
fn find_process(
    all: &[processes::ProcessInfo],
    target: &AttachTarget,
) -> Result<processes::ProcessInfo, String> {
    match target {
        AttachTarget::Pid(pid) => all.iter().find(|p| p.pid == *pid).cloned().ok_or_else(|| {
            format!(
                "there is no process {pid} on this machine (eludite.debug.processes lists them)"
            )
        }),
        AttachTarget::Name(name) => {
            let want = name.trim().trim_end_matches(".exe").to_lowercase();
            let matches: Vec<&processes::ProcessInfo> = all
                .iter()
                .filter(|p| p.name.trim_end_matches(".exe").to_lowercase() == want)
                .collect();
            match matches.as_slice() {
                [] => Err(format!(
                    "no process is named `{name}` (eludite.debug.processes lists them)"
                )),
                [one] => Ok((*one).clone()),
                many => Err(format!(
                    "{} processes are named `{name}`: {}; pass `pid`",
                    many.len(),
                    many.iter()
                        .take(10)
                        .map(|p| format!("{} ({})", p.pid, processes::cut(&p.command_line(), 80)))
                        .collect::<Vec<_>>()
                        .join(", ")
                )),
            }
        }
        AttachTarget::Dialog | AttachTarget::Tab(_) | AttachTarget::Url(_) => {
            Err("name the process: `pid` or `process_name`".into())
        }
    }
}

/// `eludite.debug.processes`: this machine's processes matching `filter`, `launched_by_eludite` from `roots` and their
/// descendants. Blocks (the process table): run it on a worker thread.
pub fn list_processes(filter: Option<&str>, roots: &[u32]) -> Result<ProcessesOutput, String> {
    let all = processes::list()?;
    let launched = processes::launched_set(roots, &all);
    let needle = filter.map(str::to_lowercase);
    let mut rows: Vec<ProcessRow> = all
        .iter()
        .filter_map(|p| {
            let line = p.command_line();
            if let Some(n) = &needle
                && !p.name.to_lowercase().contains(n)
                && !line.to_lowercase().contains(n)
            {
                return None;
            }
            Some(ProcessRow {
                pid: p.pid,
                parent: p.parent,
                name: p.name.clone(),
                command_line: processes::cut(&line, cmds::MAX_COMMAND_LINE),
                runtime: p.runtime.as_str().to_owned(),
                launched_by_eludite: launched.contains(&p.pid),
                debugger_agent: (p.runtime == processes::Runtime::Mono)
                    .then(|| processes::mono_agent(&p.argv))
                    .flatten()
                    .map(|(h, port)| format!("{h}:{port}")),
            })
        })
        .collect();
    rows.sort_by(|a, b| {
        a.name
            .to_lowercase()
            .cmp(&b.name.to_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    let total = rows.len();
    let truncated = total > cmds::MAX_PROCESSES;
    rows.truncate(cmds::MAX_PROCESSES);
    Ok(ProcessesOutput {
        processes: rows,
        total,
        truncated,
        tabs: None,
    })
}

/// `processes`' listing with the browser's tabs (brief 0038), asked of the browser worker (it never starts the
/// browser). On the listing's thread.
fn with_tabs(mut out: ProcessesOutput, bus: Option<&BrowserBus>) -> ProcessesOutput {
    out.tabs = bus.and_then(BrowserBus::tab_list).map(|rows| {
        rows.into_iter()
            .map(|(id, title, url)| cmds::AttachTabRow {
                id,
                title,
                url,
                session: None,
            })
            .collect()
    });
    out
}

/// What the Debug menu's Restart and Attach to Process... items and its Allow Agents to Drive check item read (the
/// menu bar is another entity; the shell updates this with the state). With several sessions (brief 0028) the items
/// follow the active session: Continue, the steps, Run To Cursor and Set Next Statement need it in break mode, Break
/// All needs it running; Stop Debugging is enabled while any session runs; Attach to Process... adds a session in any
/// mode.
#[derive(Debug)]
pub struct DebugMenuState {
    restart: std::sync::atomic::AtomicBool,
    attach: std::sync::atomic::AtomicBool,
    agents_allowed: std::sync::atomic::AtomicBool,
    /// The active session is in break mode, running, or any session is live.
    in_break: std::sync::atomic::AtomicBool,
    running: std::sync::atomic::AtomicBool,
    live: std::sync::atomic::AtomicBool,
    /// The setting `browser.useBuiltIn`: Debug > Open in Web Browser Window's check (brief 0037).
    pub use_built_in: std::sync::atomic::AtomicBool,
    /// The browser whose engine the check item needs, and the last look for it (at most one a second).
    pub browser: Mutex<Option<BrowserBus>>,
    engine_seen: Mutex<Option<(Instant, bool)>>,
}

impl Default for DebugMenuState {
    fn default() -> Self {
        Self {
            restart: false.into(),
            attach: true.into(),
            agents_allowed: true.into(),
            in_break: false.into(),
            running: false.into(),
            live: false.into(),
            use_built_in: true.into(),
            browser: Mutex::new(None),
            engine_seen: Mutex::new(None),
        }
    }
}

/// The setting behind Debug > Open in Web Browser Window (brief 0037).
pub const USE_BUILT_IN: &str = "browser.useBuiltIn";

impl DebugMenuState {
    /// Whether the Debug menu's item for `command` is enabled (true for the commands it does not govern).
    pub fn enabled(&self, command: &str) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        match command {
            cmds::RESTART => self.restart.load(Relaxed),
            cmds::ATTACH => self.attach.load(Relaxed),
            cmds::CONTINUE
            | cmds::STEP_OVER
            | cmds::STEP_INTO
            | cmds::STEP_OUT
            | cmds::RUN_TO_CURSOR
            | cmds::SET_NEXT_STATEMENT => self.in_break.load(Relaxed),
            cmds::PAUSE => self.running.load(Relaxed),
            cmds::STOP => self.live.load(Relaxed),
            _ => true,
        }
    }

    /// Whether the check item of `command` (or of a setting's key) is on.
    pub fn checked(&self, command: &str) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        match command {
            cmds::ALLOW_AGENTS => self.agents_allowed.load(Relaxed),
            USE_BUILT_IN => self.use_built_in.load(Relaxed),
            _ => false,
        }
    }

    /// Whether the Debug menu's item for `command` with `args` is enabled, for the items that share a command with
    /// another (brief 0037): Start in External Browser (`start` with `browser: external`) while no session runs, as
    /// Start Without Debugging is in Visual Studio; Open in Web Browser Window while the embedded engine is found.
    pub fn item_enabled(&self, command: &str, args: &Value) -> bool {
        use std::sync::atomic::Ordering::Relaxed;
        if command == cmds::START && args["browser"] == "external" {
            return !self.live.load(Relaxed);
        }
        if command == eludite_commands::settings::SET && args["key"] == USE_BUILT_IN {
            return self.engine_found();
        }
        true
    }

    /// Whether the Web Browser window can show pages (the embedded engine, or a test's), looked for at most once a
    /// second (a few file checks).
    fn engine_found(&self) -> bool {
        let mut seen = self.engine_seen.lock().unwrap_or_else(|e| e.into_inner());
        if let Some((at, found)) = *seen
            && at.elapsed() < Duration::from_secs(1)
        {
            return found;
        }
        let found = self
            .browser
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_ref()
            .is_some_and(|b| b.status().embedded);
        *seen = Some((Instant::now(), found));
        found
    }

    /// `m` is the active session's model; `live`: some session runs.
    fn update(&self, m: &DebugModel, live: bool) {
        use std::sync::atomic::Ordering::Relaxed;
        self.restart.store(
            !matches!(m.mode, Mode::Design | Mode::Stopping | Mode::Building) && !m.attached(),
            Relaxed,
        );
        // An attach adds a session beside the others (brief 0028).
        self.attach.store(true, Relaxed);
        self.agents_allowed.store(m.agents_allowed, Relaxed);
        self.in_break.store(m.mode == Mode::Break, Relaxed);
        self.running.store(m.mode == Mode::Running, Relaxed);
        self.live.store(live, Relaxed);
    }
}

/// One project a start runs (brief 0028): `None` is the startup project.
#[derive(Debug, Clone)]
struct StartEntry {
    project: Option<String>,
    debug: bool,
    profile: Option<String>,
}

/// What a start ran (project, launch profile, debug flag, build before run, Cargo options): Restart starts it again.
#[derive(Debug, Clone)]
struct StartArgs {
    project: Option<String>,
    debug: bool,
    profile: Option<String>,
    build: Option<bool>,
    cargo: cmds::CargoOptions,
    /// Where a web project's page opens (brief 0037).
    browser: Option<cmds::BrowserChoice>,
}

/// The stop a resuming command quotes, if any.
fn quoted_stop(r: &DebugRequest) -> Option<u64> {
    match r {
        DebugRequest::Continue { stop, .. }
        | DebugRequest::Step { stop, .. }
        | DebugRequest::RunToCursor { stop, .. }
        | DebugRequest::RunUntil { stop, .. }
        | DebugRequest::Trace { stop, .. }
        | DebugRequest::SetVariable { stop, .. }
        | DebugRequest::SetNextStatement { stop, .. } => *stop,
        _ => None,
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
        session: Box<SessionRow>,
        run: Option<RunHandle>,
        /// What a web project's launch opens (brief 0037): its `session.browser` is the page, waiting.
        plan: Option<Box<launch::BrowserLaunch>>,
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
    /// The launch's browser step (brief 0037): the session's page now, a line for the Output window's Debug source,
    /// and the time from Kestrel's listening line to the page opened.
    Browser {
        generation: u64,
        browser: cmds::SessionBrowser,
        line: Option<String>,
        latency: Option<Duration>,
    },
    /// A solution's persisted breakpoints were read.
    Loaded {
        solution: PathBuf,
        persisted: Option<Persisted>,
    },
    StopTimeout {
        generation: u64,
    },
    /// A process listing the person's `processes` asked for (brief 0027).
    Processes {
        listing: Result<ProcessesOutput, String>,
    },
    /// The session's adapter and runtime versions (brief 0038).
    AdapterVersion {
        generation: u64,
        version: String,
    },
    /// vscode-js-debug asked (DAP `startDebugging`) for a child session of the session of `generation` (brief 0038):
    /// its `request` and configuration, and the server its connection goes to.
    StartChild {
        generation: u64,
        request: String,
        configuration: Value,
        server: Arc<dyn AdapterServer>,
    },
}

impl DebugMsg {
    /// The session generation the message belongs to (`None`: not a session's).
    fn generation(&self) -> Option<u64> {
        match self {
            DebugMsg::Launched { generation, .. }
            | DebugMsg::Connected { generation, .. }
            | DebugMsg::Started { generation, .. }
            | DebugMsg::LaunchFailed { generation, .. }
            | DebugMsg::Client { generation, .. }
            | DebugMsg::Output { generation, .. }
            | DebugMsg::ProgramExited { generation, .. }
            | DebugMsg::Browser { generation, .. }
            | DebugMsg::StartChild { generation, .. }
            | DebugMsg::AdapterVersion { generation, .. }
            | DebugMsg::StopTimeout { generation } => Some(*generation),
            DebugMsg::Loaded { .. } | DebugMsg::Processes { .. } => None,
        }
    }
}

/// What an agent's command waits for, in which session, and the session's interrupt count when it was applied (a
/// command of the person's in that session since ends the wait; brief 0027 rule 5 per session, brief 0028).
pub struct Follow {
    pub sid: u32,
    pub epoch: u64,
    pub what: Followup,
}

/// What an agent's command waits for after it was applied, off the UI thread (brief 0025).
pub enum Followup {
    /// `evaluate`'s answer.
    Eval(oneshot::Receiver<EvaluateOutput>),
    /// `toggle_breakpoint`: the live sessions' adapters' answers to the change, then the breakpoint's row again
    /// (brief 0036).
    Breakpoint {
        edit: cmds::BreakpointEdit,
        target: BreakpointTarget,
        wait: Duration,
    },
    /// A command that runs the debuggee: wait until it settles (a start: until it runs), then answer the summary.
    /// `pause`: Break All of that generation, which fails when the adapter refuses it.
    Settle {
        start: bool,
        pause: Option<u64>,
        /// Set Next Statement: settled only past this (generation, stop), and failed if its `goto` fails.
        after: Option<(u64, u64)>,
        /// Restart by stopping and starting: settled only in a session newer than this generation (brief 0027).
        fresh: Option<u64>,
        /// `run_until`'s points: those that never bound go in the answer's `points_failed` (brief 0036).
        points: Vec<(String, u32)>,
        wait: Duration,
        budget: Budget,
    },
    /// `processes`: the listing, made on a `debug-attach` thread (brief 0027).
    Processes {
        filter: Option<String>,
        roots: Vec<u32>,
    },
    /// `trace`: until its condition holds, then its lines (brief 0026).
    Trace {
        until: TraceUntil,
        wait: Duration,
        budget: Budget,
    },
    /// `set_variable`'s answer once the adapter gives it.
    SetValue(oneshot::Receiver<Result<SetVariableOutput, String>>),
    /// `set_variable` in a frame the windows do not show: read its scope first.
    SetVariable {
        thread: Option<i64>,
        frame: usize,
        name: String,
        value: String,
    },
    /// `stop`: until the session ended (`all`: every session, Stop Debugging; brief 0028), then the state.
    Ended {
        wait: Duration,
        all: bool,
    },
    /// A compound start (brief 0028): until one of its sessions breaks.
    Compound {
        ids: Vec<u32>,
        wait: Duration,
        budget: Budget,
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
        /// When the `stopped` event was handled (an emulated tracepoint's overhead starts here).
        at: Instant,
    },
    /// An `{expression}` of an emulated tracepoint's message (brief 0026).
    TraceEval {
        generation: u64,
        hit: u64,
        ix: usize,
    },
    /// The resume after an emulated tracepoint's line: `record` is the line's absolute index.
    TraceResume {
        generation: u64,
        record: usize,
        at: Instant,
    },
    SetFunctionBreakpoints {
        generation: u64,
        names: Vec<String>,
    },
    /// `setVariable` or `setExpression` of `name` in variables reference `reference`.
    SetValue {
        generation: u64,
        stop: u64,
        reference: i64,
        name: String,
        request: &'static str,
        reply: Option<oneshot::Sender<Result<SetVariableOutput, String>>>,
    },
    /// Set Next Statement's `gotoTargets`; its answer sends `goto`.
    GotoTargets {
        generation: u64,
        stop: u64,
        thread: i64,
        driver: String,
        line: u32,
    },
    Goto {
        generation: u64,
        stop: u64,
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
    /// DAP `restart` was sent (brief 0027).
    Restart {
        generation: u64,
    },
    /// An attached session's `disconnect` (detach) was sent: its answer ends the session (brief 0027).
    Detach {
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

/// A line a tracepoint printed (brief 0026): which point, its hit, when, and for an emulated one what the stop cost.
#[derive(Debug, Clone)]
pub struct TraceRecord {
    pub generation: u64,
    pub path: String,
    pub line: u32,
    pub hit: u32,
    pub text: String,
    pub at: Instant,
    /// The debugger printed it (a stop and a resume); `overhead` is from the stop to the resume's answer.
    pub emulated: bool,
    pub overhead: Option<Duration>,
}

/// An emulated tracepoint's hit waiting for its expressions' values.
struct PendingHit {
    generation: u64,
    path: String,
    line: u32,
    hit: u32,
    thread: i64,
    function: String,
    caller: Option<String>,
    segments: Vec<Segment>,
    values: Vec<Option<String>>,
    waiting: usize,
    at: Instant,
}

/// `trace` while it collects.
#[derive(Debug, Clone)]
struct TraceJob {
    generation: u64,
    /// The points, and the breakpoints they replaced (put back after).
    points: Vec<(String, u32)>,
    saved: Vec<Option<Breakpoint>>,
    /// The absolute index of the first record it may collect.
    base: usize,
    max_hits: usize,
    count: Option<usize>,
    hits: usize,
    started: Instant,
    /// The stop it started from: a later visible stop ends it.
    stop: u64,
    /// `count` or `max_hits` reached.
    done: bool,
    truncated: bool,
    /// Whether each point was bound, and why not, as the session left them (it unbinds everything at its end).
    bound: Option<Vec<(bool, Option<String>)>>,
}

/// Lines of [`TraceRecord`]s kept (older ones are dropped in blocks).
const TRACE_RECORDS: usize = 20_000;

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
    /// When each batch of the debugger's messages held the UI thread, and for how long (brief 0026's frame cost while
    /// a tracepoint fires).
    pub msgs_ui: Vec<(Instant, Duration)>,
    /// Step sent to its break shown (locals loaded and the windows given them).
    pub steps: Vec<Duration>,
    /// The last time the windows were given a break's locals.
    pub locals_shown: Option<Instant>,
    /// Build before run (brief 0020): the build was requested, its result arrived, the launch thread started.
    pub build_requested: Option<Instant>,
    pub build_finished: Option<Instant>,
    pub launched: Option<Instant>,
    /// An agent's interrupted wait was answered (brief 0027; from `Debugger::interrupted_at`).
    pub interrupt_answered: Option<Instant>,
    /// A web project's page opened and its debugger started attaching (brief 0038), and how long from then until
    /// the page's child session ran (the budget: the fake attach adds under 50 ms to the launch).
    pub page_attach_started: Option<Instant>,
    #[cfg_attr(not(test), allow(dead_code))]
    pub page_attach: Option<Duration>,
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

/// How long after the person picked a session by hand a stop in another one does not take the windows (brief 0028).
pub const SELECTION_HOLD: Duration = Duration::from_secs(2);
/// Ended sessions kept aside for the agents' answers that still read them.
const ENDED_KEPT: usize = 16;

/// One debugging session's own state, while another session is the one the debugger works on (brief 0028). The
/// fields are the [`Debugger`]'s of the same names; [`Debugger::enter`] swaps them.
#[derive(Default)]
struct Slot {
    id: u32,
    name: String,
    model: DebugModel,
    client: Option<DapClient>,
    run: Option<RunHandle>,
    caps: Capabilities,
    pending: HashMap<i64, Pending>,
    run_to_cursor: Option<(String, u32)>,
    exec: Option<ExecPoint>,
    console_partial: String,
    early_breakpoints: Vec<eludite_dap::types::Breakpoint>,
    pending_launch: Option<PendingLaunch>,
    pause_error: Option<(u64, String)>,
    counts: HashMap<i64, usize>,
    cargo_options: cmds::CargoOptions,
    trace_hits: HashMap<u64, PendingHit>,
    trace_job: Option<TraceJob>,
    goto_error: Option<(u64, u64, String)>,
    console_partial_adapter: String,
    interrupt: u64,
    agent_stale: bool,
    last_start: Option<StartArgs>,
    restart_pending: Option<(StartArgs, String)>,
}

/// The debugger: the model, the windows and the session's plumbing. With several sessions (brief 0028) the fields
/// marked "per session" are the current session's (the one being worked on, which between messages is the active
/// one); the others wait in `others` until [`Debugger::enter`] swaps one in.
pub struct Debugger {
    /// Per session: the session's model (with the shared breakpoints, exception settings and watch expressions).
    pub model: DebugModel,
    /// The next launch's configuration as the Test Explorer computed it (brief 0035: a test application with
    /// `--server --client-port`), instead of the project's own; taken by that launch.
    pub test_launch: Option<launch::LaunchConfig>,
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
    /// How Cargo packages are debugged (brief 0029): the lldb-dap search and the Rust formatters.
    native: native::NativeSetup,
    /// The last start's Cargo options (target, test, arguments), for its launch after the build.
    cargo_options: cmds::CargoOptions,
    /// Emulate tracepoints even where the adapter has log points (tests measure the emulation on `eludite-dbg-mono`).
    pub(super) shell_log_points: bool,
    /// The lines tracepoints printed, from absolute index `traces_base` on.
    traces: Vec<TraceRecord>,
    traces_base: usize,
    /// Emulated tracepoint hits waiting for their values, by hit id.
    trace_hits: HashMap<u64, PendingHit>,
    next_trace_hit: u64,
    /// `trace` while it collects.
    trace_job: Option<TraceJob>,
    /// Set Next Statement failed at (generation, stop): the message.
    goto_error: Option<(u64, u64, String)>,
    /// The adapter's console text without its newline yet (its log point lines are matched whole).
    console_partial_adapter: String,
    /// The programs this shell started (Ctrl+F5's, F5's debuggee), by process id: `processes`' `launched_by_eludite`
    /// and the attach hook's (brief 0027).
    pub launched: Arc<Mutex<Vec<u32>>>,
    /// Grows with every command of the person's that resumes, pauses, stops or restarts the debuggee: an agent's
    /// command that waits from another value was interrupted (proposal 0001 rule 5).
    pub interrupt: u64,
    /// When the last such command was applied.
    pub interrupted_at: Option<Instant>,
    /// An agent's wait was interrupted: its next resuming command is stale until it reads the state.
    agent_stale: bool,
    /// What the last start ran, for Restart.
    last_start: Option<StartArgs>,
    /// Restart without the adapter's `restart`: start this (by this driver) once the session has ended.
    restart_pending: Option<(StartArgs, String)>,
    /// The last process listing: the dialog's rows and the UI thread's answer to `processes`.
    processes: Option<ProcessesOutput>,
    /// Ctrl+F5 programs an attach left running: no session kills them.
    background_runs: Vec<RunHandle>,
    /// The Attach to Process dialog, while open.
    pub attach_dialog: Option<gpui::Entity<windows::AttachDialog>>,
    /// The Startup Projects dialog, while open (brief 0028).
    pub startup_dialog: Option<gpui::Entity<eludite_ui::startup::StartupProjectsDialog>>,
    /// What the Debug menu reads.
    pub menu: Arc<DebugMenuState>,
    /// Per session: its id (0 before the first session) and name (brief 0028).
    pub session_id: u32,
    session_name: String,
    /// The sessions not being worked on now: the live ones and the last ended ones.
    others: Vec<Slot>,
    /// The session the windows show, the execution point follows and commands without `session` act on.
    pub active: u32,
    next_session: u32,
    /// The highest generation given to any session (generations are unique across sessions).
    last_generation: u64,
    /// When the person last picked a session by hand (the Call Stack and Threads selectors).
    selected_at: Option<Instant>,
    /// The execution point the editors show (the active session's).
    shown_exec: Option<ExecPoint>,
    /// The sessions of the last compound start, in launch order.
    compound: Vec<u32>,
    /// The session the command being applied started (start, attach, trace's start), for its follow-up.
    started: Option<u32>,
    /// A restart is starting the current session again (it keeps its id).
    restarting: bool,
    /// The Cargo options of the start being applied (they become its session's).
    start_cargo: cmds::CargoOptions,
    /// The `browser` of the start being applied (brief 0037; it becomes its sessions').
    start_browser: Option<cmds::BrowserChoice>,
    /// The launch's browser step (brief 0037): its settings, and the bus and browser it opens pages through.
    pub browser_launch: LaunchBrowserSettings,
    browser_commands: Option<Arc<CommandRegistry>>,
    browser_bus: Option<BrowserBus>,
    /// The setting `debugger.attachBrowser` (brief 0038).
    pub attach_browser: bool,
    /// The `browser` entries of the start being applied (brief 0038; they become their sessions').
    start_browsers: Vec<cmds::BrowserEntry>,
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
                native: native::NativeSetup::from_env(),
                cargo_options: cmds::CargoOptions::default(),
                shell_log_points: false,
                traces: Vec::new(),
                traces_base: 0,
                trace_hits: HashMap::new(),
                next_trace_hit: 1,
                trace_job: None,
                goto_error: None,
                console_partial_adapter: String::new(),
                launched: Arc::default(),
                interrupt: 0,
                interrupted_at: None,
                agent_stale: false,
                last_start: None,
                restart_pending: None,
                processes: None,
                background_runs: Vec::new(),
                attach_dialog: None,
                startup_dialog: None,
                menu: Arc::default(),
                session_id: 0,
                session_name: String::new(),
                others: Vec::new(),
                active: 0,
                next_session: 1,
                last_generation: 0,
                selected_at: None,
                shown_exec: None,
                compound: Vec::new(),
                started: None,
                restarting: false,
                start_cargo: cmds::CargoOptions::default(),
                start_browser: None,
                browser_launch: LaunchBrowserSettings::default(),
                browser_commands: None,
                browser_bus: None,
                attach_browser: true,
                start_browsers: Vec::new(),
                test_launch: None,
            },
            rx,
        )
    }

    // ----- Sessions (brief 0028) -----

    /// Exchange the current session's own fields with `slot`'s, keeping what every session shares in place.
    fn swap_slot(&mut self, slot: &mut Slot) {
        use std::mem::swap;
        swap(&mut self.session_id, &mut slot.id);
        swap(&mut self.session_name, &mut slot.name);
        swap(&mut self.model, &mut slot.model);
        swap(&mut self.client, &mut slot.client);
        swap(&mut self.run, &mut slot.run);
        swap(&mut self.caps, &mut slot.caps);
        swap(&mut self.pending, &mut slot.pending);
        swap(&mut self.run_to_cursor, &mut slot.run_to_cursor);
        swap(&mut self.exec, &mut slot.exec);
        swap(&mut self.console_partial, &mut slot.console_partial);
        swap(&mut self.early_breakpoints, &mut slot.early_breakpoints);
        swap(&mut self.pending_launch, &mut slot.pending_launch);
        swap(&mut self.pause_error, &mut slot.pause_error);
        swap(&mut self.counts, &mut slot.counts);
        swap(&mut self.cargo_options, &mut slot.cargo_options);
        swap(&mut self.trace_hits, &mut slot.trace_hits);
        swap(&mut self.trace_job, &mut slot.trace_job);
        swap(&mut self.goto_error, &mut slot.goto_error);
        swap(
            &mut self.console_partial_adapter,
            &mut slot.console_partial_adapter,
        );
        swap(&mut self.interrupt, &mut slot.interrupt);
        swap(&mut self.agent_stale, &mut slot.agent_stale);
        swap(&mut self.last_start, &mut slot.last_start);
        swap(&mut self.restart_pending, &mut slot.restart_pending);
        // What every session shares stays here (the slot's copies are stale).
        let (live, aside) = (&mut self.model, &mut slot.model);
        swap(&mut live.breakpoints, &mut aside.breakpoints);
        swap(&mut live.exceptions, &mut aside.exceptions);
        swap(&mut live.startup_project, &mut aside.startup_project);
        swap(&mut live.startup_projects, &mut aside.startup_projects);
        swap(&mut live.agents_default, &mut aside.agents_default);
        swap(&mut live.agents_next, &mut aside.agents_next);
        // The watch expressions are shared; their values are the session's.
        let names: Vec<String> = aside.watches.iter().map(|w| w.name.clone()).collect();
        let mut mine = std::mem::take(&mut live.watches);
        live.watches = names
            .iter()
            .map(|n| match mine.iter().position(|w| &w.name == n) {
                Some(i) => mine.remove(i),
                None => VarNode::watch(n),
            })
            .collect();
        live.breakpoints.switch_session(self.session_id);
    }

    /// Make session `id` the current one (the one the fields hold). False when there is no such session.
    pub fn enter(&mut self, id: u32) -> bool {
        if id == self.session_id {
            return true;
        }
        let Some(ix) = self.others.iter().position(|s| s.id == id) else {
            return false;
        };
        let mut slot = self.others.remove(ix);
        self.swap_slot(&mut slot);
        self.others.push(slot);
        true
    }

    /// Whether a session in this mode is live (listed, addressable).
    fn live_mode(mode: Mode) -> bool {
        mode != Mode::Design
    }

    /// The live sessions' ids, in the order they started.
    pub fn live_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .others
            .iter()
            .filter(|s| Self::live_mode(s.model.mode))
            .map(|s| s.id)
            .collect();
        if Self::live_mode(self.model.mode) && self.session_id > 0 {
            ids.push(self.session_id);
        }
        ids.sort_unstable();
        ids
    }

    /// The page a start of `project` debugs once it is up (brief 0038): the start's `browser` entry for it (taken,
    /// so each goes to one session; one without `project` goes to the first debugged project), else the implied one of
    /// a debugged web project whose page opens in the Web Browser window, with the setting debugger.attachBrowser on.
    fn take_browser_entry(
        &mut self,
        project: Option<&str>,
        debug: bool,
    ) -> Option<cmds::BrowserEntry> {
        if !debug {
            return None;
        }
        let matches = |e: &cmds::BrowserEntry| match (&e.project, project) {
            (None, _) => true,
            (Some(want), Some(p)) => {
                want == p || project_name(want) == project_name(p) || norm(want) == norm(p)
            }
            (Some(_), None) => false,
        };
        if let Some(i) = self.start_browsers.iter().position(matches) {
            return Some(self.start_browsers.remove(i));
        }
        let page_in_window = !matches!(
            self.start_browser,
            Some(cmds::BrowserChoice::External | cmds::BrowserChoice::None)
        );
        (self.attach_browser && page_in_window).then(cmds::BrowserEntry::default)
    }

    /// The folder of the project whose launch opened tab `tab` (brief 0037's `session.browser.tab`), live or ended.
    fn project_of_tab(&self, tab: &str) -> Option<PathBuf> {
        let of = |m: &DebugModel| {
            let s = m.session.as_ref()?;
            let b = s.browser.as_ref()?;
            (b.tab.as_deref() == Some(tab))
                .then(|| Path::new(&s.project).parent().map(Path::to_path_buf))
                .flatten()
        };
        of(&self.model).or_else(|| self.others.iter().rev().find_map(|s| of(&s.model)))
    }

    /// The live sessions with their adapter's family (brief 0038): which breakpoints each takes.
    fn live_families(&self) -> Vec<(u32, AdapterFamily)> {
        let mut rows: Vec<(u32, AdapterFamily)> = self
            .others
            .iter()
            .filter(|s| Self::live_mode(s.model.mode))
            .map(|s| (s.id, s.model.family()))
            .collect();
        if Self::live_mode(self.model.mode) && self.session_id > 0 {
            rows.push((self.session_id, self.model.family()));
        }
        rows.sort_unstable_by_key(|r| r.0);
        rows
    }

    /// The browser tabs whose page is stopped in the debugger (brief 0038): the tab of every browser session that
    /// is, or has a child session that is, in break mode.
    fn paused_tabs(&self) -> Vec<String> {
        let rows = self.sessions_info();
        let root_tab = |mut id: u32| {
            for _ in 0..rows.len() {
                let r = rows.iter().find(|r| r.id == id)?;
                match r.parent {
                    Some(p) => id = p,
                    None => return r.tab.clone(),
                }
            }
            None
        };
        rows.iter()
            .filter(|r| r.stopped.is_some())
            .filter_map(|r| root_tab(r.id))
            .collect()
    }

    /// Tell the browser which tabs are stopped in the debugger, for `eludite.browser.input` (brief 0038).
    fn publish_pauses(&self) {
        if let Some(bus) = &self.browser_bus {
            bus.set_debugger_pauses(self.paused_tabs());
        }
    }

    /// The live sessions whose `parent` is `id` (brief 0038), at any depth.
    fn children_of(&self, id: u32) -> Vec<u32> {
        let parent_of = |m: &DebugModel| m.session.as_ref().and_then(|s| s.parent);
        let all: Vec<(u32, Option<u32>, Mode)> = self
            .others
            .iter()
            .map(|s| (s.id, parent_of(&s.model), s.model.mode))
            .chain(std::iter::once((
                self.session_id,
                parent_of(&self.model),
                self.model.mode,
            )))
            .collect();
        let mut out = Vec::new();
        let mut frontier = vec![id];
        while let Some(p) = frontier.pop() {
            for (c, parent, mode) in &all {
                if *parent == Some(p) && Self::live_mode(*mode) && !out.contains(c) {
                    out.push(*c);
                    frontier.push(*c);
                }
            }
        }
        out.sort_unstable();
        out
    }

    /// The ids of the live sessions that have an adapter connected.
    fn connected_ids(&self) -> Vec<u32> {
        let mut ids: Vec<u32> = self
            .others
            .iter()
            .filter(|s| s.client.is_some())
            .map(|s| s.id)
            .collect();
        if self.client.is_some() {
            ids.push(self.session_id);
        }
        ids.sort_unstable();
        ids
    }

    /// Run `f` in every session with a connected adapter (a shared setting changed: every adapter gets it), then come
    /// back to the current one.
    fn each_connected(&mut self, mut f: impl FnMut(&mut Self)) {
        let back = self.session_id;
        for id in self.connected_ids() {
            if self.enter(id) {
                f(self);
            }
        }
        self.enter(back);
    }

    /// Session `id`'s mode, if it is known.
    fn mode_of(&self, id: u32) -> Option<Mode> {
        if id == self.session_id {
            return Some(self.model.mode);
        }
        self.others
            .iter()
            .find(|s| s.id == id)
            .map(|s| s.model.mode)
    }

    /// The session of generation `g`, if it is still known.
    fn session_of_generation(&self, g: u64) -> Option<u32> {
        if self.model.generation == g {
            return Some(self.session_id);
        }
        self.others
            .iter()
            .find(|s| s.model.generation == g)
            .map(|s| s.id)
    }

    /// The session a command names (brief 0028): `None` is the active one; an id must be a live session.
    pub fn resolve_session(&self, session: Option<u32>) -> Result<u32, CommandError> {
        let Some(id) = session else {
            return Ok(self.active);
        };
        let live = self.live_ids();
        if live.contains(&id) {
            return Ok(id);
        }
        let ids = if live.is_empty() {
            "no session is running".to_owned()
        } else {
            format!(
                "the live sessions are {}",
                live.iter()
                    .map(|i| i.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        };
        let what = if id > 0 && id < self.next_session {
            "has ended"
        } else {
            "does not exist"
        };
        Err(CommandError::Failed(format!(
            "session {id} {what}: {ids} (eludite.debug.sessions lists them)"
        )))
    }

    /// Begin a new session named `name` and make it the current one: the current session's model when it is not
    /// live, else a fresh one beside the others. Its generation is above every other session's.
    fn new_session(&mut self, name: String) -> u32 {
        let id = self.next_session;
        self.next_session += 1;
        if Self::live_mode(self.model.mode) {
            let mut slot = Slot {
                id,
                ..Slot::default()
            };
            slot.model.agents_allowed = self.model.agents_default;
            self.swap_slot(&mut slot);
            self.others.push(slot);
        } else {
            // The ended (or never used) model becomes the new session's.
            let old = self.session_id;
            self.session_id = id;
            self.model.breakpoints.switch_session(id);
            self.model.breakpoints.forget_session(old);
        }
        self.model.generation = self.last_generation;
        self.session_name = name;
        id
    }

    /// What a start resets: the session's console, and when it is the only session, the timings and the Output
    /// window's Debug source (a second session's start keeps the first one's lines).
    fn reset_for_start(&mut self) {
        let alone = self.live_ids().iter().all(|id| *id == self.session_id);
        if alone {
            self.timings = DebugTimings {
                start: Some(Instant::now()),
                ..DebugTimings::default()
            };
            self.output_queue.clear();
            self.output_clear = true;
        }
        self.model.console.clear();
        self.console_partial.clear();
    }

    /// The current session begins (again): a generation above every other session's (rule 4 of brief 0018 with
    /// several sessions: an answer is dropped unless its generation is its session's).
    fn begin(&mut self, mode: Mode, driver: &str) {
        self.model.generation = self.last_generation.max(self.model.generation);
        self.model.begin(mode, driver);
        self.last_generation = self.model.generation;
        if self.session_name.is_empty() {
            self.session_name = "the startup project".into();
        }
    }

    /// Keep at most [`ENDED_KEPT`] ended sessions aside (a restart waiting to start is kept).
    fn prune(&mut self) {
        let ended: Vec<u32> = self
            .others
            .iter()
            .filter(|s| !Self::live_mode(s.model.mode) && s.restart_pending.is_none())
            .map(|s| s.id)
            .collect();
        if ended.len() <= ENDED_KEPT {
            return;
        }
        let drop: Vec<u32> = ended[..ended.len() - ENDED_KEPT].to_vec();
        self.others.retain(|s| !drop.contains(&s.id));
        for id in drop {
            self.model.breakpoints.forget_session(id);
        }
    }

    /// The name a session shows: its project's (without the extension; a Cargo package by its folder), the attached
    /// process's, or what the start named.
    fn name_of(model: &DebugModel, fallback: &str) -> String {
        match model.session.as_ref() {
            Some(s) if s.attached => s.project.clone(),
            Some(s) => project_name(&s.project),
            None => fallback.to_owned(),
        }
    }

    /// The live sessions as `eludite.debug.sessions` lists them.
    pub fn sessions_info(&self) -> Vec<cmds::SessionInfo> {
        let row = |id: u32, name: &str, m: &DebugModel| cmds::SessionInfo {
            id,
            name: Self::name_of(m, name),
            mode: m.mode.as_str().into(),
            active: id == self.active,
            generation: m.generation,
            stop: m.stop,
            runtime: m.session.as_ref().and_then(|s| s.runtime.clone()),
            adapter: m.session.as_ref().and_then(|s| s.adapter.clone()),
            adapter_version: m.adapter_version.clone(),
            process_id: m.session.as_ref().and_then(|s| s.process_id),
            project: m.session.as_ref().map(|s| s.project.clone()),
            attached: m.attached(),
            parent: m.session.as_ref().and_then(|s| s.parent),
            tab: m.session.as_ref().and_then(|s| s.tab.clone()),
            url: m.session.as_ref().and_then(|s| s.url.clone()),
            agents_allowed: m.agents_allowed,
            stopped: (m.mode == Mode::Break)
                .then(|| m.stopped.as_ref().map(|s| s.reason.clone()))
                .flatten(),
        };
        let mut rows: Vec<cmds::SessionInfo> = self
            .others
            .iter()
            .filter(|s| Self::live_mode(s.model.mode))
            .map(|s| row(s.id, &s.name, &s.model))
            .collect();
        if Self::live_mode(self.model.mode) && self.session_id > 0 {
            rows.push(row(self.session_id, &self.session_name, &self.model));
        }
        rows.sort_by_key(|r| r.id);
        rows
    }

    /// What the status bar says of each live session (brief 0028): (id, name, mode, its state in parentheses).
    fn status_sessions(&self) -> Vec<(u32, String, &'static str, String)> {
        let part = |name: &str, m: &DebugModel| -> String {
            let driving = match (m.agent_driving(), m.agents_allowed) {
                (_, false) => ", agents not allowed",
                (true, true) => ", agent driving",
                (false, true) => "",
            };
            match m.mode {
                Mode::Design => "ended".to_owned(),
                Mode::Building => "building\u{2026}".to_owned(),
                Mode::Launching => "starting\u{2026}".to_owned(),
                Mode::Running => format!("running{driving}"),
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
                    let _ = name;
                    format!(
                        "break: {}{at}{driving}",
                        m.stopped.as_ref().map_or("", |s| s.reason.as_str())
                    )
                }
                Mode::Stopping => "stopping\u{2026}".to_owned(),
                Mode::RunningWithoutDebugging => "running without debugging".to_owned(),
            }
        };
        let mut rows: Vec<(u32, String, &'static str, String)> = self
            .others
            .iter()
            .filter(|s| Self::live_mode(s.model.mode))
            .map(|s| {
                let name = Self::name_of(&s.model, &s.name);
                (
                    s.id,
                    name.clone(),
                    s.model.mode.as_str(),
                    part(&name, &s.model),
                )
            })
            .collect();
        if Self::live_mode(self.model.mode) && self.session_id > 0 {
            let name = Self::name_of(&self.model, &self.session_name);
            rows.push((
                self.session_id,
                name.clone(),
                self.model.mode.as_str(),
                part(&name, &self.model),
            ));
        }
        rows.sort_by_key(|r| r.0);
        rows
    }

    /// `eludite.debug.state` for the current session: its model, its id, every session, and each breakpoint's
    /// binding per session.
    pub fn state(&self) -> cmds::DebugState {
        let mut s = self.model.state();
        s.breakpoints = self.model.breakpoints.rows_for(&self.live_families());
        if let Some(row) = s.session.as_mut() {
            row.id = Some(self.session_id).filter(|id| *id > 0);
        }
        s.sessions = self.sessions_info();
        s
    }

    /// `eludite.debug.toggle_breakpoint`'s compact answer (brief 0034): what the call did, the row of the breakpoint at
    /// `target` while it exists (its binding over the live sessions), whether a live session bound it, and how many
    /// breakpoints there are. The change was just sent to every live session's adapter: `pending` while one has not
    /// answered it, and once they have, `message` is the first refusal among them (brief 0036).
    fn breakpoint_answer(
        &self,
        action: cmds::BreakpointEdit,
        target: Option<&BreakpointTarget>,
    ) -> DebugOutput {
        let live = self.live_ids();
        let rows = self.model.breakpoints.rows_for(&self.live_families());
        let breakpoints_total = rows.len();
        let breakpoint = target.and_then(|t| {
            rows.into_iter().find(|r| match t {
                BreakpointTarget::Line(path, line) => {
                    r.function.is_none() && r.path.as_deref() == Some(path) && r.line == Some(*line)
                }
                BreakpointTarget::Function(name) => r.function.as_deref() == Some(name),
            })
        });
        let running = !live.is_empty();
        let message = breakpoint.as_ref().and_then(|r| {
            r.sessions
                .iter()
                .filter(|s| !s.verified)
                .filter_map(|s| s.message.clone())
                .find(|m| !cmds::pending_message(m))
        });
        DebugOutput::Breakpoint(Box::new(cmds::ToggleBreakpointOutput {
            action,
            verified: running && breakpoint.as_ref().is_some_and(|r| r.verified),
            pending: running && breakpoint.is_some() && target.is_some_and(|t| self.awaiting(t)),
            message,
            session: live.contains(&self.active).then_some(self.active),
            breakpoint,
            breakpoints_total,
        }))
    }

    /// The bindings of `run_until`'s temporary points (those not `trace`'s `job_points`) as they are now, before the
    /// stop or the session's end removes them (brief 0036).
    fn temporary_bindings(
        &self,
        job_points: &[(String, u32)],
    ) -> Vec<(String, u32, bool, Option<String>)> {
        self.model
            .breakpoints
            .all()
            .iter()
            .filter(|b| b.temporary && !job_points.contains(&(b.path.clone(), b.line)))
            .map(|b| (b.path.clone(), b.line, b.verified, b.message.clone()))
            .collect()
    }

    /// `run_until`'s points that its session's adapter had not bound by `summary` (brief 0036): a point the summary
    /// stopped on is bound; the others are read on their breakpoint while it exists (it timed out, or the line had a
    /// breakpoint of its own), else as they were when the stop or the session's end removed them.
    fn points_failed(
        &self,
        points: &[(String, u32)],
        summary: &StopSummary,
    ) -> Vec<cmds::FailedBreakpointRow> {
        let at = summary
            .stopped
            .as_ref()
            .and_then(|s| s.location.as_ref())
            .and_then(|l| {
                let path = normalize_path(Path::new(l.path.as_deref()?))
                    .to_string_lossy()
                    .into_owned();
                Some((path, l.line?))
            });
        points
            .iter()
            .filter(|(p, l)| at.as_ref().is_none_or(|(ap, al)| (ap, al) != (p, l)))
            .filter_map(|(p, l)| {
                let (verified, message) = self
                    .model
                    .breakpoints
                    .binding(p, *l)
                    .or_else(|| {
                        self.model
                            .removed_points
                            .iter()
                            .find(|r| (&r.0, r.1) == (p, *l))
                            .map(|r| (r.2, r.3.clone()))
                    })
                    .unwrap_or((false, None));
                (!verified).then(|| cmds::FailedBreakpointRow {
                    path: Some(p.clone()),
                    line: Some(*l),
                    function: None,
                    session: self.session_id,
                    message: message.unwrap_or_else(|| NOT_BOUND.into()),
                })
            })
            .collect()
    }

    /// `toggle_breakpoint`'s answer, and for an agent while a live session's adapter has not answered the change, the
    /// wait for that answer (at most [`BREAKPOINT_ANSWER_WAIT`]; the person's call never waits).
    fn breakpoint_reply(
        &self,
        edit: cmds::BreakpointEdit,
        target: Option<BreakpointTarget>,
        agent: bool,
    ) -> (DebugOutput, Option<Followup>) {
        let out = self.breakpoint_answer(edit, target.as_ref());
        let follow = match target {
            Some(t) if agent && self.awaiting(&t) => Some(Followup::Breakpoint {
                edit,
                target: t,
                wait: BREAKPOINT_ANSWER_WAIT,
            }),
            _ => None,
        };
        (out, follow)
    }

    /// Whether a live session's adapter has not answered the last change of `target` yet: a `setBreakpoints` of its
    /// file, or a `setFunctionBreakpoints`, still outstanding (brief 0036).
    fn awaiting(&self, target: &BreakpointTarget) -> bool {
        let outstanding = |pending: &HashMap<i64, Pending>| {
            pending.values().any(|p| match (p, target) {
                (Pending::SetBreakpoints { path, .. }, BreakpointTarget::Line(t, _)) => path == t,
                (Pending::SetFunctionBreakpoints { .. }, BreakpointTarget::Function(_)) => true,
                _ => false,
            })
        };
        outstanding(&self.pending) || self.others.iter().any(|s| outstanding(&s.pending))
    }

    /// The stop summary of the current session, with its id.
    fn summary(&self, budget: &Budget) -> StopSummary {
        let mut out = self.model.summary(budget);
        out.session = Some(self.session_id).filter(|id| *id > 0);
        out
    }

    /// Whether the adapter prints tracepoints (it has log points and the emulation is not forced).
    fn log_points(&self) -> bool {
        self.caps.supports_log_points && !self.shell_log_points
    }

    /// The session's adapter, for refusals: `` `coreclr` (netcoredbg --interpreter=vscode (stdio)) ``.
    fn adapter_name(&self) -> String {
        let id = self
            .model
            .capabilities
            .as_ref()
            .map(|c| c.adapter.clone())
            .unwrap_or_else(|| "unknown".into());
        match self.model.session.as_ref().and_then(|s| s.adapter.clone()) {
            Some(d) => format!("the debug adapter `{id}` ({d})"),
            None => format!("the debug adapter `{id}`"),
        }
    }

    /// Send `path`'s breakpoints to every session's adapter: the breakpoints are shared (brief 0028).
    fn send_breakpoints(&mut self, path: &str) {
        self.each_connected(|d| d.send_breakpoints_here(path));
    }

    /// Send the function breakpoints to every session's adapter that has them.
    fn send_function_breakpoints(&mut self) {
        self.each_connected(Self::send_function_breakpoints_here);
    }

    /// Send the exception settings to every session's adapter.
    fn send_exception_settings(&mut self) {
        self.each_connected(Self::send_exception_settings_here);
    }

    /// Send `path`'s breakpoints (and Run To Cursor's one-shot line) to the current session.
    fn send_breakpoints_here(&mut self, path: &str) {
        // Only the adapters that can bind the file get it (brief 0038).
        if self.client.is_none() || !self.model.family().takes(path) {
            return;
        }
        let extra = self
            .run_to_cursor
            .as_ref()
            .filter(|(p, _)| p == path)
            .map(|(_, l)| *l);
        let log_points = self.log_points();
        let (lines, sbps) = self.model.breakpoints.source_breakpoints(
            path,
            self.caps.supports_hit_conditional_breakpoints,
            log_points,
            extra,
        );
        let generation = self.generation();
        let _ = self.send(
            "setBreakpoints",
            dap_session::set_breakpoints_arguments(path, &sbps),
            Pending::SetBreakpoints {
                generation,
                path: path.to_owned(),
                lines,
            },
        );
    }

    /// Whether the session is a native (Cargo, lldb-dap) one (brief 0029).
    fn native_session(&self) -> bool {
        self.model
            .session
            .as_ref()
            .and_then(|s| s.runtime.as_deref())
            == Some("native")
    }

    /// Send the function breakpoints to the current session, if its adapter has them.
    fn send_function_breakpoints_here(&mut self) {
        if self.client.is_none() || !self.caps.supports_function_breakpoints {
            return;
        }
        let (names, mut bps) = self
            .model
            .breakpoints
            .function_breakpoints(self.caps.supports_hit_conditional_breakpoints);
        // `setFunctionBreakpoints` replaces the whole list: a native session's Rust panics row (brief 0029) goes in
        // it after the user's, whose answers `names` matches in order.
        if self.native_session() {
            bps = native::with_rust_panics(bps, self.model.exceptions.break_on_rust_panic);
        }
        let generation = self.generation();
        let _ = self.send(
            "setFunctionBreakpoints",
            dap_session::set_function_breakpoints_arguments(&bps),
            Pending::SetFunctionBreakpoints { generation, names },
        );
    }

    /// Send the exception settings to the current session (types as filter options where the adapter takes them).
    fn send_exception_settings_here(&mut self) {
        let family = self.model.family();
        if self.client.is_none() || !family.takes_exceptions() {
            return;
        }
        let args = exception_plan_for(family, &self.model.exceptions)
            .arguments(self.caps.supports_exception_filter_options);
        let _ = self.send("setExceptionBreakpoints", args, Pending::Other);
    }

    /// Change `name` in variables reference `reference` (a frame's scope or a value's members): `setVariable`, or
    /// `setExpression` in frame `frame_id` where the adapter has only that. Returns the request used.
    /// `scope`: `reference` is a frame's variables (its locals scope), so `name` is an expression in that frame.
    fn send_set_value(
        &mut self,
        reference: i64,
        scope: bool,
        frame_id: Option<i64>,
        name: &str,
        value: &str,
        reply: Option<oneshot::Sender<Result<SetVariableOutput, String>>>,
    ) -> Result<&'static str, CommandError> {
        let (request, args) = if self.caps.supports_set_variable {
            (
                "setVariable",
                json!({"variablesReference": reference, "name": name, "value": value}),
            )
        } else {
            // setExpression needs the member's expression: the model's row has it (or the name, at the top).
            let expression = if scope || reference == self.model.locals_reference {
                self.model
                    .locals
                    .iter()
                    .find(|v| v.name == name)
                    .and_then(|v| v.evaluate_name.clone())
                    .unwrap_or_else(|| name.to_owned())
            } else {
                find_reference(&self.model.locals, reference)
                    .or_else(|| find_reference(&self.model.watches, reference))
                    .and_then(|n| n.children.as_ref())
                    .and_then(|c| c.iter().find(|v| v.name == name))
                    .and_then(|v| v.evaluate_name.clone())
                    .ok_or_else(|| {
                        CommandError::Failed(format!(
                            "{} has only setExpression, which needs the member's expression: expand the value in the \
                             Locals window first, or pass its full expression as `name` in the frame",
                            self.adapter_name()
                        ))
                    })?
            };
            let mut args = json!({"expression": expression, "value": value});
            if let Some(f) = frame_id {
                args["frameId"] = json!(f);
            }
            ("setExpression", args)
        };
        let (generation, stop) = (self.generation(), self.model.stop);
        self.send(
            request,
            args,
            Pending::SetValue {
                generation,
                stop,
                reference,
                name: name.to_owned(),
                request,
                reply,
            },
        )?;
        Ok(request)
    }

    /// Record a line a tracepoint printed; returns its absolute index. A `trace` collecting it counts it, and at its
    /// `max_hits` (or `count`) disables its points so the debuggee runs on unhindered.
    fn record_trace(&mut self, record: TraceRecord) -> usize {
        if self.traces.len() >= TRACE_RECORDS {
            let drop = TRACE_RECORDS / 2;
            self.traces.drain(..drop);
            self.traces_base += drop;
        }
        let key = (record.path.clone(), record.line);
        let generation = record.generation;
        self.traces.push(record);
        let index = self.traces_base + self.traces.len() - 1;
        let mut disable = None;
        if let Some(job) = self.trace_job.as_mut()
            && !job.done
            && job.generation == generation
            && job.points.contains(&key)
        {
            job.hits += 1;
            if job.hits >= job.max_hits {
                job.truncated = true;
                job.done = true;
            } else if job.count.is_some_and(|c| job.hits >= c) {
                job.done = true;
            }
            if job.done {
                disable = Some(job.points.clone());
            }
        }
        if let Some(points) = disable {
            let mut files: Vec<String> = Vec::new();
            for (path, line) in points {
                if let Some(b) = self.model.breakpoints.at_mut(&path, line) {
                    b.enabled = false;
                }
                if !files.contains(&path) {
                    files.push(path);
                }
            }
            for f in files {
                self.send_breakpoints(&f);
            }
        }
        index
    }

    /// The adapter printed console text: the lines that a tracepoint it prints would print are that tracepoint's.
    fn adapter_log_text(&mut self, text: &str) {
        let log_points = self.log_points();
        if !log_points {
            return;
        }
        let mut buf = std::mem::take(&mut self.console_partial_adapter);
        buf.push_str(&text.replace('\r', ""));
        let mut parts: Vec<&str> = buf.split('\n').collect();
        let rest = parts.pop().unwrap_or_default().to_owned();
        let lines: Vec<String> = parts.into_iter().map(str::to_owned).collect();
        self.console_partial_adapter = rest;
        for line in lines {
            let found = self
                .model
                .breakpoints
                .iter_mut()
                .find(|b| {
                    b.enabled
                        && b.adapter_logs(log_points)
                        && b.log_message
                            .as_deref()
                            .is_some_and(|m| state::message_matches(m, &line))
                })
                .map(|b| {
                    b.hits += 1;
                    (b.path.clone(), b.line, b.hits)
                });
            if let Some((path, l, hit)) = found {
                self.model
                    .output_mut(OutputKind::Debug)
                    .push_line(line.clone(), None);
                let generation = self.generation();
                self.record_trace(TraceRecord {
                    generation,
                    path,
                    line: l,
                    hit,
                    text: line,
                    at: Instant::now(),
                    emulated: false,
                    overhead: None,
                });
            }
        }
    }

    /// An emulated tracepoint was hit: ask for its expressions' values (or print at once), never showing the stop.
    #[allow(clippy::too_many_arguments)]
    fn start_trace_hit(
        &mut self,
        path: String,
        line: u32,
        hit: u32,
        message: &str,
        thread: i64,
        frames: &[Frame],
        at: Instant,
    ) {
        let generation = self.generation();
        let id = self.next_trace_hit;
        self.next_trace_hit += 1;
        let segments = parse_message(message);
        let mut values = vec![None; segments.len()];
        let mut waiting = 0;
        let frame_id = frames.first().map(|f| f.id);
        for (ix, seg) in segments.iter().enumerate() {
            let Segment::Expression(e) = seg else {
                continue;
            };
            let sent = match frame_id {
                Some(f) => self
                    .send(
                        "evaluate",
                        json!({"expression": e, "frameId": f, "context": "watch"}),
                        Pending::TraceEval {
                            generation,
                            hit: id,
                            ix,
                        },
                    )
                    .map_err(|e| e.to_string()),
                None => Err("no frame".to_owned()),
            };
            match sent {
                Ok(_) => waiting += 1,
                Err(m) => values[ix] = Some(format!("{{{e}: {m}}}")),
            }
        }
        self.trace_hits.insert(
            id,
            PendingHit {
                generation,
                path,
                line,
                hit,
                thread,
                function: frames
                    .first()
                    .map(|f| f.row.name.clone())
                    .unwrap_or_default(),
                caller: frames.get(1).map(|f| f.row.name.clone()),
                segments,
                values,
                waiting,
                at,
            },
        );
        if waiting == 0 {
            self.finish_trace_hit(id);
        }
    }

    /// An `{expression}` of an emulated hit was evaluated (or failed: written in place).
    fn trace_value(&mut self, id: u64, ix: usize, result: Result<Value, String>) {
        let Some(h) = self.trace_hits.get_mut(&id) else {
            return;
        };
        let expression = match h.segments.get(ix) {
            Some(Segment::Expression(e)) => e.clone(),
            _ => return,
        };
        let text = match result
            .and_then(|b| serde_json::from_value::<EvaluateResponse>(b).map_err(|e| e.to_string()))
        {
            Ok(e) => cmds::null_spelling(e.result),
            Err(m) => format!("{{{expression}: {m}}}"),
        };
        h.values[ix] = Some(text);
        h.waiting = h.waiting.saturating_sub(1);
        if h.waiting == 0 {
            self.finish_trace_hit(id);
        }
    }

    /// Every value of an emulated hit is in: print the line, record it, and resume.
    fn finish_trace_hit(&mut self, id: u64) {
        let Some(h) = self.trace_hits.remove(&id) else {
            return;
        };
        if h.generation != self.generation() {
            return;
        }
        let tname = self
            .model
            .threads
            .iter()
            .find(|t| t.id == h.thread)
            .map(|t| t.name.clone())
            .filter(|n| !n.is_empty())
            .unwrap_or_else(|| "<No Name>".into());
        let mut text = String::new();
        for (seg, value) in h.segments.iter().zip(&h.values) {
            match seg {
                Segment::Text(t) => text.push_str(t),
                Segment::Expression(e) => {
                    text.push_str(value.as_deref().unwrap_or(&format!("{{{e}: no value}}")))
                }
                Segment::Special("$FUNCTION") => text.push_str(&h.function),
                Segment::Special("$CALLER") => text.push_str(h.caller.as_deref().unwrap_or("")),
                Segment::Special("$TID") => text.push_str(&h.thread.to_string()),
                Segment::Special("$TNAME") => text.push_str(&tname),
                Segment::Special(other) => text.push_str(other),
            }
        }
        self.console_line(text.clone());
        let record = self.record_trace(TraceRecord {
            generation: h.generation,
            path: h.path,
            line: h.line,
            hit: h.hit,
            text,
            at: Instant::now(),
            emulated: true,
            overhead: None,
        });
        let generation = h.generation;
        let _ = self.send(
            "continue",
            json!({ "threadId": h.thread }),
            Pending::TraceResume {
                generation,
                record,
                at: h.at,
            },
        );
    }

    /// The trace's state: why it should end now, if it should.
    fn trace_ended(&self) -> Option<&'static str> {
        let Some(job) = &self.trace_job else {
            return Some("terminated");
        };
        let m = &self.model;
        if job.done {
            Some("hits")
        } else if m.generation != job.generation || m.mode == Mode::Design {
            Some("terminated")
        } else if m.mode == Mode::Break && m.settled() && m.stop > job.stop {
            Some("stopped")
        } else {
            None
        }
    }

    /// End `trace`: its lines and points, the breakpoints it replaced put back.
    fn finish_trace(&mut self, stopped_by: &'static str) -> TraceOutput {
        let Some(job) = self.trace_job.take() else {
            return TraceOutput {
                stopped_by: stopped_by.into(),
                ..Default::default()
            };
        };
        let first = job.base.saturating_sub(self.traces_base);
        let mine: Vec<&TraceRecord> = self
            .traces
            .iter()
            .skip(first)
            .filter(|r| {
                r.generation == job.generation && job.points.contains(&(r.path.clone(), r.line))
            })
            .take(job.max_hits)
            .collect();
        let points: Vec<TracePointRow> = job
            .points
            .iter()
            .enumerate()
            .map(|(i, (path, line))| {
                let b = self.model.breakpoints.at(path, *line);
                let (verified, message) = match job.bound.as_ref().and_then(|v| v.get(i)) {
                    Some((v, m)) => (*v, m.clone()),
                    None => (
                        b.is_some_and(|b| b.verified),
                        b.and_then(|b| b.message.clone()),
                    ),
                };
                let hits = mine
                    .iter()
                    .filter(|r| (&r.path, r.line) == (path, *line))
                    .count() as u32;
                TracePointRow {
                    path: path.clone(),
                    line: *line,
                    hits,
                    verified: verified || hits > 0,
                    message: message.filter(|_| hits == 0),
                }
            })
            .collect();
        let overheads: Vec<Duration> = mine.iter().filter_map(|r| r.overhead).collect();
        let emulated = mine.iter().any(|r| r.emulated);
        let lines: Vec<TraceLine> = mine
            .iter()
            .enumerate()
            .map(|(i, r)| TraceLine {
                seq: i as u64,
                path: r.path.clone(),
                line: r.line,
                hit: r.hit.max(1),
                time_ms: r.at.saturating_duration_since(job.started).as_secs_f64() * 1e3,
                text: cmds::cut_value(&r.text, cmds::MAX_TRACE_TEXT).0,
            })
            .collect();
        // The points that never bound, with the adapter's reason (brief 0036).
        let points_failed = points
            .iter()
            .filter(|p| !p.verified)
            .map(|p| cmds::FailedBreakpointRow {
                path: Some(p.path.clone()),
                line: Some(p.line),
                function: None,
                session: self.session_id,
                message: p.message.clone().unwrap_or_else(|| NOT_BOUND.into()),
            })
            .collect();
        let out = TraceOutput {
            hits: lines.len() as u64,
            lines,
            points_failed,
            truncated: job.truncated,
            stopped_by: stopped_by.into(),
            summary: None,
            exit_code: (stopped_by == "terminated")
                .then_some(self.model.exit_code)
                .flatten(),
            points,
            overhead_ms_per_hit: (!overheads.is_empty()).then(|| {
                overheads.iter().map(Duration::as_secs_f64).sum::<f64>() * 1e3
                    / overheads.len() as f64
            }),
            emulated,
            generation: Some(job.generation),
        };
        // The points go; what they replaced comes back.
        let mut files: Vec<String> = Vec::new();
        for ((path, line), saved) in job.points.into_iter().zip(job.saved) {
            if self
                .model
                .breakpoints
                .at(&path, line)
                .is_some_and(|b| b.temporary)
            {
                self.model.breakpoints.delete(&path, line);
            }
            if let Some(b) = saved {
                self.model.breakpoints.put(b);
            }
            if !files.contains(&path) {
                files.push(path);
            }
        }
        for f in files {
            self.send_breakpoints(&f);
        }
        out
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

    /// The setting `debugger.lldbDapPath` (or `ELUDITE_LLDB_DAP`): where the next native session looks for lldb-dap
    /// first (brief 0029).
    pub fn set_lldb_dap_path(&mut self, path: Option<PathBuf>) {
        self.native.lldb.configured = path;
    }

    /// The setting `debugger.rustFormatters`: whether the next native session loads the Rust formatters.
    pub fn set_rust_formatters(&mut self, on: bool) {
        self.native.formatters = on;
    }

    /// The setting `debugger.allowAgentsByDefault`: what each new session starts with.
    pub fn set_agents_default(&mut self, on: bool) {
        self.model.agents_default = on;
    }

    /// The setting `debugger.jsDebugPath` (brief 0038).
    pub fn set_js_debug_path(&mut self, path: Option<PathBuf>) {
        self.setup.js.search.configured = path;
    }

    /// The setting `debugger.nodePath` (brief 0038).
    pub fn set_node_path(&mut self, path: Option<PathBuf>) {
        self.setup.js.node.configured = path;
    }

    /// The setting `debugger.attachBrowser` (brief 0038): a web project's start debugs its page too.
    pub fn set_attach_browser(&mut self, on: bool) {
        self.attach_browser = on;
    }

    /// The settings `browser.useBuiltIn` and `debugger.launchBrowser` (brief 0037): the next start's browser step.
    pub fn set_launch_browser(&mut self, use_built_in: bool, launch_browser: bool) {
        self.browser_launch.use_built_in = use_built_in;
        self.browser_launch.launch_browser = launch_browser;
        self.menu
            .use_built_in
            .store(use_built_in, std::sync::atomic::Ordering::Relaxed);
    }

    /// The bus and the browser the launch's browser step opens pages through (brief 0037).
    pub fn set_browser(&mut self, commands: Arc<CommandRegistry>, bus: BrowserBus) {
        *self.menu.browser.lock().unwrap_or_else(|e| e.into_inner()) = Some(bus.clone());
        self.browser_commands = Some(commands);
        self.browser_bus = Some(bus);
    }

    /// The adapter restarted the program in the same session (DAP `restart`): its page opens again once the server
    /// answers, in the same tab (brief 0037), on a `debug-browser` thread.
    fn rerun_browser_step(&mut self) {
        let (Some(plan), Some(row), Some(watch)) = (
            self.model.browser_plan.clone(),
            self.model.session.as_ref().and_then(|s| s.browser.clone()),
            self.model.browser_watch.clone(),
        ) else {
            return;
        };
        watch.rearm();
        let embedded = row.engine == "embedded";
        let mut step = self.browser_step(watch, self.model.browser_reuse.clone());
        step.choice = Some(if embedded {
            cmds::BrowserChoice::BuiltIn
        } else {
            cmds::BrowserChoice::External
        });
        // The page as the launch resolved it: the https rule is not asked again.
        let planned = PlannedBrowser {
            plan: launch::BrowserLaunch {
                url: Some(row.url.clone()).filter(|u| !u.is_empty()).or(plan.url),
                http_url: None,
                ..plan
            },
            embedded,
            note: None,
        };
        if let Some(s) = self.model.session.as_mut() {
            s.browser = Some(browser_row(
                &planned,
                planned.plan.url.as_deref(),
                step.reuse_tab.clone(),
                "waiting",
                None,
            ));
        }
        let name = Self::name_of(&self.model, &self.session_name);
        let (generation, tx) = (self.generation(), self.tx.clone());
        let _ = std::thread::Builder::new()
            .name("debug-browser".into())
            .spawn(move || run_browser_step(&step, planned, &name, generation, &tx));
    }

    /// The browser step of the current session's next launch (or of a restart through the adapter).
    fn browser_step(&self, watch: Arc<ServerWatch>, reuse_tab: Option<String>) -> BrowserStep {
        BrowserStep {
            choice: self.model.browser_choice,
            settings: self.browser_launch.clone(),
            reuse_tab,
            watch,
            dotnet: self.setup.dotnet.clone(),
            session: self.session_id,
            commands: self.browser_commands.clone(),
            bus: self.browser_bus.clone(),
        }
    }

    /// Which processes this shell started, for the escalation hooks (brief 0027): by id, the roots and their
    /// descendants (a parent chain walk); by name, a scan of the process table. Called on the hook's thread.
    pub fn launched_processes(&self) -> eludite_commands::policy::LaunchedProcesses {
        let roots = self.launched.clone();
        eludite_commands::policy::LaunchedProcesses::new(move |pid, name| {
            let roots = roots.lock().unwrap_or_else(|e| e.into_inner()).clone();
            if roots.is_empty() {
                return false;
            }
            match (pid, name) {
                (Some(pid), _) => processes::is_launched(pid, &roots),
                (None, Some(name)) => processes::list().is_ok_and(|all| {
                    let set = processes::launched_set(&roots, &all);
                    let want = name.trim().trim_end_matches(".exe").to_lowercase();
                    all.iter().any(|p| {
                        set.contains(&p.pid)
                            && p.name.trim_end_matches(".exe").to_lowercase() == want
                    })
                }),
                (None, None) => false,
            }
        })
    }

    /// Proposal 0001 rule 5: after an interrupted wait, an agent's driving command is stale until it reads the state
    /// (or quotes the current stop).
    fn stale_check(&mut self, request: &DebugRequest) -> Result<(), CommandError> {
        if !self.agent_stale {
            return Ok(());
        }
        match request {
            DebugRequest::Snapshot { .. } | DebugRequest::State | DebugRequest::Wait { .. } => {
                self.agent_stale = false;
                Ok(())
            }
            r if r.drives() => {
                let m = &self.model;
                if quoted_stop(r) == Some(m.stop) && m.mode == Mode::Break {
                    self.agent_stale = false;
                    return Ok(());
                }
                Err(CommandError::Failed(format!(
                    "stale: the person drove the session while your command waited (now stop {}, {}, generation \
                     {}); read eludite.debug.snapshot (or state, or wait) and decide again, or quote `stop: {}`",
                    m.stop,
                    m.mode.as_str(),
                    m.generation,
                    m.stop
                )))
            }
            _ => Ok(()),
        }
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

    /// Where per-solution state is kept (brief 0049 keeps the configuration selection beside the breakpoints).
    pub(super) fn store_dir(&self) -> Option<PathBuf> {
        self.setup.store_dir.clone()
    }
}

/// A project's name as the sessions show it: the project file without its extension, a Cargo package by its folder.
fn project_name(project: &str) -> String {
    let p = Path::new(project);
    if p.file_name().is_some_and(|n| n == "Cargo.toml") {
        return p
            .parent()
            .and_then(|d| d.file_name())
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| project.to_owned());
    }
    p.file_stem()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| project.to_owned())
}

fn driver_of(caller: &Caller) -> String {
    match caller {
        Caller::User => "user".into(),
        Caller::Agent { agent, .. } => format!("agent:{agent}"),
        Caller::Session { session, .. } => format!("session:{session}"),
    }
}

/// A session's mode as the Call Stack and Threads windows' selector says it: `running without debugging` for the
/// state's `running_without_debugging` (a Ctrl+F5 program listed beside a browser session, brief 0038).
fn mode_words(mode: &str) -> String {
    mode.replace('_', " ")
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

/// What the toolbar chose for a launch (brief 0049): the configuration per project (the solution configuration's
/// mapping), the Target Framework list's framework, the Debug toolbar's launch profile, and `eludite.debug.start`'s
/// own `framework`.
#[derive(Debug, Clone, Default)]
pub(super) struct LaunchChoice {
    /// The solution configuration, for a project the mapping does not name.
    pub configuration: String,
    pub configurations: BTreeMap<String, String>,
    pub frameworks: BTreeMap<String, String>,
    pub profiles: BTreeMap<String, String>,
    pub framework: Option<String>,
}

impl LaunchChoice {
    fn lookup<'a>(map: &'a BTreeMap<String, String>, project: &Path) -> Option<&'a String> {
        let wanted = normalize_path(project);
        map.iter()
            .find(|(k, _)| normalize_path(Path::new(k)) == wanted)
            .map(|(_, v)| v)
    }

    /// The configuration, framework and default profile of `project`.
    fn for_project(&self, project: &Path) -> (String, Option<String>, Option<String>) {
        (
            Self::lookup(&self.configurations, project)
                .cloned()
                .unwrap_or_else(|| self.configuration.clone()),
            self.framework
                .clone()
                .or_else(|| Self::lookup(&self.frameworks, project).cloned()),
            Self::lookup(&self.profiles, project).cloned(),
        )
    }
}

/// Resolve the project to run and its launch configuration (on the launch thread: it reads files).
pub(super) fn resolve_launch(
    hint: Option<&str>,
    profile: Option<&str>,
    projects: &[PathBuf],
    solution_dir: Option<&Path>,
    startup: Option<&Path>,
    choice: &LaunchChoice,
) -> Result<launch::LaunchConfig, String> {
    let project = resolve_project(hint, projects, solution_dir, startup)?;
    let (configuration, framework, selected) = choice.for_project(&project);
    // The Debug toolbar's profile, while the file still has it.
    let selected = selected.filter(|s| {
        project
            .parent()
            .and_then(|d| launch::read_launch_settings(d).ok())
            .is_some_and(|ps| ps.iter().any(|p| &p.name == s))
    });
    launch::launch_config_in(
        &project,
        profile.or(selected.as_deref()),
        &configuration,
        framework.as_deref(),
    )
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

/// The breakpoint a `toggle_breakpoint` call edited (brief 0034).
pub enum BreakpointTarget {
    Line(String, u32),
    Function(String),
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
    functions: Vec<eludite_dap::types::FunctionBreakpoint>,
    exceptions: state::ExceptionPlan,
    setup: DebugSetup,
    /// A Cargo package's start (brief 0029).
    native: native::NativeJob,
    /// The Test Explorer's launch configuration instead of the project's (brief 0035).
    config: Option<launch::LaunchConfig>,
    /// A web project's page (brief 0037).
    browser: BrowserStep,
    /// The configuration, framework and profile the toolbar chose (brief 0049).
    choice: LaunchChoice,
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
        functions,
        exceptions,
        setup,
        native,
        config: test_config,
        browser,
        choice,
        tx,
    } = job;
    let fail = |message: String| {
        let _ = tx.unbounded_send(DebugMsg::LaunchFailed {
            generation,
            message,
        });
    };
    // A Cargo package (brief 0029), else a .NET project.
    let dotnet_default = native::dotnet_default(hint.as_deref(), startup.as_deref(), &projects);
    let package = native::resolve_cargo(
        hint.as_deref(),
        startup.as_deref(),
        dotnet_default,
        native.cargo.as_ref(),
    );
    let mut native_launch = None;
    let package = if test_config.is_some() { None } else { package };
    let config = match (&package, test_config) {
        (_, Some(c)) => c,
        (Some(p), None) => {
            let console = |line: &str| {
                let _ = tx.unbounded_send(DebugMsg::Client {
                    generation,
                    event: native::console_event(line),
                });
            };
            match native::prepare(&native, p, debug, console) {
                Ok(n) => native_launch.insert(n).config.clone(),
                Err(e) => return fail(e),
            }
        }
        (None, None) if native.options.is_set() => {
            return fail(
                "`target`, `test` and `args` apply to Cargo packages; a .NET project's arguments come from its \
                 launchSettings.json profile"
                    .into(),
            );
        }
        (None, None) => match resolve_launch(
            hint.as_deref(),
            profile.as_deref(),
            &projects,
            solution_dir.as_deref(),
            startup.as_deref(),
            &choice,
        ) {
            Ok(c) => c,
            Err(e) => return fail(e),
        },
    };
    let platform = setup.platform;
    let mut session = SessionRow {
        id: None,
        project: config.project.to_string_lossy().into_owned(),
        program: config.program.to_string_lossy().into_owned(),
        args: config.args.clone(),
        cwd: config.cwd.to_string_lossy().into_owned(),
        profile: config.profile.clone(),
        debug,
        adapter: None,
        runtime: Some(launch::runtime_name(config.kind, platform).to_owned()),
        process_id: None,
        attached: false,
        parent: None,
        tab: None,
        url: None,
        browser: None,
    };
    // Only the breakpoints the adapter can bind (brief 0038): a .NET program's `.cs`, a Cargo package's `.rs`.
    let mut breakpoints = breakpoints;
    let family = AdapterFamily::of_runtime(session.runtime.as_deref(), false);
    breakpoints.retain(|(path, _)| family.takes(path));
    // A web project's page (brief 0037): planned here (it reads the launch profile), opened once the program runs.
    let planned = plan_browser(&browser, &config);
    let name = project_name(&config.project.to_string_lossy());
    if let Some(p) = &planned {
        session.browser = Some(browser_row(
            p,
            p.plan.url.as_deref(),
            browser.reuse_tab.clone(),
            "waiting",
            p.note.clone(),
        ));
    }
    let browser_plan = planned.as_ref().map(|p| Box::new(p.plan.clone()));
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
            session: Box::new(session),
            run: Some(handle.clone()),
            plan: browser_plan,
        });
        let mut readers = Vec::new();
        for (s, stream) in streams.into_iter().zip(["stdout", "stderr"]) {
            let Some(s) = s else { continue };
            let tx = tx.clone();
            let watch = browser.watch.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(s).lines() {
                    let Ok(line) = line else { break };
                    let text = format!("{line}\n");
                    // Kestrel's listening line (brief 0037).
                    watch.feed(&text);
                    let _ = tx.unbounded_send(DebugMsg::Output {
                        generation,
                        text,
                        stream,
                    });
                }
            }));
        }
        if let Some(p) = planned {
            run_browser_step(&browser, p, &name, generation, &tx);
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
    let (adapter_id, arguments) = match (kind, &mono, &native_launch) {
        (AdapterKind::Mono, Some(m), _) => ("mono", config.mono_arguments(&m.mono)),
        (AdapterKind::Lldb, _, Some(n)) => ("lldb", n.arguments()),
        _ => ("coreclr", config.netcoredbg_arguments()),
    };
    // `mono --version`, read once here: the adapter's description names the Mono that runs it.
    let mono_version = mono
        .as_ref()
        .map(|m| m.version().unwrap_or_else(|| "(unknown version)".into()));
    let reached = match kind {
        AdapterKind::Lldb => Some(native::connect(&native, platform, setup.connect.as_ref())),
        _ => None,
    };
    let (connection, adapter) = match (reached, &setup.connect) {
        (Some(Ok(c)), _) => c,
        (Some(Err(e)), _) => return fail(e),
        (None, Some(connect)) => match connect() {
            Ok(c) => {
                let d = match &mono_version {
                    Some(v) => format!("eludite-dbg-mono under mono {v} ({})", c.description),
                    None => c.description.clone(),
                };
                (c, d)
            }
            Err(e) => return fail(format!("cannot reach the debug adapter: {e}")),
        },
        (None, None) => match (kind, &mono) {
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
        session: Box::new(session),
        run: None,
        plan: browser_plan,
    });
    let sink_tx = tx.clone();
    // lldb-dap's pause stops and standard library frames, as the shell expects them (brief 0029).
    let rust_src = native_launch.as_ref().map(|n| n.rust_src.clone());
    let watch = browser.watch.clone();
    let client = DapClient::start(
        connection,
        Arc::new(move |event| {
            let event = match &rust_src {
                Some(src) => native::adapt(event, src.as_deref()),
                None => event,
            };
            // The program's output, read for Kestrel's listening line (brief 0037).
            if let ClientEvent::Event(eludite_dap::types::Event::Output(o)) = &event
                && matches!(
                    o.category.as_deref(),
                    None | Some("stdout") | Some("stderr") | Some("console")
                )
            {
                watch.feed(&o.output);
            }
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
        exception_filters: exceptions.filters,
        exception_options: exceptions.options,
        // One list (setFunctionBreakpoints replaces them all): the user's, then a native session's Rust panics row.
        function_breakpoints: match kind {
            AdapterKind::Lldb => native::with_rust_panics(functions, native.rust_panics),
            _ => functions,
        },
    };
    let mut result =
        dap_session::start(&client, &plan, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    if let (AdapterKind::Lldb, Ok(started)) = (kind, &mut result) {
        native::adapt_capabilities(&mut started.capabilities);
    }
    // The tests' fake adapter says so in its connection's description.
    let adapter_id = if client.description().starts_with("fake adapter") {
        "fake".to_owned()
    } else {
        plan.adapter_id.clone()
    };
    let started = result.is_ok();
    let _ = tx.unbounded_send(DebugMsg::Started {
        generation,
        result,
        adapter_id,
    });
    // The program runs: open its page once its server answers (brief 0037).
    if let (true, Some(p)) = (started, planned) {
        run_browser_step(&browser, p, &name, generation, &tx);
    }
}

// ----- The launch's browser step (brief 0037) -----

/// How long the launch waits for a web project's server before saying the page could not be opened.
pub const BROWSER_READY_TIMEOUT: Duration = Duration::from_secs(30);
/// The longest one probe of the server's url may take.
const BROWSER_PROBE_CAP: Duration = Duration::from_secs(1);

/// How the launch's browser step behaves (brief 0037): the settings, and what tests change.
#[derive(Debug, Clone)]
pub struct LaunchBrowserSettings {
    /// `browser.useBuiltIn` (Debug > Open in Web Browser Window).
    pub use_built_in: bool,
    /// `debugger.launchBrowser`.
    pub launch_browser: bool,
    /// How long the server has to come up ([`BROWSER_READY_TIMEOUT`]).
    pub timeout: Duration,
    /// `Some`: the answer of `dotnet dev-certs https --check` (tests); `None`: run it.
    pub dev_cert: Option<bool>,
}

impl Default for LaunchBrowserSettings {
    fn default() -> Self {
        Self {
            use_built_in: true,
            launch_browser: true,
            timeout: BROWSER_READY_TIMEOUT,
            dev_cert: None,
        }
    }
}

/// What the launch thread needs to open a web project's page (brief 0037).
#[derive(Clone)]
struct BrowserStep {
    /// The start's `browser`; `None`: the launch profile's `launchBrowser` and the settings decide.
    choice: Option<cmds::BrowserChoice>,
    settings: LaunchBrowserSettings,
    /// The tab a restart navigates.
    reuse_tab: Option<String>,
    watch: Arc<ServerWatch>,
    dotnet: String,
    session: u32,
    commands: Option<Arc<CommandRegistry>>,
    bus: Option<BrowserBus>,
}

/// The page a launch will open: the profile's plan, where, and why not the Web Browser window when it was asked for.
#[derive(Debug, Clone)]
struct PlannedBrowser {
    plan: launch::BrowserLaunch,
    embedded: bool,
    note: Option<String>,
}

impl PlannedBrowser {
    fn engine(&self) -> &'static str {
        if self.embedded { "embedded" } else { "system" }
    }
}

/// Whether and where `config`'s start opens a page (on the launch thread: it reads the launch profile and looks for
/// the embedded engine). `None`: nothing to open (a console program, a Cargo package, `browser: none`, a profile
/// without `launchBrowser` when the start does not ask, or the setting debugger.launchBrowser off).
fn plan_browser(step: &BrowserStep, config: &launch::LaunchConfig) -> Option<PlannedBrowser> {
    let plan = launch::browser_launch(config)?;
    let choice = match step.choice {
        Some(c) => c,
        None if !plan.requested || !step.settings.launch_browser => return None,
        None if step.settings.use_built_in => cmds::BrowserChoice::BuiltIn,
        None => cmds::BrowserChoice::External,
    };
    match choice {
        cmds::BrowserChoice::None => None,
        cmds::BrowserChoice::External => Some(PlannedBrowser {
            plan,
            embedded: false,
            note: None,
        }),
        cmds::BrowserChoice::BuiltIn => {
            let status = step.bus.as_ref().map(BrowserBus::status);
            let embedded = status.as_ref().is_some_and(|s| s.embedded);
            let note = (!embedded).then(|| {
                format!(
                    "The Web Browser window cannot show the page ({}), so it opens in the system browser.",
                    status
                        .and_then(|s| s.message)
                        .unwrap_or_else(|| "its engine was not found".into())
                        .trim_end_matches('.')
                )
            });
            Some(PlannedBrowser {
                plan,
                embedded,
                note,
            })
        }
    }
}

/// The session's page as the state shows it.
fn browser_row(
    planned: &PlannedBrowser,
    url: Option<&str>,
    tab: Option<String>,
    state: &str,
    message: Option<String>,
) -> cmds::SessionBrowser {
    cmds::SessionBrowser {
        tab: tab.filter(|_| planned.embedded),
        url: url.unwrap_or_default().to_owned(),
        engine: planned.engine().into(),
        state: state.into(),
        message,
    }
}

/// The launch's browser step (brief 0037), on the launch thread once the program runs: the https rule (the
/// development certificate, checked once), then the wait for the server (Kestrel's listening line in the program's
/// output, or the url answering, each probe capped at a second, for up to the timeout), then the page: in the Web
/// Browser window through the bus as the session (`tab_open`, or on a restart `navigate` of the same tab), or in the
/// system browser (`open_external`). Every outcome reaches the UI thread as [`DebugMsg::Browser`]; the UI never
/// waits for any of it.
fn run_browser_step(
    step: &BrowserStep,
    planned: PlannedBrowser,
    name: &str,
    generation: u64,
    tx: &UnboundedSender<DebugMsg>,
) {
    let send = |browser: cmds::SessionBrowser, line: Option<String>, latency: Option<Duration>| {
        let _ = tx.unbounded_send(DebugMsg::Browser {
            generation,
            browser,
            line,
            latency,
        });
    };
    let plan = &planned.plan;
    let (url, cert_note) = launch::https_choice(plan, || {
        step.settings
            .dev_cert
            .unwrap_or_else(|| launch::dev_cert_found(&step.dotnet))
    });
    let note = [planned.note.clone(), cert_note.clone()]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(" ");
    let note = (!note.is_empty()).then_some(note);
    if url != plan.url || note.is_some() {
        send(
            browser_row(
                &planned,
                url.as_deref(),
                step.reuse_tab.clone(),
                "waiting",
                note.clone(),
            ),
            note.clone(),
            None,
        );
    }
    let waited = Instant::now();
    let (url, heard) = match step.watch.wait_up(
        url.as_deref(),
        step.settings.timeout,
        BROWSER_PROBE_CAP,
    ) {
        Readiness::Cancelled => return,
        Readiness::TimedOut => {
            let why = format!(
                "the server did not answer within {} s",
                step.settings.timeout.as_secs_f64()
            );
            let page = url
                .clone()
                .unwrap_or_else(|| "of the launch profile".into());
            send(
                browser_row(&planned, url.as_deref(), None, "failed", Some(why.clone())),
                Some(format!(
                    "The page {page} could not be opened: {why} (no \"{}\" line in the program's output). The \
                     session continues.",
                    launch::LISTENING
                )),
                None,
            );
            return;
        }
        Readiness::Listening { url: heard, at } => (
            url.unwrap_or_else(|| launch::join_url(&heard, &plan.launch_url)),
            Some(at),
        ),
        Readiness::Answered { .. } => (url.unwrap_or_default(), None),
    };
    trace(format_args!(
        "debug start: {} is up ({}) {:.1} ms into the wait; opening it ({})",
        url,
        if heard.is_some() {
            "Kestrel's listening line"
        } else {
            "it answered"
        },
        waited.elapsed().as_secs_f64() * 1e3,
        planned.engine()
    ));
    let Some(commands) = step.commands.clone() else {
        send(
            browser_row(
                &planned,
                Some(&url),
                None,
                "failed",
                Some("the browser commands are not registered".into()),
            ),
            None,
            None,
        );
        return;
    };
    let caller = Caller::Session {
        session: step.session,
        name: name.to_owned(),
    };
    let opened = eludite_commands::with_caller(caller, || {
        if planned.embedded {
            open_in_window(&commands, &url, step.reuse_tab.as_deref()).map(Some)
        } else {
            commands
                .invoke(
                    eludite_commands::browser::OPEN_EXTERNAL,
                    json!({ "url": url }),
                )
                .map(|_| None)
        }
    });
    let latency = heard.map(|at| at.elapsed());
    match opened {
        Ok(tab) => {
            let line = match &tab {
                Some(t) => format!("Opened {url} in the Web Browser window (tab {t})."),
                None => format!("Opened {url} in the system browser."),
            };
            match latency {
                Some(l) => trace(format_args!(
                    "debug start: page opened {:.1} ms after the listening line",
                    l.as_secs_f64() * 1e3
                )),
                None => trace(format_args!(
                    "debug start: page opened (the url answered before the listening line was read)"
                )),
            }
            send(
                browser_row(&planned, Some(&url), tab, "opened", note),
                Some(line),
                latency,
            );
        }
        Err(e) => send(
            browser_row(&planned, Some(&url), None, "failed", Some(e.to_string())),
            Some(format!(
                "The page {url} could not be opened: {e}. The session continues."
            )),
            None,
        ),
    }
}

/// Open `url` in the Web Browser window through the bus (the caller is the session): a restart's tab, when it is
/// still open, is navigated (reloaded when it shows the url already) and selected; otherwise a new tab. Then the
/// window comes forward (`eludite.view.show`). Answers the tab.
fn open_in_window(
    commands: &CommandRegistry,
    url: &str,
    reuse: Option<&str>,
) -> Result<String, CommandError> {
    use eludite_commands::browser as b;
    let mut tab = None;
    if let Some(t) = reuse {
        let tabs = commands.invoke(b::TABS, json!({}))?;
        let row = tabs["tabs"]
            .as_array()
            .and_then(|rows| rows.iter().find(|r| r["id"] == t))
            .cloned();
        if let Some(row) = row {
            let args = if row["url"] == url {
                json!({ "tab": t, "action": "reload" })
            } else {
                json!({ "tab": t, "url": url })
            };
            commands.invoke(b::NAVIGATE, args)?;
            if row["active"] != true {
                commands.invoke(b::TAB_SELECT, json!({ "tab": t }))?;
            }
            tab = Some(t.to_owned());
        }
    }
    let tab = match tab {
        Some(t) => t,
        None => commands.invoke(b::TAB_OPEN, json!({ "url": url }))?["id"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
    };
    commands.invoke(
        eludite_commands::view::SHOW,
        json!({ "id": ids::WEB_BROWSER }),
    )?;
    Ok(tab)
}

/// What the attach thread needs (brief 0027).
struct AttachJob {
    generation: u64,
    target: AttachTarget,
    adapter: Option<String>,
    transport: Option<(String, u16)>,
    mono: Option<(String, u16)>,
    breakpoints: Vec<(String, Vec<eludite_dap::types::SourceBreakpoint>)>,
    functions: Vec<eludite_dap::types::FunctionBreakpoint>,
    exceptions: state::ExceptionPlan,
    setup: DebugSetup,
    native: native::NativeJob,
    tx: UnboundedSender<DebugMsg>,
}

/// Attach (brief 0027), on the `debug-attach` thread: the process from this machine's listing (one on another machine,
/// reached through `transport`, by id only), the adapter by its runtime unless named, its attach plan, the adapter
/// reached, the handshake with `attach`; then the session goes on as a launched one does.
fn attach_thread(job: AttachJob) {
    let AttachJob {
        generation,
        target,
        adapter,
        transport,
        mono,
        breakpoints,
        functions,
        exceptions,
        setup,
        native,
        tx,
    } = job;
    let fail = |message: String| {
        let _ = tx.unbounded_send(DebugMsg::LaunchFailed {
            generation,
            message,
        });
    };
    let platform = setup.platform;
    let info = match (&target, &transport) {
        (AttachTarget::Pid(pid), Some(_)) => processes::ProcessInfo {
            pid: *pid,
            parent: None,
            name: format!("process {pid}"),
            argv: Vec::new(),
            runtime: processes::Runtime::Unknown,
        },
        _ => {
            let all = match processes::list() {
                Ok(a) => a,
                Err(e) => return fail(format!("Cannot list processes: {e}")),
            };
            match find_process(&all, &target) {
                Ok(p) => p,
                Err(e) => return fail(format!("Cannot attach: {e}")),
            }
        }
    };
    let adapter = match adapter.as_deref().and_then(AttachAdapter::parse) {
        Some(a) => a,
        None => match AttachAdapter::for_runtime(info.runtime) {
            Ok(a) => a,
            Err(e) => return fail(format!("Cannot attach to process {}: {e}", info.pid)),
        },
    };
    let agent = mono.or_else(|| processes::mono_agent(&info.argv));
    let plan = match attach_plan(adapter, info.pid, agent, platform) {
        Ok(p) => p,
        Err(e) => return fail(format!("Cannot attach to process {}: {e}", info.pid)),
    };
    let mut session = SessionRow {
        id: None,
        project: info.name.clone(),
        program: info
            .argv
            .first()
            .cloned()
            .unwrap_or_else(|| info.name.clone()),
        args: info.argv.iter().skip(1).cloned().collect(),
        cwd: processes::cwd_of(info.pid)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_default(),
        profile: None,
        debug: true,
        adapter: None,
        runtime: Some(adapter.runtime_name().to_owned()),
        process_id: Some(i64::from(info.pid)),
        attached: true,
        parent: None,
        tab: None,
        url: None,
        browser: None,
    };
    let Some(kind) = adapter.kind() else {
        return fail(format!("Cannot attach to process {}: no adapter", info.pid));
    };
    let mut breakpoints = breakpoints;
    let family = AdapterFamily::of_runtime(Some(adapter.runtime_name()), false);
    breakpoints.retain(|(path, _)| family.takes(path));
    let reached: Result<(Connection, String), String> = if let Some((host, port)) = &transport {
        let t = eludite_dap::AdapterTransport::Tcp {
            host: host.clone(),
            port: *port,
        };
        transport::connect(&t)
            .map(|c| (c, transport::describe(&t)))
            .map_err(|e| format!("cannot reach the debug adapter at {host}:{port}: {e}"))
    } else if let Some(connect) = &setup.connect {
        connect()
            .map(|c| {
                let d = c.description.clone();
                (c, d)
            })
            .map_err(|e| format!("cannot reach the debug adapter: {e}"))
    } else {
        match kind {
            AdapterKind::Lldb => native::connect(&native, platform, None),
            AdapterKind::Mono => setup.mono.find_mono().and_then(|m| {
                let exe = setup.mono_adapter.find()?;
                let t = m.adapter_transport(&exe);
                transport::connect_with_env(&t, &m.env)
                    .map(|c| {
                        (
                            c,
                            format!(
                                "eludite-dbg-mono under mono {} (stdio)",
                                m.version().unwrap_or_else(|| "(unknown version)".into())
                            ),
                        )
                    })
                    .map_err(|e| format!("cannot start eludite-dbg-mono: {e}"))
            }),
            AdapterKind::Netcoredbg => setup.search.find_netcoredbg().and_then(|found| {
                let t = found.transport();
                transport::connect(&t)
                    .map(|c| (c, transport::describe(&t)))
                    .map_err(|e| format!("cannot start netcoredbg: {e}"))
            }),
        }
    };
    let (connection, description) = match reached {
        Ok(r) => r,
        Err(e) => return fail(e),
    };
    session.adapter = Some(description);
    let _ = tx.unbounded_send(DebugMsg::Launched {
        generation,
        session: Box::new(session),
        run: None,
        plan: None,
    });
    let sink_tx = tx.clone();
    let lldb = kind == AdapterKind::Lldb;
    let client = DapClient::start(
        connection,
        Arc::new(move |event| {
            let event = if lldb {
                native::adapt(event, None)
            } else {
                event
            };
            let _ = sink_tx.unbounded_send(DebugMsg::Client { generation, event });
        }),
    );
    let _ = tx.unbounded_send(DebugMsg::Connected {
        generation,
        client: client.clone(),
    });
    let start = StartPlan {
        adapter_id: plan.adapter_id.into(),
        kind: StartKind::Attach,
        arguments: plan.arguments,
        breakpoints,
        exception_filters: exceptions.filters,
        exception_options: exceptions.options,
        function_breakpoints: if lldb {
            native::with_rust_panics(functions, native.rust_panics)
        } else {
            functions
        },
    };
    let mut result =
        dap_session::start(&client, &start, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    if lldb && let Ok(started) = &mut result {
        native::adapt_capabilities(&mut started.capabilities);
    }
    let adapter_id = if client.description().starts_with("fake adapter") {
        "fake".to_owned()
    } else {
        start.adapter_id.clone()
    };
    let _ = tx.unbounded_send(DebugMsg::Started {
        generation,
        result,
        adapter_id,
    });
}

// ---- vscode-js-debug (brief 0038) ----

/// What a browser session attaches to.
#[derive(Debug, Clone, PartialEq, Eq)]
enum PageTarget {
    /// A tab of the browser Eludite runs (`t1`).
    Tab(String),
    /// A page url of that browser, or a Chrome DevTools websocket url.
    Url(String),
}

/// What the browser session's attach thread needs.
struct JsAttachJob {
    generation: u64,
    target: PageTarget,
    /// The `web_root` given, else where to look for one: the project whose launch opened the page, then the
    /// solution's or folder's root.
    web_root: Option<PathBuf>,
    project_dir: Option<PathBuf>,
    root: Option<PathBuf>,
    js: JsSetup,
    browser: Option<BrowserBus>,
    tx: UnboundedSender<DebugMsg>,
}

/// The web root js-debug maps the page's urls under: the given one, else the project's `wwwroot` (Visual Studio's web
/// root), else the project's folder, else the root. Reads the disk: on the attach thread.
fn web_root_for(
    given: Option<PathBuf>,
    project_dir: Option<&Path>,
    root: Option<&Path>,
) -> PathBuf {
    if let Some(w) = given {
        return w;
    }
    if let Some(p) = project_dir {
        let www = p.join("wwwroot");
        return if www.is_dir() { www } else { p.to_path_buf() };
    }
    root.map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
}

/// The sink of a browser session's connection: `stackTrace` answers get their frames' generated places through the
/// source maps on disk (on the client's reader thread; `eludite_dap::sourcemap`), then everything goes to the UI.
fn js_sink(generation: u64, tx: UnboundedSender<DebugMsg>) -> eludite_dap::EventSink {
    let maps = Mutex::new(eludite_dap::sourcemap::MapCache::new());
    Arc::new(move |event| {
        let event = match event {
            ClientEvent::Response {
                request_seq,
                command,
                result: Ok(mut body),
            } if command == "stackTrace" => {
                eludite_dap::sourcemap::adapt_stack(
                    &mut body,
                    &mut maps.lock().unwrap_or_else(|e| e.into_inner()),
                );
                ClientEvent::Response {
                    request_seq,
                    command,
                    result: Ok(body),
                }
            }
            e => e,
        };
        let _ = tx.unbounded_send(DebugMsg::Client { generation, event });
    })
}

/// The reverse-request handler of a session of `server` (generation `generation`): `startDebugging` is answered at
/// once and becomes a child session on the UI thread ([`DebugMsg::StartChild`]).
fn js_reverse(
    generation: u64,
    server: Arc<dyn AdapterServer>,
    tx: UnboundedSender<DebugMsg>,
) -> eludite_dap::ReverseHandler {
    Arc::new(move |command, args| {
        if command != "startDebugging" {
            return None;
        }
        let request = args["request"].as_str().unwrap_or("attach").to_owned();
        if StartKind::from_request(&request).is_none() {
            return Some(Err(format!("unknown request `{request}`")));
        }
        let _ = tx.unbounded_send(DebugMsg::StartChild {
            generation,
            request,
            configuration: args["configuration"].clone(),
            server: server.clone(),
        });
        Some(Ok(json!({})))
    })
}

/// The browser session's attach (brief 0038), on its `debug-attach` thread: the tab's debug endpoint from the
/// browser worker (or a DevTools websocket url's), vscode-js-debug and Node.js located and the server started, the
/// first connection with the `pwa-chrome` attach. Its children come through `startDebugging`.
fn js_attach_thread(job: JsAttachJob) {
    let JsAttachJob {
        generation,
        target,
        web_root,
        project_dir,
        root,
        js,
        browser,
        tx,
    } = job;
    let fail = |message: String| {
        let _ = tx.unbounded_send(DebugMsg::LaunchFailed {
            generation,
            message,
        });
    };
    let page = match &target {
        PageTarget::Url(u) if u.starts_with("ws://") || u.starts_with("wss://") => {
            match eludite_dap::attach::parse_devtools_url(u) {
                Some((address, port, id)) => (
                    None,
                    eludite_dap::attach::BrowserTarget {
                        address,
                        port,
                        target_id: Some(id.clone()),
                        url: u.clone(),
                        title: format!("page {id}"),
                    },
                ),
                None => {
                    return fail(format!(
                        "Cannot attach: {u} is not a Chrome DevTools page url (ws://HOST:PORT/devtools/page/ID)"
                    ));
                }
            }
        }
        _ => {
            let Some(bus) = &browser else {
                return fail("Cannot attach: the browser is not available".into());
            };
            let want = match &target {
                PageTarget::Tab(t) => super::browser::DebugTab::Id(t.clone()),
                PageTarget::Url(u) => super::browser::DebugTab::Url(u.clone()),
            };
            match bus.debug_target(want) {
                Ok(d) => (
                    Some(d.tab.clone()),
                    eludite_dap::attach::BrowserTarget {
                        address: d.address,
                        port: d.port,
                        target_id: d.target_id,
                        url: d.url,
                        title: d.title,
                    },
                ),
                Err(e) => return fail(format!("Cannot attach: {e}")),
            }
        }
    };
    let (tab, page) = page;
    let web_root = web_root_for(web_root, project_dir.as_deref(), root.as_deref());
    let (server, description, version) = match js.start_server() {
        Ok(s) => s,
        Err(e) => return fail(format!("Cannot debug the page: {e}")),
    };
    let title = if page.title.is_empty() {
        page.url.clone()
    } else {
        page.title.clone()
    };
    let session = SessionRow {
        id: None,
        project: title.clone(),
        program: page.url.clone(),
        args: Vec::new(),
        cwd: web_root.to_string_lossy().into_owned(),
        profile: None,
        debug: true,
        adapter: Some(description),
        runtime: Some("javascript".into()),
        process_id: None,
        attached: true,
        parent: None,
        tab,
        url: Some(page.url.clone()),
        browser: None,
    };
    let _ = tx.unbounded_send(DebugMsg::Launched {
        generation,
        session: Box::new(session),
        run: None,
        plan: None,
    });
    let _ = tx.unbounded_send(DebugMsg::AdapterVersion {
        generation,
        version,
    });
    let connection = match server.connect() {
        Ok(c) => c,
        Err(e) => return fail(format!("cannot reach vscode-js-debug: {e}")),
    };
    let client = DapClient::start_with(
        connection,
        js_sink(generation, tx.clone()),
        Some(js_reverse(generation, server.clone(), tx.clone())),
    );
    let _ = tx.unbounded_send(DebugMsg::Connected {
        generation,
        client: client.clone(),
    });
    let plan = eludite_dap::attach::browser_attach(&page, &web_root);
    // The browser session gets no breakpoints and no exception filters: its children own the page's scripts.
    let start = StartPlan {
        adapter_id: plan.adapter_id.into(),
        kind: StartKind::Attach,
        arguments: plan.arguments,
        breakpoints: Vec::new(),
        exception_filters: Vec::new(),
        exception_options: Vec::new(),
        function_breakpoints: Vec::new(),
    };
    let result = dap_session::start(&client, &start, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    let _ = tx.unbounded_send(DebugMsg::Started {
        generation,
        result,
        adapter_id: "javascript".into(),
    });
}

/// A child session's handshake (brief 0038), on its own thread: a new connection to the same server, then
/// `initialize` and the `request` js-debug named with its configuration, the page's breakpoints and exception filters.
struct JsChildJob {
    generation: u64,
    kind: StartKind,
    configuration: Value,
    server: Arc<dyn AdapterServer>,
    breakpoints: Vec<(String, Vec<eludite_dap::types::SourceBreakpoint>)>,
    exceptions: state::ExceptionPlan,
    tx: UnboundedSender<DebugMsg>,
}

fn js_child_thread(job: JsChildJob) {
    let JsChildJob {
        generation,
        kind,
        configuration,
        server,
        breakpoints,
        exceptions,
        tx,
    } = job;
    let connection = match server.connect() {
        Ok(c) => c,
        Err(e) => {
            let _ = tx.unbounded_send(DebugMsg::LaunchFailed {
                generation,
                message: format!("cannot reach vscode-js-debug: {e}"),
            });
            return;
        }
    };
    let client = DapClient::start_with(
        connection,
        js_sink(generation, tx.clone()),
        Some(js_reverse(generation, server.clone(), tx.clone())),
    );
    let _ = tx.unbounded_send(DebugMsg::Connected {
        generation,
        client: client.clone(),
    });
    let start = StartPlan {
        adapter_id: eludite_dap::attach::JS_ADAPTER_ID.into(),
        kind,
        arguments: configuration,
        breakpoints,
        exception_filters: exceptions.filters,
        exception_options: exceptions.options,
        function_breakpoints: Vec::new(),
    };
    let result = dap_session::start(&client, &start, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    let _ = tx.unbounded_send(DebugMsg::Started {
        generation,
        result,
        adapter_id: "javascript".into(),
    });
}

impl Debugger {
    /// List the processes on a `debug-attach` thread; the answer comes back as [`DebugMsg::Processes`].
    fn list_processes_later(&self, filter: Option<String>, roots: Vec<u32>) {
        let tx = self.tx.clone();
        let bus = self.browser_bus.clone();
        std::thread::Builder::new()
            .name("debug-attach".into())
            .spawn(move || {
                let listing =
                    list_processes(filter.as_deref(), &roots).map(|o| with_tabs(o, bus.as_ref()));
                let _ = tx.unbounded_send(DebugMsg::Processes { listing });
            })
            .expect("spawn debug-attach");
    }
}

impl Shell {
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn debugger(&self) -> &Debugger {
        &self.debug
    }

    /// `eludite.debug.state`'s output now.
    pub fn debug_state(&self) -> DebugOutput {
        DebugOutput::State(Box::new(self.debug.state()))
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
        let m = &self.debug.model;
        // A start waits for its web project's page too (brief 0037), while the program runs.
        let page = start
            && matches!(m.mode, Mode::Running | Mode::RunningWithoutDebugging)
            && m.session
                .as_ref()
                .and_then(|s| s.browser.as_ref())
                .is_some_and(|b| b.waiting());
        // And for its page's debugger to attach (brief 0038).
        let attaching = start
            && matches!(&m.browser_attach, Some(BrowserAttach::Session(id))
                if self.debug.mode_of(*id) == Some(Mode::Launching));
        !page && !attaching && ((start && m.mode == Mode::Running) || m.settled())
    }

    /// A start's answer names the browser session its page's debugger started, and that session's children (brief
    /// 0038): the server first.
    fn compound_rows(&mut self, sid: u32) -> Vec<cmds::CompoundSessionRow> {
        let Some(BrowserAttach::Session(browser)) =
            self.in_session(sid, |s| s.debug.model.browser_attach.clone())
        else {
            return Vec::new();
        };
        let mut ids = vec![sid, browser];
        ids.extend(self.debug.children_of(browser));
        ids.into_iter()
            .filter_map(|id| {
                self.in_session(id, |s| {
                    (s.debug.session_id == id).then(|| cmds::CompoundSessionRow {
                        id,
                        name: Debugger::name_of(&s.debug.model, &s.debug.session_name),
                        mode: s.debug.model.mode.as_str().into(),
                    })
                })
            })
            .collect()
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
                        let t = Instant::now();
                        shell.on_debug_msgs(batch, window, cx);
                        shell.debug.timings.msgs_ui.push((t, t.elapsed()));
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
                    session,
                    request,
                    reply,
                    caller,
                } = job;
                // One queue (rule 1): the command is applied here, in order; what it then waits for runs in a task of
                // its own, so a long `wait` never holds the next agent's command.
                let applied = this.update_in(cx, |shell, window, cx| {
                    let t = Instant::now();
                    // What the person does from here on interrupts what this command waits for (rule 5): the
                    // follow-up carries its session's interrupt count.
                    let r = eludite_commands::with_caller(caller.clone(), || {
                        shell.apply_debug(session, request, &caller, true, window, cx)
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

    /// Run `f` with session `id` as the current one (brief 0028), then come back to the session that was current.
    pub(super) fn in_session<R>(&mut self, id: u32, f: impl FnOnce(&mut Self) -> R) -> R {
        let back = self.debug.session_id;
        self.debug.enter(id);
        let r = f(self);
        self.debug.enter(back);
        r
    }

    /// Apply a debug command (the UI-thread half of [`DebugBus`]) to the session it names (brief 0028; `None`: the
    /// active one, for `stop` every one). For an agent (`agent`: the caller waits off the UI thread) a command may also
    /// return what to wait for before answering ([`Follow`]); the UI thread gets its answer at once.
    pub fn apply_debug(
        &mut self,
        session: Option<u32>,
        request: DebugRequest,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<Follow>), CommandError> {
        // What acts on no single session, or on every one.
        match &request {
            DebugRequest::Sessions => {
                let live = self.debug.live_ids();
                return Ok((
                    DebugOutput::Sessions(cmds::SessionsOutput {
                        sessions: self.debug.sessions_info(),
                        active: live
                            .contains(&self.debug.active)
                            .then_some(self.debug.active),
                    }),
                    None,
                ));
            }
            DebugRequest::Stop if session.is_none() && self.debug.live_ids().len() > 1 => {
                return self.debug_stop_all(caller, agent, window, cx);
            }
            DebugRequest::Start { .. }
            | DebugRequest::Attach { .. }
            | DebugRequest::Processes { .. }
            | DebugRequest::Breakpoint { .. }
            | DebugRequest::ExceptionSettings { .. }
            | DebugRequest::Trace {
                run: TraceRun::Start,
                ..
            } => {
                if session.is_some() {
                    return Err(CommandError::InvalidInput(
                        "`trace` with `run: start` starts a session: it takes no `session`".into(),
                    ));
                }
                self.debug.started = None;
                let sid = self.debug.session_id;
                let r = self.apply_debug_in(request, caller, agent, window, cx);
                let sid = self.debug.started.take().unwrap_or(sid);
                // A start or an attach may leave its new session current: the active one is again.
                self.settle_active(cx);
                self.refresh_debug(cx);
                return r.map(|(out, follow)| {
                    let epoch = self.in_session(sid, |s| s.debug.interrupt);
                    (out, follow.map(|what| Follow { sid, epoch, what }))
                });
            }
            _ => {}
        }
        let sid = self.debug.resolve_session(session)?;
        // Picking a session by hand (the Call Stack and Threads selectors: `select_frame` with only `session`) makes
        // it the active one, whatever its mode.
        if let DebugRequest::SelectFrame {
            thread: None,
            frame: None,
            stop: None,
        } = request
            && session.is_some()
        {
            self.activate_session(sid, !caller.is_agent(), cx);
            self.refresh_debug(cx);
            return Ok((self.debug_state(), None));
        }
        let selects = matches!(request, DebugRequest::SelectFrame { .. }) && session.is_some();
        let r = self.in_session(sid, |s| {
            let r = s.apply_debug_in(request, caller, agent, window, cx);
            let epoch = s.debug.interrupt;
            r.map(|(out, follow)| (out, follow.map(|what| Follow { sid, epoch, what })))
        });
        if selects && r.is_ok() && sid != self.debug.active {
            self.activate_session(sid, !caller.is_agent(), cx);
            self.refresh_debug(cx);
            return Ok((self.debug_state(), None));
        }
        r
    }

    /// Debug > Stop Debugging with several sessions (brief 0028): every session stops; an agent's call waits until
    /// they all ended.
    fn debug_stop_all(
        &mut self,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<Follow>), CommandError> {
        let live = self.debug.live_ids();
        if caller.is_agent() {
            for id in &live {
                self.in_session(*id, |s| {
                    if !s.debug.model.agents_allowed {
                        return Err(CommandError::Failed(cmds::AGENTS_NOT_ALLOWED.into()));
                    }
                    s.debug.stale_check(&DebugRequest::Stop)
                })?;
            }
        }
        let driver = driver_of(caller);
        for id in live {
            self.in_session(id, |s| {
                if matches!(s.debug.model.mode, Mode::Design | Mode::Stopping) {
                    return;
                }
                if !caller.is_agent() {
                    // The person takes over every session (proposal 0001 rule 5).
                    s.debug.interrupt += 1;
                    s.debug.interrupted_at = Some(Instant::now());
                }
                s.debug_stop(&driver, window, cx);
            });
        }
        self.settle_active(cx);
        self.refresh_debug(cx);
        let sid = self.debug.session_id;
        let follow = agent.then(|| Follow {
            sid,
            epoch: self.debug.interrupt,
            what: Followup::Ended {
                wait: AGENT_WAIT,
                all: true,
            },
        });
        Ok((self.debug_state(), follow))
    }

    /// Make session `id` the active one: the windows, the execution point and commands without `session` follow it
    /// (brief 0028). `by_person`: picked by hand, which holds for [`SELECTION_HOLD`] against stops elsewhere.
    pub(super) fn activate_session(&mut self, id: u32, by_person: bool, cx: &mut Context<Self>) {
        if by_person {
            self.debug.selected_at = Some(Instant::now());
        }
        if self.debug.active == id {
            return;
        }
        trace(format_args!("debug: session {id} is active"));
        self.debug.active = id;
        self.debug.enter(id);
        self.apply_exec(cx);
        self.refresh_glyphs(cx);
    }

    /// After commands and messages: the active session is a live one when any is (the one that ended gives way to
    /// another), and it is the current one again; the execution point follows it.
    pub(super) fn settle_active(&mut self, cx: &mut Context<Self>) {
        let live = self.debug.live_ids();
        let before = self.debug.active;
        if !live.contains(&self.debug.active)
            && let Some(first) = live.first()
        {
            self.debug.active = *first;
        }
        let active = self.debug.active;
        self.debug.enter(active);
        self.debug.prune();
        if before != active {
            trace(format_args!("debug: session {active} is active"));
            self.refresh_glyphs(cx);
        }
        if self.debug.exec != self.debug.shown_exec {
            self.apply_exec(cx);
        }
    }

    /// The commands of [`Shell::apply_debug`], on the current session.
    fn apply_debug_in(
        &mut self,
        request: DebugRequest,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<Followup>), CommandError> {
        // Who may drive (brief 0027): Allow Agents to Drive, then rule 5's stale check after an interrupted wait.
        if caller.is_agent() {
            if request.drives() && !self.debug.model.agents_allowed {
                return Err(CommandError::Failed(cmds::AGENTS_NOT_ALLOWED.into()));
            }
            if matches!(request, DebugRequest::AllowAgents { enabled: true }) {
                return Err(CommandError::Failed(
                    "only the person can allow agents to drive (Debug > Allow Agents to Drive); an agent may turn \
                     it off"
                        .into(),
                ));
            }
        }
        match &request {
            // A start or an attach adds a session beside the live ones (brief 0028): checked by what it names.
            DebugRequest::Start { .. } => {}
            DebugRequest::Attach { target, .. } => self.check_attach(target)?,
            _ => self.debug.model.check(&request)?,
        }
        if caller.is_agent() {
            self.debug.stale_check(&request)?;
        } else if request.resumes()
            || matches!(
                request,
                DebugRequest::Pause { .. } | DebugRequest::Restart { .. }
            )
        {
            // The person takes over: an agent's waiting command ends at once (proposal 0001 rule 5).
            self.debug.interrupt += 1;
            self.debug.interrupted_at = Some(Instant::now());
        }
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
                after: None,
                fresh: None,
                points: Vec::new(),
                wait,
                budget,
            })
        };
        let follow = match request {
            DebugRequest::Sessions => unreachable!("answered by apply_debug"),
            DebugRequest::State => {
                self.refresh_debug(cx);
                return Ok((self.debug_state(), None));
            }
            DebugRequest::Start {
                project,
                debug,
                profile,
                framework,
                build,
                cargo,
                compound,
                browser,
                browsers,
                ..
            } => {
                // `framework` applies to the launch this start makes (brief 0049).
                self.properties.start_framework = framework;
                let plain = project.is_none() && compound.is_none();
                let entries = self.start_entries(project, debug, profile, compound);
                self.check_start(&entries, plain)?;
                if entries.len() > 1 && cargo.is_set() {
                    return Err(CommandError::InvalidInput(
                        "the Cargo options (`target`, `test`, `args`) are for one package: the startup projects \
                         are several"
                            .into(),
                    ));
                }
                self.debug.start_cargo = cargo;
                self.debug.start_browser = browser;
                self.debug.start_browsers = browsers;
                let ids = self.debug_start_set(entries, build, &driver, window, cx);
                self.debug.start_browser = None;
                self.debug.start_browsers.clear();
                if ids.len() > 1 {
                    self.debug.compound = ids.clone();
                    (agent && !wait.is_zero()).then_some(Followup::Compound { ids, wait, budget })
                } else {
                    settle(true, None)
                }
            }
            DebugRequest::Stop => {
                self.debug_stop(&driver, window, cx);
                self.refresh_debug(cx);
                let follow =
                    (agent && !wait.is_zero()).then_some(Followup::Ended { wait, all: false });
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
                self.debug.send_breakpoints_here(&path);
                self.debug_resume("continue", None, &driver)?;
                settle(false, None)
            }
            DebugRequest::Pause { thread, .. } => {
                let generation = self.debug_pause(thread, &driver)?;
                settle(false, Some(generation))
            }
            DebugRequest::Breakpoint {
                function: Some(function),
                action,
                enabled,
                condition,
                hit_condition,
                remove_after,
                ..
            } => {
                let existed = self
                    .debug
                    .model
                    .breakpoints
                    .functions()
                    .iter()
                    .any(|f| f.name == function);
                let name = function.clone();
                self.debug_function_breakpoint(
                    function,
                    action,
                    enabled,
                    condition,
                    hit_condition,
                    remove_after,
                    cx,
                )?;
                self.refresh_debug(cx);
                let (edit, target) = match action {
                    BreakpointAction::Delete => (cmds::BreakpointEdit::Deleted, None),
                    _ if existed => (
                        cmds::BreakpointEdit::Changed,
                        Some(BreakpointTarget::Function(name)),
                    ),
                    _ => (
                        cmds::BreakpointEdit::Added,
                        Some(BreakpointTarget::Function(name)),
                    ),
                };
                return Ok(self.debug.breakpoint_reply(edit, target, agent));
            }
            DebugRequest::Breakpoint {
                path,
                line,
                action,
                enabled,
                condition,
                hit_condition,
                log_message,
                remove_after,
                ..
            } => {
                let edited = self.debug_breakpoint(
                    path,
                    line,
                    action,
                    enabled,
                    condition,
                    hit_condition,
                    log_message,
                    remove_after,
                    cx,
                )?;
                self.refresh_debug(cx);
                // The compact answer (brief 0034): what happened to the breakpoint, and its row while it exists.
                let (edit, target) = match edited {
                    None => (cmds::BreakpointEdit::DeletedAll, None),
                    Some((path, line, existed)) => {
                        let target = Some(BreakpointTarget::Line(path, line));
                        match action {
                            BreakpointAction::Delete => (cmds::BreakpointEdit::Deleted, None),
                            BreakpointAction::Toggle if existed => {
                                (cmds::BreakpointEdit::Deleted, None)
                            }
                            BreakpointAction::Set if existed => {
                                (cmds::BreakpointEdit::Changed, target)
                            }
                            _ => (cmds::BreakpointEdit::Added, target),
                        }
                    }
                };
                return Ok(self.debug.breakpoint_reply(edit, target, agent));
            }
            DebugRequest::RunUntil {
                points,
                remove_after,
                ..
            } => {
                let mut resolved = Vec::new();
                for p in &points {
                    resolved.push(self.debug_location(Some(&p.path), Some(p.line), cx)?);
                }
                let mut files: Vec<String> = Vec::new();
                let sid = self.debug.session_id;
                let run_points = resolved.clone();
                let b = &mut self.debug.model.breakpoints;
                for ((path, line), p) in resolved.into_iter().zip(points) {
                    // A line that has a breakpoint keeps it (it stops there anyway).
                    if b.at(&path, line).is_none() {
                        let bp = b.ensure(&path, line);
                        bp.condition = p.condition.filter(|c| !c.trim().is_empty());
                        bp.temporary = remove_after;
                        bp.remove_after = remove_after;
                        // Only this session's adapter gets a temporary point (brief 0028).
                        bp.owner = remove_after.then_some(sid);
                    }
                    if !files.contains(&path) {
                        files.push(path);
                    }
                }
                for f in &files {
                    self.debug.send_breakpoints(f);
                }
                self.refresh_glyphs(cx);
                if !remove_after {
                    self.debug_persist(cx);
                }
                self.debug_resume("continue", None, &driver)?;
                let mut follow = settle(false, None);
                if let Some(Followup::Settle { points, .. }) = follow.as_mut() {
                    *points = run_points;
                }
                follow
            }
            DebugRequest::Trace {
                points,
                run,
                start,
                until,
                max_hits,
                ..
            } => {
                if !agent {
                    return Err(ui_thread_refusal(cmds::TRACE));
                }
                if self.debug.trace_job.is_some() {
                    return Err(CommandError::Failed(
                        "a trace is already collecting in this session; wait for its answer".into(),
                    ));
                }
                let mut resolved = Vec::new();
                for p in &points {
                    resolved.push(self.debug_location(Some(&p.path), Some(p.line), cx)?);
                }
                let mut job_points = Vec::new();
                let mut saved = Vec::new();
                let owner = self.debug.session_id;
                let mut files: Vec<String> = Vec::new();
                for ((path, line), p) in resolved.into_iter().zip(points) {
                    if job_points.contains(&(path.clone(), line)) {
                        continue;
                    }
                    let b = &mut self.debug.model.breakpoints;
                    saved.push(b.take(&path, line));
                    b.put(Breakpoint {
                        condition: p.condition.filter(|c| !c.trim().is_empty()),
                        log_message: Some(p.message),
                        temporary: true,
                        // This session's (brief 0028); a trace that starts a session gives them to it below.
                        owner: (run == TraceRun::Continue).then_some(owner),
                        ..Breakpoint::new(&path, line)
                    });
                    job_points.push((path.clone(), line));
                    if !files.contains(&path) {
                        files.push(path);
                    }
                }
                for f in &files {
                    self.debug.send_breakpoints(f);
                }
                self.refresh_glyphs(cx);
                let job = TraceJob {
                    generation: 0,
                    points: job_points,
                    saved,
                    base: 0,
                    max_hits,
                    count: match until {
                        TraceUntil::Hits(n) => Some(n),
                        _ => None,
                    },
                    hits: 0,
                    started: Instant::now(),
                    stop: 0,
                    done: false,
                    truncated: false,
                    bound: None,
                };
                self.debug.trace_job = Some(job);
                let resumed = match run {
                    TraceRun::Continue => self.debug_resume("continue", None, &driver),
                    TraceRun::Start => {
                        // `trace` starts as `start` without Cargo options (brief 0029's `target`, `test`, `args`):
                        // a previous start's options do not carry over. The trace and its points go with the
                        // session it starts (brief 0028).
                        self.debug.start_cargo = cmds::CargoOptions::default();
                        let job = self.debug.trace_job.take();
                        self.debug_start(
                            start.project,
                            true,
                            start.profile,
                            start.build,
                            &driver,
                            window,
                            cx,
                        );
                        self.debug.trace_job = job;
                        let sid = self.debug.session_id;
                        let points = self
                            .debug
                            .trace_job
                            .as_ref()
                            .map(|j| j.points.clone())
                            .unwrap_or_default();
                        for (path, line) in points {
                            if let Some(b) = self.debug.model.breakpoints.at_mut(&path, line) {
                                b.owner = Some(sid);
                            }
                        }
                        Ok(())
                    }
                };
                let d = &mut self.debug;
                let base = d.traces_base + d.traces.len();
                let (generation, stop) = (d.model.generation, d.model.stop);
                if let Some(job) = d.trace_job.as_mut() {
                    job.generation = generation;
                    job.base = base;
                    job.stop = stop;
                    job.started = Instant::now();
                }
                if let Err(e) = resumed {
                    self.debug.finish_trace("terminated");
                    self.refresh_glyphs(cx);
                    return Err(e);
                }
                Some(Followup::Trace {
                    until,
                    wait,
                    budget,
                })
            }
            DebugRequest::SetVariable {
                target,
                name,
                value,
                ..
            } => {
                let d = &mut self.debug;
                if !d.caps.supports_set_variable && !d.caps.supports_set_expression {
                    return Err(CommandError::Failed(format!(
                        "{} cannot change values: it has neither setVariable nor setExpression (DAP); \
                         capabilities.set_variable is false",
                        d.adapter_name()
                    )));
                }
                let m = &d.model;
                let stopped = m.stopped.as_ref().map(|s| s.thread);
                // From the UI the frame is the selected one; an agent's defaults to the top of the stop (rule 3).
                let shown = match target {
                    SetTarget::Reference(r) => Some(r),
                    SetTarget::Frame { thread, frame } => {
                        let thread = thread.or(if agent { stopped } else { m.thread });
                        let frame = frame.unwrap_or(if agent { 0 } else { m.frame });
                        (thread == m.thread
                            && frame == m.frame
                            && !m.locals_loading
                            && m.locals_reference > 0)
                            .then_some(m.locals_reference)
                    }
                };
                let stop = m.stop;
                match shown {
                    Some(reference) => {
                        let frame_id = m.frames.get(m.frame).map(|f| f.id);
                        let (reply, rx) = if agent {
                            let (tx, rx) = oneshot::channel();
                            (Some(tx), Some(rx))
                        } else {
                            (None, None)
                        };
                        let scope = matches!(target, SetTarget::Frame { .. });
                        let request =
                            d.send_set_value(reference, scope, frame_id, &name, &value, reply)?;
                        self.refresh_debug(cx);
                        let out = DebugOutput::SetVariable(SetVariableOutput {
                            name,
                            value,
                            type_name: None,
                            reference: 0,
                            request: Some(request.into()),
                            pending: true,
                            stop,
                        });
                        return Ok((out, rx.map(Followup::SetValue)));
                    }
                    None if agent => {
                        let SetTarget::Frame { thread, frame } = target else {
                            unreachable!("a reference is always sent at once")
                        };
                        return Ok((
                            DebugOutput::SetVariable(SetVariableOutput {
                                name: name.clone(),
                                value: value.clone(),
                                pending: true,
                                stop,
                                ..Default::default()
                            }),
                            Some(Followup::SetVariable {
                                thread,
                                frame: frame.unwrap_or(0),
                                name,
                                value,
                            }),
                        ));
                    }
                    None => return Err(ui_thread_refusal(cmds::SET_VARIABLE)),
                }
            }
            DebugRequest::SetNextStatement {
                path, line, thread, ..
            } => {
                if !self.debug.caps.supports_goto_targets_request {
                    return Err(CommandError::Failed(format!(
                        "Set Next Statement is not supported by {}: it has no gotoTargets and goto (DAP); \
                         capabilities.set_next_statement is false (netcoredbg and eludite-dbg-mono refuse it)",
                        self.debug.adapter_name()
                    )));
                }
                let (path, line) = self.debug_location(path.as_deref(), line, cx)?;
                let d = &mut self.debug;
                let thread = thread
                    .or(d.model.stopped.as_ref().map(|s| s.thread))
                    .or(d.model.thread)
                    .unwrap_or(0);
                let (generation, stop) = (d.generation(), d.model.stop);
                d.goto_error = None;
                d.send(
                    "gotoTargets",
                    json!({"source": {"name": file_name(&path), "path": path}, "line": line}),
                    Pending::GotoTargets {
                        generation,
                        stop,
                        thread,
                        driver: driver.clone(),
                        line,
                    },
                )?;
                (agent && !wait.is_zero()).then_some(Followup::Settle {
                    start: false,
                    pause: None,
                    after: Some((generation, stop)),
                    fresh: None,
                    points: Vec::new(),
                    wait,
                    budget,
                })
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
                break_on_rust_panic,
                types,
                remove,
                clear,
            } => {
                let typed = !types.is_empty() || remove.is_some() || clear;
                let d = &mut self.debug;
                if typed && d.client.is_some() && !d.caps.supports_exception_filter_options {
                    return Err(CommandError::Failed(format!(
                        "{} cannot break on exception types: it has no exceptionFilterOptions (DAP); \
                         capabilities.exception_filter_options is false",
                        d.adapter_name()
                    )));
                }
                let mut next = d.model.exceptions.types.clone();
                if clear {
                    next.clear();
                }
                if let Some(r) = &remove {
                    if !next.iter().any(|t| &t.type_name == r) {
                        return Err(CommandError::InvalidInput(format!(
                            "there is no exception type `{r}` in the settings"
                        )));
                    }
                    next.retain(|t| &t.type_name != r);
                }
                for t in types {
                    match next.iter_mut().find(|x| x.type_name == t.type_name) {
                        Some(x) => *x = t,
                        None => next.push(t),
                    }
                }
                if next.len() > cmds::MAX_EXCEPTION_TYPES {
                    return Err(CommandError::InvalidInput(format!(
                        "the settings hold at most {} exception types",
                        cmds::MAX_EXCEPTION_TYPES
                    )));
                }
                let e = &mut d.model.exceptions;
                e.types = next;
                if let Some(v) = break_when_thrown {
                    e.break_when_thrown = v;
                }
                if let Some(v) = break_when_user_unhandled {
                    e.break_when_user_unhandled = v;
                }
                if let Some(v) = break_on_rust_panic {
                    e.break_on_rust_panic = v;
                    // A native session's Rust panics row is a function breakpoint (brief 0029), sent in the same
                    // list as the user's function breakpoints.
                    d.send_function_breakpoints();
                }
                d.send_exception_settings();
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
            // Brief 0027: attach, processes, restart and who may drive.
            DebugRequest::Attach {
                target: AttachTarget::Dialog,
                adapter,
                ..
            } => {
                if agent {
                    return Err(CommandError::Failed(
                        "name the process to attach to: `pid` or `process_name` (or a page: `tab` or `url`; \
                         eludite.debug.processes lists the candidates)"
                            .into(),
                    ));
                }
                // Debug > Attach to Browser Tab... (brief 0038): the same dialog, its tabs only.
                let tabs_only = adapter.as_deref() == Some("javascript");
                self.open_attach_dialog_with(tabs_only, window, cx);
                None
            }
            // A page, with vscode-js-debug (brief 0038).
            DebugRequest::Attach {
                target: AttachTarget::Tab(tab),
                web_root,
                ..
            } => {
                self.debug_attach_page(
                    PageTarget::Tab(tab),
                    web_root.map(PathBuf::from),
                    &driver,
                    None,
                    cx,
                );
                settle(true, None)
            }
            DebugRequest::Attach {
                target: AttachTarget::Url(url),
                web_root,
                ..
            } => {
                self.debug_attach_page(
                    PageTarget::Url(url),
                    web_root.map(PathBuf::from),
                    &driver,
                    None,
                    cx,
                );
                settle(true, None)
            }
            DebugRequest::Attach {
                target,
                adapter,
                transport,
                mono,
                ..
            } => {
                self.debug_attach(target, adapter, transport, mono, &driver, cx);
                settle(true, None)
            }
            DebugRequest::Processes { filter } => {
                let roots = self
                    .debug
                    .launched
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .clone();
                if agent {
                    return Ok((
                        DebugOutput::Processes(ProcessesOutput::default()),
                        Some(Followup::Processes { filter, roots }),
                    ));
                }
                // The UI thread never waits: the listing comes back as a message (the dialog shows it); the answer
                // is the last one.
                self.debug.list_processes_later(filter, roots);
                let out = self.debug.processes.clone().unwrap_or_default();
                return Ok((DebugOutput::Processes(out), None));
            }
            DebugRequest::Restart { .. } => {
                let generation = self.debug.model.generation;
                let fresh = self.debug_restart(&driver, window, cx)?;
                (agent && !wait.is_zero()).then_some(Followup::Settle {
                    start: true,
                    pause: None,
                    after: None,
                    fresh: fresh.then_some(generation),
                    points: Vec::new(),
                    wait,
                    budget,
                })
            }
            DebugRequest::AllowAgents { enabled } => {
                let m = &mut self.debug.model;
                m.agents_allowed = enabled;
                if m.mode == Mode::Design {
                    m.agents_next = Some(enabled);
                } else {
                    let line = if enabled {
                        "Agents may drive this session."
                    } else {
                        "Agents may not drive this session (Debug > Allow Agents to Drive)."
                    };
                    self.debug.console_line(line);
                }
                self.refresh_debug(cx);
                let m = &self.debug.model;
                return Ok((
                    DebugOutput::AllowAgents(AllowAgentsOutput {
                        agents_allowed: m.agents_allowed,
                        default: m.agents_default,
                        mode: m.mode.as_str().into(),
                    }),
                    None,
                ));
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
                    let mut out = self.debug.summary(&budget);
                    let m = &self.debug.model;
                    match wait_satisfied(m, until, stop, baseline) {
                        Some(why) => out.satisfied = Some(why.into()),
                        None => out.timed_out = Some(true),
                    }
                    return Ok((DebugOutput::Summary(Box::new(out)), None));
                }
            }
        };
        self.refresh_debug(cx);
        let out = DebugOutput::Summary(Box::new(self.debug.summary(&budget)));
        Ok((out, follow))
    }

    /// What a start runs (brief 0028): the compound's projects, the multiple startup projects when F5 names none, or
    /// the one project (`None`: the startup project); in solution order.
    fn start_entries(
        &self,
        project: Option<String>,
        debug: bool,
        profile: Option<String>,
        compound: Option<cmds::Compound>,
    ) -> Vec<StartEntry> {
        let startup = |m: &DebugModel| -> Vec<StartEntry> {
            m.startup_set()
                .into_iter()
                .map(|(path, action)| StartEntry {
                    project: Some(path),
                    debug: debug && action == StartupAction::Start,
                    profile: None,
                })
                .collect()
        };
        let mut entries = match compound {
            Some(cmds::Compound::Projects(list)) => list
                .into_iter()
                .map(|e| StartEntry {
                    project: Some(e.project),
                    debug: debug && e.debug,
                    profile: e.profile,
                })
                .collect(),
            Some(cmds::Compound::Startup) => startup(&self.debug.model),
            None if project.is_none() && !self.debug.model.startup_projects.is_empty() => {
                startup(&self.debug.model)
            }
            None => Vec::new(),
        };
        if entries.is_empty() {
            entries.push(StartEntry {
                project,
                debug,
                profile,
            });
        }
        // Solution order (Visual Studio launches its startup projects in the order the solution lists them).
        let order: Vec<(String, String)> = self
            .tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| (p.name.clone(), norm(&p.path)))
            .collect();
        let rank = |e: &StartEntry| {
            e.project.as_deref().map_or(usize::MAX, |p| {
                order
                    .iter()
                    .position(|(name, path)| name == p || *path == norm(p))
                    .unwrap_or(usize::MAX)
            })
        };
        entries.sort_by_key(|e| rank(e));
        entries
    }

    /// A start beside live sessions (brief 0028): a plain start (F5 with one startup project) is refused as before
    /// (F5 in break mode is Continue); a start that names projects adds sessions, a project already being debugged
    /// included (Visual Studio's Debug > Start New Instance).
    fn check_start(&self, entries: &[StartEntry], plain: bool) -> Result<(), CommandError> {
        if self.debug.live_ids().is_empty() || !(plain && entries.len() == 1) {
            return Ok(());
        }
        if !self.debug.live_ids().contains(&self.debug.session_id) {
            return Err(CommandError::Failed(
                "a debugging session is already running; stop it (eludite.debug.stop) or resume it \
                 (eludite.debug.continue), or start another project with `project`"
                    .into(),
            ));
        }
        let m = &self.debug.model;
        let (mode, g) = (m.mode.as_str(), m.generation);
        Err(CommandError::Failed(format!(
            "a session is already {mode} (generation {g}); stop it (eludite.debug.stop) or resume it \
             (eludite.debug.continue), or start another project with `project`"
        )))
    }

    /// An attach beside live sessions (brief 0028): refused for a process a session already debugs, or a tab a browser
    /// session already debugs (brief 0038).
    fn check_attach(&self, target: &AttachTarget) -> Result<(), CommandError> {
        if let AttachTarget::Tab(tab) = target
            && let Some(s) = self
                .debug
                .sessions_info()
                .into_iter()
                .find(|s| s.parent.is_none() && s.tab.as_deref() == Some(tab.as_str()))
        {
            return Err(CommandError::Failed(format!(
                "tab {tab} is already being debugged in session {} ({})",
                s.id, s.name
            )));
        }
        if let AttachTarget::Pid(pid) = target
            && let Some(s) = self
                .debug
                .sessions_info()
                .into_iter()
                // Ctrl+F5's program may be attached to (brief 0027): its run is not a debugging session.
                .find(|s| {
                    s.process_id == Some(i64::from(*pid)) && s.mode != "running_without_debugging"
                })
        {
            return Err(CommandError::Failed(format!(
                "process {pid} is already being debugged in session {} ({})",
                s.id, s.name
            )));
        }
        Ok(())
    }

    /// Start `entries`, each in a session of its own (brief 0028): one start as before; several build once for the
    /// whole set (the solution's build, when build before run is on) and then launch in order. Returns the sessions.
    fn debug_start_set(
        &mut self,
        entries: Vec<StartEntry>,
        build: Option<bool>,
        driver: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<u32> {
        if entries.len() == 1 {
            let e = entries.into_iter().next().expect("one entry");
            self.debug_start(e.project, e.debug, e.profile, build, driver, window, cx);
            return self.debug.started.into_iter().collect();
        }
        let build = build.unwrap_or(self.builds.build_before_run)
            && (self.solution.is_some() || self.cargo_workspace().is_some());
        let mut ids = Vec::new();
        if !build {
            for e in entries {
                self.debug_start(
                    e.project,
                    e.debug,
                    e.profile,
                    Some(false),
                    driver,
                    window,
                    cx,
                );
                ids.extend(self.debug.started);
            }
            return ids;
        }
        for e in entries {
            let name = e
                .project
                .as_deref()
                .map(project_name)
                .unwrap_or_else(|| "the startup project".into());
            let id = self.debug.new_session(name.clone());
            if !self.debug.live_ids().contains(&self.debug.active) {
                self.debug.active = id;
            }
            let entry = self.debug.take_browser_entry(e.project.as_deref(), e.debug);
            let d = &mut self.debug;
            d.cargo_options = cmds::CargoOptions::default();
            d.model.browser_entry = entry;
            d.model.browser_choice = d.start_browser;
            d.model.browser_reuse = None;
            d.last_start = Some(StartArgs {
                project: e.project.clone(),
                debug: e.debug,
                profile: e.profile.clone(),
                build: None,
                cargo: cmds::CargoOptions::default(),
                browser: d.start_browser,
            });
            d.begin(Mode::Building, driver);
            d.reset_for_start();
            d.console_line(format!(
                "Building the solution before starting {name}\u{2026}"
            ));
            d.pending_launch = Some(PendingLaunch {
                generation: d.model.generation,
                ticket: None,
                project: e.project,
                debug: e.debug,
                profile: e.profile,
                driver: driver.to_owned(),
            });
            ids.push(id);
        }
        trace(format_args!(
            "debug start: building the solution before starting sessions {ids:?}"
        ));
        let sessions = ids.clone();
        cx.spawn_in(window, async move |this, cx| {
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.compound_build(sessions, window, cx);
                shell.settle_active(cx);
                shell.refresh_debug(cx);
            });
        })
        .detach();
        self.show_debug_windows();
        self.output
            .update(cx, |o, cx| o.select(OutputSource::Debug, cx));
        ids
    }

    /// The one build of a compound start: the solution's, through the bus like Build > Build Solution.
    fn compound_build(&mut self, ids: Vec<u32>, window: &mut Window, cx: &mut Context<Self>) {
        let waiting: Vec<u32> = ids
            .into_iter()
            .filter(|id| {
                self.in_session(*id, |s| {
                    s.debug.session_id == *id
                        && s.debug.model.mode == Mode::Building
                        && s.debug.pending_launch.is_some()
                })
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        let started = self.invoke(eludite_commands::build::SOLUTION, json!({}), window, cx);
        let ticket = self.builds.current.as_ref().map(|b| b.ticket);
        self.debug.timings.build_requested = Some(Instant::now());
        for id in waiting {
            self.in_session(id, |s| match &started {
                Ok(_) => {
                    if let Some(p) = s.debug.pending_launch.as_mut() {
                        p.ticket = ticket;
                    }
                }
                Err(e) => s.prelaunch_failed(
                    format!("Cannot start: the build did not start: {e}"),
                    false,
                    cx,
                ),
            });
        }
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
        // Each start is a session of its own (brief 0028); a restart starts its session again under the same id.
        if self.debug.restarting {
            self.debug.started = Some(self.debug.session_id);
        } else {
            let name = project
                .as_deref()
                .map(project_name)
                .unwrap_or_else(|| "the startup project".into());
            let id = self.debug.new_session(name);
            self.debug.started = Some(id);
            if !self.debug.live_ids().contains(&self.debug.active) {
                self.debug.active = id;
            }
        }
        self.debug.cargo_options = std::mem::take(&mut self.debug.start_cargo);
        // Its page's debugger (brief 0038).
        self.debug.model.browser_entry = self.debug.take_browser_entry(project.as_deref(), debug);
        // Where a web project's page opens (brief 0037); a restart navigates the tab its page opened in.
        self.debug.model.browser_choice = self.debug.start_browser;
        if !self.debug.restarting {
            self.debug.model.browser_reuse = None;
        }
        // Restart starts this again (brief 0027).
        self.debug.last_start = Some(StartArgs {
            project: project.clone(),
            debug,
            profile: profile.clone(),
            build,
            cargo: self.debug.cargo_options.clone(),
            browser: self.debug.start_browser,
        });
        // Build first (brief 0020): a .NET solution's projects, or the open folder's Cargo packages (brief 0029).
        let build = build.unwrap_or(self.builds.build_before_run)
            && (self.solution.is_some() || self.cargo_workspace().is_some());
        if !build {
            self.debug_launch(project, debug, profile, driver, false, cx);
            return;
        }
        let what = project
            .clone()
            .unwrap_or_else(|| "the startup project".into());
        let d = &mut self.debug;
        d.begin(Mode::Building, driver);
        d.reset_for_start();
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
        let cargo = self.cargo_context();
        let resolve = cx.background_spawn(async move {
            let dotnet_default =
                native::dotnet_default(project.as_deref(), startup.as_deref(), &projects);
            if let Some(p) = native::resolve_cargo(
                project.as_deref(),
                startup.as_deref(),
                dotnet_default,
                cargo.as_ref(),
            ) {
                return Ok(p.manifest);
            }
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
                if let Some(sid) = shell.debug.session_of_generation(generation) {
                    shell.in_session(sid, |s| s.prelaunch_build(generation, resolved, window, cx));
                }
                shell.settle_active(cx);
                shell.refresh_debug(cx);
            });
        })
        .detach();
        self.refresh_debug(cx);
    }

    /// Attach to a running process (brief 0027): a session in mode `launching` whose handshake (`attach`) runs on a
    /// `debug-attach` thread. Ctrl+F5's program, if one runs, keeps running: it may be the process attached to.
    fn debug_attach(
        &mut self,
        target: AttachTarget,
        adapter: Option<String>,
        transport: Option<(String, u16)>,
        mono: Option<(String, u16)>,
        driver: &str,
        cx: &mut Context<Self>,
    ) {
        // A session of its own beside the live ones (brief 0028). Ctrl+F5's program, when the current session runs
        // it, keeps running: it may be the process attached to.
        let name = match &target {
            AttachTarget::Pid(pid) => format!("process {pid}"),
            AttachTarget::Name(n) | AttachTarget::Tab(n) | AttachTarget::Url(n) => n.clone(),
            AttachTarget::Dialog => String::new(),
        };
        if self.debug.model.mode == Mode::RunningWithoutDebugging
            && let Some(run) = self.debug.run.take()
        {
            self.debug.background_runs.push(run);
            self.debug.model.mode = Mode::Design;
        }
        let id = self.debug.new_session(name);
        self.debug.started = Some(id);
        if !self.debug.live_ids().contains(&self.debug.active) {
            self.debug.active = id;
        }
        let d = &mut self.debug;
        d.begin(Mode::Launching, driver);
        d.reset_for_start();
        d.pending.clear();
        d.caps = Capabilities::default();
        d.run_to_cursor = None;
        d.early_breakpoints.clear();
        d.trace_hits.clear();
        d.goto_error = None;
        d.console_partial_adapter.clear();
        d.restart_pending = None;
        let what = match &target {
            AttachTarget::Pid(pid) => format!("process {pid}"),
            AttachTarget::Name(name) | AttachTarget::Tab(name) | AttachTarget::Url(name) => {
                format!("`{name}`")
            }
            AttachTarget::Dialog => String::new(),
        };
        d.console_line(format!("Attaching to {what}\u{2026}"));
        let breakpoints = d
            .model
            .breakpoints
            .files()
            .into_iter()
            .map(|f| {
                let (_, sbps) = d
                    .model
                    .breakpoints
                    .source_breakpoints(&f, false, false, None);
                (f, sbps)
            })
            .collect();
        let job = AttachJob {
            generation: d.model.generation,
            target,
            adapter,
            transport,
            mono,
            breakpoints,
            functions: d.model.breakpoints.function_breakpoints(false).1,
            exceptions: exception_plan(&d.model.exceptions),
            setup: d.setup.clone(),
            native: native::NativeJob {
                setup: d.native.clone(),
                cargo: None,
                options: cmds::CargoOptions::default(),
                rust_panics: d.model.exceptions.break_on_rust_panic,
            },
            tx: d.tx.clone(),
        };
        std::thread::Builder::new()
            .name("debug-attach".into())
            .spawn(move || attach_thread(job))
            .expect("spawn debug-attach");
        self.show_debug_windows();
        self.output
            .update(cx, |o, cx| o.select(OutputSource::Debug, cx));
        self.refresh_glyphs(cx);
    }

    /// A browser session (brief 0038): vscode-js-debug attached to a page, a session in mode `launching` whose attach
    /// runs on a `debug-attach` thread; `attached_for` is the server session whose page it is (a compound start).
    /// Returns the new session.
    fn debug_attach_page(
        &mut self,
        target: PageTarget,
        web_root: Option<PathBuf>,
        driver: &str,
        attached_for: Option<u32>,
        cx: &mut Context<Self>,
    ) -> u32 {
        let (name, what) = match &target {
            PageTarget::Tab(t) => (format!("tab {t}"), format!("tab {t}")),
            PageTarget::Url(u) => (u.clone(), u.clone()),
        };
        // The project whose launch opened the page: its wwwroot is the default web root.
        let project_dir = match &target {
            PageTarget::Tab(t) => self.debug.project_of_tab(t),
            PageTarget::Url(_) => None,
        }
        .or_else(|| {
            let id = attached_for?;
            self.in_session(id, |s| {
                s.debug
                    .model
                    .session
                    .as_ref()
                    .and_then(|r| Path::new(&r.project).parent().map(Path::to_path_buf))
            })
        });
        let root = self.workspace_root();
        let id = self.debug.new_session(name);
        self.debug.started = Some(id);
        if !self.debug.live_ids().contains(&self.debug.active) {
            self.debug.active = id;
        }
        let d = &mut self.debug;
        d.begin(Mode::Launching, driver);
        d.reset_for_start();
        d.pending.clear();
        d.caps = Capabilities::default();
        d.run_to_cursor = None;
        d.early_breakpoints.clear();
        d.trace_hits.clear();
        d.goto_error = None;
        d.console_partial_adapter.clear();
        d.restart_pending = None;
        d.last_start = None;
        d.model.browser_entry = None;
        d.model.attached_for = attached_for;
        d.console_line(format!("Attaching vscode-js-debug to {what}\u{2026}"));
        let job = JsAttachJob {
            generation: d.model.generation,
            target,
            web_root,
            project_dir,
            root,
            js: d.setup.js.clone(),
            browser: d.browser_bus.clone(),
            tx: d.tx.clone(),
        };
        std::thread::Builder::new()
            .name("debug-attach".into())
            .spawn(move || js_attach_thread(job))
            .expect("spawn debug-attach");
        self.show_debug_windows();
        self.refresh_glyphs(cx);
        id
    }

    /// vscode-js-debug's `startDebugging` (brief 0038), with the parent session current: a child session with
    /// `parent`, named after the target, whose handshake runs on a thread of its own on a new connection.
    fn start_child_session(
        &mut self,
        request: &str,
        configuration: Value,
        server: Arc<dyn AdapterServer>,
        cx: &mut Context<Self>,
    ) {
        let d = &self.debug;
        if matches!(d.model.mode, Mode::Design | Mode::Stopping) {
            return;
        }
        let parent = d.session_id;
        let Some(kind) = StartKind::from_request(request) else {
            return;
        };
        let row = d.model.session.clone().unwrap_or_default();
        let version = d.model.adapter_version.clone();
        let driver = d.model.last_driver.clone().unwrap_or_else(|| "user".into());
        let name = configuration["name"]
            .as_str()
            .filter(|n| !n.is_empty())
            .map(str::to_owned)
            .unwrap_or_else(|| row.project.clone());
        let was_active = d.active == parent;
        let id = self.debug.new_session(name.clone());
        if was_active || !self.debug.live_ids().contains(&self.debug.active) {
            self.debug.active = id;
        }
        let d = &mut self.debug;
        d.begin(Mode::Launching, &driver);
        d.pending.clear();
        d.caps = Capabilities::default();
        d.early_breakpoints.clear();
        d.last_start = None;
        d.model.browser_entry = None;
        d.model.session = Some(SessionRow {
            project: name.clone(),
            program: row.program.clone(),
            cwd: row.cwd.clone(),
            debug: true,
            adapter: row.adapter.clone(),
            runtime: Some("javascript".into()),
            attached: true,
            parent: Some(parent),
            tab: row.tab.clone(),
            url: row.url.clone(),
            ..SessionRow::default()
        });
        d.model.adapter_version = version;
        d.console_line(format!("Debugging {name} (session {parent}'s target)."));
        let family = d.model.family();
        let breakpoints = d
            .model
            .breakpoints
            .files()
            .into_iter()
            .filter(|f| family.takes(f))
            .map(|f| {
                let (_, sbps) = d
                    .model
                    .breakpoints
                    .source_breakpoints(&f, false, false, None);
                (f, sbps)
            })
            .collect();
        let job = JsChildJob {
            generation: d.model.generation,
            kind,
            configuration,
            server,
            breakpoints,
            exceptions: exception_plan_for(family, &d.model.exceptions),
            tx: d.tx.clone(),
        };
        let _ = std::thread::Builder::new()
            .name("debug-attach".into())
            .spawn(move || js_child_thread(job));
        trace(format_args!("debug child session {id} of {parent}: {name}"));
        self.refresh_glyphs(cx);
    }

    /// The tabs the Web Browser window shows changed (brief 0038): a browser session whose tab is gone ends, with
    /// its children; one whose tab's title changed is renamed.
    pub(super) fn debug_tabs_changed(
        &mut self,
        tabs: &[(String, Option<String>)],
        cx: &mut Context<Self>,
    ) {
        let browsers: Vec<(u32, String, String)> = self
            .debug
            .sessions_info()
            .into_iter()
            .filter(|s| s.parent.is_none() && s.runtime.as_deref() == Some("javascript"))
            .filter_map(|s| Some((s.id, s.tab.clone()?, s.name.clone())))
            .collect();
        if browsers.is_empty() {
            return;
        }
        for (id, tab, name) in browsers {
            match tabs.iter().find(|(t, _)| *t == tab) {
                None => self.in_session(id, |s| {
                    if s.debug.model.mode == Mode::Design {
                        return;
                    }
                    let m = format!("The tab {tab} closed.");
                    s.debug.console_line(m.clone());
                    s.end_session(Some(m), cx);
                }),
                Some((_, Some(title))) if *title != name => {
                    let title = title.clone();
                    let kids = self.debug.children_of(id);
                    self.in_session(id, |s| {
                        if let Some(r) = s.debug.model.session.as_mut() {
                            r.project = title.clone();
                        }
                        s.debug.session_name = title.clone();
                    });
                    // Children named after the old title (the page's own target) follow it.
                    for k in kids {
                        self.in_session(k, |s| {
                            if let Some(r) = s.debug.model.session.as_mut()
                                && r.project == name
                            {
                                r.project = title.clone();
                            }
                        });
                    }
                }
                _ => {}
            }
        }
        self.settle_active(cx);
        self.refresh_debug(cx);
    }

    /// Restart (brief 0027): DAP `restart` where the adapter has it, else stop and start the last start again once the
    /// session has ended. Returns whether it stops and starts (a new session generation follows).
    fn debug_restart(
        &mut self,
        driver: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<bool, CommandError> {
        let d = &mut self.debug;
        if d.client.is_some() && d.caps.supports_restart_request {
            let generation = d.generation();
            d.send("restart", json!({}), Pending::Restart { generation })?;
            d.model.resume(driver);
            d.exec = None;
            d.console_line("Restarting\u{2026}");
            self.apply_exec(cx);
            self.refresh_debug(cx);
            return Ok(false);
        }
        let Some(start) = d.last_start.clone() else {
            return Err(CommandError::Failed(
                "this session cannot be restarted: Eludite did not start it".into(),
            ));
        };
        d.restart_pending = Some((start, driver.to_owned()));
        d.console_line("Restarting: stopping the session first\u{2026}");
        self.debug_stop(driver, window, cx);
        self.maybe_restart(window, cx);
        Ok(true)
    }

    /// A restart waiting for its session to end: start it now that it has.
    fn maybe_restart(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let mut ids: Vec<u32> = self
            .debug
            .others
            .iter()
            .filter(|s| s.restart_pending.is_some())
            .map(|s| s.id)
            .collect();
        if self.debug.restart_pending.is_some() {
            ids.push(self.debug.session_id);
        }
        for id in ids {
            self.in_session(id, |s| s.maybe_restart_here(window, cx));
        }
    }

    /// The current session's restart, once it ended: it starts again under the same id (brief 0028).
    fn maybe_restart_here(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.model.mode != Mode::Design {
            return;
        }
        let Some((start, driver)) = self.debug.restart_pending.take() else {
            return;
        };
        self.debug.start_cargo = start.cargo;
        self.debug.start_browser = start.browser;
        self.debug.restarting = true;
        self.debug_start(
            start.project,
            start.debug,
            start.profile,
            start.build,
            &driver,
            window,
            cx,
        );
        self.debug.restarting = false;
        self.debug.start_browser = None;
    }

    /// Debug > Attach to Process... (Ctrl+Alt+P): the dialog, with a fresh listing.
    /// The Attach to Process dialog; `tabs_only`: Debug > Attach to Browser Tab... (brief 0038).
    pub(super) fn open_attach_dialog_with(
        &mut self,
        tabs_only: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let dialog = match &self.debug.attach_dialog {
            Some(d) => d.clone(),
            None => {
                let theme = self.theme;
                let d = cx.new(|cx| windows::AttachDialog::new(theme, cx));
                cx.subscribe_in(&d, window, Self::on_attach_event).detach();
                self.debug.attach_dialog = Some(d.clone());
                d
            }
        };
        dialog.update(cx, |d, cx| d.set_tabs_only(tabs_only, cx));
        if let Some(out) = &self.debug.processes {
            let rows = out.processes.clone();
            let tabs = out.tabs.clone().unwrap_or_default();
            dialog.update(cx, |d, cx| {
                d.set_rows(rows, None, cx);
                d.set_tabs(tabs, cx);
            });
        }
        gpui::Focusable::focus_handle(dialog.read(cx), cx).focus(window, cx);
        let filter = dialog.read(cx).filter_text();
        self.run(
            cmds::PROCESSES,
            match filter {
                Some(f) => json!({ "filter": f }),
                None => json!({}),
            },
            window,
            cx,
        );
        cx.notify();
    }

    pub(super) fn close_attach_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.debug.attach_dialog.take().is_some() {
            self.focus.focus(window, cx);
            cx.notify();
        }
    }

    /// The dialog's Refresh and Attach run `eludite.debug.processes` and `eludite.debug.attach` through the bus.
    fn on_attach_event(
        &mut self,
        _: &gpui::Entity<windows::AttachDialog>,
        event: &windows::AttachEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            windows::AttachEvent::Refresh { filter } => self.run(
                cmds::PROCESSES,
                match filter {
                    Some(f) => json!({ "filter": f }),
                    None => json!({}),
                },
                window,
                cx,
            ),
            windows::AttachEvent::Attach { pid } => {
                let pid = *pid;
                self.close_attach_dialog(window, cx);
                self.run(cmds::ATTACH, json!({ "pid": pid }), window, cx);
            }
            windows::AttachEvent::AttachTab { tab } => {
                let tab = tab.clone();
                self.close_attach_dialog(window, cx);
                self.run(cmds::ATTACH, json!({ "tab": tab }), window, cx);
            }
            windows::AttachEvent::Close => self.close_attach_dialog(window, cx),
        }
    }

    /// The status bar's Allow Agents to Drive toggle while a session runs (brief 0027).
    pub(super) fn debug_status_controls(&self, cx: &mut Context<Self>) -> Vec<gpui::AnyElement> {
        use gpui::{IntoElement as _, StatefulInteractiveElement as _};
        let m = &self.debug.model;
        if m.mode == Mode::Design {
            return Vec::new();
        }
        let allowed = m.agents_allowed;
        vec![
            eludite_ui::status_toggle(
                DEBUG_AGENTS_TOGGLE,
                "Allow agents to drive",
                allowed,
                &self.theme,
            )
            .on_click(cx.listener(move |this, _, window, cx| {
                this.run(
                    cmds::ALLOW_AGENTS,
                    json!({ "enabled": !allowed }),
                    window,
                    cx,
                )
            }))
            .into_any_element(),
        ]
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

    /// The build before a launch ended (`build`'s handlers call this for every build): every session waiting for it
    /// launches (a compound's in the order they started; brief 0028).
    pub(super) fn prelaunch_build_done(
        &mut self,
        ticket: u64,
        outcome: PrelaunchBuild,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        // The Test Explorer's builds end here too (brief 0035).
        self.test_build_done(ticket, &outcome, window, cx);
        let mut waiting: Vec<u32> = self
            .debug
            .others
            .iter()
            .filter(|s| s.pending_launch.as_ref().and_then(|p| p.ticket) == Some(ticket))
            .map(|s| s.id)
            .collect();
        if self.debug.pending_launch.as_ref().and_then(|p| p.ticket) == Some(ticket) {
            waiting.push(self.debug.session_id);
        }
        waiting.sort_unstable();
        for id in waiting {
            let outcome = outcome.clone();
            self.in_session(id, |s| {
                s.prelaunch_build_done_here(ticket, outcome, window, cx)
            });
        }
        self.settle_active(cx);
    }

    fn prelaunch_build_done_here(
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
        let cargo = self.cargo_context();
        let choice = self.launch_choice();
        let d = &mut self.debug;
        if after_build {
            // The session began with the build: same generation, its console lines kept.
            d.model.mode = Mode::Launching;
        } else {
            d.begin(Mode::Launching, driver);
            d.reset_for_start();
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
        d.trace_hits.clear();
        d.goto_error = None;
        d.console_partial_adapter.clear();
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
                let (_, sbps) = d
                    .model
                    .breakpoints
                    .source_breakpoints(&f, false, false, None);
                (f, sbps)
            })
            .collect();
        let functions = d.model.breakpoints.function_breakpoints(false).1;
        let exceptions = exception_plan(&d.model.exceptions);
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
            functions,
            exceptions,
            setup: d.setup.clone(),
            native: native::NativeJob {
                setup: d.native.clone(),
                cargo,
                options: d.cargo_options.clone(),
                rust_panics: d.model.exceptions.break_on_rust_panic,
            },
            config: d.test_launch.take(),
            browser: {
                // A fresh watch per launch: the program's output feeds it (brief 0037).
                let watch = ServerWatch::new();
                d.model.browser_watch = Some(watch.clone());
                d.model.browser_plan = None;
                d.model.browser_latency = None;
                let reuse = d.model.browser_reuse.clone();
                d.browser_step(watch, reuse)
            },
            choice,
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
        // A browser session's children detach first, and the session itself once they have ended (vscode-js-debug
        // answers the parent's `disconnect` once its children are gone; brief 0038): the sessions end in one order.
        let children = d.children_of(d.session_id);
        let wait_for_children = !children.is_empty() && d.client.is_some() && d.model.attached();
        d.model.detach_after_children = wait_for_children;
        if !children.is_empty() {
            for c in children {
                self.in_session(c, |s| {
                    let d = &mut s.debug;
                    if matches!(d.model.mode, Mode::Design | Mode::Stopping) || d.client.is_none() {
                        return;
                    }
                    d.model.last_driver = Some(driver.to_owned());
                    d.model.mode = Mode::Stopping;
                    let generation = d.generation();
                    let _ = d.send(
                        "disconnect",
                        json!({ "terminateDebuggee": false }),
                        Pending::Detach { generation },
                    );
                });
            }
        }
        let d = &mut self.debug;
        if wait_for_children {
            // Detached when the last child ends (`end_session`).
        } else if d.client.is_some() {
            // The session ends with the answer: an attached one detaches and the process keeps running (an adapter may
            // stay up after detaching; brief 0027); a launched one's adapter is asked to end the debuggee, and is killed
            // if still up (netcoredbg can take long to exit after answering on a loaded machine, and the headless
            // tests' stop timer never fires).
            let attached = d.model.attached();
            let _ = d.send(
                "disconnect",
                json!({ "terminateDebuggee": !attached }),
                Pending::Detach { generation },
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
        log_message: Option<String>,
        remove_after: Option<bool>,
        cx: &mut Context<Self>,
    ) -> Result<Option<(String, u32, bool)>, CommandError> {
        let mut edited = None;
        let files = if action == BreakpointAction::DeleteAll {
            let had_functions = !self.debug.model.breakpoints.functions().is_empty();
            let files = self.debug.model.breakpoints.delete_all();
            if had_functions {
                self.debug.send_function_breakpoints();
            }
            files
        } else {
            let (path, line) = self.debug_location(path.as_deref(), line, cx)?;
            let b = &mut self.debug.model.breakpoints;
            edited = Some((path.clone(), line, b.at(&path, line).is_some()));
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
                    if condition.is_some() {
                        b.forget_condition_error(&path, line);
                    }
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
                    if let Some(m) = log_message {
                        bp.log_message = (!m.is_empty()).then_some(m);
                    }
                    if let Some(r) = remove_after {
                        bp.remove_after = r;
                    }
                    // The person or an agent made it theirs: no longer run_until's or trace's.
                    bp.temporary = false;
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
        Ok(edited)
    }

    /// A function breakpoint by name: set (or change) or delete it, through `setFunctionBreakpoints`.
    #[allow(clippy::too_many_arguments)]
    fn debug_function_breakpoint(
        &mut self,
        function: String,
        action: BreakpointAction,
        enabled: Option<bool>,
        condition: Option<String>,
        hit_condition: Option<Option<cmds::HitCondition>>,
        remove_after: Option<bool>,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        let d = &mut self.debug;
        if d.client.is_some() && !d.caps.supports_function_breakpoints {
            return Err(CommandError::Failed(format!(
                "{} has no function breakpoints (DAP setFunctionBreakpoints); capabilities.function_breakpoints is \
                 false: set a breakpoint on the method's first line instead",
                d.adapter_name()
            )));
        }
        let b = &mut d.model.breakpoints;
        match action {
            BreakpointAction::Delete => {
                if !b.delete_function(&function) {
                    return Err(CommandError::InvalidInput(format!(
                        "there is no function breakpoint on {function}"
                    )));
                }
            }
            _ => {
                let f = b.ensure_function(&function);
                if let Some(e) = enabled {
                    f.enabled = e;
                }
                if let Some(c) = condition {
                    f.condition = (!c.trim().is_empty()).then(|| c.trim().to_owned());
                }
                if let Some(h) = hit_condition {
                    f.hit_condition = h;
                }
                if let Some(r) = remove_after {
                    f.remove_after = r;
                }
            }
        }
        d.send_function_breakpoints();
        self.debug_persist(cx);
        Ok(())
    }

    /// Send `path`'s breakpoints (and Run To Cursor's one-shot line) to a running session.
    fn debug_send_breakpoints(&mut self, path: &str) {
        self.debug.send_breakpoints(path);
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
        if let Some(e) = exec.filter(|_| self.debug.session_id == self.debug.active) {
            // Visual Studio brings the statement's document forward (opening it if needed) without moving the
            // caret; `apply_exec` scrolls to the statement.
            let _ = self.open_file(&e.path, None, window, cx);
        }
        self.apply_exec(cx);
    }

    /// Draw the execution point in its document, and nowhere else.
    fn apply_exec(&mut self, cx: &mut Context<Self>) {
        // Only the active session's execution point is drawn (brief 0028).
        if self.debug.session_id != self.debug.active {
            return;
        }
        let exec = self.debug.exec.clone();
        self.debug.shown_exec = exec.clone();
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
        let in_session = !self.debug.connected_ids().is_empty();
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
        // Another solution's startup projects are not this one's (brief 0020, 0028).
        self.debug.model.startup_project = None;
        self.debug.model.startup_projects.clear();
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

    /// Push the active session's model into the windows, the margin and the status bar (which names every session),
    /// and wake waiting agents. Whichever session is current, the windows show the active one (brief 0028).
    pub(super) fn refresh_debug(&mut self, cx: &mut Context<Self>) {
        let back = self.debug.session_id;
        let active = self.debug.active;
        self.debug.enter(active);
        self.refresh_debug_shown(cx);
        self.debug.enter(back);
        self.debug.publish_pauses();
    }

    fn refresh_debug_shown(&mut self, cx: &mut Context<Self>) {
        let sessions = self.debug.status_sessions();
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
        let parents = state::flatten_parents(&m.locals);
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
        let breakpoints = m.breakpoints.rows_for(&d.live_families());
        let exceptions = m.exceptions.clone();
        // The Call Stack and Threads windows' session selector (brief 0028): shown with two sessions or more.
        let choices: Vec<windows::SessionChoice> = if sessions.len() > 1 {
            // A child session (brief 0038) is listed under its parent, indented.
            let parents: HashMap<u32, u32> = d
                .sessions_info()
                .iter()
                .filter_map(|s| Some((s.id, s.parent?)))
                .collect();
            let depth = |mut id: u32| {
                let mut n = 0;
                while let Some(p) = parents.get(&id) {
                    n += 1;
                    id = *p;
                }
                n
            };
            let mut ordered: Vec<&(u32, String, &'static str, String)> = Vec::new();
            fn place<'a>(
                id: Option<u32>,
                all: &'a [(u32, String, &'static str, String)],
                parents: &HashMap<u32, u32>,
                out: &mut Vec<&'a (u32, String, &'static str, String)>,
            ) {
                for s in all.iter().filter(|s| parents.get(&s.0).copied() == id) {
                    out.push(s);
                    place(Some(s.0), all, parents, out);
                }
            }
            place(None, &sessions, &parents, &mut ordered);
            // A child whose parent is not live is listed at the top level.
            for s in &sessions {
                if !ordered.iter().any(|o| o.0 == s.0) {
                    ordered.push(s);
                }
            }
            ordered
                .into_iter()
                .map(|(id, name, mode, _)| windows::SessionChoice {
                    id: *id,
                    label: format!(
                        "{}{id}: {name} ({})",
                        "    ".repeat(depth(*id)),
                        mode_words(mode)
                    ),
                    active: *id == d.active,
                })
                .collect()
        } else {
            Vec::new()
        };
        let w = d.windows.clone();
        let c = choices.clone();
        w.call_stack.update(cx, |v, cx| v.set_sessions(c, cx));
        w.threads.update(cx, |v, cx| v.set_sessions(choices, cx));
        w.locals.update(cx, |v, cx| {
            v.set_parents(parents);
            v.set_rows(locals, locals_note, cx)
        });
        w.watch.update(cx, |v, cx| v.set_rows(watches, None, cx));
        w.call_stack.update(cx, |v, cx| v.set_rows(frames, cx));
        w.threads.update(cx, |v, cx| v.set_rows(threads, cx));
        w.breakpoints
            .update(cx, |v, cx| v.set_rows(breakpoints, cx));
        // The JavaScript group while a browser session is live (brief 0038).
        let javascript = d
            .live_families()
            .iter()
            .any(|(_, f)| matches!(f, AdapterFamily::Javascript | AdapterFamily::Browser));
        w.exceptions.update(cx, |v, cx| {
            v.set(exceptions, cx);
            v.set_javascript(javascript, cx);
        });
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
        let driving = match (m.agent_driving(), m.agents_allowed) {
            (_, false) => ", agents not allowed",
            (true, true) => ", agent driving",
            (false, true) => "",
        };
        d.menu.update(m, !d.live_ids().is_empty());
        let status = if sessions.len() > 1 {
            // Every session, the active one first: `Debugging: App (break: breakpoint, Program.cs line 12), Web
            // (running)`.
            let mut parts: Vec<&(u32, String, &'static str, String)> = sessions.iter().collect();
            parts.sort_by_key(|(id, ..)| *id != d.active);
            format!(
                "Debugging: {}",
                parts
                    .iter()
                    .map(|(_, name, _, state)| format!("{name} ({state})"))
                    .collect::<Vec<_>>()
                    .join(", ")
            )
        } else {
            match m.mode {
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
            }
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
        // A page still waiting for the server is not opened (brief 0037); one already open stays (Visual Studio's
        // behavior).
        if let Some(w) = d.model.browser_watch.take() {
            w.cancel();
        }
        if let Some(b) = d
            .model
            .session
            .as_mut()
            .and_then(|s| s.browser.as_mut())
            .filter(|b| b.waiting())
        {
            b.state = "failed".into();
            b.message = Some("the session ended before the server answered".into());
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
        d.trace_hits.clear();
        // run_until's points end with the session (a trace's are put back when it answers).
        let job_points = d
            .trace_job
            .as_ref()
            .map(|j| j.points.clone())
            .unwrap_or_default();
        d.model.removed_points = d.temporary_bindings(&job_points);
        d.model
            .breakpoints
            .remove_temporary(|b| !job_points.contains(&(b.path.clone(), b.line)));
        // A trace answering after the end reports its points as the session had them.
        if let Some(job) = d.trace_job.as_mut()
            && job.bound.is_none()
        {
            let b = &d.model.breakpoints;
            job.bound = Some(
                job.points
                    .iter()
                    .map(|(p, l)| {
                        b.at(p, *l)
                            .map_or((false, None), |b| (b.verified, b.message.clone()))
                    })
                    .collect(),
            );
        }
        trace(format_args!("debug ended {message:?}"));
        // An attached session that ends without the process exiting detached from it (brief 0027).
        let detached = (d.model.attached() && d.model.exit_code.is_none() && message.is_none())
            .then(|| {
                let s = d.model.session.as_ref().expect("attached");
                match (&s.url, s.process_id) {
                    // A page (brief 0038).
                    (Some(url), None) => format!(
                        "Detached from {} ({url}); the page keeps running.",
                        s.project
                    ),
                    _ => format!(
                        "Detached from {} (process {}); it keeps running.",
                        s.project,
                        s.process_id.unwrap_or_default()
                    ),
                }
            });
        if let Some(m) = &detached {
            d.console_line(m.clone());
        }
        let never_ran = d.model.capabilities.is_none();
        let parent = d.model.session.as_ref().and_then(|s| s.parent);
        d.model.end();
        d.model.message = detached.or(message);
        // Brief 0038: a browser session's children end with it; a browser attach of a compound that failed says so in
        // its server session's answer and the Output window, the server running on.
        let (sid, message, attached_for) =
            (d.session_id, d.model.message.clone(), d.model.attached_for);
        for c in self.debug.children_of(sid) {
            self.in_session(c, |s| {
                if let Some(client) = s.debug.client.take() {
                    client.kill();
                }
                s.end_session(None, cx);
            });
        }
        // The last child of a stopping browser session ended: the session detaches now.
        if let Some(p) = parent
            && self.debug.children_of(p).is_empty()
        {
            self.in_session(p, |s| {
                let d = &mut s.debug;
                if !std::mem::take(&mut d.model.detach_after_children)
                    || d.model.mode != Mode::Stopping
                    || d.session_id != p
                {
                    return;
                }
                let generation = d.generation();
                let _ = d.send(
                    "disconnect",
                    json!({ "terminateDebuggee": false }),
                    Pending::Detach { generation },
                );
            });
        }
        if let (Some(server), true, Some(m)) = (attached_for, never_ran, message) {
            self.in_session(server, |s| {
                if s.debug.session_id != server {
                    return;
                }
                let line =
                    format!("The page could not be debugged: {m} The server's session goes on.");
                s.debug.console_line(line.clone());
                s.debug.model.browser_attach = Some(BrowserAttach::Failed(m.clone()));
                if s.debug.model.mode != Mode::Design {
                    s.debug.model.message = Some(line);
                }
            });
        }
        self.apply_exec(cx);
        self.refresh_glyphs(cx);
    }

    /// The server session's page is up (brief 0038): when its start debugs the page (a compound's `browser` entry, or
    /// `browser: built_in` with the setting debugger.attachBrowser on), vscode-js-debug attaches to the tab, the URL
    /// or the tab the launch opened (`opened`), once.
    fn attach_page_of_server(&mut self, opened: Option<String>, cx: &mut Context<Self>) {
        let d = &self.debug;
        let Some(entry) = d.model.browser_entry.clone() else {
            return;
        };
        if matches!(d.model.mode, Mode::Design | Mode::Stopping)
            || matches!(&d.model.browser_attach, Some(BrowserAttach::Session(id)) if d.live_ids().contains(id))
        {
            return;
        }
        let target = match (entry.tab, entry.url, opened) {
            (Some(t), _, _) => PageTarget::Tab(t),
            (None, Some(u), _) => PageTarget::Url(u),
            (None, None, Some(t)) => PageTarget::Tab(t),
            (None, None, None) => {
                self.debug.console_line(
                    "The page opened in the system browser: the JavaScript debugger attaches to a tab of the Web \
                     Browser window (Debug > Open in Web Browser Window)."
                        .to_owned(),
                );
                return;
            }
        };
        let server = d.session_id;
        let driver = d.model.last_driver.clone().unwrap_or_else(|| "user".into());
        self.debug.timings.page_attach_started = Some(Instant::now());
        let id = self.debug_attach_page(
            target,
            entry.web_root.map(PathBuf::from),
            &driver,
            Some(server),
            cx,
        );
        self.in_session(server, |s| {
            s.debug.model.browser_attach = Some(BrowserAttach::Session(id));
        });
    }

    /// Apply a batch of messages from the launch thread, the adapter and the program.
    pub(super) fn on_debug_msgs(
        &mut self,
        batch: Vec<DebugMsg>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        for msg in batch {
            // Each message goes to its session, found by the generation it carries (brief 0028); one of a session
            // that is gone is dropped (an adapter connected for it is ended).
            match msg.generation() {
                None => self.on_debug_msg(msg, window, cx),
                Some(g) => match self.debug.session_of_generation(g) {
                    Some(sid) => self.in_session(sid, |s| s.on_debug_msg(msg, window, cx)),
                    None => {
                        if let DebugMsg::Connected { client, .. } = msg {
                            client.kill();
                        }
                    }
                },
            }
        }
        self.maybe_restart(window, cx);
        self.settle_active(cx);
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
                plan,
            } if generation == current => {
                let session = *session;
                self.debug.model.browser_plan = plan.map(|p| *p);
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
                let pid = session.process_id.and_then(|p| u32::try_from(p).ok());
                self.debug.model.session = Some(session);
                if let Some(run) = run {
                    if let Some(pid) = pid {
                        self.debug
                            .launched
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(pid);
                    }
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
                    self.debug.model.capabilities = Some(capabilities_row(
                        &started.capabilities,
                        &adapter_id,
                        self.debug.shell_log_points,
                    ));
                    let names = self.debug.model.breakpoints.function_breakpoints(false).0;
                    self.debug
                        .model
                        .breakpoints
                        .apply_function_answer(&names, &started.function_breakpoints);
                    for (path, answer) in &started.breakpoints {
                        let (lines, _) = self
                            .debug
                            .model
                            .breakpoints
                            .source_breakpoints(path, false, false, None);
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
                    // A page's child session runs (brief 0038's budget).
                    if self.debug.model.family() == AdapterFamily::Javascript
                        && let Some(at) = self.debug.timings.page_attach_started.take()
                    {
                        self.debug.timings.page_attach = Some(at.elapsed());
                        trace(format_args!(
                            "debug page attached {:.1} ms after the page opened",
                            at.elapsed().as_secs_f64() * 1e3
                        ));
                    }
                    // A compound's browser entry naming its page, for a server whose launch opens none (brief 0038).
                    if self.debug.model.browser_plan.is_none()
                        && self
                            .debug
                            .model
                            .browser_entry
                            .as_ref()
                            .is_some_and(|e| e.tab.is_some() || e.url.is_some())
                    {
                        self.attach_page_of_server(None, cx);
                    }
                    // What the handshake could not know: hit conditions and log points go to an adapter that has
                    // them.
                    let tracepoints = self
                        .debug
                        .model
                        .breakpoints
                        .all()
                        .iter()
                        .any(|b| b.log_message.is_some());
                    if self.debug.caps.supports_hit_conditional_breakpoints
                        || (self.debug.log_points() && tracepoints)
                    {
                        for f in self.debug.model.breakpoints.files() {
                            self.debug.send_breakpoints_here(&f);
                        }
                    }
                    let hit_functions = self
                        .debug
                        .model
                        .breakpoints
                        .functions()
                        .iter()
                        .any(|f| f.hit_condition.is_some());
                    if self.debug.caps.supports_hit_conditional_breakpoints && hit_functions {
                        self.debug.send_function_breakpoints_here();
                    }
                    if !self.debug.caps.supports_exception_filter_options
                        && !self.debug.model.exceptions.types.is_empty()
                    {
                        let line = format!(
                            "The exception types in Exception Settings are not used: {} has no exception filter \
                             options.",
                            self.debug.adapter_name()
                        );
                        self.debug.console_line(line);
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
            DebugMsg::Browser {
                generation,
                browser,
                line,
                latency,
            } if generation == current => {
                if let Some(line) = line {
                    self.debug.console_line(line);
                }
                if latency.is_some() {
                    self.debug.model.browser_latency = latency;
                }
                // A restart navigates this tab (brief 0037).
                if browser.state == "opened" && browser.tab.is_some() {
                    self.debug.model.browser_reuse = browser.tab.clone();
                }
                let opened = (browser.state == "opened").then(|| browser.tab.clone());
                if let Some(s) = self.debug.model.session.as_mut() {
                    s.browser = Some(browser);
                }
                // The page is up: debug it too (brief 0038).
                if let Some(tab) = opened {
                    self.attach_page_of_server(tab, cx);
                }
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
            DebugMsg::StartChild {
                generation,
                request,
                configuration,
                server,
            } if generation == current => {
                self.start_child_session(&request, configuration, server, cx);
            }
            DebugMsg::AdapterVersion {
                generation,
                version,
            } if generation == current => {
                self.debug.model.adapter_version = Some(version);
            }
            DebugMsg::Processes { listing } => match listing {
                Ok(mut out) => {
                    // Which browser session debugs each tab (brief 0038).
                    let sessions = self.debug.sessions_info();
                    for row in out.tabs.iter_mut().flatten() {
                        row.session = sessions
                            .iter()
                            .find(|s| {
                                s.parent.is_none() && s.tab.as_deref() == Some(row.id.as_str())
                            })
                            .map(|s| s.id);
                    }
                    if let Some(dialog) = self.debug.attach_dialog.clone() {
                        let rows = out.processes.clone();
                        let tabs = out.tabs.clone().unwrap_or_default();
                        dialog.update(cx, |d, cx| {
                            d.set_rows(rows, None, cx);
                            d.set_tabs(tabs, cx);
                        });
                    }
                    self.debug.processes = Some(out);
                }
                Err(e) => {
                    if let Some(dialog) = self.debug.attach_dialog.clone() {
                        dialog.update(cx, |d, cx| {
                            d.set_rows(Vec::new(), Some(format!("Cannot list processes: {e}")), cx)
                        });
                    }
                }
            },
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
                    let mut changed = false;
                    for (path, line, message) in condition_errors(&o.output) {
                        let hit = d
                            .model
                            .breakpoints
                            .set_condition_error(&path, line, &message);
                        changed |= hit;
                    }
                    if changed {
                        self.refresh_glyphs(cx);
                    }
                }
                // `console` (DAP's default), `important` and the rest: the adapter's messages.
                _ => {
                    d.model
                        .output_mut(OutputKind::Adapter)
                        .push_text(&o.output, None);
                    d.console(&o.output);
                    // Lines of the tracepoints the adapter prints (brief 0026).
                    d.adapter_log_text(&o.output);
                }
            },
            Event::Process(p) => {
                trace(format_args!("debug process {:?}", p.system_process_id));
                if let Some(s) = d.model.session.as_mut() {
                    s.process_id = p.system_process_id;
                    // The debuggee F5 started is Eludite's (brief 0027); an attached one is not.
                    if !s.attached
                        && let Some(pid) = p.system_process_id.and_then(|p| u32::try_from(p).ok())
                    {
                        d.launched
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .push(pid);
                    }
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
                    *row = capabilities_row(&merged, &adapter, d.shell_log_points);
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
                    // The `disconnect` answer ends the session, as a detach's does: vscode-js-debug keeps its socket
                    // open after it (brief 0038), and netcoredbg can take long to exit after it on a loaded machine
                    // (the session sat in `stopping` for the agents' 20 s waits on CI). Ending the session kills an
                    // adapter still up; one that closes first ends the session the same way.
                    let generation = d.generation();
                    let _ = d.send("disconnect", json!({}), Pending::Detach { generation });
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
                        at: Instant::now(),
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
                at,
            } if generation == current => {
                let st: StackTraceResponse = result
                    .ok()
                    .and_then(|b| serde_json::from_value(b).ok())
                    .unwrap_or_default();
                self.on_stop(stopped, thread, st, at, window, cx);
            }
            Pending::TraceEval {
                generation,
                hit,
                ix,
            } if generation == current => {
                self.debug.trace_value(hit, ix, result);
                self.refresh_glyphs(cx);
            }
            Pending::TraceResume {
                generation,
                record,
                at,
            } if generation == current => {
                let took = at.elapsed();
                if let Some(r) = record
                    .checked_sub(self.debug.traces_base)
                    .and_then(|i| self.debug.traces.get_mut(i))
                {
                    r.overhead = Some(took);
                }
                if let Err(e) = result {
                    self.debug
                        .console_line(format!("Tracepoint: continue failed: {e}"));
                }
            }
            Pending::SetFunctionBreakpoints { generation, names } if generation == current => {
                if let Ok(b) = result {
                    let r: SetBreakpointsResponse = serde_json::from_value(b).unwrap_or_default();
                    self.debug
                        .model
                        .breakpoints
                        .apply_function_answer(&names, &r.breakpoints);
                }
            }
            Pending::SetValue {
                generation,
                stop,
                reference,
                name,
                request,
                reply,
            } => {
                let fresh = generation == current
                    && stop == stop_now
                    && self.debug.model.mode == Mode::Break;
                let answer = if !fresh {
                    Err(format!(
                        "stale: the debuggee moved on (now stop {stop_now}, generation {current}) before the change \
                         was answered; read it again"
                    ))
                } else {
                    result.and_then(|b| {
                        serde_json::from_value::<SetVariableResponse>(b).map_err(|e| e.to_string())
                    })
                };
                match answer {
                    Ok(r) => {
                        let out = self
                            .debug
                            .value_changed(reference, &name, &r, request, stop);
                        for i in 0..self.debug.model.watches.len() {
                            self.debug_eval_watch(i);
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(Ok(out));
                        }
                    }
                    Err(e) => {
                        if fresh {
                            self.debug.model.message = Some(format!("Set value of {name}: {e}"));
                        }
                        if let Some(reply) = reply {
                            let _ = reply.send(Err(e));
                        }
                    }
                }
            }
            Pending::GotoTargets {
                generation,
                stop,
                thread,
                driver,
                line,
            } if generation == current => {
                let fresh = stop == stop_now && self.debug.model.mode == Mode::Break;
                let target = result.map(|b| {
                    serde_json::from_value::<GotoTargetsResponse>(b)
                        .unwrap_or_default()
                        .targets
                        .into_iter()
                        .next()
                });
                let d = &mut self.debug;
                match (fresh, target) {
                    (false, _) => {}
                    (true, Ok(Some(t))) => {
                        let sent = d.send(
                            "goto",
                            json!({"threadId": thread, "targetId": t.id}),
                            Pending::Goto { generation, stop },
                        );
                        match sent {
                            Ok(_) => {
                                d.model.resume(&driver);
                                d.exec = None;
                                self.apply_exec(cx);
                            }
                            Err(e) => d.goto_error = Some((generation, stop, e.to_string())),
                        }
                    }
                    (true, Ok(None)) => {
                        let m = format!(
                            "Set Next Statement: line {line} is not a statement the debugger can move to in this \
                             method"
                        );
                        d.model.message = Some(m.clone());
                        d.goto_error = Some((generation, stop, m));
                    }
                    (true, Err(e)) => {
                        d.model.message = Some(format!("Set Next Statement: {e}"));
                        d.goto_error = Some((generation, stop, e));
                    }
                }
            }
            Pending::Goto { generation, stop } if generation == current => {
                if let Err(e) = result {
                    let d = &mut self.debug;
                    if d.model.mode == Mode::Running && d.model.stop == stop {
                        d.model.mode = Mode::Break;
                    }
                    d.model.message = Some(format!("Set Next Statement: {e}"));
                    d.goto_error = Some((generation, stop, e));
                }
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
                let scope = locals_scope(&scopes.scopes);
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
            Pending::Detach { generation } if generation == current => {
                if let Err(e) = &result {
                    self.debug.console_line(format!("disconnect: {e}"));
                }
                if self.debug.model.mode != Mode::Design {
                    self.end_session(None, cx);
                }
            }
            Pending::Restart { generation } if generation == current => {
                if let Err(e) = result {
                    // The adapter could not restart: stop and start instead.
                    self.debug
                        .console_line(format!("restart: {e}; stopping and starting again"));
                    if let Some(start) = self.debug.last_start.clone() {
                        let driver = self
                            .debug
                            .model
                            .last_driver
                            .clone()
                            .unwrap_or_else(|| "user".into());
                        self.debug.restart_pending = Some((start, driver.clone()));
                        self.debug_stop(&driver, window, cx);
                    }
                } else {
                    self.debug.console_line("Restarted.");
                    self.debug.rerun_browser_step();
                }
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
        at: Instant,
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
            let log_points = d.log_points();
            if let Some(bp) = d.model.breakpoints.at_mut(path, *line) {
                bp.hits += 1;
                let hits = bp.hits;
                let skip = !d.caps.supports_hit_conditional_breakpoints
                    && !at_cursor
                    && bp.hit_condition.is_some_and(|h| !h.breaks_on(hits));
                // A tracepoint the adapter does not print (brief 0026): print it here and resume, never showing
                // the stop.
                let trace = (!skip && !at_cursor && !bp.adapter_logs(log_points))
                    .then(|| bp.log_message.clone())
                    .flatten();
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
                if let Some(message) = trace {
                    d.run_to_cursor = cursor;
                    let (path, line) = (path.clone(), *line);
                    d.start_trace_hit(path, line, hits, &message, thread, &frames, at);
                    return;
                }
            }
        }
        // A function breakpoint's stop: its hits, and its hit condition where the adapter ignores it.
        let function_hit = (s.reason == "function breakpoint")
            .then(|| {
                let b = &mut d.model.breakpoints;
                let name = b
                    .functions()
                    .iter()
                    .find(|f| {
                        f.adapter_id
                            .is_some_and(|id| s.hit_breakpoint_ids.contains(&id))
                    })
                    .or_else(|| {
                        frames.first().and_then(|top| {
                            b.functions()
                                .iter()
                                .find(|f| f.matches_frame(&top.row.name))
                        })
                    })
                    .map(|f| f.name.clone())?;
                let f = b.function_mut(&name)?;
                f.hits += 1;
                Some((name, f.hits, f.hit_condition))
            })
            .flatten();
        if let Some((_, hits, Some(h))) = &function_hit
            && !d.caps.supports_hit_conditional_breakpoints
            && !h.breaks_on(*hits)
        {
            d.run_to_cursor = cursor;
            let (generation, stop) = (d.generation(), d.model.stop);
            let _ = d.send(
                "continue",
                json!({ "threadId": thread }),
                Pending::Resume { generation, stop },
            );
            return;
        }
        // The stop is shown. run_until's temporary points end here, whatever the stop (as Run To Cursor's line); a
        // Delete-when-hit breakpoint ends at its own stop.
        let job_points = d
            .trace_job
            .as_ref()
            .map(|j| j.points.clone())
            .unwrap_or_default();
        d.model.removed_points = d.temporary_bindings(&job_points);
        let mut changed = d
            .model
            .breakpoints
            .remove_temporary(|b| !job_points.contains(&(b.path.clone(), b.line)));
        let mut persist = false;
        if s.reason == "breakpoint"
            && let Some((path, line)) = &top
            && d.model
                .breakpoints
                .at(path, *line)
                .is_some_and(|b| b.remove_after && !b.temporary)
        {
            d.model.breakpoints.delete(path, *line);
            if !changed.contains(path) {
                changed.push(path.clone());
            }
            persist = true;
        }
        if let Some((name, _, _)) = &function_hit
            && d.model
                .breakpoints
                .functions()
                .iter()
                .any(|f| &f.name == name && f.remove_after)
        {
            d.model.breakpoints.delete_function(name);
            d.send_function_breakpoints();
            persist = true;
        }
        for f in &changed {
            d.send_breakpoints(f);
        }
        if !changed.is_empty() || persist {
            self.refresh_glyphs(cx);
        }
        if persist {
            self.debug_persist(cx);
        }
        let d = &mut self.debug;
        if let Some((path, _)) = cursor {
            d.send_breakpoints_here(&path);
        }
        // Visual Studio switches to the process that broke (brief 0028), unless the person picked another a moment
        // ago, or the active session is itself at a break: the windows keep the session that broke first.
        if d.active != d.session_id {
            let held = d.live_ids().contains(&d.active)
                && (d.selected_at.is_some_and(|t| t.elapsed() < SELECTION_HOLD)
                    || d.mode_of(d.active) == Some(Mode::Break));
            if !held {
                trace(format_args!(
                    "debug: session {} broke and is active",
                    d.session_id
                ));
                d.active = d.session_id;
            }
        }
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
                            w.value = cmds::null_spelling(e.result);
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
                    Ok(e) => cmds::null_spelling(e.result),
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
                            cmds::null_spelling(e.result),
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
                        result: Some(cmds::null_spelling(e.result)),
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
    /// The session read (brief 0028).
    sid: u32,
    generation: u64,
    stop: u64,
}

const WINDOW_CLOSED: &str = "the window is closed";

impl Reader {
    fn new(
        this: &WeakEntity<Shell>,
        cx: &mut AsyncWindowContext,
        sid: u32,
    ) -> Result<Self, String> {
        let (generation, stop) = this
            .update(cx, |s, _| {
                s.in_session(sid, |s| (s.debug.model.generation, s.debug.model.stop))
            })
            .map_err(|_| WINDOW_CLOSED.to_owned())?;
        Ok(Self {
            this: this.clone(),
            sid,
            generation,
            stop,
        })
    }

    fn model<R>(
        &self,
        cx: &mut AsyncWindowContext,
        f: impl FnOnce(&Debugger) -> R,
    ) -> Result<R, String> {
        let sid = self.sid;
        self.this
            .update(cx, |s, _| s.in_session(sid, |s| f(&s.debug)))
            .map_err(|_| WINDOW_CLOSED.to_owned())
    }

    /// Send `requests` together and wait for every answer (at most [`AGENT_WAIT`]); the UI thread only sends them.
    async fn dap(
        &self,
        cx: &mut AsyncWindowContext,
        requests: Vec<(&'static str, Value)>,
    ) -> Result<Vec<Result<Value, String>>, String> {
        let (g, s, sid) = (self.generation, self.stop, self.sid);
        let receivers = self
            .this
            .update(cx, |shell, _| {
                let t = Instant::now();
                let r: Result<Vec<_>, String> = shell.in_session(sid, |shell| {
                    requests
                        .into_iter()
                        .map(|(c, a)| shell.debug.agent_request(g, s, c, a))
                        .collect()
                });
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
            .find(|s| !s.expensive && s.name.starts_with("Local:"))
            .or_else(|| {
                scopes
                    .scopes
                    .iter()
                    .find(|s| named(s, "locals", &["locals"]))
            })
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
        let (stop, sid) = (self.stop, self.sid);
        self.this
            .update(cx, |s, _| {
                s.in_session(sid, |s| {
                    if s.debug.model.stop == stop {
                        s.debug.counts.extend(counts);
                    }
                })
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
        // netcoredbg's `$exception` pseudo-local at an exception stop has two dozen members (Watson buckets, HResult,
        // the stack trace twice); the stop's `exception` already says what was thrown, so a depth snapshot leaves it
        // folded (its reference stays, for `variables` on demand) and spends the budget on the program's own locals.
        let mut frontier: Vec<Vec<usize>> = rows
            .iter()
            .enumerate()
            .filter(|(_, r)| r.reference > 0 && r.name != EXCEPTION_PSEUDO_LOCAL)
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
            let mut base = m.summary_base(budget);
            base.session = Some(d.session_id).filter(|id| *id > 0);
            (
                base,
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

    /// `set_variable` in a frame the windows do not show: its frame id and its locals scope, then the change.
    async fn set_value(
        &self,
        cx: &mut AsyncWindowContext,
        thread: Option<i64>,
        frame: usize,
        name: &str,
        value: &str,
    ) -> Result<oneshot::Receiver<Result<SetVariableOutput, String>>, String> {
        let stopped = self.model(cx, |d| d.model.stopped.as_ref().map(|s| s.thread))?;
        let thread = thread
            .or(stopped)
            .ok_or_else(|| "no thread has stopped".to_owned())?;
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
        let scope = locals_scope(&scopes.scopes)
            .or(scopes.scopes.first())
            .ok_or_else(|| format!("frame {frame} has no variables"))?
            .variables_reference;
        let (g, st, sid) = (self.generation, self.stop, self.sid);
        let (name, value) = (name.to_owned(), value.to_owned());
        self.this
            .update(cx, |s, _| {
                s.in_session(sid, |s| {
                    let m = &s.debug.model;
                    if m.generation != g || m.stop != st || m.mode != Mode::Break {
                        return Err(
                            "stale: the debuggee moved on before the change was sent".to_owned()
                        );
                    }
                    let (tx, rx) = oneshot::channel();
                    s.debug
                        .send_set_value(scope, true, Some(frame_id), &name, &value, Some(tx))
                        .map_err(|e| e.to_string())?;
                    Ok(rx)
                })
            })
            .map_err(|_| WINDOW_CLOSED.to_owned())?
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

/// The summary an agent's interrupted wait answers with (proposal 0001 rule 5): the state the person caused, at once.
/// The agent's next driving command is stale until it reads the state.
fn interrupted(s: &mut Shell, budget: &Budget) -> StopSummary {
    let d = &mut s.debug;
    d.agent_stale = true;
    d.timings.interrupt_answered = Some(Instant::now());
    let mut summary = d.summary(budget);
    summary.interrupted_by = Some("user".into());
    summary
}

/// What an agent's command waited for, then its answer (brief 0025). `epoch` is [`Debugger::interrupt`] when the
/// command was applied: a command of the person's since ends a wait (brief 0027).
async fn follow_up(
    this: WeakEntity<Shell>,
    cx: &mut AsyncWindowContext,
    out: DebugOutput,
    follow: Follow,
) -> Outcome {
    let closed = || CommandError::Failed(WINDOW_CLOSED.into());
    let Follow {
        sid,
        epoch,
        what: follow,
    } = follow;
    match follow {
        Followup::Compound { ids, wait, budget } => {
            // A compound start (brief 0028): the first session to break, or every session's mode on a timeout.
            let deadline = Instant::now() + wait;
            let broke = loop {
                let (found, waiter) = this
                    .update(cx, |s, _| {
                        let state = |s: &mut Shell, id: u32| {
                            s.in_session(id, |s| {
                                (s.debug.session_id == id).then(|| {
                                    (
                                        s.debug.model.mode == Mode::Break
                                            && s.debug.model.settled(),
                                        s.debug.model.mode == Mode::Design,
                                    )
                                })
                            })
                        };
                        let states: Vec<(u32, Option<(bool, bool)>)> =
                            ids.iter().map(|id| (*id, state(s, *id))).collect();
                        let hit = states
                            .iter()
                            .find(|(_, st)| st.is_some_and(|(b, _)| b))
                            .map(|(id, _)| *id);
                        let over = states.iter().all(|(_, st)| st.is_none_or(|(_, d)| d));
                        let found = hit.or(over.then(|| ids[0]));
                        (found, found.is_none().then(|| s.debug_waiter()))
                    })
                    .map_err(|_| closed())?;
                if let Some(id) = found {
                    break Some(id);
                }
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break None;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            };
            match broke {
                Some(id) => summarize(&this, cx, id, None, None, &budget)
                    .await
                    .map(|s| DebugOutput::Summary(Box::new(s))),
                None => {
                    let mut summary = this
                        .update(cx, |s, _| {
                            let mut out = s.in_session(ids[0], |s| s.debug.summary(&budget));
                            out.sessions = ids
                                .iter()
                                .filter_map(|id| {
                                    s.in_session(*id, |s| {
                                        (s.debug.session_id == *id).then(|| {
                                            cmds::CompoundSessionRow {
                                                id: *id,
                                                name: Debugger::name_of(
                                                    &s.debug.model,
                                                    &s.debug.session_name,
                                                ),
                                                mode: s.debug.model.mode.as_str().into(),
                                            }
                                        })
                                    })
                                })
                                .collect();
                            out
                        })
                        .map_err(|_| closed())?;
                    summary.timed_out = Some(true);
                    Ok(DebugOutput::Summary(Box::new(summary)))
                }
            }
        }
        Followup::Processes { filter, roots } => {
            let (tx, rx) = oneshot::channel();
            let bus = this
                .update(cx, |s, _| s.debug.browser_bus.clone())
                .map_err(|_| closed())?;
            std::thread::Builder::new()
                .name("debug-attach".into())
                .spawn(move || {
                    let _ = tx.send(
                        list_processes(filter.as_deref(), &roots)
                            .map(|o| with_tabs(o, bus.as_ref())),
                    );
                })
                .map_err(|e| CommandError::Failed(e.to_string()))?;
            match rx.await {
                Ok(Ok(mut out)) => {
                    // Which browser session debugs each tab (brief 0038).
                    let sessions = this
                        .update(cx, |s, _| s.debug.sessions_info())
                        .map_err(|_| closed())?;
                    for row in out.tabs.iter_mut().flatten() {
                        row.session = sessions
                            .iter()
                            .find(|s| {
                                s.parent.is_none() && s.tab.as_deref() == Some(row.id.as_str())
                            })
                            .map(|s| s.id);
                    }
                    Ok(DebugOutput::Processes(out))
                }
                Ok(Err(e)) => Err(CommandError::Failed(format!("cannot list processes: {e}"))),
                Err(_) => Err(CommandError::Failed("the process listing failed".into())),
            }
        }
        Followup::Breakpoint { edit, target, wait } => {
            let deadline = Instant::now() + wait;
            loop {
                let waiter = this
                    .update(cx, |s, _| {
                        s.debug.awaiting(&target).then(|| s.debug_waiter())
                    })
                    .map_err(|_| closed())?;
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            }
            this.update(cx, |s, _| s.debug.breakpoint_answer(edit, Some(&target)))
                .map_err(|_| closed())
        }
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
            after,
            fresh,
            points,
            wait,
            budget,
        } => {
            let deadline = Instant::now() + wait;
            let settled = loop {
                let cut = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| {
                            (s.debug.interrupt != epoch).then(|| interrupted(s, &budget))
                        })
                    })
                    .map_err(|_| closed())?;
                if let Some(summary) = cut {
                    return Ok(DebugOutput::Summary(Box::new(summary)));
                }
                let (done, failed, waiter) = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| {
                            let failed = pause
                                .and_then(|g| {
                                    s.debug
                                        .pause_error
                                        .as_ref()
                                        .filter(|(pg, _)| *pg == g)
                                        .map(|(_, m)| format!("Break All failed: {m}"))
                                })
                                .or_else(|| {
                                    let (g, st) = after?;
                                    s.debug
                                        .goto_error
                                        .as_ref()
                                        .filter(|(eg, es, _)| (*eg, *es) == (g, st))
                                        .map(|(_, _, m)| m.clone())
                                });
                            // Set Next Statement: the break it answers is the one after the goto.
                            let moved = after.is_none_or(|(g, st)| {
                                let m = &s.debug.model;
                                m.generation != g || m.mode == Mode::Design || m.stop > st
                            });
                            // A restart that stops and starts answers in the new session (or once it fails to start).
                            let renewed = fresh.is_none_or(|g| {
                                let m = &s.debug.model;
                                m.generation > g && s.debug.restart_pending.is_none()
                            });
                            let done = moved && renewed && s.debug_settled(start);
                            // A waiter only while waiting: none is left behind once the command answers.
                            let waiter = (!done && failed.is_none()).then(|| s.debug_waiter());
                            (done, failed, waiter)
                        })
                    })
                    .map_err(|_| closed())?;
                if let Some(m) = failed {
                    return Err(CommandError::Failed(m));
                }
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break done;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            };
            let mut summary = summarize(&this, cx, sid, None, None, &budget).await?;
            if !settled {
                summary.timed_out = Some(true);
            }
            if start && summary.sessions.is_empty() {
                summary.sessions = this
                    .update(cx, |s, _| s.compound_rows(sid))
                    .map_err(|_| closed())?;
            }
            if !points.is_empty() {
                summary.points_failed = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| s.debug.points_failed(&points, &summary))
                    })
                    .map_err(|_| closed())?;
            }
            Ok(DebugOutput::Summary(Box::new(summary)))
        }
        Followup::Trace {
            until,
            wait,
            budget,
        } => {
            let deadline = Instant::now() + wait;
            let stopped_by = loop {
                let (ended, waiter) = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| {
                            // The person resumed, paused, stopped or restarted: the trace answers what it has.
                            let ended = if s.debug.interrupt != epoch {
                                s.debug.agent_stale = true;
                                s.debug.timings.interrupt_answered = Some(Instant::now());
                                Some("interrupted")
                            } else {
                                s.debug.trace_ended()
                            };
                            (ended, ended.is_none().then(|| s.debug_waiter()))
                        })
                    })
                    .map_err(|_| closed())?;
                if let Some(e) = ended {
                    break e;
                }
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break "timeout";
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            };
            let _ = until;
            let mut out = this
                .update(cx, |s, cx| {
                    s.in_session(sid, |s| {
                        let out = s.debug.finish_trace(stopped_by);
                        s.refresh_glyphs(cx);
                        s.refresh_debug(cx);
                        out
                    })
                })
                .map_err(|_| closed())?;
            if stopped_by == "stopped" {
                out.summary = Some(Box::new(
                    summarize(&this, cx, sid, None, None, &budget).await?,
                ));
            }
            Ok(DebugOutput::Trace(Box::new(out)))
        }
        Followup::SetValue(rx) => {
            let timer = real_timer(AGENT_WAIT);
            match futures::future::select(rx, timer).await {
                futures::future::Either::Left((Ok(Ok(v)), _)) => Ok(DebugOutput::SetVariable(v)),
                futures::future::Either::Left((Ok(Err(e)), _)) => Err(CommandError::Failed(e)),
                futures::future::Either::Left((Err(_), _)) => Err(CommandError::Failed(
                    "the session ended before the change was answered".into(),
                )),
                futures::future::Either::Right(_) => Err(CommandError::Failed(
                    "the debug adapter did not answer the change in time".into(),
                )),
            }
        }
        Followup::SetVariable {
            thread,
            frame,
            name,
            value,
        } => {
            let r = Reader::new(&this, cx, sid).map_err(CommandError::Failed)?;
            let rx = r
                .set_value(cx, thread, frame, &name, &value)
                .await
                .map_err(CommandError::Failed)?;
            let follow = Follow {
                sid,
                epoch,
                what: Followup::SetValue(rx),
            };
            Box::pin(follow_up(this, cx, out, follow)).await
        }
        Followup::Ended { wait, all } => {
            let deadline = Instant::now() + wait;
            loop {
                let waiter = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| {
                            let ended = if all {
                                s.debug
                                    .live_ids()
                                    .into_iter()
                                    .all(|id| s.in_session(id, |s| s.debug_settled(false)))
                            } else {
                                s.debug_settled(false)
                            };
                            (!ended).then(|| s.debug_waiter())
                        })
                    })
                    .map_err(|_| closed())?;
                let left = deadline.saturating_duration_since(Instant::now());
                let Some(waiter) = waiter.filter(|_| !left.is_zero()) else {
                    break;
                };
                let timer = real_timer(left);
                let _ = futures::future::select(waiter, timer).await;
            }
            this.update(cx, |s, _| s.in_session(sid, |s| s.debug_state()))
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
                let cut = this
                    .update(cx, |s, _| {
                        s.in_session(sid, |s| {
                            (s.debug.interrupt != epoch).then(|| interrupted(s, &budget))
                        })
                    })
                    .map_err(|_| closed())?;
                if let Some(summary) = cut {
                    return Ok(DebugOutput::Summary(Box::new(summary)));
                }
                let (holds, waiter) = this
                    .update(cx, |s, _| {
                        let own = s.in_session(sid, |s| {
                            wait_satisfied(&s.debug.model, until, stop, baseline).map(|w| (w, sid))
                        });
                        // A browser session's wait ends at its first stop or its children's (brief 0038).
                        let mut child = || {
                            if !matches!(until, WaitUntil::Stopped | WaitUntil::Any) {
                                return None;
                            }
                            s.debug.children_of(sid).into_iter().find_map(|c| {
                                s.in_session(c, |s| {
                                    let m = &s.debug.model;
                                    (s.debug.session_id == c
                                        && m.mode == Mode::Break
                                        && m.settled())
                                    .then_some(("stopped", c))
                                })
                            })
                        };
                        let holds = match own {
                            Some(("terminated", _)) | None => child().or(own),
                            other => other,
                        };
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
            let (satisfied, sid) = match satisfied {
                Some((why, id)) => (Some(why), id),
                None => (None, sid),
            };
            let mut summary = summarize(&this, cx, sid, None, None, &budget).await?;
            match satisfied {
                Some(why) => summary.satisfied = Some(why.into()),
                None => summary.timed_out = Some(true),
            }
            this.update(cx, |s, _| {
                s.in_session(sid, |s| {
                    s.debug.timings.wait_answered = Some(Instant::now())
                })
            })
            .map_err(|_| closed())?;
            Ok(DebugOutput::Summary(Box::new(summary)))
        }
        Followup::Snapshot {
            thread,
            frame,
            budget,
        } => {
            let r = Reader::new(&this, cx, sid).map_err(CommandError::Failed)?;
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
            let r = Reader::new(&this, cx, sid).map_err(CommandError::Failed)?;
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
            let r = Reader::new(&this, cx, sid).map_err(CommandError::Failed)?;
            r.variables(cx, target, start, count, depth, filter, max_value_chars)
                .await
                .map(DebugOutput::Variables)
                .map_err(CommandError::Failed)
        }
        Followup::ExceptionInfo { thread } => {
            let r = Reader::new(&this, cx, sid).map_err(CommandError::Failed)?;
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
    sid: u32,
    thread: Option<i64>,
    frame: Option<usize>,
    budget: &Budget,
) -> Result<StopSummary, CommandError> {
    for _ in 0..3 {
        let r = Reader::new(this, cx, sid).map_err(CommandError::Failed)?;
        match r.summary(cx, thread, frame, budget).await {
            Ok(s) => return Ok(s),
            Err(e) if e.starts_with("stale") => continue,
            Err(e) => return Err(CommandError::Failed(e)),
        }
    }
    this.update(cx, |s, _| s.in_session(sid, |s| s.debug.summary(budget)))
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
/// The pseudo-local netcoredbg (and vsdbg-style adapters) list at an exception stop, holding the thrown exception.
const EXCEPTION_PSEUDO_LOCAL: &str = "$exception";

/// The breakpoints whose condition netcoredbg could not evaluate at a hit, from the lines it prints on stderr as it
/// stops there (Visual Studio's behavior): `Breakpoint error: The condition for a breakpoint failed to execute. The
/// condition was 'c'. The error returned was 'e'. - <path>:<line>`. Each is (the normalized path, the line, the message
/// up to the location).
fn condition_errors(text: &str) -> Vec<(String, u32, String)> {
    text.lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("Breakpoint error: ")?;
            let (message, at) = rest.rsplit_once(" - ")?;
            let (path, line) = at.trim().rsplit_once(':')?;
            let line = line.trim().parse::<u32>().ok()?;
            let path = normalize_path(Path::new(path.trim()))
                .to_string_lossy()
                .into_owned();
            Some((path, line, message.trim().to_owned()))
        })
        .collect()
}

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
fn capabilities_row(c: &Capabilities, adapter: &str, shell_log_points: bool) -> CapabilitiesRow {
    let by = |adapter: bool| if adapter { "adapter" } else { "shell" }.to_owned();
    CapabilitiesRow {
        adapter: adapter.to_owned(),
        pause: true,
        set_variable: c.supports_set_variable || c.supports_set_expression,
        exception_info: c.supports_exception_info_request,
        function_breakpoints: c.supports_function_breakpoints,
        log_points: by(c.supports_log_points && !shell_log_points),
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
        // lldb-dap honors `start` and `count` too (brief 0029).
        variable_paging: c.supports_variable_paging || adapter == "mono" || adapter == "lldb",
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

/// The node (at any depth) whose members are variables reference `reference`.
fn find_reference(nodes: &[VarNode], reference: i64) -> Option<&VarNode> {
    nodes.iter().find_map(|n| {
        if n.reference == reference {
            Some(n)
        } else {
            find_reference(n.children.as_deref()?, reference)
        }
    })
}

fn find_reference_mut(nodes: &mut [VarNode], reference: i64) -> Option<&mut VarNode> {
    for n in nodes.iter_mut() {
        if n.reference == reference {
            return Some(n);
        }
        if let Some(found) = n
            .children
            .as_deref_mut()
            .and_then(|c| find_reference_mut(c, reference))
        {
            return Some(found);
        }
    }
    None
}

impl Debugger {
    /// A value changed (brief 0026): the Locals and Watch windows' node shows it; the answer row.
    fn value_changed(
        &mut self,
        reference: i64,
        name: &str,
        r: &SetVariableResponse,
        request: &str,
        stop: u64,
    ) -> SetVariableOutput {
        let m = &mut self.model;
        let update = |n: &mut VarNode| {
            n.value = cmds::null_spelling(r.value.clone());
            if let Some(t) = r.type_name.clone().filter(|t| !t.is_empty()) {
                n.type_name = Some(t);
            }
            n.reference = r.variables_reference;
            n.indexed = r.indexed_variables;
            n.named = r.named_variables;
            n.children = None;
            n.expanded = false;
        };
        if reference == m.locals_reference {
            if let Some(n) = m.locals.iter_mut().find(|v| v.name == name) {
                update(n);
            }
        } else {
            for tree in [&mut m.locals, &mut m.watches] {
                if let Some(n) = find_reference_mut(tree, reference)
                    .and_then(|p| p.children.as_mut())
                    .and_then(|c| c.iter_mut().find(|v| v.name == name))
                {
                    update(n);
                }
            }
        }
        SetVariableOutput {
            name: name.to_owned(),
            value: cmds::null_spelling(r.value.clone()),
            type_name: r.type_name.clone().filter(|t| !t.is_empty()),
            reference: r.variables_reference,
            request: Some(request.to_owned()),
            pending: false,
            stop,
        }
    }
}

fn set_children(nodes: &mut [VarNode], path: &[usize], vars: Vec<VarNode>) {
    if let Some(n) = node_mut(nodes, path) {
        n.children = Some(vars);
        n.loading = false;
    }
}
