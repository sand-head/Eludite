//! One supervised JSON-RPC connection to a language server process: the code `eludite-host`'s client
//! ([`crate::HostClient`]) and the generic LSP client ([`crate::ServerClient`]) share. It owns process supervision
//! (spawn, stderr kept off the protocol stream, exit events, restarts under a [`RestartPolicy`]), `Content-Length`
//! framing, request correlation, `$/cancelRequest` cancellation, generation pinning of results and the document
//! notifications. What differs between the two peers (the handshake, which requests carry `eluditeGeneration`, which
//! notifications and requests the peer sends) is the [`Dialect`].

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader};
use std::io::{Read, Write};
use std::marker::PhantomData;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::host::{
    self, BuildFinished, BuildOutput, BuildProgress, ContentModifiedData, Generation,
    LanguageServerStatus, SolutionStatus, WithGeneration, error_codes, methods,
};
use eludite_protocol::jsonrpc::ResponsePayload;
use eludite_protocol::lsp::{ApplyWorkspaceEditParams, ApplyWorkspaceEditResult, CancelParams};
use eludite_protocol::lsp::{
    DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    PublishDiagnosticsParams,
};
use eludite_protocol::{
    ErrorObject, Id, Message, Notification, NotificationType, Request, RequestType, Response,
    framing,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How to start a server process (`eludite-host`, or a language server the shell launches directly).
#[derive(Debug, Clone)]
pub struct HostCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub envs: Vec<(OsString, OsString)>,
    pub current_dir: Option<PathBuf>,
    pub stderr: StderrMode,
}

/// The same thing under the name the generic client uses.
pub type ServerCommand = HostCommand;

/// Where the server's stderr (its log) goes. Never mixed into the protocol stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StderrMode {
    /// The shell's own stderr.
    Inherit,
    /// Delivered line by line as [`Event::Log`].
    Capture,
    Discard,
}

impl HostCommand {
    pub fn new(program: impl Into<OsString>) -> Self {
        Self {
            program: program.into(),
            args: Vec::new(),
            envs: Vec::new(),
            current_dir: None,
            stderr: StderrMode::Inherit,
        }
    }

    /// `dotnet <eludite-host.dll> --stdio`.
    pub fn dotnet_host(dll: &std::path::Path) -> Self {
        Self::new("dotnet").arg(dll.as_os_str()).arg("--stdio")
    }

    pub fn arg(mut self, arg: impl Into<OsString>) -> Self {
        self.args.push(arg.into());
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    pub fn current_dir(mut self, dir: impl Into<PathBuf>) -> Self {
        self.current_dir = Some(dir.into());
        self
    }

    pub fn stderr(mut self, mode: StderrMode) -> Self {
        self.stderr = mode;
        self
    }
}

/// Opens a fresh connection to a server that runs in this process (tests, embedders): the stream the server writes
/// to and the stream it reads from. Called again for every restart.
pub type Connector =
    Arc<dyn Fn() -> io::Result<(Box<dyn Read + Send>, Box<dyn Write + Send>)> + Send + Sync>;

pub(crate) enum Launch {
    Process(HostCommand),
    InProcess(Connector),
}

/// Who the client is (`eludite/host/initialize`, or LSP `initialize`'s `clientInfo`).
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Restarts after the server exits on its own. `max_restarts == 0` disables them.
#[derive(Debug, Clone, Copy)]
pub struct RestartPolicy {
    pub max_restarts: u32,
    pub backoff: Duration,
}

impl Default for RestartPolicy {
    fn default() -> Self {
        Self {
            max_restarts: 3,
            backoff: Duration::from_millis(500),
        }
    }
}

/// State changes of the server process (named for the host, where they began; the generic client sends the same).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The process exited (or its stdout closed). Pending requests failed with [`Error::HostExited`].
    Exited { code: Option<i32>, expected: bool },
    /// A replacement process is up and initialized. For the host the generation is back to 0 (reopen the solution);
    /// for a generic server the generation moved on. Either way, replay the open documents.
    Restarted { attempt: u32, pid: u32 },
    /// The restart budget is spent, or the restart failed.
    GaveUp { reason: String },
}

