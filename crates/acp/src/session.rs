//! One agent session, entirely off the caller's thread (CLAUDE.md invariant 1): the production form of brief 0005's
//! spike driver.
//!
//! - **Lifecycle.** [`AgentSession::start`] spawns the agent and runs `initialize` and `session/new` on the
//!   `acp-driver` thread, then sends each queued prompt (`session/prompt` blocks that thread for the whole turn).
//!   States: [`AgentState::Starting`], `Ready`, `NeedsLogin` (from `_auth/status_update` or an `-32000` answer),
//!   `Running` (a turn), `Error` (spawn, handshake or prompt failure) and `Exited` (the agent's stdout closed).
//! - **Events** reach the owner through one callback, stamped with the session's generation so the owner drops
//!   events of a session it already replaced (invariant 12). They arrive on `acp-reader` (updates, permission
//!   requests), `acp-stderr` and `acp-driver`, never on the owner's thread.
//! - **Permissions.** Each `session/request_permission` goes to the [`PermissionPolicy`] on the reader thread first:
//!   an answer it gives is sent at once ([`SessionEvent::PermissionAnswered`]); otherwise the request waits for
//!   [`AgentSession::answer`] ([`SessionEvent::Permission`]).
//! - **Writes** (permission answers, `session/cancel`, shutdown) go through one `acp-writer` thread, so a full pipe
//!   never blocks the owner. Cancel answers every pending request `cancelled`, as ACP requires.

use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use crate::protocol::{
    AuthMethod, Implementation, McpServer, PermissionOption, PermissionOptionKind,
    RequestPermissionOutcome, RequestPermissionRequest, SessionUpdate, StopReason,
};
use crate::{AcpClient, AcpError, AgentDescriptor, ClientEvent, Id};

/// A login method the user runs in a terminal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LoginMethod {
    pub name: String,
    pub description: String,
    /// The full command line.
    pub command: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AgentState {
    Starting,
    Ready,
    NeedsLogin {
        label: String,
        methods: Vec<LoginMethod>,
    },
    /// A turn is in progress.
    Running,
    Error(String),
    /// The agent's stdout closed.
    Exited,
}

/// What [`AgentSession`] reports.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    State(AgentState),
    /// `initialize` and `session/new` succeeded.
    Ready {
        session_id: String,
        agent_info: Option<Implementation>,
        protocol_version: u16,
    },
    /// Milestones since [`AgentSession::start`] (`spawned`, `initialized`, `session_ready`), in ms.
    Timing {
        name: &'static str,
        ms: f64,
    },
    Update(SessionUpdate),
    /// A permission request the owner must answer with [`AgentSession::answer`].
    Permission {
        key: u64,
        request: RequestPermissionRequest,
    },
    /// A permission request the policy answered on the reader thread.
    PermissionAnswered {
        key: u64,
        request: RequestPermissionRequest,
        allowed: bool,
        reason: String,
    },
    TurnEnded(Result<StopReason, String>),
    Stderr(String),
    /// A notification the client does not interpret (adapter extensions).
    Notification {
        method: String,
        params: Option<Value>,
    },
}

/// Receives every event with the session's generation.
pub type SessionSink = Arc<dyn Fn(u64, SessionEvent) + Send + Sync>;

/// The policy's verdict on a permission request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyAnswer {
    /// Leave it to the owner (a prompt, or a pending change to review).
    Ask,
    Allow(String),
    Deny(String),
}

/// Decides permission requests on the reader thread.
pub type PermissionPolicy = Arc<dyn Fn(&RequestPermissionRequest) -> PolicyAnswer + Send + Sync>;

/// Connects to an agent without spawning its command (in-process agents for tests): the agent's stdout and stdin.
pub type StreamConnector = Arc<
    dyn Fn(
            &AgentDescriptor,
            &std::path::Path,
        ) -> std::io::Result<(
            Box<dyn std::io::Read + Send>,
            Box<dyn std::io::Write + Send>,
        )> + Send
        + Sync,
>;

