//! The ACP client connection: JSON-RPC 2.0, newline-delimited, over a child
//! process's stdio (or any pair of streams, for tests).
//!
//! Threading: one reader thread decodes the agent's stdout and one drains its
//! stderr. Events go to the caller's callback on the reader thread; requests
//! are blocking calls that wait on a channel. Nothing here touches a UI
//! thread: the shell calls these methods from background threads and forwards
//! events to its UI (CLAUDE.md invariant 1).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::protocol::{
    CancelNotification, ClientCapabilities, ContentBlock, Implementation, InitializeRequest,
    InitializeResponse, LoadSessionRequest, LoadSessionResponse, McpServer, NewSessionRequest,
    NewSessionResponse, PROTOCOL_VERSION, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, RequestPermissionResponse, SessionNotification,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest, methods,
};
use crate::{AgentDescriptor, ErrorObject, Id, Message, Notification, Request, Response};

/// Something the agent sent or did that the UI should know about.
#[derive(Debug, Clone)]
pub enum ClientEvent {
    /// A `session/update` notification.
    Update(SessionNotification),
    /// `session/request_permission`: answer with [`AcpClient::respond_permission`].
    PermissionRequest {
        id: Id,
        request: RequestPermissionRequest,
    },
    /// A notification this client does not interpret (e.g. adapter extensions).
    OtherNotification {
        method: String,
        params: Option<Value>,
    },
    /// A line on the agent's stderr.
    Stderr(String),
    /// A line on stdout that is not a JSON-RPC message.
    ProtocolError(String),
    /// The agent's stdout closed.
    Closed,
}

pub type EventSink = Arc<dyn Fn(ClientEvent) + Send + Sync>;

#[derive(Debug, Clone, PartialEq)]
pub enum AcpError {
    Io(String),
    Rpc(ErrorObject),
    Decode(String),
    Timeout,
    Closed,
}

impl std::fmt::Display for AcpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AcpError::Io(e) => write!(f, "I/O error: {e}"),
            AcpError::Rpc(e) => write!(f, "agent returned error {}: {}", e.code, e.message),
            AcpError::Decode(e) => write!(f, "could not decode agent response: {e}"),
            AcpError::Timeout => f.write_str("timed out waiting for the agent"),
            AcpError::Closed => f.write_str("the agent connection is closed"),
        }
    }
}

impl std::error::Error for AcpError {}

impl AcpError {
    /// ACP's "authentication required" error.
    pub fn is_auth_required(&self) -> bool {
        matches!(self, AcpError::Rpc(e) if e.code == crate::protocol::AUTH_REQUIRED)
    }
}

type Pending = Mutex<HashMap<i64, mpsc::Sender<Result<Value, AcpError>>>>;

struct Inner {
    writer: Mutex<Box<dyn Write + Send>>,
    pending: Pending,
    next_id: AtomicI64,
    closed: AtomicBool,
    child: Mutex<Option<Child>>,
}

/// A connection to one ACP agent. Cheap to clone; all clones share it.
#[derive(Clone)]
pub struct AcpClient {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for AcpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AcpClient")
            .field("closed", &self.inner.closed.load(Ordering::Relaxed))
            .finish_non_exhaustive()
    }
}

fn io_err(e: impl std::fmt::Display) -> AcpError {
    AcpError::Io(e.to_string())
}

/// `npx` is `npx.cmd` on Windows, which `Command` does not resolve by itself.
fn program(command: &str) -> String {
    if cfg!(windows) && matches!(command, "npx" | "npm") {
        format!("{command}.cmd")
    } else {
        command.to_owned()
    }
}

