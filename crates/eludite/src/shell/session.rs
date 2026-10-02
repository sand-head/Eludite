//! The shell's side of a language server: `eludite-host` (brief 0012) or a generic server the shell launches itself
//! (rust-analyzer, brief 0019). A [`ServerSession`] starts and supervises its server through `eludite-lsp`, sends the
//! editors' LSP document notifications and requests, and, for the host, opens and closes solutions, asks for the
//! Workspace tree and runs builds. The editor features hold a [`ServerSession`] per document and never ask which
//! kind it is: both kinds share `eludite-lsp`'s [`Connection`] for documents, requests and cancellation.
//!
//! Nothing here runs on the UI thread. The UI hands work to a worker thread through a channel ([`ServerSession`]'s
//! methods never block), and a pump thread turns the host's notifications into [`SessionEvent`]s on a `futures`
//! channel that a foreground task drains. A stalled host therefore stalls only these threads: writes to it wait in
//! the worker, never in a frame.
//!
//! Text sync: the UI sends the whole text of a document after its debounce ([`super::documents::DIDCHANGE_DEBOUNCE`]);
//! the worker diffs it against the text it last sent and sends one incremental `textDocument/didChange` (UTF-16
//! positions, as LSP requires). After a host restart the worker reopens the solution and replays every open
//! document, because the generation and the host's copies are gone.
//!
//! Requests (brief 0013: completion, resolve, hover, signature help) go through the same worker, so each one is
//! written after every document notification queued before it and sees the text the user sees. The worker sends
//! it and a waiter thread blocks on the reply; the UI gets a [`Reply`] on a oneshot channel and never waits.
//! [`RequestHandle::cancel`] sends `$/cancelRequest` for it.
//!
//! The host's one request to the shell, `workspace/applyEdit` (brief 0015), arrives as [`SessionEvent::ApplyEdit`];
//! the shell answers it with [`ServerSession::respond_apply_edit`], through the worker like everything else.
//!
//! Builds (brief 0017): [`ServerSession::build_start`] and [`ServerSession::build_cancel`] go through the worker too; the
//! host's reply arrives as [`SessionEvent::BuildStarted`] (or `BuildRefused`), and the build's streamed output,
//! progress and result as `BuildOutput`, `BuildProgress` and `BuildFinished`. The host's stderr is captured and
//! arrives line by line as [`SessionEvent::HostLog`] (the Output window's Host source) as well as on the shell's
//! stderr. After the host restarts, the worker asks `eludite/build/status` before reopening the solution and reports it
//! as [`SessionEvent::BuildStatus`], so the shell can replay a build that is still running or end the one that died
//! with the old host (brief 0020).
//!
//! Generic servers (brief 0019, [`ServerSession::spawn_generic`]): the server starts with the first document opened
//! for it, rooted at the workspace root the shell computed (the Cargo workspace root from `cargo metadata`). The
//! executable is located from the registration (off the UI thread: it runs `--version`). Its state, work-done
//! progress and `experimental/serverStatus` arrive as [`SessionEvent::LanguageServer`], [`SessionEvent::Progress`]
//! and [`SessionEvent::ServerStatus`]; its generation (one more per restart) as [`SessionEvent::ServerGeneration`];
//! its stderr and `window/logMessage` as [`SessionEvent::HostLog`]. A crash restarts it under the brief 0007 policy
//! and replays the open documents.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_lsp::host::{
    self, Generation, LanguageServerState, LanguageServerStatus, ServerInfo, SolutionState,
    SolutionStatus, SolutionTree,
};
use eludite_lsp::lsp::{
    self, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    Position, Range, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    VersionedTextDocumentIdentifier,
};
use eludite_lsp::{
    ClientInfo, Connection, Connector, Event, HostClient, HostCommand, HostEvent, Id, Progress,
    RestartPolicy, ServerClient, ServerCommand, ServerRegistration, ServerSetup, ServerStatus,
    StderrMode,
};
use eludite_protocol::RequestType;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;

/// How long the worker waits for the host's reply to `eludite/solution/open` and `close`.
const SOLUTION_TIMEOUT: Duration = Duration::from_secs(60);

/// How to start the host.
#[derive(Clone)]
pub enum HostLaunch {
    Process(HostCommand),
    /// A host in this process (the fake host in tests).
    #[cfg_attr(not(test), allow(dead_code))]
    InProcess(Connector),
    /// No host was found; opening a solution reports why.
    Missing(String),
}

impl std::fmt::Debug for HostLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            HostLaunch::Process(c) => f.debug_tuple("Process").field(&c.program).finish(),
            HostLaunch::InProcess(_) => f.write_str("InProcess"),
            HostLaunch::Missing(why) => f.debug_tuple("Missing").field(why).finish(),
        }
    }
}

impl HostLaunch {
    /// `eludite-host` beside this executable (an apphost `eludite-host`, or `eludite-host.dll` run with `dotnet`),
    /// then `ELUDITE_HOST` (either form), then `eludite-host` on `PATH`. The host gets `--stdio`.
    pub fn locate() -> Self {
        let beside = std::env::current_exe()
            .ok()
            .and_then(|exe| exe.parent().map(Path::to_path_buf));
        let from_env = std::env::var_os("ELUDITE_HOST").map(PathBuf::from);
        Self::locate_in(beside.as_deref(), from_env, std::env::var_os("PATH"))
    }