/// What a session needs.
#[derive(Clone)]
pub struct SessionConfig {
    pub agent: AgentDescriptor,
    /// Instead of spawning `agent.command`.
    pub connect: Option<StreamConnector>,
    pub cwd: PathBuf,
    /// The MCP servers passed in `session/new` (Eludite's relay).
    pub mcp_servers: Vec<McpServer>,
    pub client_info: Implementation,
    /// For `initialize` and `session/new` (the first `npx` run downloads the adapter).
    pub handshake_timeout: Duration,
    pub policy: PermissionPolicy,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("agent", &self.agent)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

enum Write {
    Respond(Id, RequestPermissionOutcome),
    Cancel(String, Vec<Id>),
    Shutdown,
}

type Pending = Arc<Mutex<BTreeMap<u64, (Id, RequestPermissionRequest)>>>;

/// A running agent session. Dropping it stops nothing; call [`AgentSession::shutdown`].
pub struct AgentSession {
    generation: u64,
    prompts: mpsc::Sender<String>,
    writes: mpsc::Sender<Write>,
    session_id: Arc<OnceLock<String>>,
    pending: Pending,
}

impl std::fmt::Debug for AgentSession {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentSession")
            .field("generation", &self.generation)
            .finish_non_exhaustive()
    }
}

static NEXT_KEY: AtomicU64 = AtomicU64::new(1);