/// Work-done progress from a generic server (`$/progress`, LSP 3.17 `WorkDoneProgress`).
#[derive(Debug, Clone, PartialEq)]
pub struct Progress {
    /// The progress token, as JSON text.
    pub token: String,
    /// `begin`, `report` or `end`.
    pub kind: String,
    pub title: Option<String>,
    pub message: Option<String>,
    pub percentage: Option<u32>,
}

impl Progress {
    /// The `WorkDoneProgress` notification `params`, or `None` for another kind of progress.
    pub fn from_params(params: &Value) -> Option<Self> {
        let value = params.get("value")?;
        let kind = value.get("kind")?.as_str()?.to_owned();
        let text = |k: &str| value.get(k).and_then(Value::as_str).map(str::to_owned);
        Some(Self {
            token: params
                .get("token")
                .map(Value::to_string)
                .unwrap_or_default(),
            kind,
            title: text("title"),
            message: text("message"),
            percentage: value
                .get("percentage")
                .and_then(Value::as_u64)
                .map(|p| p.min(100) as u32),
        })
    }
}

/// rust-analyzer's `experimental/serverStatus` (host-rpc.md, "Generic language servers and Cargo").
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    /// `ok`, `warning` or `error`.
    pub health: String,
    /// True when the server has no pending work (indexing, loading the workspace, a check).
    pub quiescent: bool,
    pub message: Option<String>,
}

impl ServerStatus {
    pub fn from_params(params: &Value) -> Option<Self> {
        Some(Self {
            health: params.get("health")?.as_str()?.to_owned(),
            quiescent: params.get("quiescent")?.as_bool()?,
            message: params
                .get("message")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
    }
}

/// Everything the server tells the shell without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    SolutionStatus(SolutionStatus),
    LanguageServerStatus(LanguageServerStatus),
    /// Only for the current generation; stale ones are dropped.
    Diagnostics(WithGeneration<PublishDiagnosticsParams>),
    /// Untyped notifications (`window/showMessage`, and `$/progress` from the host).
    Notification(Notification),
    /// `$/progress` work-done progress from a generic server.
    Progress(Progress),
    /// `experimental/serverStatus` from a generic server.
    ServerStatus(ServerStatus),
    /// A chunk of the build log (`eludite/build/output`, brief 0017), in order.
    BuildOutput(BuildOutput),
    /// `eludite/build/progress`.
    BuildProgress(BuildProgress),
    /// `eludite/build/finished`: the build's result and diagnostics.
    BuildFinished(BuildFinished),
    /// `eludite/test/update` (brief 0035): the next piece of a test discovery or run, any generation (the shell drops
    /// a stale one).
    TestUpdate(Box<eludite_protocol::host::TestUpdate>),
    /// `eludite/nuget/update` (brief 0048): output, progress or metadata of a NuGet call, any generation (the shell
    /// drops a stale one).
    NuGetUpdate(Box<eludite_protocol::host::NuGetUpdate>),
    /// `eludite/nuget/credentials` (brief 0048): a private feed asks for credentials during the person's call. Answer
    /// with `HostClient::respond_nuget_credentials` and `id`; until then the host waits.
    NuGetCredentials {
        id: Id,
        params: eludite_protocol::host::NuGetCredentialsParams,
    },
    /// `workspace/applyEdit` from the server. Answer it with `respond_apply_edit` and `id`; until then the server
    /// waits.
    ApplyEdit {
        id: Id,
        params: WithGeneration<ApplyWorkspaceEditParams>,
    },
    Host(HostEvent),
    /// A line of the server's stderr ([`StderrMode::Capture`]), or a generic server's `window/logMessage`.
    Log(String),
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// An error response other than cancellation or a stale generation.
    Rpc(ErrorObject),
    /// The request was canceled; any result was discarded.
    Canceled,
    /// The result belongs to a generation that is no longer current, or the server answered -32801.
    Stale {
        requested: Generation,
        current: Generation,
    },
    /// The server process exited (named for the host, where it began).
    HostExited,
    Timeout,
    Decode(serde_json::Error),
    /// A generational request whose params are not a JSON object.
    NotAnObject,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "server i/o: {e}"),
            Error::Rpc(e) => write!(f, "server error {}: {}", e.code, e.message),
            Error::Canceled => f.write_str("request canceled"),
            Error::Stale { requested, current } => write!(
                f,
                "result for generation {requested} is stale (current {current})"
            ),
            Error::HostExited => f.write_str("the server exited"),
            Error::Timeout => f.write_str("timed out waiting for the server"),
            Error::Decode(e) => write!(f, "unexpected message shape: {e}"),
            Error::NotAnObject => f.write_str("params must be a JSON object"),
        }
    }
}