    fn locate_in(
        beside: Option<&Path>,
        from_env: Option<PathBuf>,
        path_var: Option<std::ffi::OsString>,
    ) -> Self {
        let exe = format!("eludite-host{}", std::env::consts::EXE_SUFFIX);
        let command = |p: &Path| {
            let cmd = if p.extension().is_some_and(|e| e == "dll") {
                HostCommand::new("dotnet").arg(p.as_os_str())
            } else {
                HostCommand::new(p.as_os_str())
            };
            // The host's log is captured for the Output window (and still copied to stderr).
            HostLaunch::Process(cmd.arg("--stdio").stderr(eludite_lsp::StderrMode::Capture))
        };
        if let Some(dir) = beside {
            for name in [exe.as_str(), "eludite-host.dll"] {
                let p = dir.join(name);
                if p.is_file() {
                    return command(&p);
                }
            }
        }
        if let Some(p) = from_env {
            return if p.is_file() {
                command(&p)
            } else {
                HostLaunch::Missing(format!("ELUDITE_HOST={} does not exist", p.display()))
            };
        }
        if let Some(paths) = path_var {
            for dir in std::env::split_paths(&paths) {
                let p = dir.join(&exe);
                if p.is_file() {
                    return command(&p);
                }
            }
        }
        HostLaunch::Missing(
            "eludite-host not found beside eludite, in ELUDITE_HOST or on PATH \
             (build it with `dotnet build dotnet/Eludite.slnx`)"
                .into(),
        )
    }
}

/// How to start a generic language server (brief 0019).
#[derive(Clone)]
pub struct GenericLaunch {
    pub registration: ServerRegistration,
    /// The workspace root (`rootUri`, the one workspace folder, the process's working directory).
    pub root: PathBuf,
    /// A server in this process instead of the located executable (the fake server in tests).
    pub connector: Option<Connector>,
}

impl std::fmt::Debug for GenericLaunch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("GenericLaunch")
            .field("server", &self.registration.id)
            .field("root", &self.root)
            .field("in_process", &self.connector.is_some())
            .finish()
    }
}

/// What the session tells the UI.
#[derive(Debug, Clone, PartialEq)]
pub enum SessionEvent {
    /// `eludite.solution.open` was accepted; the host is starting or loading.
    Opening {
        path: PathBuf,
    },
    HostStarted {
        version: String,
    },
    /// The host could not start or gave up restarting.
    HostFailed {
        reason: String,
    },
    /// The host exited unexpectedly and is being restarted.
    HostRestarting,
    LanguageServer(LanguageServerStatus),
    Solution(SolutionStatus),
    /// The Workspace tree for the current generation.
    Tree(SolutionTree),
    /// `textDocument/publishDiagnostics` for the current generation.
    Diagnostics(lsp::PublishDiagnosticsParams),
    Closed,
    /// `workspace/applyEdit` from the server, computed under `generation`; answer with
    /// [`ServerSession::respond_apply_edit`] and `id`.
    ApplyEdit {
        id: Id,
        generation: Generation,
        params: lsp::ApplyWorkspaceEditParams,
    },
    /// The host accepted the build started with [`ServerSession::build_start`] `ticket`.
    BuildStarted {
        ticket: u64,
        result: host::BuildStartResult,
    },
    /// The host refused it (no solution, a build already running) or could not be asked.
    BuildRefused {
        ticket: u64,
        message: String,
    },
    BuildOutput(host::BuildOutput),
    BuildProgress(host::BuildProgress),
    /// `eludite/build/finished`, with when the pump received it (the Error List budget is measured from there).
    BuildFinished {
        finished: Box<host::BuildFinished>,
        received: Instant,
    },
    /// A line of the server's own log (stderr, or a generic server's `window/logMessage` and `window/showMessage`).
    HostLog(String),
    /// A generic server's work-done progress (`$/progress`).
    Progress(Progress),
    /// A generic server's `experimental/serverStatus`.
    ServerStatus(ServerStatus),
    /// A generic server's generation: after it started, and after every restart.
    ServerGeneration(Generation),
    /// `eludite/build/status` after the host restarted (brief 0020): the running build to replay, if any.
    BuildStatus(host::BuildStatusResult),
}

enum Cmd {
    Open(PathBuf),
    Close,
    DidOpen {
        uri: String,
        language_id: String,
        version: i32,
        text: String,
    },
    DidChange {
        uri: String,
        version: i32,
        text: String,
    },
    DidSave {
        uri: String,
    },
    DidClose {
        uri: String,
    },
    /// The server restarted on its own: reopen and replay.
    Replay,
    /// Send a request (after the notifications queued before it).
    Request(RequestJob),
    /// Cancel the request with this ticket if it is still in flight.
    Cancel(u64),
    /// Answer the server's `workspace/applyEdit`.
    RespondApplyEdit(Id, lsp::ApplyWorkspaceEditResult),
    /// An untyped notification (`workspace/didChangeWatchedFiles`).
    Notify(String, serde_json::Value),
    BuildStart(u64, host::BuildStartParams),
    BuildCancel,
    /// Kill the server without a shutdown (a simulated crash: tests and the harness).
    Kill,
    Shutdown(std::sync::mpsc::SyncSender<()>),
}

