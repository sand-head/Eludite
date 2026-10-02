//! The DAP client: one adapter connection, a reader thread, a writer thread and a stderr drain.
//!
//! Nothing here blocks the caller on the adapter unless it asks to: [`DapClient::request`] queues the request for
//! the writer thread and returns its `seq`; the response arrives through the [`EventSink`] as
//! [`ClientEvent::Response`], like events do. [`DapClient::request_wait`] and [`DapClient::request_channel`] are for
//! worker threads (the launch handshake in [`crate::session`]) and tests.
//!
//! When the connection closes (the adapter exited, crashed or hung up) every pending request fails with
//! [`DapError::Closed`] and the sink gets one [`ClientEvent::Closed`] with the exit code, whether the adapter said
//! `terminated` first, and the last lines of its stderr.

use std::collections::{HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::Child;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::types::{Capabilities, Event};
use crate::{Connection, ProtocolMessage, framing};

/// Lines of the adapter's stderr kept for the message when it exits unexpectedly.
const STDERR_TAIL: usize = 40;

/// What the client reports, on its reader thread.
#[derive(Debug, Clone, PartialEq)]
pub enum ClientEvent {
    Event(Event),
    /// The answer to a request sent with [`DapClient::request`]: its body, or the adapter's error message.
    Response {
        request_seq: i64,
        command: String,
        result: Result<Value, String>,
    },
    /// The connection closed. `terminated`: the adapter sent `terminated` first (a normal end); otherwise it exited or
    /// hung up on its own (a crash).
    Closed {
        exit_code: Option<i32>,
        terminated: bool,
        stderr_tail: Vec<String>,
    },
}

/// Where the client delivers events and responses. Called on the client's reader thread; it must not block.
pub type EventSink = Arc<dyn Fn(ClientEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DapError {
    /// The connection is closed (the adapter exited).
    Closed,
    /// No answer in time.
    Timeout(String),
    /// The adapter answered `success: false`.
    Failed {
        command: String,
        message: String,
    },
    Io(String),
}

impl std::fmt::Display for DapError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            DapError::Closed => write!(f, "the debug adapter is not running"),
            DapError::Timeout(what) => write!(f, "the debug adapter did not answer {what} in time"),
            DapError::Failed { command, message } => write!(f, "{command}: {message}"),
            DapError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for DapError {}

enum Pending {
    Sink,
    Channel(mpsc::Sender<Result<Value, DapError>>),
}

struct Inner {
    tx: Mutex<Option<mpsc::Sender<Vec<u8>>>>,
    seq: AtomicI64,
    pending: Mutex<HashMap<i64, (String, Pending)>>,
    closed: AtomicBool,
    initialized: (Mutex<bool>, Condvar),
    capabilities: Mutex<Capabilities>,
    child: Mutex<Option<Child>>,
    shutdown: Option<Box<dyn Fn() + Send + Sync>>,
    description: String,
    stderr_tail: Arc<Mutex<VecDeque<String>>>,
    trace: bool,
}

/// A connection to one debug adapter. Cheap to clone; every clone talks to the same adapter.
#[derive(Clone)]
pub struct DapClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for DapClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DapClient")
            .field("adapter", &self.inner.description)
            .field("closed", &self.is_closed())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl DapClient {
    /// Start the client's threads on `connection`; events and responses go to `sink`.
    pub fn start(connection: Connection, sink: EventSink) -> Self {
        let Connection {
            reader,
            writer,
            child,
            stderr,
            shutdown,
            description,
        } = connection;
        let (tx, rx) = mpsc::channel::<Vec<u8>>();
        let stderr_tail: Arc<Mutex<VecDeque<String>>> = Arc::default();
        let trace = std::env::var_os("ELUDITE_TRACE_DAP").is_some_and(|v| v != "0");
        let inner = Arc::new(Inner {
            tx: Mutex::new(Some(tx)),
            seq: AtomicI64::new(1),
            pending: Mutex::default(),
            closed: AtomicBool::new(false),
            initialized: (Mutex::new(false), Condvar::new()),
            capabilities: Mutex::default(),
            child: Mutex::new(child),
            shutdown,
            description,
            stderr_tail: stderr_tail.clone(),
            trace,
        });
        std::thread::Builder::new()
            .name("dap-writer".into())
            .spawn(move || writer_loop(writer, rx))
            .expect("spawn dap-writer");
        if let Some(stderr) = stderr {
            std::thread::Builder::new()
                .name("dap-stderr".into())
                .spawn(move || stderr_loop(stderr, stderr_tail, trace))
                .expect("spawn dap-stderr");
        }
        let reader_inner = inner.clone();
        std::thread::Builder::new()
            .name("dap-reader".into())
            .spawn(move || reader_loop(reader, reader_inner, sink))
            .expect("spawn dap-reader");
        Self { inner }
    }

    /// `netcoredbg --interpreter=vscode (stdio)`.
    pub fn description(&self) -> &str {
        &self.inner.description
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// The adapter's capabilities: from the `initialize` response ([`crate::session::start`] records it) and any
    /// later `capabilities` event.
    pub fn capabilities(&self) -> Capabilities {
        lock(&self.inner.capabilities).clone()
    }

    pub fn set_capabilities(&self, caps: Capabilities) {
        *lock(&self.inner.capabilities) = caps;
    }

    /// The adapter process id, for a stdio or ssh transport.
    pub fn adapter_pid(&self) -> Option<u32> {
        lock(&self.inner.child).as_ref().map(Child::id)
    }

    fn send(&self, command: &str, arguments: Value, pending: Pending) -> Result<i64, DapError> {
        if self.is_closed() {
            return Err(DapError::Closed);
        }
        let seq = self.inner.seq.fetch_add(1, Ordering::AcqRel);
        let msg = ProtocolMessage::Request {
            seq,
            command: command.to_owned(),
            arguments: (!arguments.is_null()).then_some(arguments),
        };
        let bytes = serde_json::to_vec(&msg).map_err(|e| DapError::Io(e.to_string()))?;
        if self.inner.trace {
            trace(">>", &bytes);
        }
        lock(&self.inner.pending).insert(seq, (command.to_owned(), pending));
        let sent = lock(&self.inner.tx)
            .as_ref()
            .is_some_and(|tx| tx.send(bytes).is_ok());
        if !sent {
            lock(&self.inner.pending).remove(&seq);
            return Err(DapError::Closed);
        }
        Ok(seq)
    }

    /// Send a request; its response arrives through the sink as [`ClientEvent::Response`] with this `seq`. Never
    /// waits on the adapter.
    pub fn request(&self, command: &str, arguments: Value) -> Result<i64, DapError> {
        self.send(command, arguments, Pending::Sink)
    }

    /// Send a request and get its answer on a channel.
    pub fn request_channel(
        &self,
        command: &str,
        arguments: Value,
    ) -> Result<mpsc::Receiver<Result<Value, DapError>>, DapError> {
        let (tx, rx) = mpsc::channel();
        self.send(command, arguments, Pending::Channel(tx))?;
        Ok(rx)
    }

    /// Send a request and wait for its answer. For worker threads and tests, never the UI thread.
    pub fn request_wait(
        &self,
        command: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<Value, DapError> {
        self.request_channel(command, arguments)?
            .recv_timeout(timeout)
            .unwrap_or_else(|e| match e {
                mpsc::RecvTimeoutError::Timeout => Err(DapError::Timeout(command.to_owned())),
                mpsc::RecvTimeoutError::Disconnected => Err(DapError::Closed),
            })
    }

    /// Wait until the adapter has sent `initialized` (before or after `launch`: adapters differ).
    pub fn wait_initialized(&self, timeout: Duration) -> bool {
        let (m, cv) = &self.inner.initialized;
        let deadline = Instant::now() + timeout;
        let mut done = lock(m);
        while !*done {
            let now = Instant::now();
            if now >= deadline || self.is_closed() {
                return *done;
            }
            done = cv
                .wait_timeout(done, (deadline - now).min(Duration::from_millis(50)))
                .map(|(g, _)| g)
                .unwrap_or_else(|e| e.into_inner().0);
        }
        true
    }

    /// Close the connection: stop writing, hang up a socket, kill a child adapter. The reader then reports
    /// [`ClientEvent::Closed`].
    pub fn kill(&self) {
        lock(&self.inner.tx).take();
        if let Some(shutdown) = &self.inner.shutdown {
            shutdown();
        }
        if let Some(child) = lock(&self.inner.child).as_mut() {
            let _ = child.kill();
        }
    }
}

fn trace(dir: &str, bytes: &[u8]) {
    let ms = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let text = String::from_utf8_lossy(bytes);
    let cut: String = text.chars().take(400).collect();
    eprintln!("[dap {ms}] {dir} {cut}");
}

fn writer_loop(mut writer: Box<dyn Write + Send>, rx: mpsc::Receiver<Vec<u8>>) {
    while let Ok(bytes) = rx.recv() {
        if framing::write_message(&mut writer, &bytes).is_err() {
            break;
        }
    }
}

fn stderr_loop(stderr: impl Read, tail: Arc<Mutex<VecDeque<String>>>, trace: bool) {
    for line in BufReader::new(stderr).lines() {
        let Ok(line) = line else { break };
        if trace {
            eprintln!("[dap stderr] {line}");
        }
        let mut t = lock(&tail);
        if t.len() == STDERR_TAIL {
            t.pop_front();
        }
        t.push_back(line);
    }
}

fn reader_loop(reader: Box<dyn Read + Send>, inner: Arc<Inner>, sink: EventSink) {
    let mut reader = BufReader::new(reader);
    let mut terminated = false;
    while let Ok(Some(body)) = framing::read_message(&mut reader) {
        if inner.trace {
            trace("<<", &body);
        }
        let Ok(msg) = serde_json::from_slice::<ProtocolMessage>(&body) else {
            continue;
        };
        match msg {
            ProtocolMessage::Response {
                request_seq,
                success,
                command,
                message,
                body,
                ..
            } => {
                let Some((_, pending)) = lock(&inner.pending).remove(&request_seq) else {
                    continue;
                };
                let body = body.unwrap_or(Value::Null);
                let message = || message.clone().unwrap_or_else(|| "failed".to_owned());
                match pending {
                    Pending::Sink => sink(ClientEvent::Response {
                        request_seq,
                        command,
                        result: if success { Ok(body) } else { Err(message()) },
                    }),
                    Pending::Channel(tx) => {
                        let _ = tx.send(if success {
                            Ok(body)
                        } else {
                            Err(DapError::Failed {
                                command,
                                message: message(),
                            })
                        });
                    }
                }
            }
            ProtocolMessage::Event { event, body, .. } => {
                let event = Event::decode(&event, body.unwrap_or(Value::Null));
                match &event {
                    Event::Initialized => {
                        let (m, cv) = &inner.initialized;
                        *lock(m) = true;
                        cv.notify_all();
                    }
                    Event::Capabilities(c) => merge_capabilities(&mut lock(&inner.capabilities), c),
                    Event::Terminated => terminated = true,
                    _ => {}
                }
                sink(ClientEvent::Event(event));
            }
            ProtocolMessage::Request { seq, command, .. } => {
                // Reverse requests (runInTerminal, startDebugging) are not supported: say so, so the adapter does not
                // wait forever.
                let reply = ProtocolMessage::Response {
                    seq: 0,
                    request_seq: seq,
                    success: false,
                    command,
                    message: Some("not supported by Eludite".into()),
                    body: None,
                };
                if let (Ok(bytes), Some(tx)) =
                    (serde_json::to_vec(&reply), lock(&inner.tx).as_ref())
                {
                    let _ = tx.send(bytes);
                }
            }
        }
    }
    inner.closed.store(true, Ordering::Release);
    lock(&inner.tx).take();
    {
        let (m, cv) = &inner.initialized;
        let _guard = lock(m);
        cv.notify_all();
    }
    let pending: Vec<_> = lock(&inner.pending).drain().collect();
    for (request_seq, (command, p)) in pending {
        match p {
            Pending::Channel(tx) => {
                let _ = tx.send(Err(DapError::Closed));
            }
            Pending::Sink => sink(ClientEvent::Response {
                request_seq,
                command,
                result: Err(DapError::Closed.to_string()),
            }),
        }
    }
    // A child that closed its stdout is exiting: give it a moment to report its code.
    let mut exit_code = None;
    let deadline = Instant::now() + Duration::from_millis(500);
    loop {
        let status = match lock(&inner.child).as_mut() {
            Some(c) => c.try_wait().ok().flatten(),
            None => break,
        };
        if let Some(s) = status {
            exit_code = s.code();
            break;
        }
        if Instant::now() >= deadline {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    let stderr_tail = lock(&inner.stderr_tail).iter().cloned().collect();
    sink(ClientEvent::Closed {
        exit_code,
        terminated,
        stderr_tail,
    });
}

/// Apply a `capabilities` event: it carries only what changed, so a member present in it wins.
fn merge_capabilities(into: &mut Capabilities, update: &Capabilities) {
    let d = Capabilities::default();
    macro_rules! merge {
        ($($f:ident),*) => {$(if update.$f != d.$f { into.$f = update.$f.clone(); })*};
    }
    merge!(
        supports_configuration_done_request,
        supports_conditional_breakpoints,
        supports_hit_conditional_breakpoints,
        supports_exception_info_request,
        supports_terminate_request,
        supports_evaluate_for_hovers,
        supports_cancel_request,
        support_terminate_debuggee,
        exception_breakpoint_filters
    );
}
