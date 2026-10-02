//! Run and debug (brief 0018): F5 runs the startup project under netcoredbg through `eludite-dap`, Ctrl+F5 without
//! the debugger, with Visual Studio's debugger windows, breakpoints in the margin, the execution point, data tips,
//! the Debug menu and keys and a status bar slot. Every action is an `eludite.debug.*` command, and agents drive a
//! session through the same commands and read the same state the windows render ([`state`], whose module docs give
//! the two-driver rules).
//!
//! - **Never waiting on the adapter.** The launch (project resolution, the launch configuration, finding and
//!   starting the adapter, DAP's handshake) runs on a `debug-launch` thread. Every later request is sent with
//!   [`DapClient::request`], which only queues it; answers and events come back through one channel and are applied
//!   on the UI thread in batches, tagged with the session generation so an old session's never land.
//! - **Hit counts.** netcoredbg ignores `hitCondition`, so the shell counts hits itself: a stop at a breakpoint whose
//!   hit condition is not met resumes at once and is never shown. An adapter that supports hit conditions gets them.
//! - **Persistence.** Breakpoints, exception settings and watch expressions are saved per solution under
//!   `<config dir>/eludite/breakpoints/solutions/`, written off the UI thread.

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

use eludite_commands::debug::{
    self as cmds, BreakpointAction, DebugOutput, DebugRequest, EvalContext, EvaluateOutput,
    SessionRow, StoppedRow, ThreadRow, VariableRow,
};
use eludite_commands::view::{DockEdge, DockTarget, ViewRequest, ViewTarget as _};
use eludite_commands::{Caller, CommandError};
use eludite_dap::discovery::AdapterSearch;
use eludite_dap::session::{self as dap_session, StartKind, StartPlan, Started};
use eludite_dap::types::{
    Capabilities, EvaluateResponse, Event, ExceptionInfoResponse, ScopesResponse,
    SetBreakpointsResponse, StackTraceResponse, StoppedEvent, ThreadsResponse, VariablesResponse,
};
use eludite_dap::{ClientEvent, Connection, DapClient, launch, transport};
use eludite_docking::ids;
use eludite_editor::{EditorEvent, ExecutionKind};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use gpui::{AppContext as _, Context, Window};
use serde_json::{Value, json};

use self::state::{
    DebugModel, Frame, Mode, Persisted, VarNode, exception_filters, flatten, node_mut,
};
use self::windows::{DebugWindows, StackRow, ThreadLine};
use super::Shell;
use super::documents::normalize_path;

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
    /// Connect here instead of locating netcoredbg (tests).
    pub connect: Option<Connector>,
    pub search: AdapterSearch,
    /// The directory breakpoints persist in (`None`: `<config dir>/eludite/breakpoints`).
    pub store_dir: Option<PathBuf>,
    /// The `dotnet` Start Without Debugging runs (`dotnet` on `PATH`).
    pub dotnet: String,
}

impl DebugSetup {
    pub fn from_env() -> Self {
        Self {
            connect: None,
            search: AdapterSearch::from_env(),
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
    },
    LaunchFailed {
        generation: u64,
        message: String,
    },
    Client {
        generation: u64,
        event: ClientEvent,
    },
    Output {
        generation: u64,
        text: String,
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
    /// Step sent to its break shown (locals loaded and the windows given them).
    pub steps: Vec<Duration>,
    /// The last time the windows were given a break's locals.
    pub locals_shown: Option<Instant>,
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
    console_seen: (u64, usize),
    /// `breakpoint` events for ids not known yet: netcoredbg binds breakpoints (events) before the handshake's
    /// `setBreakpoints` answers reach the shell.
    early_breakpoints: Vec<eludite_dap::types::Breakpoint>,
    pub timings: DebugTimings,
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
                console_seen: (u64::MAX, 0),
                early_breakpoints: Vec::new(),
                timings: DebugTimings::default(),
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

    fn console(&mut self, text: &str) {
        let text = text.replace('\r', "");
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
        }
        self.model.push_console(line);
    }

    fn generation(&self) -> u64 {
        self.model.generation
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
) -> Result<launch::LaunchConfig, String> {
    let project = match hint {
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
                "open a solution first: there is no startup project".to_owned()
            } else {
                "the solution has no executable project to start".to_owned()
            }
        })?,
    };
    launch::launch_config(&project, profile)
}

