//! The shell's side of `eludite-host` (brief 0012): starts and supervises the host through `eludite-lsp`, opens and
//! closes solutions, asks for the Solution Explorer tree, and sends the editors' LSP document notifications.
//!
//! Nothing here runs on the UI thread. The UI hands work to a worker thread through a channel ([`HostSession`]'s
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
//! the shell answers it with [`HostSession::respond_apply_edit`], through the worker like everything else.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_lsp::host::{
    self, Generation, LanguageServerStatus, SolutionState, SolutionStatus, SolutionTree,
};
use eludite_lsp::lsp::{
    self, DidChangeTextDocumentParams, DidCloseTextDocumentParams, DidOpenTextDocumentParams,
    Position, Range, TextDocumentContentChangeEvent, TextDocumentIdentifier, TextDocumentItem,
    VersionedTextDocumentIdentifier,
};
use eludite_lsp::{
    ClientInfo, Connector, Event, HostClient, HostCommand, HostEvent, Id, RestartPolicy,
};
use eludite_protocol::RequestType;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use futures::channel::oneshot;
use serde_json::json;

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
            HostLaunch::Process(cmd.arg("--stdio"))
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
    /// The Solution Explorer tree for the current generation.
    Tree(SolutionTree),
    /// `textDocument/publishDiagnostics` for the current generation.
    Diagnostics(lsp::PublishDiagnosticsParams),
    Closed,
    /// `workspace/applyEdit` from the host, computed under `generation`; answer with
    /// [`HostSession::respond_apply_edit`] and `id`.
    ApplyEdit {
        id: Id,
        generation: Generation,
        params: lsp::ApplyWorkspaceEditParams,
    },
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
    /// The host restarted on its own: reopen and replay.
    Replay,
    /// Send a request (after the notifications queued before it).
    Request(RequestJob),
    /// Cancel the request with this ticket if it is still in flight.
    Cancel(u64),
    /// Answer the host's `workspace/applyEdit`.
    RespondApplyEdit(Id, lsp::ApplyWorkspaceEditResult),
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

/// The shell's handle on the host. Cheap to clone, `Send + Sync`; every method returns at once.
#[derive(Clone)]
pub struct HostSession {
    tx: Arc<Mutex<Sender<Cmd>>>,
    shared: Arc<Mutex<Shared>>,
    tickets: Arc<AtomicU64>,
}

/// Why a request has no result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RequestError {
    /// No host is running (no solution was opened, or it failed to start).
    NoHost,
    /// Canceled (by the shell, or the host answered RequestCancelled).
    Canceled,
    /// Computed under a solution generation that is no longer current.
    Stale,
    Failed(String),
}

/// The answer to [`HostSession::request`], with when the request was written and answered (for the latency
/// measurements). A result computed under an older solution generation is already [`RequestError::Stale`].
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
type RequestJob = Box<dyn FnOnce(Option<&HostClient>, &Inflight) + Send>;

/// A request in flight; [`RequestHandle::cancel`] cancels it. Dropping it does not.
#[derive(Debug, Clone)]
pub struct RequestHandle {
    ticket: u64,
    session: HostSession,
}

impl RequestHandle {
    pub fn cancel(&self) {
        self.session.send(Cmd::Cancel(self.ticket));
    }
}

impl std::fmt::Debug for HostSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("HostSession")
            .field("solution", &self.shared().solution)
            .finish()
    }
}

impl HostSession {
    /// Starts the worker (the host itself starts on the first `open`). Events arrive on the returned receiver.
    pub fn spawn(launch: HostLaunch) -> (Self, UnboundedReceiver<SessionEvent>) {
        let (tx, rx) = mpsc::channel();
        let (events, events_rx) = unbounded();
        let shared = Arc::new(Mutex::new(Shared::default()));
        let worker = Worker {
            launch,
            client: None,
            events,
            shared: shared.clone(),
            docs: HashMap::new(),
            tx: tx.clone(),
            inflight: Inflight::default(),
        };
        thread::Builder::new()
            .name("eludite-host-session".into())
            .spawn(move || worker.run(rx))
            .expect("spawn the host session thread");
        (
            Self {
                tx: Arc::new(Mutex::new(tx)),
                shared,
                tickets: Arc::new(AtomicU64::new(1)),
            },
            events_rx,
        )
    }

    /// Send forwarded request `R` after the document notifications already queued. The reply arrives on the returned
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

    /// Answer the host's `workspace/applyEdit` request `id` ([`SessionEvent::ApplyEdit`]).
    pub fn respond_apply_edit(&self, id: Id, result: lsp::ApplyWorkspaceEditResult) {
        self.send(Cmd::RespondApplyEdit(id, result));
    }

    /// Shuts the host down; the returned receiver fires when done (or the worker is gone).
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

struct Worker {
    launch: HostLaunch,
    client: Option<HostClient>,
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