/// State shared with command handlers on any thread.
#[derive(Debug, Default)]
pub struct Shared {
    /// The solution the user asked for (set at once by `open`, cleared by `close`).
    pub solution: Option<PathBuf>,
    pub generation: Generation,
    pub status: Option<SolutionStatus>,
}

/// The shell's handle on one language server: `eludite-host`, or a generic server. Cheap to clone,
/// `Send + Sync`; every method returns at once.
#[derive(Clone)]
pub struct ServerSession {
    tx: Arc<Mutex<Sender<Cmd>>>,
    shared: Arc<Mutex<Shared>>,
    tickets: Arc<AtomicU64>,
}

/// Why a request has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// No server is running (no solution was opened, or it failed to start).
    NoHost,
    /// Canceled (by the shell, or the server answered RequestCancelled).
    Canceled,
    /// Computed under a generation that is no longer current.
    Stale,
    Failed(String),
}

/// The answer to [`ServerSession::request`], with when the request was written and answered (for the latency
/// measurements). A result computed under an older generation is already [`RequestError::Stale`].
#[derive(Debug)]
pub struct Reply<T> {
    pub result: Result<T, RequestError>,
    pub sent: Option<Instant>,
    pub received: Instant,
}

impl<T> Reply<T> {
    fn err(e: RequestError) -> Self {
        Self {
            result: Err(e),
            sent: None,
            received: Instant::now(),
        }
    }
}

/// Request ticket to request id, while in flight.
type Inflight = Arc<Mutex<HashMap<u64, Id>>>;
type RequestJob = Box<dyn FnOnce(Option<&Connection>, &Inflight) + Send>;

/// A request in flight; [`RequestHandle::cancel`] cancels it. Dropping it does not.
#[derive(Debug, Clone)]
pub struct RequestHandle {
    ticket: u64,
    session: ServerSession,
}

impl RequestHandle {
    pub fn cancel(&self) {
        self.session.send(Cmd::Cancel(self.ticket));
    }
}

impl std::fmt::Debug for ServerSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ServerSession")
            .field("solution", &self.shared().solution)
            .finish()
    }
}

impl ServerSession {
    /// The `eludite-host` session. Starts the worker (the host itself starts on the first `open`). Events arrive on
    /// the returned receiver.
    pub fn spawn(launch: HostLaunch) -> (Self, UnboundedReceiver<SessionEvent>) {
        Self::spawn_worker(Launch::Host(launch), "eludite-host-session")
    }

    /// A generic server's session (brief 0019). The server starts with the first `did_open`.
    pub fn spawn_generic(launch: GenericLaunch) -> (Self, UnboundedReceiver<SessionEvent>) {
        let name = format!("eludite-{}-session", launch.registration.id);
        Self::spawn_worker(Launch::Generic(Box::new(launch)), &name)
    }

    fn spawn_worker(launch: Launch, name: &str) -> (Self, UnboundedReceiver<SessionEvent>) {
        let (tx, rx) = mpsc::channel();
        let (events, events_rx) = unbounded();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = Worker {
            launch,
            client: None,
            gave_up: false,
            events,
            shared: shared.clone(),
            docs: HashMap::new(),
            tx: tx.clone(),
            inflight: Inflight::default(),
        };
        thread::Builder::new()
            .name(name.into())
            .spawn(move || worker.run(rx))
            .expect("spawn the language server session thread");
        (
            Self {
                tx: Arc::new(Mutex::new(tx)),
                shared,
                tickets: Arc::new(AtomicU64::new(1)),
            },
            events_rx,
        )
    }

    /// Send request `R` after the document notifications already queued. The reply arrives on the returned
    /// receiver; the UI awaits it and never blocks.
    pub fn request<R>(
        &self,
        params: R::Params,
    ) -> (RequestHandle, oneshot::Receiver<Reply<R::Result>>)
    where
        R: RequestType + 'static,
        R::Params: Send + 'static,
        R::Result: Send + 'static,
    {
        let ticket = self.tickets.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        let job: RequestJob = Box::new(move |client, inflight| {
            let Some(client) = client else {
                let _ = tx.send(Reply::err(RequestError::NoHost));
                return;
            };
            let sent = Instant::now();
            let pending = match client.request::<R>(params) {
                Ok(p) => p,
                Err(e) => {
                    let _ = tx.send(Reply::err(request_error(e)));
                    return;
                }
            };
            lock(inflight).insert(ticket, pending.id());
            let inflight = inflight.clone();
            let spawned = thread::Builder::new()
                .name("eludite-lsp-reply".into())
                .spawn(move || {
                    let result = pending.wait().map_err(request_error);
                    let received = Instant::now();
                    lock(&inflight).remove(&ticket);
                    let _ = tx.send(Reply {
                        result,
                        sent: Some(sent),
                        received,
                    });
                });
            if spawned.is_err() {
                eprintln!("eludite: cannot start a request waiter thread");
            }
        });
        self.send(Cmd::Request(job));
        (
            RequestHandle {
                ticket,
                session: self.clone(),
            },
            rx,
        )
    }