/// What the launch thread needs.
struct LaunchJob {
    generation: u64,
    debug: bool,
    hint: Option<String>,
    profile: Option<String>,
    projects: Vec<PathBuf>,
    solution_dir: Option<PathBuf>,
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
    ) {
        Ok(c) => c,
        Err(e) => return fail(e),
    };
    let mut session = SessionRow {
        project: config.project.to_string_lossy().into_owned(),
        program: config.program.to_string_lossy().into_owned(),
        args: config.args.clone(),
        cwd: config.cwd.to_string_lossy().into_owned(),
        profile: config.profile.clone(),
        debug,
        adapter: None,
        process_id: None,
    };
    if !debug {
        let (_, args) = config.without_debugging();
        let cmd = setup.dotnet.clone();
        let child = Command::new(&cmd)
            .args(&args)
            .current_dir(&config.cwd)
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
        for s in streams.into_iter().flatten() {
            let tx = tx.clone();
            readers.push(std::thread::spawn(move || {
                for line in std::io::BufReader::new(s).lines() {
                    let Ok(line) = line else { break };
                    let _ = tx.unbounded_send(DebugMsg::Output {
                        generation,
                        text: format!("{line}\n"),
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
    let (connection, adapter) = match &setup.connect {
        Some(connect) => match connect() {
            Ok(c) => {
                let d = c.description.clone();
                (c, d)
            }
            Err(e) => return fail(format!("cannot reach the debug adapter: {e}")),
        },
        None => {
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
    };
    session.adapter = Some(adapter);
    let arguments = config.netcoredbg_arguments();
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
        adapter_id: "coreclr".into(),
        kind: StartKind::Launch,
        arguments,
        breakpoints,
        exception_filters: filters,
    };
    let result = dap_session::start(&client, &plan, HANDSHAKE_TIMEOUT).map_err(|e| e.to_string());
    let _ = tx.unbounded_send(DebugMsg::Started { generation, result });
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
        use futures::future::Either;
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
                let resumes = request.resumes();
                let start = matches!(request, DebugRequest::Start { .. });
                let wait = request
                    .wait_ms()
                    .map(Duration::from_millis)
                    .unwrap_or(AGENT_WAIT);
                let applied = this.update_in(cx, |shell, window, cx| {
                    eludite_commands::with_caller(caller.clone(), || {
                        shell.apply_debug(request, &caller, true, window, cx)
                    })
                });
                let outcome = match applied {
                    Err(_) => Err(CommandError::Failed("the window is closed".into())),
                    Ok(Err(e)) => Err(e),
                    Ok(Ok((out, Some(rx)))) => {
                        let timer = cx.background_executor().timer(AGENT_WAIT);
                        match futures::future::select(rx, timer).await {
                            Either::Left((Ok(e), _)) => Ok(DebugOutput::Evaluate(e)),
                            Either::Left((Err(_), _)) => Ok(match out {
                                DebugOutput::Evaluate(p) => DebugOutput::Evaluate(eval_failed(
                                    &p.expression,
                                    p.stop,
                                    "the session ended before the answer arrived",
                                )),
                                other => other,
                            }),
                            Either::Right(_) => Ok(out),
                        }
                    }
                    Ok(Ok((out, None))) if resumes && !wait.is_zero() => {
                        let deadline = Instant::now() + wait;
                        loop {
                            let Ok((settled, waiter)) =
                                this.update(cx, |s, _| (s.debug_settled(start), s.debug_waiter()))
                            else {
                                break;
                            };
                            let left = deadline.saturating_duration_since(Instant::now());
                            if settled || left.is_zero() {
                                break;
                            }
                            let timer = cx.background_executor().timer(left);
                            let _ = futures::future::select(waiter, timer).await;
                        }
                        Ok(this.update(cx, |s, _| s.debug_state()).unwrap_or(out))
                    }
                    Ok(Ok((out, None))) => Ok(out),
                };
                let _ = reply.send(outcome);
            }
        });
        (msg_task, job_task)
    }

    /// Apply a debug command (the UI-thread half of [`DebugBus`]). An agent's evaluate also returns a receiver for
    /// the answer.
    pub fn apply_debug(
        &mut self,
        request: DebugRequest,
        caller: &Caller,
        agent: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(DebugOutput, Option<oneshot::Receiver<EvaluateOutput>>), CommandError> {
        self.debug.model.check(&request)?;
        let driver = driver_of(caller);
        match request {
            DebugRequest::State => {}
            DebugRequest::Start {
                project,
                debug,
                profile,
                ..
            } => self.debug_start(project, debug, profile, &driver, cx),
            DebugRequest::Stop => self.debug_stop(&driver, cx),
            DebugRequest::Continue { .. } => self.debug_resume("continue", None, &driver)?,
            DebugRequest::Step { kind, thread, .. } => {
                self.debug.timings.step_sent = Some(Instant::now());
                self.debug_resume(kind.dap_command(), thread, &driver)?;
            }
            DebugRequest::RunToCursor { path, line, .. } => {
                let (path, line) = self.debug_location(path.as_deref(), line, cx)?;
                self.debug.run_to_cursor = Some((path.clone(), line));
                self.debug_send_breakpoints(&path);
                self.debug_resume("continue", None, &driver)?;
            }
            DebugRequest::Breakpoint {
                path,
                line,
                action,
                enabled,
                condition,
                hit_condition,
            } => {
                self.debug_breakpoint(path, line, action, enabled, condition, hit_condition, cx)?
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
                        Some(rx),
                    ),
                    Err(pending) => (DebugOutput::Evaluate(pending), None),
                });
            }
            DebugRequest::SelectFrame { thread, frame, .. } => {
                self.debug_select(thread, frame, window, cx)?
            }
            DebugRequest::AddWatch(expression) => {
                self.debug.model.watches.push(VarNode::watch(&expression));
                let ix = self.debug.model.watches.len() - 1;
                self.debug_eval_watch(ix);
                self.debug_persist(cx);
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
            }
        }
        self.refresh_debug(cx);
        Ok((self.debug_state(), None))
    }

    fn debug_start(
        &mut self,
        project: Option<String>,
        debug: bool,
        profile: Option<String>,
        driver: &str,
        cx: &mut Context<Self>,
    ) {
        let solution_dir = self.solution_dir();
        let d = &mut self.debug;
        d.model.begin(Mode::Launching, driver);
        d.timings = DebugTimings {
            start: Some(Instant::now()),
            ..DebugTimings::default()
        };
        d.pending.clear();
        d.caps = Capabilities::default();
        d.run_to_cursor = None;
        d.early_breakpoints.clear();
        d.model.console.clear();
        d.console_partial.clear();
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
            let _ = self.controller.apply(ViewRequest::Show {
                id: ids::DEBUG_CONSOLE.into(),
            });
        }
        self.refresh_glyphs(cx);
    }

    /// Visual Studio's Debug layout the first time: Locals and Watch beside the Error List, Call Stack, Breakpoints
    /// and the Debug Console in a second group at the bottom. A layout the user changed is left alone.
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
            for id in [ids::BREAKPOINTS, ids::DEBUG_CONSOLE] {
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

    fn debug_stop(&mut self, driver: &str, cx: &mut Context<Self>) {
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
    fn debug_persist(&mut self, cx: &mut Context<Self>) {
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
        let seen = (d.model.console_total, d.console_partial.len());
        if seen != d.console_seen {
            d.console_seen = seen;
            let mut lines = d.model.console.clone();
            if !d.console_partial.is_empty() {
                lines.push_back(d.console_partial.clone());
            }
            w.console.update(cx, |v, cx| v.set_lines(&lines, cx));
        }
        let name = m
            .session
            .as_ref()
            .map(|s| file_name(&s.project))
            .map(|n| n.trim_end_matches(".csproj").to_owned())
            .unwrap_or_default();
        let status = match m.mode {
            Mode::Design => m.message.clone().unwrap_or_default(),
            Mode::Launching => format!("Debugging: starting {name}\u{2026}"),
            Mode::Running => format!("Debugging: {name} (running)"),
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
                    "Debugging: {name} (break: {}{at})",
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
                }
            }
            DebugMsg::Launched {
                generation,
                session,
                run,
            } if generation == current => {
                let program = file_name(&session.program);
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
            DebugMsg::Started { generation, result } if generation == current => match result {
                Ok(started) => {
                    self.debug.caps = started.capabilities.clone();
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
            DebugMsg::Output { generation, text } if generation == current => {
                self.debug.console(&text);
            }
            DebugMsg::ProgramExited { generation, code } if generation == current => {
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
            Event::Output(o) => {
                if o.category.as_deref() != Some("telemetry") {
                    d.console(&o.output);
                }
            }
            Event::Process(p) => {
                if let Some(s) = d.model.session.as_mut() {
                    s.process_id = p.system_process_id;
                }
            }
            Event::Breakpoint(b) => {
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
            }
            Event::Exited(e) => {
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
                let vars: Vec<VarNode> = match &result {
                    Ok(b) => serde_json::from_value::<VariablesResponse>(b.clone())
                        .unwrap_or_default()
                        .variables
                        .iter()
                        .take(state::MAX_VARIABLES)
                        .map(VarNode::from_dap)
                        .collect(),
                    Err(_) => Vec::new(),
                };
                match target {
                    VarTarget::Locals(path) if path.is_empty() => self.locals_done(vars),
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
                if let Ok(b) = result {
                    let e: ExceptionInfoResponse = serde_json::from_value(b).unwrap_or_default();
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
        if s.reason == "exception" && d.caps.supports_exception_info_request {
            let (generation, stop) = (d.generation(), d.model.stop);
            let _ = d.send(
                "exceptionInfo",
                json!({ "threadId": thread }),
                Pending::ExceptionInfo { generation, stop },
            );
        }
        self.debug_show_frame(0, window, cx);
    }

    /// The selected frame's locals arrived.
    fn locals_done(&mut self, vars: Vec<VarNode>) {
        let d = &mut self.debug;
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

fn set_children(nodes: &mut [VarNode], path: &[usize], vars: Vec<VarNode>) {
    if let Some(n) = node_mut(nodes, path) {
        n.children = Some(vars);
        n.loading = false;
    }
}
