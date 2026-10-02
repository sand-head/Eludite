//! The agent session, entirely off the UI thread (CLAUDE.md invariant 1).
//!
//! One driver thread spawns the agent and runs `initialize`, `session/new` and
//! each `session/prompt` (blocking calls). `niello-acp`'s reader thread
//! delivers updates; the permission policy runs there too, so a read-class
//! Niello tool is approved without a UI round trip. Everything the UI needs
//! arrives as a [`Tagged`] event on an `async_channel`, stamped with the
//! session generation so the panel can drop events from a session it already
//! replaced (invariant 12).

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use niello_acp::fake_agent::SENT_AT_META;
use niello_acp::protocol::{
    AuthMethod, EnvVariable, Implementation, McpServer as AcpMcpServer, RequestPermissionOutcome,
    RequestPermissionRequest, SessionUpdate, StopReason, ToolCall,
};
use niello_acp::{AcpClient, AcpError, AgentDescriptor, ClientEvent, Id};
use niello_commands::diagnostics::{self, DIAGNOSTICS_LIST};
use niello_commands::{CommandId, CommandRegistry, PermissionClass};
use niello_mcp::transport::{TOKEN_ENV, listen_local};
use niello_mcp::{McpServer, command_id_from_tool_name};

/// The MCP server name Niello registers with the agent. Claude prefixes tool
/// names with it: `mcp__niello__diagnostics-list`.
pub const MCP_SERVER_NAME: &str = "niello";

const INIT_TIMEOUT: Option<Duration> = Some(Duration::from_secs(120));

#[derive(Debug, Clone, PartialEq)]
pub enum AgentStatus {
    NotStarted,
    Starting,
    Ready {
        agent: String,
        version: String,
        protocol: u16,
        mcp: String,
    },
    LoginRequired {
        label: String,
        /// (method name, description, command to run in a terminal)
        methods: Vec<(String, String, String)>,
    },
    Failed(String),
    Exited,
}

#[derive(Debug, Clone)]
pub enum PanelEvent {
    Status(AgentStatus),
    /// Milestones for the ready-time budget, ms since the session started.
    Timing {
        name: &'static str,
        ms: f64,
    },
    /// A `session/update`, with the fake agent's send stamp if present.
    Update {
        update: SessionUpdate,
        sent_at_ns: Option<u128>,
    },
    /// A permission request the user must answer.
    PermissionPrompt {
        key: u64,
        request: RequestPermissionRequest,
    },
    /// A permission request the policy answered (class read).
    AutoAllowed {
        key: u64,
        request: RequestPermissionRequest,
        command: String,
    },
    /// One `tools/call` served by Niello's MCP endpoint (the audit line).
    McpCall {
        command: String,
        permission: String,
        arguments: String,
        thread: String,
        ok: bool,
        ms: f64,
    },
    TurnEnded(Result<StopReason, String>),
    Stderr(String),
}

#[derive(Debug, Clone)]
pub struct Tagged {
    pub generation: u64,
    pub event: PanelEvent,
}

pub type Sink = async_channel::Sender<Tagged>;

/// What a session needs: which agent, where, and how the agent reaches Niello.
#[derive(Clone)]
pub struct SessionConfig {
    pub agent: AgentDescriptor,
    pub cwd: PathBuf,
    /// Executable launched by the agent as Niello's stdio MCP server, with
    /// `--mcp-relay ADDR` (this spike's own binary).
    pub relay_exe: PathBuf,
    pub registry: Arc<CommandRegistry>,
}

impl std::fmt::Debug for SessionConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SessionConfig")
            .field("agent", &self.agent)
            .field("cwd", &self.cwd)
            .finish_non_exhaustive()
    }
}

/// The registry the spike serves: built-ins plus `diagnostics.list` over the fixture.
pub fn spike_registry() -> Arc<CommandRegistry> {
    let mut r = niello_commands::builtins::default_registry();
    diagnostics::register(&mut r, Arc::new(diagnostics::fixture))
        .expect("register diagnostics.list");
    Arc::new(r)
}

