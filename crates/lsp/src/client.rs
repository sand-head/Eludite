//! The shell's client of `eludite-host`: process supervision, framing, request correlation, cancellation and
//! solution-generation tracking (protocol/schemas/host-rpc.md).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader};
use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender, SyncSender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::host::{
    self, ContentModifiedData, Generation, GenerationResult, InitializeParams, InitializeResult,
    LanguageServerStatus, SolutionOpenParams, SolutionStatus, WithGeneration, error_codes, methods,
};
use eludite_protocol::jsonrpc::ResponsePayload;
use eludite_protocol::lsp::{CancelParams, PublishDiagnosticsParams};
use eludite_protocol::{
    ErrorObject, Id, Message, Notification, NotificationType, Request, RequestType, Response,
    framing,
};
use serde::de::DeserializeOwned;
use serde_json::Value;

/// How to start `eludite-host`.
#[derive(Debug, Clone)]
pub struct HostCommand {
    pub program: OsString,
    pub args: Vec<OsString>,
    pub envs: Vec<(OsString, OsString)>,
    pub current_dir: Option<PathBuf>,
    pub stderr: StderrMode,
}

/// Where the host's stderr (its log) goes. Never mixed into the protocol stream.
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
    pub fn dotnet_host(dll: &Path) -> Self {
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

    pub fn stderr(mut self, mode: StderrMode) -> Self {
        self.stderr = mode;
        self
    }
}

/// What the client sends in `eludite/host/initialize`.
#[derive(Debug, Clone)]
pub struct ClientInfo {
    pub name: String,
    pub version: String,
}

/// Restarts after the host exits on its own. `max_restarts == 0` disables them.
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

/// State changes of the host process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HostEvent {
    /// The process exited (or its stdout closed). Pending requests failed with [`Error::HostExited`].
    Exited { code: Option<i32>, expected: bool },
    /// A replacement process is up and initialized. The generation is back to 0; reopen the solution.
    Restarted { attempt: u32, pid: u32 },
    /// The restart budget is spent, or the restart failed.
    GaveUp { reason: String },
}

/// Everything the host tells the shell without being asked.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    SolutionStatus(SolutionStatus),
    LanguageServerStatus(LanguageServerStatus),
    /// Only for the current generation; stale ones are dropped.
    Diagnostics(WithGeneration<PublishDiagnosticsParams>),
    /// Untyped notifications (`window/showMessage`, `$/progress`).
    Notification(Notification),
    Host(HostEvent),
    /// A line of host stderr, with [`StderrMode::Capture`].
    Log(String),
}

#[derive(Debug)]
pub enum Error {
    Io(io::Error),
    /// An error response other than cancellation or a stale generation.
    Rpc(ErrorObject),
    /// The request was canceled; any result was discarded.
    Canceled,
    /// The result belongs to a generation that is no longer current, or the host answered -32801.
    Stale {
        requested: Generation,
        current: Generation,
    },
    HostExited,
    Timeout,
    Decode(serde_json::Error),
    /// A generational request whose params are not a JSON object.
    NotAnObject,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Io(e) => write!(f, "host i/o: {e}"),
            Error::Rpc(e) => write!(f, "host error {}: {}", e.code, e.message),
            Error::Canceled => f.write_str("request canceled"),
            Error::Stale { requested, current } => write!(
                f,
                "result for solution generation {requested} is stale (current {current})"
            ),
            Error::HostExited => f.write_str("eludite-host exited"),
            Error::Timeout => f.write_str("timed out waiting for eludite-host"),
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

type Reply = Result<Value, Error>;

struct Pending {
    tx: SyncSender<Reply>,
    /// The generation a forwarded request was pinned to.
    generation: Option<Generation>,
    canceled: bool,
}

struct Process {
    child: Child,
    stdin: ChildStdin,
}

struct Inner {
    command: HostCommand,
    client: ClientInfo,
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
    initialize: Mutex<Option<InitializeResult>>,
    exit_code: Mutex<Option<i32>>,
}

/// A running `eludite-host` and the connection to it. Cheap to clone; all methods are thread-safe and never block on
/// the host except the `wait` calls of [`PendingRequest`].
#[derive(Clone)]
pub struct HostClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for HostClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostClient")
            .field("pid", &self.pid())
            .field("generation", &self.generation())
            .finish()
    }
}

/// How long [`HostClient::start`] waits for the `eludite/host/initialize` reply.
const INITIALIZE_TIMEOUT: Duration = Duration::from_secs(30);