    pub fn shared(&self) -> MutexGuard<'_, Shared> {
        lock(&self.shared)
    }

    fn send(&self, cmd: Cmd) {
        let _ = lock(&self.tx).send(cmd);
    }

    pub fn open(&self, path: PathBuf) {
        self.shared().solution = Some(path.clone());
        self.send(Cmd::Open(path));
    }

    /// Returns the solution that was open.
    pub fn close(&self) -> Option<PathBuf> {
        let was = self.shared().solution.take();
        if was.is_some() {
            self.send(Cmd::Close);
        }
        was
    }

    pub fn did_open(&self, uri: String, language_id: &str, version: i32, text: String) {
        self.send(Cmd::DidOpen {
            uri,
            language_id: language_id.into(),
            version,
            text,
        });
    }

    pub fn did_change(&self, uri: String, version: i32, text: String) {
        self.send(Cmd::DidChange { uri, version, text });
    }

    pub fn did_save(&self, uri: String) {
        self.send(Cmd::DidSave { uri });
    }

    pub fn did_close(&self, uri: String) {
        self.send(Cmd::DidClose { uri });
    }

    /// Answer the server's `workspace/applyEdit` request `id` ([`SessionEvent::ApplyEdit`]).
    pub fn respond_apply_edit(&self, id: Id, result: lsp::ApplyWorkspaceEditResult) {
        self.send(Cmd::RespondApplyEdit(id, result));
    }

    /// Send a notification without Eludite typing (after the document notifications queued before it).
    pub fn notify_untyped(&self, method: &str, params: serde_json::Value) {
        self.send(Cmd::Notify(method.to_owned(), params));
    }

    /// Ask the host to start a build; the answer arrives as [`SessionEvent::BuildStarted`] or `BuildRefused` with
    /// `ticket`.
    pub fn build_start(&self, ticket: u64, params: host::BuildStartParams) {
        self.send(Cmd::BuildStart(ticket, params));
    }

    /// Ask the host to cancel the running build (its `eludite/build/finished` follows).
    pub fn build_cancel(&self) {
        self.send(Cmd::BuildCancel);
    }

    /// Kill the server as a crash would; the restart policy restarts it (tests and the harness).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn kill(&self) {
        self.send(Cmd::Kill);
    }

    /// Shuts the server down; the returned receiver fires when done (or the worker is gone).
    pub fn shutdown(&self) -> Receiver<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.send(Cmd::Shutdown(tx));
        rx
    }
}

struct Doc {
    language_id: String,
    version: i32,
    text: String,
}

enum Launch {
    Host(HostLaunch),
    Generic(Box<GenericLaunch>),
}

/// The running server.
enum Backend {
    Host(HostClient),
    Server(ServerClient),
}

impl Backend {
    fn conn(&self) -> &Connection {
        match self {
            Backend::Host(c) => c.connection(),
            Backend::Server(c) => c.connection(),
        }
    }

    fn host(&self) -> Option<&HostClient> {
        match self {
            Backend::Host(c) => Some(c),
            Backend::Server(_) => None,
        }
    }
}

struct Worker {
    launch: Launch,
    client: Option<Backend>,
    /// A generic server that could not start is not tried again on every document.
    gave_up: bool,
    events: UnboundedSender<SessionEvent>,
    shared: Arc<Mutex<Shared>>,
    docs: HashMap<String, Doc>,
    tx: Sender<Cmd>,
    inflight: Inflight,
}

fn request_error(e: eludite_lsp::Error) -> RequestError {
    match e {
        eludite_lsp::Error::Canceled => RequestError::Canceled,
        eludite_lsp::Error::Stale { .. } => RequestError::Stale,
        eludite_lsp::Error::HostExited => RequestError::NoHost,
        other => RequestError::Failed(other.to_string()),
    }
}

impl Worker {
    fn emit(&self, e: SessionEvent) {
        let _ = self.events.unbounded_send(e);
    }

    fn conn(&self) -> Option<&Connection> {
        self.client.as_ref().map(Backend::conn)
    }

    fn host(&self) -> Option<&HostClient> {
        self.client.as_ref().and_then(Backend::host)
    }

    fn is_generic(&self) -> bool {
        matches!(self.launch, Launch::Generic(_))
    }