/// The commands exposed to agents in this spike: exactly one.
pub fn exposed_commands() -> Vec<CommandId> {
    vec![CommandId::new(DIAGNOSTICS_LIST).expect("valid id")]
}

/// Policy (PLAN.md 5.3, brief 0005): a permission request for one of
/// Niello's own MCP tools whose command is class read is allowed without a
/// prompt. Returns that command id. Everything else must be asked.
pub fn auto_allow(tool_call: &ToolCall, registry: &CommandRegistry) -> Option<CommandId> {
    let name = tool_call.agent_tool_name().or(tool_call.title.as_deref())?;
    let tool = name
        .strip_prefix("mcp__")?
        .strip_prefix(MCP_SERVER_NAME)?
        .strip_prefix("__")?;
    // If the adapter names the server, it must be ours.
    if let Some(server) = tool_call
        .meta
        .as_ref()
        .and_then(|m| m.pointer("/claudeCode/mcpServer/name"))
        .and_then(|v| v.as_str())
        && server != MCP_SERVER_NAME
    {
        return None;
    }
    let id = command_id_from_tool_name(tool)?;
    if !exposed_commands().contains(&id) {
        return None;
    }
    (registry.lookup(id.as_str())?.permission == PermissionClass::Read).then_some(id)
}

enum Command {
    Prompt(String),
}

/// A running agent session. Dropping it does not stop the agent; call
/// [`AgentSession::shutdown`].
pub struct AgentSession {
    generation: u64,
    commands: mpsc::Sender<Command>,
    client: Arc<OnceLock<AcpClient>>,
    session_id: Arc<OnceLock<String>>,
    pending: Arc<Mutex<HashMap<u64, (Id, RequestPermissionRequest)>>>,
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
    /// Start the MCP endpoint and the agent on background threads.
    pub fn start(config: SessionConfig, generation: u64, sink: Sink) -> Self {
        let (tx, rx) = mpsc::channel();
        let session = Self {
            generation,
            commands: tx,
            client: Arc::default(),
            session_id: Arc::default(),
            pending: Arc::default(),
        };
        let client_cell = session.client.clone();
        let sid_cell = session.session_id.clone();
        let pending = session.pending.clone();
        thread::Builder::new()
            .name("acp-driver".into())
            .spawn(move || drive(config, generation, sink, rx, client_cell, sid_cell, pending))
            .expect("spawn driver");
        session
    }

    pub fn generation(&self) -> u64 {
        self.generation
    }

    /// Queue a prompt; it is sent once the session is ready.
    pub fn prompt(&self, text: &str) {
        let _ = self.commands.send(Command::Prompt(text.to_owned()));
    }

    /// Answer a permission prompt. The write happens on a background thread.
    pub fn answer(&self, key: u64, allow: bool) -> Option<String> {
        let (id, req) = self.pending.lock().ok()?.remove(&key)?;
        let option = req.option(allow)?.clone();
        let client = self.client.get()?.clone();
        let option_id = option.option_id.clone();
        thread::spawn(move || {
            let _ = client.respond_permission(id, RequestPermissionOutcome::Selected { option_id });
        });
        Some(option.name)
    }

    /// `session/cancel`, and answer every pending prompt with `cancelled`.
    pub fn cancel(&self) {
        let (Some(client), Some(sid)) =
            (self.client.get().cloned(), self.session_id.get().cloned())
        else {
            return;
        };
        let pending: Vec<_> = self
            .pending
            .lock()
            .map(|mut p| p.drain().map(|(_, v)| v.0).collect())
            .unwrap_or_default();
        thread::spawn(move || {
            let _ = client.cancel(&sid);
            for id in pending {
                let _ = client.respond_permission(id, RequestPermissionOutcome::Cancelled);
            }
        });
    }

    pub fn shutdown(&self) {
        if let Some(c) = self.client.get().cloned() {
            thread::spawn(move || c.shutdown());
        }
    }
}

fn ms(t: Instant) -> f64 {
    t.elapsed().as_secs_f64() * 1000.
}