impl std::error::Error for Error {}

impl From<io::Error> for Error {
    fn from(e: io::Error) -> Self {
        Error::Io(e)
    }
}

impl From<serde_json::Error> for Error {
    fn from(e: serde_json::Error) -> Self {
        Error::Decode(e)
    }
}

/// What differs between the peers that share [`Connection`].
pub(crate) trait Dialect: Send + Sync + 'static {
    /// Thread-name and log prefix (`eludite-host`, `rust-analyzer`).
    fn name(&self) -> &str;
    /// The generation a freshly spawned process starts at, given the spawn's epoch (1 for the first).
    fn initial_generation(&self, epoch: u64) -> Generation;
    /// The handshake after a spawn. Runs on the spawning thread; the reader is already running.
    fn handshake(&self, conn: &Connection) -> Result<(), Error>;
    /// Whether a request `method` (typed: `typed_generational` is its [`RequestType::GENERATIONAL`]) carries
    /// `eluditeGeneration` in its params.
    fn injects_generation(&self, method: &str, typed_generational: Option<bool>) -> bool;
    /// Whether the result of request `method` is pinned to the generation it was sent under.
    fn pins_result(&self, method: &str, injected: bool) -> bool;
    /// A notification from the server, as an event (`None` to drop it).
    fn notification(&self, conn: &Connection, n: Notification) -> Option<Event>;
    /// A request from the server other than `workspace/applyEdit`: the result to answer with, or an error.
    fn request(&self, conn: &Connection, r: &Request) -> Result<Value, ErrorObject>;
    /// A request from the server the shell answers later, as an event (`None`: [`Dialect::request`] answers it).
    fn deferred_request(&self, _r: &Request) -> Option<Result<Event, ErrorObject>> {
        None
    }
    /// Whether `workspace/applyEdit` params carry `eluditeGeneration` (the host's relay) or not (plain LSP).
    fn apply_edit_has_generation(&self) -> bool;
    /// The shutdown handshake before the process is waited for.
    fn shutdown(&self, conn: &Connection, timeout: Duration) -> Result<(), Error>;
    /// A notification the shell sent (document sync), after it was written.
    fn on_sent(&self, _conn: &Connection, _method: &str, _params: &Value) {}
}

type Reply = Result<Value, Error>;

struct Pending {
    tx: SyncSender<Reply>,
    /// The generation the request was pinned to.
    generation: Option<Generation>,
    canceled: bool,
}

struct Process {
    /// `None` for an in-process server.
    child: Option<Child>,
    stdin: Box<dyn Write + Send>,
}

impl Process {
    /// Kills the child, or closes an in-process server's input so it ends.
    fn kill(&mut self) -> io::Result<()> {
        match self.child.as_mut() {
            Some(c) => c.kill(),
            None => {
                self.stdin = Box::new(io::sink());
                Ok(())
            }
        }
    }
}