    fn run(mut self, rx: Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Open(path) => self.open(&path),
                Cmd::Close => {
                    if let Some(c) = self.host() {
                        match c.close_solution(SOLUTION_TIMEOUT) {
                            Ok(g) => lock(&self.shared).generation = g,
                            Err(e) => eprintln!("eludite: closing the solution: {e}"),
                        }
                    }
                    self.emit(SessionEvent::Closed);
                }
                Cmd::DidOpen {
                    uri,
                    language_id,
                    version,
                    text,
                } => {
                    // A generic server starts with its first document (and replays it on start).
                    let started = self.client.is_none() && self.is_generic() && !self.gave_up;
                    self.docs.insert(
                        uri.clone(),
                        Doc {
                            language_id: language_id.clone(),
                            version,
                            text: text.clone(),
                        },
                    );
                    if started {
                        self.ensure_started();
                    } else {
                        self.send_open(&uri, &language_id, version, &text);
                    }
                }
                Cmd::DidChange { uri, version, text } => {
                    let Some(doc) = self.docs.get_mut(&uri) else {
                        continue;
                    };
                    let change = diff(&doc.text, &text);
                    doc.version = version;
                    doc.text = text;
                    if let (Some(c), Some(change)) = (self.conn(), change) {
                        let _ = c.did_change(DidChangeTextDocumentParams {
                            text_document: VersionedTextDocumentIdentifier { uri, version },
                            content_changes: vec![change],
                        });
                    }
                }
                Cmd::DidSave { uri } => {
                    if let Some(c) = self.conn() {
                        let _ = c.did_save(&uri);
                    }
                }
                Cmd::DidClose { uri } => {
                    if self.docs.remove(&uri).is_some()
                        && let Some(c) = self.conn()
                    {
                        let _ = c.did_close(DidCloseTextDocumentParams {
                            text_document: TextDocumentIdentifier { uri },
                        });
                    }
                }
                Cmd::Replay => {
                    // A build the old host ran is either still running (a host the shell reattached to) or gone:
                    // ask before reopening the solution, which would cancel it.
                    self.build_status();
                    let solution = lock(&self.shared).solution.clone();
                    if let Some(path) = solution
                        && !self.is_generic()
                    {
                        self.open(&path);
                    }
                    for (uri, d) in &self.docs {
                        self.send_open(uri, &d.language_id, d.version, &d.text);
                    }
                }
                Cmd::Request(job) => job(self.conn(), &self.inflight),
                Cmd::Cancel(ticket) => {
                    let id = lock(&self.inflight).remove(&ticket);
                    if let (Some(id), Some(c)) = (id, self.conn()) {
                        c.cancel_id(id);
                    }
                }
                Cmd::RespondApplyEdit(id, result) => {
                    if let Some(c) = self.conn() {
                        let _ = c.respond_apply_edit(id, result);
                    }
                }
                Cmd::Notify(method, params) => {
                    if let Some(c) = self.conn() {
                        let _ = c.notify_untyped(&method, params);
                    }
                }
                Cmd::BuildStart(ticket, params) => self.build_start(ticket, params),
                Cmd::BuildCancel => {
                    if let Some(c) = self.host()
                        && let Ok(pending) =
                            c.request::<host::BuildCancel>(host::BuildCancelParams::default())
                    {
                        let _ = thread::Builder::new()
                            .name("eludite-build-cancel".into())
                            .spawn(move || {
                                if let Err(e) = pending.wait_timeout(Duration::from_secs(10)) {
                                    eprintln!("eludite: eludite/build/cancel: {e}");
                                }
                            });
                    }
                }
                Cmd::Kill => {
                    if let Some(c) = self.client.as_ref() {
                        let _ = c.conn().kill();
                    }
                }
                Cmd::Shutdown(done) => {
                    if let Some(c) = self.client.take() {
                        let _ = c.conn().shutdown(Duration::from_secs(5));
                    }
                    let _ = done.send(());
                    return;
                }
            }
        }
    }

    fn build_start(&self, ticket: u64, params: host::BuildStartParams) {
        let refuse = |message: String| {
            let _ = self
                .events
                .unbounded_send(SessionEvent::BuildRefused { ticket, message });
        };
        let Some(client) = self.host() else {
            return refuse("eludite-host is not running; open a solution first".into());
        };
        let pending = match client.request::<host::BuildStart>(params) {
            Ok(p) => p,
            Err(e) => return refuse(format!("eludite/build/start: {e}")),
        };
        let events = self.events.clone();
        let _ = thread::Builder::new()
            .name("eludite-build-start".into())
            .spawn(move || {
                let event = match pending.wait_timeout(Duration::from_secs(30)) {
                    Ok(result) => SessionEvent::BuildStarted { ticket, result },
                    Err(eludite_lsp::Error::Rpc(e)) => SessionEvent::BuildRefused {
                        ticket,
                        message: e.message,
                    },
                    Err(e) => SessionEvent::BuildRefused {
                        ticket,
                        message: e.to_string(),
                    },
                };
                let _ = events.unbounded_send(event);
            });
    }

    /// `eludite/build/status`, answered as [`SessionEvent::BuildStatus`] from a waiter thread.
    fn build_status(&self) {
        let Some(client) = self.host() else { return };
        let Ok(pending) = client.request::<host::BuildStatus>(()) else {
            return;
        };
        let events = self.events.clone();
        let _ = thread::Builder::new()
            .name("eludite-build-status".into())
            .spawn(
                move || match pending.wait_timeout(Duration::from_secs(10)) {
                    Ok(status) => {
                        let _ = events.unbounded_send(SessionEvent::BuildStatus(status));
                    }
                    Err(e) => {
                        eprintln!("eludite: eludite/build/status: {e}");
                        let _ = events.unbounded_send(SessionEvent::BuildStatus(
                            host::BuildStatusResult::default(),
                        ));
                    }
                },
            );
    }

    fn send_open(&self, uri: &str, language_id: &str, version: i32, text: &str) {
        if let Some(c) = self.conn() {
            let _ = c.did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.into(),
                    language_id: language_id.into(),
                    version,
                    text: text.into(),
                },
            });
        }
    }

    /// Starts the server if needed. Returns false (and reports why) when it cannot.
    fn ensure_started(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }
        let info = ClientInfo {
            name: "eludite".into(),
            version: eludite_commands::builtins::VERSION.into(),
        };
        let (backend, events) = match &self.launch {
            Launch::Host(launch) => {
                let started = match launch {
                    HostLaunch::Process(cmd) => {
                        HostClient::start(cmd.clone(), info, RestartPolicy::default())
                    }
                    HostLaunch::InProcess(connector) => HostClient::start_in_process(
                        connector.clone(),
                        info,
                        RestartPolicy::default(),
                    ),
                    HostLaunch::Missing(why) => {
                        self.emit(SessionEvent::HostFailed {
                            reason: why.clone(),
                        });
                        return false;
                    }
                };
                match started {
                    Ok((client, events)) => {
                        let version = client
                            .initialize_result()
                            .map(|i| i.host_version)
                            .unwrap_or_default();
                        self.emit(SessionEvent::HostStarted { version });
                        (Backend::Host(client), events)
                    }
                    Err(e) => {
                        self.emit(SessionEvent::HostFailed {
                            reason: format!("eludite-host did not start: {e}"),
                        });
                        return false;
                    }
                }
            }
            Launch::Generic(launch) => match start_generic(launch, info) {
                Ok((client, events)) => {
                    self.emit(SessionEvent::ServerGeneration(client.generation()));
                    self.emit(SessionEvent::LanguageServer(running_status(&client)));
                    (Backend::Server(client), events)
                }
                Err(message) => {
                    self.gave_up = true;
                    self.emit(SessionEvent::LanguageServer(LanguageServerStatus {
                        state: LanguageServerState::Unavailable,
                        server_info: None,
                        capabilities: None,
                        message: Some(message),
                    }));
                    return false;
                }
            },
        };
        let pump = Pump {
            backend: match &backend {
                Backend::Host(c) => Backend::Host(c.clone()),
                Backend::Server(c) => Backend::Server(c.clone()),
            },
            events: self.events.clone(),
            shared: self.shared.clone(),
            tx: self.tx.clone(),
        };
        let _ = thread::Builder::new()
            .name("eludite-server-events".into())
            .spawn(move || pump.run(events));
        // Replay documents opened before the server was up.
        for (uri, d) in &self.docs {
            let _ = backend.conn().did_open(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.clone(),
                    language_id: d.language_id.clone(),
                    version: d.version,
                    text: d.text.clone(),
                },
            });
        }
        self.client = Some(backend);
        true
    }

    fn open(&mut self, path: &Path) {
        if self.is_generic() {
            return;
        }
        self.emit(SessionEvent::Opening {
            path: path.to_path_buf(),
        });
        if !self.ensure_started() {
            return;
        }
        let Some(client) = self.host().cloned() else {
            return;
        };
        let generation = match client.open_solution(&path.to_string_lossy(), SOLUTION_TIMEOUT) {
            Ok(g) => g,
            Err(e) => {
                self.emit(SessionEvent::HostFailed {
                    reason: format!("eludite/solution/open failed: {e}"),
                });
                return;
            }
        };
        lock(&self.shared).generation = generation;
        // The tree streams in when the host's evaluation finishes; wait for it off this thread so document
        // notifications keep flowing.
        let pending = client.request::<host::SolutionTreeRequest>(());
        let events = self.events.clone();
        let _ = thread::Builder::new()
            .name("eludite-tree-wait".into())
            .spawn(move || {
                let started = Instant::now();
                match pending.and_then(|p| p.wait()) {
                    Ok(tree) if tree.generation == client.generation() => {
                        let _ = events.unbounded_send(SessionEvent::Tree(tree));
                    }
                    Ok(_) | Err(eludite_lsp::Error::Stale { .. }) => {}
                    Err(e) => eprintln!(
                        "eludite: eludite/solution/tree failed after {:?}: {e}",
                        started.elapsed()
                    ),
                }
            });
    }
}