impl AcpClient {
    /// Launch `agent` with `cwd` as its working directory.
    pub fn spawn(agent: &AgentDescriptor, cwd: &Path, events: EventSink) -> io::Result<Self> {
        let mut cmd = Command::new(program(&agent.command));
        cmd.args(&agent.args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in &agent.env_remove {
            cmd.env_remove(var);
        }
        for (k, v) in &agent.env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = child.stdout.take().expect("piped");
        let stderr = child.stderr.take().expect("piped");
        let sink = events.clone();
        thread::Builder::new()
            .name("acp-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    sink(ClientEvent::Stderr(line));
                }
            })?;
        let client = Self::connect(stdout, stdin, events);
        *client.inner.child.lock().expect("lock") = Some(child);
        Ok(client)
    }

    /// Speak ACP over an existing pair of streams (the agent's stdout and stdin).
    pub fn connect(
        from_agent: impl Read + Send + 'static,
        to_agent: impl Write + Send + 'static,
        events: EventSink,
    ) -> Self {
        let inner = Arc::new(Inner {
            writer: Mutex::new(Box::new(to_agent)),
            pending: Mutex::default(),
            next_id: AtomicI64::new(1),
            closed: AtomicBool::new(false),
            child: Mutex::new(None),
        });
        let reader_inner = inner.clone();
        thread::Builder::new()
            .name("acp-reader".into())
            .spawn(move || read_loop(reader_inner, from_agent, events))
            .expect("spawn acp reader thread");
        Self { inner }
    }

    fn send(&self, msg: &impl Serialize) -> Result<(), AcpError> {
        if self.inner.closed.load(Ordering::Acquire) {
            return Err(AcpError::Closed);
        }
        let mut line = serde_json::to_vec(msg).map_err(io_err)?;
        line.push(b'\n');
        let mut w = self.inner.writer.lock().map_err(io_err)?;
        w.write_all(&line).and_then(|()| w.flush()).map_err(io_err)
    }

    /// Send a request and wait for its result (`timeout: None` waits forever).
    pub fn call(
        &self,
        method: &str,
        params: Value,
        timeout: Option<Duration>,
    ) -> Result<Value, AcpError> {
        let id = self.inner.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = mpsc::channel();
        self.inner.pending.lock().map_err(io_err)?.insert(id, tx);
        if let Err(e) = self.send(&Request::new(id, method, Some(params))) {
            self.inner.pending.lock().map_err(io_err)?.remove(&id);
            return Err(e);
        }
        let got = match timeout {
            Some(t) => rx.recv_timeout(t).map_err(|e| match e {
                mpsc::RecvTimeoutError::Timeout => AcpError::Timeout,
                mpsc::RecvTimeoutError::Disconnected => AcpError::Closed,
            }),
            None => rx.recv().map_err(|_| AcpError::Closed),
        };
        if got.is_err() {
            self.inner.pending.lock().map_err(io_err)?.remove(&id);
        }
        got?
    }

    fn typed<T: DeserializeOwned>(
        &self,
        method: &str,
        params: impl Serialize,
        timeout: Option<Duration>,
    ) -> Result<T, AcpError> {
        let params = serde_json::to_value(params).map_err(io_err)?;
        let v = self.call(method, params, timeout)?;
        serde_json::from_value(v).map_err(|e| AcpError::Decode(e.to_string()))
    }

    /// `initialize` with no fs or terminal capabilities: the agent works on
    /// disk itself and reaches the IDE only through MCP.
    pub fn initialize(
        &self,
        client_info: Implementation,
        timeout: Option<Duration>,
    ) -> Result<InitializeResponse, AcpError> {
        self.typed(
            methods::INITIALIZE,
            InitializeRequest {
                protocol_version: PROTOCOL_VERSION,
                client_capabilities: ClientCapabilities {
                    auth: crate::protocol::AuthCapabilities { terminal: true },
                    ..ClientCapabilities::default()
                },
                client_info: Some(client_info),
            },
            timeout,
        )
    }

    pub fn new_session(
        &self,
        cwd: &Path,
        mcp_servers: Vec<McpServer>,
        timeout: Option<Duration>,
    ) -> Result<NewSessionResponse, AcpError> {
        self.new_session_with_meta(cwd, mcp_servers, None, timeout)
    }

    /// `session/new` with `_meta` (brief 0058: the remembered model and effort in `claudeCode.options`).
    pub fn new_session_with_meta(
        &self,
        cwd: &Path,
        mcp_servers: Vec<McpServer>,
        meta: Option<Value>,
        timeout: Option<Duration>,
    ) -> Result<NewSessionResponse, AcpError> {
        self.typed(
            methods::SESSION_NEW,
            NewSessionRequest {
                cwd: cwd.to_string_lossy().into_owned(),
                mcp_servers,
                meta,
            },
            timeout,
        )
    }

    /// `session/load` (brief 0061): resume `session_id`. The agent replays the conversation as `session/update`
    /// notifications before it answers; an agent without `agentCapabilities.loadSession` refuses it.
    pub fn load_session(
        &self,
        session_id: &str,
        cwd: &Path,
        mcp_servers: Vec<McpServer>,
        timeout: Option<Duration>,
    ) -> Result<LoadSessionResponse, AcpError> {
        let params = serde_json::to_value(LoadSessionRequest {
            session_id: session_id.to_owned(),
            cwd: cwd.to_string_lossy().into_owned(),
            mcp_servers,
        })
        .map_err(io_err)?;
        // ACP's answer may be `null` (an agent with nothing to report).
        match self.call(methods::SESSION_LOAD, params, timeout)? {
            Value::Null => Ok(LoadSessionResponse::default()),
            v => serde_json::from_value(v).map_err(|e| AcpError::Decode(e.to_string())),
        }
    }

    /// `session/set_mode`: the agent answers once the mode is set (and notifies `current_mode_update`).
    pub fn set_mode(
        &self,
        session_id: &str,
        mode_id: &str,
        timeout: Option<Duration>,
    ) -> Result<(), AcpError> {
        let params = serde_json::to_value(SetSessionModeRequest {
            session_id: session_id.to_owned(),
            mode_id: mode_id.to_owned(),
        })
        .map_err(io_err)?;
        self.call(methods::SESSION_SET_MODE, params, timeout)
            .map(|_| ())
    }

    /// `session/set_config_option` with a select option's value: the answer carries every option.
    pub fn set_config_option(
        &self,
        session_id: &str,
        config_id: &str,
        value: &str,
        timeout: Option<Duration>,
    ) -> Result<SetSessionConfigOptionResponse, AcpError> {
        self.typed(
            methods::SESSION_SET_CONFIG_OPTION,
            SetSessionConfigOptionRequest {
                session_id: session_id.to_owned(),
                config_id: config_id.to_owned(),
                value: value.to_owned(),
            },
            timeout,
        )
    }

    /// Send a user prompt and block until the turn ends. Updates and
    /// permission requests arrive as events meanwhile.
    pub fn prompt(&self, session_id: &str, text: &str) -> Result<PromptResponse, AcpError> {
        self.typed(
            methods::SESSION_PROMPT,
            PromptRequest {
                session_id: session_id.to_owned(),
                prompt: vec![ContentBlock::text(text)],
            },
            None,
        )
    }

    /// `session/cancel`: the agent stops the turn and answers the pending
    /// prompt with `stopReason: cancelled`.
    pub fn cancel(&self, session_id: &str) -> Result<(), AcpError> {
        let params = serde_json::to_value(CancelNotification {
            session_id: session_id.to_owned(),
        })
        .map_err(io_err)?;
        self.send(&Notification::new(methods::SESSION_CANCEL, Some(params)))
    }

    /// Answer a [`ClientEvent::PermissionRequest`].
    pub fn respond_permission(
        &self,
        id: Id,
        outcome: RequestPermissionOutcome,
    ) -> Result<(), AcpError> {
        let result = serde_json::to_value(RequestPermissionResponse { outcome }).map_err(io_err)?;
        self.send(&Response::ok(id, result))
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// Kill the agent process (if this client spawned one).
    pub fn shutdown(&self) {
        if let Ok(mut child) = self.inner.child.lock()
            && let Some(mut c) = child.take()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn read_loop(inner: Arc<Inner>, from_agent: impl Read, events: EventSink) {
    let client = AcpClient {
        inner: inner.clone(),
    };
    let mut reader = BufReader::new(from_agent);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let msg = match serde_json::from_str::<Message>(text) {
            Ok(m) => m,
            Err(_) => {
                events(ClientEvent::ProtocolError(text.to_owned()));
                continue;
            }
        };
        match msg {
            Message::Response(r) => {
                let Some(Id::Number(id)) = r.id else { continue };
                let tx = inner.pending.lock().ok().and_then(|mut p| p.remove(&id));
                if let Some(tx) = tx {
                    let _ = tx.send(match r.payload {
                        crate::jsonrpc::ResponsePayload::Result(v) => Ok(v),
                        crate::jsonrpc::ResponsePayload::Error(e) => Err(AcpError::Rpc(e)),
                    });
                }
            }
            Message::Notification(n) if n.method == methods::SESSION_UPDATE => {
                match n.params.map(serde_json::from_value::<SessionNotification>) {
                    Some(Ok(u)) => events(ClientEvent::Update(u)),
                    other => events(ClientEvent::ProtocolError(format!(
                        "bad session/update: {other:?}"
                    ))),
                }
            }
            Message::Notification(n) => events(ClientEvent::OtherNotification {
                method: n.method,
                params: n.params,
            }),
            Message::Request(r) if r.method == methods::SESSION_REQUEST_PERMISSION => {
                match r
                    .params
                    .map(serde_json::from_value::<RequestPermissionRequest>)
                {
                    Some(Ok(request)) => {
                        events(ClientEvent::PermissionRequest { id: r.id, request })
                    }
                    other => {
                        let _ = client.send(&Response::err(
                            Some(r.id),
                            ErrorObject::new(ErrorObject::INVALID_PARAMS, format!("{other:?}")),
                        ));
                    }
                }
            }
            Message::Request(r) => {
                // fs/* and terminal/* are not advertised; anything else is unknown.
                let _ = client.send(&Response::err(
                    Some(r.id),
                    ErrorObject::new(
                        ErrorObject::METHOD_NOT_FOUND,
                        format!("client does not implement {}", r.method),
                    ),
                ));
            }
        }
    }
    inner.closed.store(true, Ordering::Release);
    if let Ok(mut pending) = inner.pending.lock() {
        for (_, tx) in pending.drain() {
            let _ = tx.send(Err(AcpError::Closed));
        }
    }
    events(ClientEvent::Closed);
}