impl HostClient {
    /// Spawns the host, sends `eludite/host/initialize` and waits for its reply. Host events arrive on the returned
    /// receiver.
    pub fn start(
        command: HostCommand,
        client: ClientInfo,
        restart: RestartPolicy,
    ) -> Result<(HostClient, Receiver<Event>), Error> {
        let (events, rx) = mpsc::channel();
        let inner = Arc::new(Inner {
            command,
            client,
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
        let client = HostClient { inner };
        client.spawn_and_initialize()?;
        Ok((client, rx))
    }

    fn spawn_and_initialize(&self) -> Result<u32, Error> {
        let inner = &self.inner;
        let mut cmd = Command::new(&inner.command.program);
        cmd.args(&inner.command.args)
            .envs(inner.command.envs.iter().map(|(k, v)| (k, v)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(match inner.command.stderr {
                StderrMode::Inherit => Stdio::inherit(),
                StderrMode::Capture => Stdio::piped(),
                StderrMode::Discard => Stdio::null(),
            });
        if let Some(dir) = &inner.command.current_dir {
            cmd.current_dir(dir);
        }
        let mut child = cmd.spawn()?;
        let pid = child.id();
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        if let Some(stderr) = child.stderr.take() {
            let events = inner.events.clone();
            thread::Builder::new()
                .name("eludite-host-stderr".into())
                .spawn(move || {
                    for line in BufReader::new(stderr).lines() {
                        let Ok(line) = line else { break };
                        if events.send(Event::Log(line)).is_err() {
                            break;
                        }
                    }
                })?;
        }
        let epoch = inner.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        inner.generation.store(0, Ordering::SeqCst);
        *lock(&inner.process) = Some(Process { child, stdin });
        let reader = self.clone();
        thread::Builder::new()
            .name("eludite-host-reader".into())
            .spawn(move || reader.read_loop(BufReader::new(stdout), epoch))?;

        let init = self
            .request::<host::HostInitialize>(InitializeParams {
                client_name: inner.client.name.clone(),
                client_version: inner.client.version.clone(),
            })?
            .wait_timeout(INITIALIZE_TIMEOUT)?;
        *lock(&inner.initialize) = Some(init);
        Ok(pid)
    }

    /// The host's `eludite/host/initialize` result.
    pub fn initialize_result(&self) -> Option<InitializeResult> {
        lock(&self.inner.initialize).clone()
    }

    /// The host process id, while it runs.
    pub fn pid(&self) -> Option<u32> {
        lock(&self.inner.process).as_ref().map(|p| p.child.id())
    }

    /// The current solution generation as this client knows it.
    pub fn generation(&self) -> Generation {
        self.inner.generation.load(Ordering::SeqCst)
    }

    fn observe_generation(&self, generation: Generation) {
        self.inner
            .generation
            .fetch_max(generation, Ordering::SeqCst);
    }

    /// Sends a typed request. Forwarded LSP requests ([`RequestType::GENERATIONAL`]) are pinned to the current
    /// generation: `eluditeGeneration` is added to their params and a result that arrives after the generation moved
    /// is dropped ([`Error::Stale`]).
    pub fn request<R: RequestType>(
        &self,
        params: R::Params,
    ) -> Result<PendingRequest<R::Result>, Error> {
        let value = serde_json::to_value(params)?;
        self.send_request(R::METHOD, value, R::GENERATIONAL)
    }

    /// Sends a request with raw JSON params ("forwarded, untyped" methods). Forwarded methods are pinned to the
    /// current generation like typed ones.
    pub fn request_untyped(
        &self,
        method: &str,
        params: Value,
    ) -> Result<PendingRequest<Value>, Error> {
        let generational = methods::FORWARDED_TYPED_REQUESTS.contains(&method)
            || methods::FORWARDED_UNTYPED_REQUESTS.contains(&method);
        self.send_request(method, params, generational)
    }

    fn send_request<T>(
        &self,
        method: &str,
        mut params: Value,
        generational: bool,
    ) -> Result<PendingRequest<T>, Error> {
        let generation = if generational {
            let g = self.generation();
            params
                .as_object_mut()
                .ok_or(Error::NotAnObject)?
                .insert(host::GENERATION_FIELD.to_owned(), Value::from(g));
            Some(g)
        } else {
            None
        };
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
            client: self.clone(),
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
        self.write(&Message::Notification(Notification::new(method, params)))
    }

    /// `eludite/solution/open`: returns the new generation, which becomes current at once.
    pub fn open_solution(&self, path: &str, timeout: Duration) -> Result<Generation, Error> {
        let r: GenerationResult = self
            .request::<host::SolutionOpen>(SolutionOpenParams { path: path.into() })?
            .wait_timeout(timeout)?;
        self.observe_generation(r.generation);
        Ok(r.generation)
    }

    /// `eludite/solution/close`: returns the current generation after the close.
    pub fn close_solution(&self, timeout: Duration) -> Result<Generation, Error> {
        let r: GenerationResult = self
            .request::<host::SolutionClose>(())?
            .wait_timeout(timeout)?;
        self.observe_generation(r.generation);
        Ok(r.generation)
    }

    /// `eludite/host/shutdown`, `eludite/host/exit`, then waits for the process to exit (killing it after `timeout`).
    /// Disables restarts. Returns the exit code.
    pub fn shutdown(&self, timeout: Duration) -> Result<Option<i32>, Error> {
        self.inner.stopping.store(true, Ordering::SeqCst);
        let result = self
            .request::<host::HostShutdown>(())
            .and_then(|p| p.wait_timeout(timeout));
        if result.is_ok() {
            let _ = self.notify::<host::HostExit>(());
        }
        // The reader thread reaps the process when its stdout closes and records the exit code.
        let deadline = Instant::now() + timeout;
        let mut killed = false;
        loop {
            {
                let mut process = lock(&self.inner.process);
                match process.as_mut() {
                    None => return Ok(*lock(&self.inner.exit_code)),
                    Some(p) if !killed && Instant::now() > deadline => {
                        let _ = p.child.kill();
                        killed = true;
                    }
                    Some(_) => {}
                }
            }
            thread::sleep(Duration::from_millis(5));
        }
    }

    /// Kills the host without a shutdown. Restarts according to the policy unless [`HostClient::shutdown`] ran.
    pub fn kill(&self) -> io::Result<()> {
        match lock(&self.inner.process).as_mut() {
            Some(p) => p.child.kill(),
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
                Err(e) => {
                    let _ = self
                        .inner
                        .events
                        .send(Event::Log(format!("undecodable host message: {e}")));
                }
            }
        }
        self.on_exit(epoch);
    }

    fn dispatch(&self, message: Message) {
        match message {
            Message::Response(response) => self.on_response(response),
            Message::Notification(n) => self.on_notification(n),
            Message::Request(r) => {
                // The host sends no requests (host-rpc.md); answer anything unexpected.
                let _ = self.write(&Message::Response(Response::err(
                    Some(r.id),
                    ErrorObject::new(
                        ErrorObject::METHOD_NOT_FOUND,
                        format!("{} is not a shell method", r.method),
                    ),
                )));
            }
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

    fn on_notification(&self, n: Notification) {
        let params = n.params.clone().unwrap_or(Value::Null);
        let event = match n.method.as_str() {
            methods::SOLUTION_STATUS => match serde_json::from_value::<SolutionStatus>(params) {
                Ok(status) => {
                    self.observe_generation(status.generation);
                    Event::SolutionStatus(status)
                }
                Err(_) => Event::Notification(n),
            },
            methods::LANGUAGE_SERVER_STATUS => {
                match serde_json::from_value::<LanguageServerStatus>(params) {
                    Ok(status) => Event::LanguageServerStatus(status),
                    Err(_) => Event::Notification(n),
                }
            }
            methods::PUBLISH_DIAGNOSTICS => {
                match serde_json::from_value::<WithGeneration<PublishDiagnosticsParams>>(params) {
                    // Diagnostics computed under an older generation are not rendered (CLAUDE.md invariant 12).
                    Ok(d) if d.generation < self.generation() => return,
                    Ok(d) => Event::Diagnostics(d),
                    Err(_) => Event::Notification(n),
                }
            }
            _ => Event::Notification(n),
        };
        let _ = self.inner.events.send(event);
    }

    fn on_exit(&self, epoch: u64) {
        if self.inner.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        let code = {
            let mut process = lock(&self.inner.process);
            let code = process
                .as_mut()
                .and_then(|p| p.child.wait().ok())
                .and_then(|s| s.code());
            process.take();
            code
        };
        *lock(&self.inner.exit_code) = code;
        let failed: Vec<Pending> = lock(&self.inner.pending).drain().map(|(_, p)| p).collect();
        for p in failed {
            let _ = p.tx.try_send(Err(Error::HostExited));
        }
        let expected = self.inner.stopping.load(Ordering::SeqCst);
        let _ = self
            .inner
            .events
            .send(Event::Host(HostEvent::Exited { code, expected }));
        if expected {
            return;
        }
        let attempt = self.inner.restarts.fetch_add(1, Ordering::SeqCst) + 1;
        if attempt > self.inner.restart.max_restarts {
            let _ = self.inner.events.send(Event::Host(HostEvent::GaveUp {
                reason: format!("eludite-host exited {} times; restart budget spent", attempt),
            }));
            return;
        }
        thread::sleep(self.inner.restart.backoff);
        if self.inner.stopping.load(Ordering::SeqCst) {
            return;
        }
        let event = match self.spawn_and_initialize() {
            Ok(pid) => HostEvent::Restarted { attempt, pid },
            Err(e) => HostEvent::GaveUp {
                reason: format!("restart failed: {e}"),
            },
        };
        let _ = self.inner.events.send(Event::Host(event));
    }
}

/// A request in flight. Dropping it does not cancel it; call [`PendingRequest::cancel`].
pub struct PendingRequest<T> {
    id: i64,
    rx: Receiver<Reply>,
    client: HostClient,
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

    /// Sends `$/cancelRequest`. The wait then ends with [`Error::Canceled`] as soon as the host answers (within
    /// 50 ms by contract), and a result the host had already sent is discarded.
    pub fn cancel(&self) {
        self.client.cancel(self.id);
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

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