/// Locate (unless in process) and start a generic server. Runs `--version` and the LSP handshake: worker thread only.
fn start_generic(
    launch: &GenericLaunch,
    client: ClientInfo,
) -> Result<(ServerClient, Receiver<Event>), String> {
    let reg = &launch.registration;
    let setup = ServerSetup {
        name: reg.id.clone(),
        client,
        root: launch.root.clone(),
        initialization_options: reg.initialization_options.clone(),
        settings: reg.settings.clone(),
    };
    let started = match &launch.connector {
        Some(connector) => {
            ServerClient::start_in_process(connector.clone(), setup, RestartPolicy::default())
        }
        None => {
            let located = reg.locate()?;
            documents_trace(&format!(
                "{} {} from {} ({})",
                reg.id,
                located.version,
                located.source,
                located.path.display()
            ));
            let mut command = ServerCommand::new(located.path.as_os_str())
                .current_dir(&launch.root)
                .stderr(StderrMode::Capture);
            if let Some(spec) = &reg.command {
                for a in &spec.args {
                    command = command.arg(a);
                }
            }
            ServerClient::start(command, setup, RestartPolicy::default())
        }
    };
    started.map_err(|e| format!("{} did not start: {e}", reg.name))
}

fn documents_trace(what: &str) {
    super::documents::trace(format_args!("{what}"));
}