#[allow(clippy::too_many_arguments)]
fn drive(
    config: SessionConfig,
    generation: u64,
    sink: Sink,
    commands: mpsc::Receiver<Command>,
    client_cell: Arc<OnceLock<AcpClient>>,
    sid_cell: Arc<OnceLock<String>>,
    pending: Arc<Mutex<HashMap<u64, (Id, RequestPermissionRequest)>>>,
) {
    let t0 = Instant::now();
    let emit = {
        let sink = sink.clone();
        move |event: PanelEvent| {
            let _ = sink.send_blocking(Tagged { generation, event });
        }
    };
    emit(PanelEvent::Status(AgentStatus::Starting));

    // Niello's MCP endpoint: the command bus, read-only subset.
    let mcp_sink = sink.clone();
    let server = McpServer::new(config.registry.clone(), exposed_commands()).with_observer(
        Arc::new(move |r| {
            // The spike's audit record is a log line (brief 0005, out of scope: storage).
            eprintln!(
                "[audit] mcp tools/call {} -> {:?} class={:?} args={} ok={} {:.2} ms",
                r.tool,
                r.command.as_ref().map(|c| c.as_str()),
                r.permission,
                r.arguments,
                r.outcome.is_ok(),
                r.elapsed.as_secs_f64() * 1000.
            );
            let _ = mcp_sink.send_blocking(Tagged {
                generation,
                event: PanelEvent::McpCall {
                    command: r
                        .command
                        .as_ref()
                        .map(|c| c.to_string())
                        .unwrap_or_default(),
                    permission: format!("{:?}", r.permission.unwrap_or(PermissionClass::Read))
                        .to_lowercase(),
                    arguments: r.arguments.to_string(),
                    thread: thread::current().name().unwrap_or("?").to_owned(),
                    ok: r.outcome.is_ok(),
                    ms: r.elapsed.as_secs_f64() * 1000.,
                },
            });
        }),
    );
    let endpoint = match listen_local(Arc::new(server)) {
        Ok(e) => e,
        Err(e) => {
            return emit(PanelEvent::Status(AgentStatus::Failed(format!(
                "MCP endpoint: {e}"
            ))));
        }
    };
    let mcp_servers = vec![AcpMcpServer::Stdio {
        name: MCP_SERVER_NAME.into(),
        command: config.relay_exe.to_string_lossy().into_owned(),
        args: vec!["--mcp-relay".into(), endpoint.addr.to_string()],
        env: vec![EnvVariable {
            name: TOKEN_ENV.into(),
            value: endpoint.token.clone(),
        }],
    }];

    // Reader-thread callback: policy for permission requests, forward the rest.
    let auth_label: Arc<Mutex<String>> = Arc::default();
    let registry = config.registry.clone();
    let cb_sink = sink.clone();
    let cb_client = client_cell.clone();
    let cb_pending = pending.clone();
    let cb_auth = auth_label.clone();
    let events: niello_acp::EventSink = Arc::new(move |ev| {
        let send = |event| {
            let _ = cb_sink.send_blocking(Tagged { generation, event });
        };
        match ev {
            ClientEvent::Update(n) => {
                let sent_at_ns = match &n.update {
                    SessionUpdate::AgentMessageChunk(c) => c
                        .meta()
                        .and_then(|m| m.get(SENT_AT_META))
                        .and_then(|s| s.as_str())
                        .and_then(|s| s.parse().ok()),
                    _ => None,
                };
                send(PanelEvent::Update {
                    update: n.update,
                    sent_at_ns,
                })
            }
            ClientEvent::PermissionRequest { id, request } => {
                let key = NEXT_KEY.fetch_add(1, Ordering::Relaxed);
                match auto_allow(&request.tool_call, &registry) {
                    Some(cmd) => {
                        if let (Some(c), Some(opt)) = (cb_client.get(), request.option(true)) {
                            let _ = c.respond_permission(
                                id,
                                RequestPermissionOutcome::Selected {
                                    option_id: opt.option_id.clone(),
                                },
                            );
                        }
                        send(PanelEvent::AutoAllowed {
                            key,
                            request,
                            command: cmd.to_string(),
                        });
                    }
                    None => {
                        if let Ok(mut p) = cb_pending.lock() {
                            p.insert(key, (id, request.clone()));
                        }
                        send(PanelEvent::PermissionPrompt { key, request });
                    }
                }
            }
            ClientEvent::OtherNotification { method, params }
                if method == "_auth/status_update" =>
            {
                if let Some(label) = params
                    .as_ref()
                    .and_then(|p| p.pointer("/authStatus/label"))
                    .and_then(|v| v.as_str())
                    && let Ok(mut l) = cb_auth.lock()
                {
                    *l = label.to_owned();
                }
            }
            ClientEvent::OtherNotification { .. } => {}
            ClientEvent::Stderr(line) => send(PanelEvent::Stderr(line)),
            ClientEvent::ProtocolError(line) => {
                send(PanelEvent::Stderr(format!("[non-protocol stdout] {line}")))
            }
            ClientEvent::Closed => send(PanelEvent::Status(AgentStatus::Exited)),
        }
    });

    let client = match AcpClient::spawn(&config.agent, &config.cwd, events) {
        Ok(c) => c,
        Err(e) => {
            return emit(PanelEvent::Status(AgentStatus::Failed(format!(
                "could not start `{}`: {e}",
                config.agent.command
            ))));
        }
    };
    let _ = client_cell.set(client.clone());
    emit(PanelEvent::Timing {
        name: "spawned",
        ms: ms(t0),
    });

    let info = Implementation {
        name: "niello-spike-0005".into(),
        title: Some("Niello".into()),
        version: "0.0.0".into(),
    };
    let init = match client.initialize(info, INIT_TIMEOUT) {
        Ok(i) => i,
        Err(e) => {
            return emit(PanelEvent::Status(AgentStatus::Failed(format!(
                "initialize: {e}"
            ))));
        }
    };
    emit(PanelEvent::Timing {
        name: "initialized",
        ms: ms(t0),
    });
    let login_status = |label: String, methods: &[AuthMethod]| AgentStatus::LoginRequired {
        label,
        methods: methods
            .iter()
            .filter(|m| m.kind.as_deref() == Some("terminal"))
            .map(|m| {
                (
                    m.name.clone(),
                    m.description.clone().unwrap_or_default(),
                    config.agent.terminal_auth_command(m),
                )
            })
            .collect(),
    };
    let session = match client.new_session(&config.cwd, mcp_servers, INIT_TIMEOUT) {
        Ok(s) => s,
        Err(e) if e.is_auth_required() => {
            return emit(PanelEvent::Status(login_status(
                "Authentication required".into(),
                &init.auth_methods,
            )));
        }
        Err(e) => {
            return emit(PanelEvent::Status(AgentStatus::Failed(format!(
                "session/new: {e}"
            ))));
        }
    };
    let _ = sid_cell.set(session.session_id.clone());
    emit(PanelEvent::Timing {
        name: "session_ready",
        ms: ms(t0),
    });
    let agent = init.agent_info.clone();
    emit(PanelEvent::Status(AgentStatus::Ready {
        agent: agent
            .as_ref()
            .map(|a| a.title.clone().unwrap_or_else(|| a.name.clone()))
            .unwrap_or_else(|| config.agent.name.clone()),
        version: agent
            .map(|a| format!("{} {}", a.name, a.version))
            .unwrap_or_default(),
        protocol: init.protocol_version,
        mcp: format!("{MCP_SERVER_NAME} via stdio relay to {}", endpoint.addr),
    }));

    while let Ok(Command::Prompt(text)) = commands.recv() {
        let result = client.prompt(&session.session_id, &text);
        if let Err(e) = &result
            && e.is_auth_required()
        {
            let label = auth_label.lock().map(|l| l.clone()).unwrap_or_default();
            emit(PanelEvent::Status(login_status(
                if label.is_empty() {
                    "Not logged in".into()
                } else {
                    label
                },
                &init.auth_methods,
            )));
        }
        emit(PanelEvent::TurnEnded(
            result
                .map(|r| r.stop_reason)
                .map_err(|e: AcpError| e.to_string()),
        ));
    }
}