impl AgentSession {
    /// Spawn the agent and run the handshake on background threads.
    pub fn start(config: SessionConfig, generation: u64, sink: SessionSink) -> Self {
        let (prompts, prompt_rx) = mpsc::channel();
        let (writes, write_rx) = mpsc::channel();
        let session = Self {
            generation,
            prompts,
            writes,
            session_id: Arc::default(),
            pending: Arc::default(),
        };
        let client: Arc<OnceLock<AcpClient>> = Arc::default();
        {
            let client = client.clone();
            thread::Builder::new()
                .name("acp-writer".into())
                .spawn(move || write_loop(client, write_rx))
                .expect("spawn acp writer thread");
        }
        let sid = session.session_id.clone();
        let pending = session.pending.clone();
        let writes = session.writes.clone();
        thread::Builder::new()
            .name("acp-driver".into())
            .spawn(move || {
                drive(
                    config, generation, sink, prompt_rx, client, sid, pending, writes,
                )
            })
            .expect("spawn acp driver thread");
        session
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    pub fn session_id(&self) -> Option<String> {
        self.session_id.get().cloned()
    }

    /// Queue a prompt; it is sent once the session is ready and the previous turn ended.
    pub fn prompt(&self, text: impl Into<String>) {
        let _ = self.prompts.send(text.into());
    }

    /// The keys of the permission requests waiting for [`AgentSession::answer`], oldest first.
    pub fn pending_permissions(&self) -> Vec<u64> {
        self.pending
            .lock()
            .map(|p| p.keys().copied().collect())
            .unwrap_or_default()
    }

    /// Answer a pending permission request with the agent's option of `kind` (or the nearest one: allow once for
    /// allow always, reject once for reject always). Returns the option chosen, or `None` when `key` is not pending.
    pub fn answer(&self, key: u64, kind: PermissionOptionKind) -> Option<PermissionOption> {
        let (id, request) = self.pending.lock().ok()?.remove(&key)?;
        let option = request
            .options
            .iter()
            .find(|o| o.kind == kind)
            .or_else(|| request.option(kind.is_allow()))
            .cloned();
        let outcome = match &option {
            Some(o) => RequestPermissionOutcome::Selected {
                option_id: o.option_id.clone(),
            },
            None => RequestPermissionOutcome::Cancelled,
        };
        let _ = self.writes.send(Write::Respond(id, outcome));
        option
    }

    /// `session/cancel`, and answer every pending permission request `cancelled`. Returns their keys.
    pub fn cancel(&self) -> Vec<u64> {
        let drained: Vec<(u64, Id)> = self
            .pending
            .lock()
            .map(|mut p| {
                std::mem::take(&mut *p)
                    .into_iter()
                    .map(|(k, (id, _))| (k, id))
                    .collect()
            })
            .unwrap_or_default();
        let keys = drained.iter().map(|(k, _)| *k).collect();
        if let Some(sid) = self.session_id.get() {
            let _ = self.writes.send(Write::Cancel(
                sid.clone(),
                drained.into_iter().map(|(_, id)| id).collect(),
            ));
        }
        keys
    }

    /// Kill the agent process.
    pub fn shutdown(&self) {
        let _ = self.writes.send(Write::Shutdown);
    }
}

fn write_loop(client: Arc<OnceLock<AcpClient>>, rx: mpsc::Receiver<Write>) {
    while let Ok(w) = rx.recv() {
        // A write before the client exists (cancel during spawn) waits for it briefly.
        let deadline = Instant::now() + Duration::from_secs(5);
        let c = loop {
            if let Some(c) = client.get() {
                break Some(c.clone());
            }
            if Instant::now() > deadline || matches!(w, Write::Shutdown) {
                break None;
            }
            thread::sleep(Duration::from_millis(10));
        };
        let Some(c) = c else {
            if matches!(w, Write::Shutdown) {
                return;
            }
            continue;
        };
        match w {
            Write::Respond(id, outcome) => {
                let _ = c.respond_permission(id, outcome);
            }
            Write::Cancel(sid, pending) => {
                let _ = c.cancel(&sid);
                for id in pending {
                    let _ = c.respond_permission(id, RequestPermissionOutcome::Cancelled);
                }
            }
            Write::Shutdown => {
                c.shutdown();
                return;
            }
        }
    }
}

fn answer_now(
    writes: &mpsc::Sender<Write>,
    id: Id,
    request: &RequestPermissionRequest,
    allow: bool,
) {
    let outcome = match request.option(allow) {
        Some(o) => RequestPermissionOutcome::Selected {
            option_id: o.option_id.clone(),
        },
        None => RequestPermissionOutcome::Cancelled,
    };
    let _ = writes.send(Write::Respond(id, outcome));
}

fn login_methods(agent: &AgentDescriptor, methods: &[AuthMethod]) -> Vec<LoginMethod> {
    methods
        .iter()
        .filter(|m| m.kind.as_deref() == Some("terminal"))
        .map(|m| LoginMethod {
            name: m.name.clone(),
            description: m.description.clone().unwrap_or_default(),
            command: agent.terminal_auth_command(m),
        })
        .collect()
}

#[allow(clippy::too_many_arguments)]
fn drive(
    config: SessionConfig,
    generation: u64,
    sink: SessionSink,
    prompts: mpsc::Receiver<String>,
    client_cell: Arc<OnceLock<AcpClient>>,
    sid_cell: Arc<OnceLock<String>>,
    pending: Pending,
    writes: mpsc::Sender<Write>,
) {
    let t0 = Instant::now();
    let ms = move || t0.elapsed().as_secs_f64() * 1000.;
    let emit = {
        let sink = sink.clone();
        move |e: SessionEvent| sink(generation, e)
    };
    emit(SessionEvent::State(AgentState::Starting));

    // Methods from `initialize`, for a login state reported before or after the handshake.
    let auth_methods: Arc<Mutex<Vec<AuthMethod>>> = Arc::default();
    let events: crate::EventSink = {
        let sink = sink.clone();
        let policy = config.policy.clone();
        let agent = config.agent.clone();
        let auth_methods = auth_methods.clone();
        Arc::new(move |ev| {
            let send = |e| sink(generation, e);
            match ev {
                ClientEvent::Update(n) => send(SessionEvent::Update(n.update)),
                ClientEvent::PermissionRequest { id, request } => {
                    let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
                    match policy(&request) {
                        PolicyAnswer::Ask => {
                            if let Ok(mut p) = pending.lock() {
                                p.insert(key, (id, request.clone()));
                            }
                            send(SessionEvent::Permission { key, request });
                        }
                        PolicyAnswer::Allow(reason) => {
                            answer_now(&writes, id, &request, true);
                            send(SessionEvent::PermissionAnswered {
                                key,
                                request,
                                allowed: true,
                                reason,
                            });
                        }
                        PolicyAnswer::Deny(reason) => {
                            answer_now(&writes, id, &request, false);
                            send(SessionEvent::PermissionAnswered {
                                key,
                                request,
                                allowed: false,
                                reason,
                            });
                        }
                    }
                }
                ClientEvent::OtherNotification { method, params } => {
                    // Claude's adapters report the login state in this extension; the account e-mail it may carry
                    // is not kept.
                    if method == "_auth/status_update"
                        && let Some(status) = params.as_ref().and_then(|p| p.get("authStatus"))
                        && status.get("kind").and_then(Value::as_str) == Some("none")
                    {
                        let label = status
                            .get("label")
                            .and_then(Value::as_str)
                            .unwrap_or("Not logged in")
                            .to_owned();
                        let methods = auth_methods.lock().map(|m| m.clone()).unwrap_or_default();
                        send(SessionEvent::State(AgentState::NeedsLogin {
                            label,
                            methods: login_methods(&agent, &methods),
                        }));
                    } else {
                        send(SessionEvent::Notification { method, params })
                    }
                }
                ClientEvent::Stderr(line) => send(SessionEvent::Stderr(line)),
                ClientEvent::ProtocolError(line) => {
                    send(SessionEvent::Stderr(format!("[not ACP] {line}")))
                }
                ClientEvent::Closed => send(SessionEvent::State(AgentState::Exited)),
            }
        })
    };

    let spawned = match &config.connect {
        Some(connect) => connect(&config.agent, &config.cwd)
            .map(|(from_agent, to_agent)| AcpClient::connect(from_agent, to_agent, events)),
        None => AcpClient::spawn(&config.agent, &config.cwd, events),
    };
    let client = match spawned {
        Ok(c) => c,
        Err(e) => {
            return emit(SessionEvent::State(AgentState::Error(format!(
                "could not start `{}`: {e}",
                config.agent.command
            ))));
        }
    };
    let _ = client_cell.set(client.clone());
    emit(SessionEvent::Timing {
        name: "spawned",
        ms: ms(),
    });
    let timeout = Some(config.handshake_timeout);
    let init = match client.initialize(config.client_info.clone(), timeout) {
        Ok(i) => i,
        Err(e) => {
            return emit(SessionEvent::State(AgentState::Error(format!(
                "initialize: {e}"
            ))));
        }
    };
    if let Ok(mut m) = auth_methods.lock() {
        *m = init.auth_methods.clone();
    }
    emit(SessionEvent::Timing {
        name: "initialized",
        ms: ms(),
    });
    let needs_login = |label: String| AgentState::NeedsLogin {
        label,
        methods: login_methods(&config.agent, &init.auth_methods),
    };
    let session = match client.new_session(&config.cwd, config.mcp_servers.clone(), timeout) {
        Ok(s) => s,
        Err(e) if e.is_auth_required() => {
            return emit(SessionEvent::State(needs_login(
                "Authentication required".into(),
            )));
        }
        Err(e) => {
            return emit(SessionEvent::State(AgentState::Error(format!(
                "session/new: {e}"
            ))));
        }
    };
    let _ = sid_cell.set(session.session_id.clone());
    emit(SessionEvent::Timing {
        name: "session_ready",
        ms: ms(),
    });
    emit(SessionEvent::Ready {
        session_id: session.session_id.clone(),
        agent_info: init.agent_info.clone(),
        protocol_version: init.protocol_version,
    });

    while let Ok(text) = prompts.recv() {
        emit(SessionEvent::State(AgentState::Running));
        let result = client.prompt(&session.session_id, &text);
        match &result {
            Err(e) if e.is_auth_required() => {
                emit(SessionEvent::TurnEnded(Err(e.to_string())));
                emit(SessionEvent::State(needs_login("Not logged in".into())));
            }
            Err(AcpError::Closed) => {
                emit(SessionEvent::TurnEnded(Err(AcpError::Closed.to_string())));
                emit(SessionEvent::State(AgentState::Exited));
                return;
            }
            Err(e) => {
                emit(SessionEvent::TurnEnded(Err(e.to_string())));
                emit(SessionEvent::State(AgentState::Ready));
            }
            Ok(r) => {
                emit(SessionEvent::TurnEnded(Ok(r.stop_reason)));
                emit(SessionEvent::State(AgentState::Ready));
            }
        }
    }
}