/// `running`, with the server's `serverInfo` and capabilities, as the host reports its language server.
fn running_status(client: &ServerClient) -> LanguageServerStatus {
    LanguageServerStatus {
        state: LanguageServerState::Running,
        server_info: client
            .server_info()
            .map(|(name, version)| ServerInfo { name, version }),
        capabilities: client.capabilities(),
        message: None,
    }
}

/// Turns server events into session events.
struct Pump {
    backend: Backend,
    events: UnboundedSender<SessionEvent>,
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Cmd>,
}

impl Pump {
    fn run(self, rx: Receiver<Event>) {
        let generic = matches!(self.backend, Backend::Server(_));
        while let Ok(event) = rx.recv() {
            let out = match event {
                Event::SolutionStatus(status) => {
                    {
                        let mut s = lock(&self.shared);
                        if status.generation < s.generation {
                            continue;
                        }
                        s.generation = status.generation;
                        if status.state != SolutionState::Closed {
                            s.status = Some(status.clone());
                        } else {
                            s.status = None;
                        }
                    }
                    SessionEvent::Solution(status)
                }
                Event::LanguageServerStatus(s) => SessionEvent::LanguageServer(s),
                Event::Diagnostics(d) if d.generation == self.backend.conn().generation() => {
                    SessionEvent::Diagnostics(d.params)
                }
                Event::Diagnostics(_) => continue,
                Event::ApplyEdit { id, params } => SessionEvent::ApplyEdit {
                    id,
                    generation: params.generation,
                    params: params.params,
                },
                Event::Host(HostEvent::Restarted { .. }) => {
                    let _ = self.tx.send(Cmd::Replay);
                    match &self.backend {
                        Backend::Server(c) => {
                            let _ = self
                                .events
                                .unbounded_send(SessionEvent::ServerGeneration(c.generation()));
                            SessionEvent::LanguageServer(running_status(c))
                        }
                        Backend::Host(_) => continue,
                    }
                }
                Event::Host(HostEvent::Exited {
                    expected: false,
                    code,
                }) if generic => SessionEvent::LanguageServer(LanguageServerStatus {
                    state: LanguageServerState::Restarting,
                    server_info: None,
                    capabilities: None,
                    message: Some(match code {
                        Some(c) => format!("exited with code {c}"),
                        None => "exited".into(),
                    }),
                }),
                Event::Host(HostEvent::Exited {
                    expected: false, ..
                }) => SessionEvent::HostRestarting,
                Event::Host(HostEvent::GaveUp { reason }) if generic => {
                    SessionEvent::LanguageServer(LanguageServerStatus {
                        state: LanguageServerState::Exited,
                        server_info: None,
                        capabilities: None,
                        message: Some(reason),
                    })
                }
                Event::Host(HostEvent::GaveUp { reason }) => SessionEvent::HostFailed { reason },
                Event::Notification(n) if generic && n.method == "window/showMessage" => {
                    SessionEvent::HostLog(
                        n.params
                            .as_ref()
                            .and_then(|p| p.get("message"))
                            .and_then(serde_json::Value::as_str)
                            .unwrap_or_default()
                            .to_owned(),
                    )
                }
                Event::Host(HostEvent::Exited { .. }) | Event::Notification(_) => continue,
                Event::Progress(p) => SessionEvent::Progress(p),
                Event::ServerStatus(s) => SessionEvent::ServerStatus(s),
                Event::BuildOutput(o) => SessionEvent::BuildOutput(o),
                Event::BuildProgress(p) => SessionEvent::BuildProgress(p),
                Event::BuildFinished(f) => SessionEvent::BuildFinished {
                    finished: Box::new(f),
                    received: Instant::now(),
                },
                Event::Log(line) => {
                    if !generic {
                        eprintln!("[eludite-host] {line}");
                    }
                    SessionEvent::HostLog(line)
                }
            };
            if self.events.unbounded_send(out).is_err() {
                return;
            }
        }
    }
}