    fn run(mut self, rx: Receiver<Cmd>) {
        while let Ok(cmd) = rx.recv() {
            match cmd {
                Cmd::Open(path) => self.open(&path),
                Cmd::Close => {
                    if let Some(c) = &self.client {
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
                    self.send_open(&uri, &language_id, version, &text);
                    self.docs.insert(
                        uri,
                        Doc {
                            language_id,
                            version,
                            text,
                        },
                    );
                }
                Cmd::DidChange { uri, version, text } => {
                    let Some(doc) = self.docs.get_mut(&uri) else {
                        continue;
                    };
                    let change = diff(&doc.text, &text);
                    doc.version = version;
                    doc.text = text;
                    if let (Some(c), Some(change)) = (&self.client, change) {
                        let _ =
                            c.notify::<lsp::DidChangeTextDocument>(DidChangeTextDocumentParams {
                                text_document: VersionedTextDocumentIdentifier { uri, version },
                                content_changes: vec![change],
                            });
                    }
                }
                Cmd::DidSave { uri } => {
                    if let Some(c) = &self.client {
                        let _ = c.notify_untyped(
                            "textDocument/didSave",
                            json!({"textDocument": {"uri": uri}}),
                        );
                    }
                }
                Cmd::DidClose { uri } => {
                    if self.docs.remove(&uri).is_some()
                        && let Some(c) = &self.client
                    {
                        let _ = c.notify::<lsp::DidCloseTextDocument>(DidCloseTextDocumentParams {
                            text_document: TextDocumentIdentifier { uri },
                        });
                    }
                }
                Cmd::Replay => {
                    let solution = lock(&self.shared).solution.clone();
                    if let Some(path) = solution {
                        self.open(&path);
                    }
                    for (uri, d) in &self.docs {
                        self.send_open(uri, &d.language_id, d.version, &d.text);
                    }
                }
                Cmd::Request(job) => job(self.client.as_ref(), &self.inflight),
                Cmd::Cancel(ticket) => {
                    let id = lock(&self.inflight).remove(&ticket);
                    if let (Some(id), Some(c)) = (id, &self.client) {
                        let _ = c.notify::<lsp::Cancel>(lsp::CancelParams { id });
                    }
                }
                Cmd::RespondApplyEdit(id, result) => {
                    if let Some(c) = &self.client {
                        let _ = c.respond_apply_edit(id, result);
                    }
                }
                Cmd::Shutdown(done) => {
                    if let Some(c) = self.client.take() {
                        let _ = c.shutdown(Duration::from_secs(5));
                    }
                    let _ = done.send(());
                    return;
                }
            }
        }
    }

    fn send_open(&self, uri: &str, language_id: &str, version: i32, text: &str) {
        if let Some(c) = &self.client {
            let _ = c.notify::<lsp::DidOpenTextDocument>(DidOpenTextDocumentParams {
                text_document: TextDocumentItem {
                    uri: uri.into(),
                    language_id: language_id.into(),
                    version,
                    text: text.into(),
                },
            });
        }
    }

    /// Starts the host if needed. Returns false (and reports why) when it cannot.
    fn ensure_host(&mut self) -> bool {
        if self.client.is_some() {
            return true;
        }
        let info = ClientInfo {
            name: "eludite".into(),
            version: eludite_commands::builtins::VERSION.into(),
        };
        let started = match &self.launch {
            HostLaunch::Process(cmd) => {
                HostClient::start(cmd.clone(), info, RestartPolicy::default())
            }
            HostLaunch::InProcess(connector) => {
                HostClient::start_in_process(connector.clone(), info, RestartPolicy::default())
            }
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
                let pump = Pump {
                    client: client.clone(),
                    events: self.events.clone(),
                    shared: self.shared.clone(),
                    tx: self.tx.clone(),
                };
                let _ = thread::Builder::new()
                    .name("eludite-host-events".into())
                    .spawn(move || pump.run(events));
                // Replay documents opened before the host was up.
                for (uri, d) in &self.docs {
                    let _ = client.notify::<lsp::DidOpenTextDocument>(DidOpenTextDocumentParams {
                        text_document: TextDocumentItem {
                            uri: uri.clone(),
                            language_id: d.language_id.clone(),
                            version: d.version,
                            text: d.text.clone(),
                        },
                    });
                }
                self.client = Some(client);
                true
            }
            Err(e) => {
                self.emit(SessionEvent::HostFailed {
                    reason: format!("eludite-host did not start: {e}"),
                });
                false
            }
        }
    }

    fn open(&mut self, path: &Path) {
        self.emit(SessionEvent::Opening {
            path: path.to_path_buf(),
        });
        if !self.ensure_host() {
            return;
        }
        let Some(client) = self.client.clone() else {
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

/// Turns host events into session events.
struct Pump {
    client: HostClient,
    events: UnboundedSender<SessionEvent>,
    shared: Arc<Mutex<Shared>>,
    tx: Sender<Cmd>,
}

impl Pump {
    fn run(self, rx: Receiver<Event>) {
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
                Event::Diagnostics(d) if d.generation == self.client.generation() => {
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
                    continue;
                }
                Event::Host(HostEvent::Exited {
                    expected: false, ..
                }) => SessionEvent::HostRestarting,
                Event::Host(HostEvent::GaveUp { reason }) => SessionEvent::HostFailed { reason },
                Event::Host(HostEvent::Exited { .. }) | Event::Notification(_) => continue,
                Event::Log(line) => {
                    eprintln!("[eludite-host] {line}");
                    continue;
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