struct Inner {
    launch: Launch,
    dialect: Box<dyn Dialect>,
    restart: RestartPolicy,
    process: Mutex<Option<Process>>,
    pending: Mutex<HashMap<i64, Pending>>,
    next_id: AtomicI64,
    generation: AtomicU64,
    /// Incremented on every spawn; a reader only reports the exit of its own process.
    epoch: AtomicU64,
    restarts: AtomicU32,
    stopping: AtomicBool,
    events: Sender<Event>,
    /// The handshake's result (`eludite/host/initialize`'s or LSP `initialize`'s), as JSON.
    initialize: Mutex<Option<Value>>,
    exit_code: Mutex<Option<i32>>,
}

/// A running language server process and the JSON-RPC connection to it: the one abstraction the shell's editor
/// features use whether the server is `eludite-host` (Roslyn behind it) or a server the shell launched directly.
/// Cheap to clone; every method is thread-safe and never blocks on the server except the `wait` calls of
/// [`PendingRequest`] (and [`Connection::shutdown`]).
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("server", &self.inner.dialect.name())
            .field("pid", &self.pid())
            .field("generation", &self.generation())
            .finish()
    }
}

impl Connection {
    /// Spawns the server, runs the dialect's handshake and returns once it is done. Events arrive on the returned
    /// receiver.
    pub(crate) fn launch(
        launch: Launch,
        dialect: Box<dyn Dialect>,
        restart: RestartPolicy,
    ) -> Result<(Connection, Receiver<Event>), Error> {
        let (events, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            launch,
            dialect,
            restart,
            process: Mutex::new(None),
            pending: Mutex::new(HashMap::new()),
            next_id: AtomicI64::new(1),
            generation: AtomicU64::new(0),
            epoch: AtomicU64::new(0),
            restarts: AtomicU32::new(0),
            stopping: AtomicBool::new(false),
            events,
            initialize: Mutex::new(None),
            exit_code: Mutex::new(None),
        });
        let conn = Connection { inner };
        conn.spawn_and_initialize()?;
        Ok((conn, rx))
    }

    fn spawn_and_initialize(&self) -> Result<u32, Error> {
        let inner = &self.inner;
        let name = inner.dialect.name().to_owned();
        let (child, stdin, stdout): (Option<Child>, Box<dyn Write + Send>, Box<dyn Read + Send>) =
            match &inner.launch {
                Launch::InProcess(connector) => {
                    let (stdout, stdin) = connector()?;
                    (None, stdin, stdout)
                }
                Launch::Process(command) => {
                    let mut child = spawn(command)?;
                    let stdin = child.stdin.take().expect("piped stdin");
                    let stdout = child.stdout.take().expect("piped stdout");
                    if let Some(stderr) = child.stderr.take() {
                        let events = inner.events.clone();
                        thread::Builder::new()
                            .name(format!("{name}-stderr"))
                            .spawn(move || {
                                for line in BufReader::new(stderr).lines() {
                                    let Ok(line) = line else { break };
                                    if events.send(Event::Log(line)).is_err() {
                                        break;
                                    }
                                }
                            })?;
                    }
                    (Some(child), Box::new(stdin), Box::new(stdout))
                }
            };
        let pid = child.as_ref().map_or(0, Child::id);
        let epoch = inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        inner
            .generation
            .store(inner.dialect.initial_generation(epoch), Ordering::SeqCst);
        *lock(&inner.process) = Some(Process { child, stdin });
        let reader = self.clone();
        thread::Builder::new()
            .name(format!("{name}-reader"))
            .spawn(move || reader.read_loop(BufReader::new(stdout), epoch))?;
        inner.dialect.handshake(self)?;
        Ok(pid)
    }

    pub(crate) fn set_initialize_result(&self, result: Value) {
        *lock(&self.inner.initialize) = Some(result);
    }

    /// The handshake's result as JSON: `eludite/host/initialize`'s for the host, LSP `initialize`'s (with
    /// `capabilities` and `serverInfo`) for a generic server.
    pub fn initialize_result_value(&self) -> Option<Value> {
        lock(&self.inner.initialize).clone()
    }

    /// The server's process id, while it runs (`None` for an in-process server).
    pub fn pid(&self) -> Option<u32> {
        lock(&self.inner.process)
            .as_ref()
            .and_then(|p| p.child.as_ref().map(Child::id))
    }

    /// The current generation as this client knows it.
    pub fn generation(&self) -> Generation {
        self.inner.generation.load(Ordering::SeqCst)
    }

    pub(crate) fn observe_generation(&self, generation: Generation) {
        self.inner
            .generation
            .fetch_max(generation, Ordering::SeqCst);
    }

    pub(crate) fn emit(&self, event: Event) {
        let _ = self.inner.events.send(event);
    }

    /// Whether [`Connection::shutdown`] ran.
    pub(crate) fn stopping(&self) -> bool {
        self.inner.stopping.load(Ordering::SeqCst)
    }

    /// Sends a typed request. Requests the dialect pins (the host's forwarded LSP requests; every request to a
    /// generic server) are tied to the current generation: a result that arrives after it moved is dropped
    /// ([`Error::Stale`]). The host's forwarded requests also carry `eluditeGeneration`.
    pub fn request<R: RequestType>(
        &self,
        params: R::Params,
    ) -> Result<PendingRequest<R::Result>, Error> {
        let value = serde_json::to_value(params)?;
        let inject = self
            .inner
            .dialect
            .injects_generation(R::METHOD, Some(R::GENERATIONAL));
        self.send_request(R::METHOD, value, inject)
    }

    /// Sends a request with raw JSON params. Pinned and generation-carrying like typed ones.
    pub fn request_untyped(
        &self,
        method: &str,
        params: Value,
    ) -> Result<PendingRequest<Value>, Error> {
        let inject = self.inner.dialect.injects_generation(method, None);
        self.send_request(method, params, inject)
    }

    fn send_request<T>(
        &self,
        method: &str,
        mut params: Value,
        inject: bool,
    ) -> Result<PendingRequest<T>, Error> {
        let g = self.generation();
        if inject {
            params
                .as_object_mut()
                .ok_or(Error::NotAnObject)?
                .insert(host::GENERATION_FIELD.to_owned(), Value::from(g));
        }
        let generation = self.inner.dialect.pins_result(method, inject).then_some(g);
        let id = self.inner.next_id.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::sync_channel(1);
        lock(&self.inner.pending).insert(
            id,
            Pending {
                tx,
                generation,
                canceled: false,
            },
        );
        let params = (!params.is_null()).then_some(params);
        if let Err(e) = self.write(&Message::Request(Request::new(id, method, params))) {
            lock(&self.inner.pending).remove(&id);
            return Err(e);
        }
        Ok(PendingRequest {
            id,
            rx,
            conn: self.clone(),
            _result: PhantomData,
        })
    }

    /// Sends a typed notification.
    pub fn notify<N: NotificationType>(&self, params: N::Params) -> Result<(), Error> {
        self.notify_untyped(N::METHOD, serde_json::to_value(params)?)
    }

    /// Sends a notification with raw JSON params.
    pub fn notify_untyped(&self, method: &str, params: Value) -> Result<(), Error> {
        let params = (!params.is_null()).then_some(params);
        self.write(&Message::Notification(Notification::new(
            method,
            params.clone(),
        )))?;
        if let Some(p) = &params {
            self.inner.dialect.on_sent(self, method, p);
        }
        Ok(())
    }

    /// `textDocument/didOpen`.
    pub fn did_open(&self, params: DidOpenTextDocumentParams) -> Result<(), Error> {
        self.notify::<eludite_protocol::lsp::DidOpenTextDocument>(params)
    }

    /// `textDocument/didChange`.
    pub fn did_change(&self, params: DidChangeTextDocumentParams) -> Result<(), Error> {
        self.notify::<eludite_protocol::lsp::DidChangeTextDocument>(params)
    }

    /// `textDocument/didSave` (untyped: only the document's URI).
    pub fn did_save(&self, uri: &str) -> Result<(), Error> {
        self.notify_untyped(
            "textDocument/didSave",
            serde_json::json!({"textDocument": {"uri": uri}}),
        )
    }

    /// `textDocument/didClose`.
    pub fn did_close(&self, params: DidCloseTextDocumentParams) -> Result<(), Error> {
        self.notify::<eludite_protocol::lsp::DidCloseTextDocument>(params)
    }

    /// `$/cancelRequest` for a request id (normally through [`PendingRequest::cancel`]).
    pub fn cancel_id(&self, id: Id) {
        if let Id::Number(n) = id {
            self.cancel(n);
        }
    }

    /// Answer the server's `workspace/applyEdit` request `id` ([`Event::ApplyEdit`]).
    pub fn respond_apply_edit(
        &self,
        id: Id,
        result: ApplyWorkspaceEditResult,
    ) -> Result<(), Error> {
        let value = serde_json::to_value(result)?;
        self.write(&Message::Response(Response::ok(id, value)))
    }

    /// Answer a request of the server's that the shell answers later (an [`Event::NuGetCredentials`]).
    pub fn respond(&self, id: Id, result: Value) -> Result<(), Error> {
        self.write(&Message::Response(Response::ok(id, result)))
    }

    /// The dialect's shutdown handshake, then waits for the process to exit (killing it after `timeout`). Disables
    /// restarts. Returns the exit code.
    pub fn shutdown(&self, timeout: Duration) -> Result<Option<i32>, Error> {
        self.inner.stopping.store(true, Ordering::SeqCst);
        let _ = self.inner.dialect.shutdown(self, timeout);
        // The reader thread reaps the process when its stdout closes and records the exit code.
        let deadline = Instant::now() + timeout;
        let mut killed = false;
        loop {
            {
                let mut process = lock(&self.inner.process);
                match process.as_mut() {
                    None => return Ok(*lock(&self.inner.exit_code)),
                    Some(p) if !killed && Instant::now() > deadline => {
                        let _ = p.kill();
                        killed = true;
                    }
                    Some(_) => {}
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Kills the server without a shutdown. Restarts according to the policy unless [`Connection::shutdown`] ran.
    pub fn kill(&self) -> io::Result<()> {
        match lock(&self.inner.process).as_mut() {
            Some(p) => p.kill(),
            None => Ok(()),
        }
    }

    fn write(&self, message: &Message) -> Result<(), Error> {
        let body = serde_json::to_vec(message)?;
        let mut process = lock(&self.inner.process);
        let p = process.as_mut().ok_or(Error::HostExited)?;
        framing::write_message(&mut p.stdin, &body).map_err(|e| {
            if e.kind() == io::ErrorKind::BrokenPipe {
                Error::HostExited
            } else {
                Error::Io(e)
            }
        })
    }

    fn cancel(&self, id: i64) {
        let found = match lock(&self.inner.pending).get_mut(&id) {
            Some(p) => {
                p.canceled = true;
                true
            }
            None => false,
        };
        if found {
            let params = serde_json::to_value(CancelParams { id: Id::Number(id) })
                .expect("cancel params serialize");
            let _ = self.notify_untyped(methods::CANCEL_REQUEST, params);
        }
    }

    fn read_loop(self, mut stdout: impl BufRead, epoch: u64) {
        while let Ok(Some(body)) = framing::read_message(&mut stdout) {
            match serde_json::from_slice::<Message>(&body) {
                Ok(message) => self.dispatch(message),
                Err(e) => self.emit(Event::Log(format!(
                    "undecodable {} message: {e}",
                    self.inner.dialect.name()
                ))),
            }
        }
        self.on_exit(epoch);
    }

    fn dispatch(&self, message: Message) {
        match message {
            Message::Response(response) => self.on_response(response),
            Message::Notification(n) => {
                if let Some(event) = self.inner.dialect.notification(self, n) {
                    self.emit(event);
                }
            }
            Message::Request(r) => self.on_request(r),
        }
    }

    fn on_response(&self, response: Response) {
        let Some(Id::Number(id)) = response.id else {
            return;
        };
        let Some(pending) = lock(&self.inner.pending).remove(&id) else {
            return;
        };
        let current = self.generation();
        let reply = if pending.canceled {
            Err(Error::Canceled)
        } else {
            match response.payload {
                ResponsePayload::Error(e) if e.code == error_codes::CONTENT_MODIFIED => {
                    let data = e
                        .data
                        .clone()
                        .and_then(|d| serde_json::from_value::<ContentModifiedData>(d).ok());
                    Err(Error::Stale {
                        requested: data
                            .map(|d| d.requested_generation)
                            .or(pending.generation)
                            .unwrap_or_default(),
                        current: data.map(|d| d.current_generation).unwrap_or(current),
                    })
                }
                ResponsePayload::Error(e) if e.code == error_codes::REQUEST_CANCELLED => {
                    Err(Error::Canceled)
                }
                ResponsePayload::Error(e) => Err(Error::Rpc(e)),
                ResponsePayload::Result(_) if pending.generation.is_some_and(|g| g != current) => {
                    Err(Error::Stale {
                        requested: pending.generation.unwrap_or_default(),
                        current,
                    })
                }
                ResponsePayload::Result(v) => Ok(v),
            }
        };
        let _ = pending.tx.try_send(reply);
    }

    /// Requests from the server: `workspace/applyEdit` becomes [`Event::ApplyEdit`]; the dialect answers the rest.
    fn on_request(&self, r: Request) {
        if r.method == methods::APPLY_EDIT {
            let params = r.params.clone().unwrap_or(Value::Null);
            let decoded = if self.inner.dialect.apply_edit_has_generation() {
                serde_json::from_value::<WithGeneration<ApplyWorkspaceEditParams>>(params)
            } else {
                serde_json::from_value::<ApplyWorkspaceEditParams>(params).map(|params| {
                    WithGeneration {
                        params,
                        generation: self.generation(),
                    }
                })
            };
            match decoded {
                Ok(params) => self.emit(Event::ApplyEdit { id: r.id, params }),
                Err(e) => {
                    let _ = self.write(&Message::Response(Response::err(
                        Some(r.id),
                        ErrorObject::new(
                            ErrorObject::INVALID_PARAMS,
                            format!("workspace/applyEdit: {e}"),
                        ),
                    )));
                }
            }
            return;
        }
        match self.inner.dialect.deferred_request(&r) {
            Some(Ok(event)) => {
                self.emit(event);
                return;
            }
            Some(Err(e)) => {
                let _ = self.write(&Message::Response(Response::err(Some(r.id), e)));
                return;
            }
            None => {}
        }
        let response = match self.inner.dialect.request(self, &r) {
            Ok(result) => Response::ok(r.id, result),
            Err(e) => Response::err(Some(r.id), e),
        };
        let _ = self.write(&Message::Response(response));
    }

    fn on_exit(&self, epoch: u64) {
        if self.inner.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let code = {
            let mut process = lock(&self.inner.process);
            let code = process
                .as_mut()
                .and_then(|p| p.child.as_mut())
                .and_then(|c| c.wait().ok())
                .and_then(|s| s.code());
            // Recorded before the process entry goes, under the same lock `shutdown` reads both under: it sees
            // either the process or its exit code, never neither.
            *lock(&self.inner.exit_code) = code;
            process.take();
            code
        };
        let failed: Vec<Pending> = lock(&self.inner.pending).drain().map(|(_, p)| p).collect();
        for p in failed {
            let _ = p.tx.try_send(Err(Error::HostExited));
        }
        let expected = self.stopping();
        self.emit(Event::Host(HostEvent::Exited { code, expected }));
        if expected {
            return;
        }
        let attempt = self.inner.restarts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt > self.inner.restart.max_restarts {
            self.emit(Event::Host(HostEvent::GaveUp {
                reason: format!(
                    "{} exited {} times; restart budget spent",
                    self.inner.dialect.name(),
                    attempt
                ),
            }));
            return;
        }
        thread::sleep(self.inner.restart.backoff);
        if self.stopping() {
            return;
        }
        let event = match self.spawn_and_initialize() {
            Ok(pid) => HostEvent::Restarted { attempt, pid },
            Err(e) => HostEvent::GaveUp {
                reason: format!("restart failed: {e}"),
            },
        };
        self.emit(Event::Host(event));
    }
}

/// A request in flight. Dropping it does not cancel it; call [`PendingRequest::cancel`].
pub struct PendingRequest<T> {
    id: i64,
    rx: Receiver<Reply>,
    conn: Connection,
    _result: PhantomData<fn() -> T>,
}

impl<T> std::fmt::Debug for PendingRequest<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PendingRequest")
            .field("id", &self.id)
            .finish()
    }
}

impl<T: DeserializeOwned> PendingRequest<T> {
    pub fn id(&self) -> Id {
        Id::Number(self.id)
    }

    /// Sends `$/cancelRequest`. The wait then ends with [`Error::Canceled`] as soon as the server answers, and a
    /// result it had already sent is discarded.
    pub fn cancel(&self) {
        self.conn.cancel(self.id);
    }

    /// Blocks until the reply. Call it off the UI thread.
    pub fn wait(self) -> Result<T, Error> {
        let reply = self.rx.recv().map_err(|_| Error::HostExited)?;
        Ok(serde_json::from_value(reply?)?)
    }

    pub fn wait_timeout(self, timeout: Duration) -> Result<T, Error> {
        match self.rx.recv_timeout(timeout) {
            Ok(reply) => Ok(serde_json::from_value(reply?)?),
            Err(RecvTimeoutError::Timeout) => Err(Error::Timeout),
            Err(RecvTimeoutError::Disconnected) => Err(Error::HostExited),
        }
    }

    /// Non-blocking: `None` while the reply has not arrived.
    pub fn try_take(&mut self) -> Option<Result<T, Error>> {
        match self.rx.try_recv() {
            Ok(reply) => Some(reply.and_then(|v| Ok(serde_json::from_value(v)?))),
            Err(mpsc::TryRecvError::Empty) => None,
            Err(mpsc::TryRecvError::Disconnected) => Some(Err(Error::HostExited)),
        }
    }
}

fn spawn(command: &HostCommand) -> io::Result<Child> {
    let mut cmd = Command::new(&command.program);
    cmd.args(&command.args)
        .envs(command.envs.iter().map(|(k, v)| (k, v)))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(match command.stderr {
            StderrMode::Inherit => Stdio::inherit(),
            StderrMode::Capture => Stdio::piped(),
            StderrMode::Discard => Stdio::null(),
        });
    if let Some(dir) = &command.current_dir {
        cmd.current_dir(dir);
    }
    cmd.spawn()
}

pub(crate) fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Decodes a notification's params as `T`, or keeps it untyped.
pub(crate) fn typed_or_untyped<T: DeserializeOwned>(
    n: Notification,
    wrap: impl FnOnce(T) -> Event,
) -> Event {
    match serde_json::from_value::<T>(n.params.clone().unwrap_or(Value::Null)) {
        Ok(v) => wrap(v),
        Err(_) => Event::Notification(n),
    }
}