/// One incremental change that turns `old` into `new` (common prefix and suffix), or `None` when equal.
pub fn diff(old: &str, new: &str) -> Option<TextDocumentContentChangeEvent> {
    if old == new {
        return None;
    }
    let mut prefix = old
        .bytes()
        .zip(new.bytes())
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(prefix) || !new.is_char_boundary(prefix) {
        prefix -= 1;
    }
    let max_suffix = (old.len() - prefix).min(new.len() - prefix);
    let mut suffix = old
        .bytes()
        .rev()
        .zip(new.bytes().rev())
        .take(max_suffix)
        .take_while(|(a, b)| a == b)
        .count();
    while !old.is_char_boundary(old.len() - suffix) || !new.is_char_boundary(new.len() - suffix) {
        suffix -= 1;
    }
    let start = utf16_position(old, prefix);
    let end = utf16_position(old, old.len() - suffix);
    Some(TextDocumentContentChangeEvent {
        range: Some(Range { start, end }),
        range_length: None,
        text: new[prefix..new.len() - suffix].to_owned(),
    })
}

/// LSP position (line, UTF-16 column) of byte `offset` in `text` (`\n` line breaks).
pub fn utf16_position(text: &str, offset: usize) -> Position {
    let before = &text[..offset];
    let line_start = before.rfind('\n').map_or(0, |i| i + 1);
    Position {
        line: before.bytes().filter(|&b| b == b'\n').count() as u32,
        character: before[line_start..].encode_utf16().count() as u32,
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn apply(old: &str, c: &TextDocumentContentChangeEvent) -> String {
        // Map UTF-16 positions back to byte offsets.
        let offset = |p: Position| {
            let mut line = 0;
            let mut start = 0;
            for (i, b) in old.bytes().enumerate() {
                if line == p.line {
                    break;
                }
                if b == b'\n' {
                    line += 1;
                    start = i + 1;
                }
            }
            let mut units = 0;
            for (i, ch) in old[start..].char_indices() {
                if units >= p.character {
                    return start + i;
                }
                units += ch.len_utf16() as u32;
            }
            old.len()
        };
        let r = c.range.unwrap();
        let (s, e) = (offset(r.start), offset(r.end));
        format!("{}{}{}", &old[..s], c.text, &old[e..])
    }

    #[test]
    fn diff_is_one_minimal_utf16_change() {
        for (old, new) in [
            ("class A {}\n", "class AB {}\n"),
            ("a\nb\nc", "a\nc"),
            ("", "x"),
            ("x", ""),
            ("héllo 𝄞 world", "héllo 𝄢 world"),
            ("aaa", "aaaa"),
            ("line1\nline2\n", "line1\nline2\nline3\n"),
        ] {
            let c = diff(old, new).unwrap();
            assert_eq!(apply(old, &c), new, "{old:?} -> {new:?}");
            assert!(c.text.len() <= new.len());
        }
        assert!(diff("same", "same").is_none());
        let c = diff("a\n😀b", "a\n😀xb").unwrap();
        assert_eq!(
            c.range.unwrap().start,
            Position {
                line: 1,
                character: 2
            }
        );
        assert_eq!(c.text, "x");
    }

    #[test]
    fn locate_prefers_beside_then_env_then_path() {
        let dir = tempfile::tempdir().unwrap();
        let beside = dir.path().join("bin");
        let env_dir = dir.path().join("env");
        let path_dir = dir.path().join("path");
        for d in [&beside, &env_dir, &path_dir] {
            std::fs::create_dir_all(d).unwrap();
        }
        let exe = format!("eludite-host{}", std::env::consts::EXE_SUFFIX);
        std::fs::write(path_dir.join(&exe), "").unwrap();
        let program = |l: HostLaunch| match l {
            HostLaunch::Process(c) => (c.program, c.args),
            other => panic!("{other:?}"),
        };
        let path_var = Some(std::env::join_paths([&path_dir]).unwrap());
        let (p, args) = program(HostLaunch::locate_in(Some(&beside), None, path_var.clone()));
        assert_eq!(p, path_dir.join(&exe));
        assert_eq!(args, ["--stdio"]);
        assert!(matches!(
            HostLaunch::locate_in(Some(&beside), None, path_var.clone()),
            HostLaunch::Process(c) if c.stderr == eludite_lsp::StderrMode::Capture
        ));

        let dll = env_dir.join("eludite-host.dll");
        std::fs::write(&dll, "").unwrap();
        let (p, args) = program(HostLaunch::locate_in(
            Some(&beside),
            Some(dll.clone()),
            path_var.clone(),
        ));
        assert_eq!(p, "dotnet");
        assert_eq!(args, [dll.as_os_str(), "--stdio".as_ref()]);

        std::fs::write(beside.join(&exe), "").unwrap();
        let (p, _) = program(HostLaunch::locate_in(Some(&beside), Some(dll), path_var));
        assert_eq!(p, beside.join(&exe));

        assert!(matches!(
            HostLaunch::locate_in(None, Some(dir.path().join("nope")), None),
            HostLaunch::Missing(_)
        ));
        assert!(matches!(
            HostLaunch::locate_in(None, None, None),
            HostLaunch::Missing(_)
        ));
    }
}
