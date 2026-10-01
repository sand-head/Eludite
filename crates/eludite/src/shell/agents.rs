//! The Agents window (brief 0016): hosting ACP agents in the shell, Eludite's MCP endpoint inside this process, the
//! permission policy and the agents' edits held as pending changes.
//!
//! - **Registry.** The native Claude Code adapter when found (beside the executable, `ELUDITE_CLAUDE_ACP`, `PATH`),
//!   the npx adapter as the fallback, then the agents the user adds in `agents.json` (`eludite_acp::settings`). The
//!   search runs off the UI thread at startup.
//! - **Sessions.** `eludite_acp::AgentSession` runs each agent on its own threads; its events reach the UI through
//!   one channel, are applied in batches (a burst of streamed chunks costs one frame), and carry the session's
//!   generation so a restarted session never shows the old one's events (CLAUDE.md invariant 12).
//! - **MCP.** The endpoint starts with the first session ([`endpoint`]).

pub mod endpoint;
pub mod transcript;
pub mod window;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_acp::protocol::{Implementation, PermissionOptionKind, RequestPermissionRequest};
use eludite_acp::session::StreamConnector;
use eludite_acp::{
    AdapterSearch, AgentSession, AgentSettings, AgentState, LoginMethod, PermissionPolicy,
    PolicyAnswer, RegisteredAgent, SessionConfig, SessionEvent,
};
use eludite_commands::{CommandRegistry, PermissionClass};
use eludite_mcp::{GateDecision, ToolCallRecord, command_id_from_tool_name};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AppContext as _, Context, Entity, Window};

use self::endpoint::{EndpointHooks, MCP_SERVER_NAME, McpEndpoint};
use self::transcript::{McpLink, Permission};
use self::window::{AgentsWindow, AgentsWindowEvent, Decision, HeaderState, StateKind};
use super::Shell;

/// Status bar slot: the agent's state (right).
pub const AGENTS_SLOT: &str = "agents";

/// How long `initialize` and `session/new` may take (the first `npx` run downloads the adapter).
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(120);

/// Where agents come from and how they reach Eludite's MCP endpoint.
#[derive(Clone)]
pub struct AgentsSetup {
    /// The registry, when known up front (tests); else it is searched off the UI thread at startup.
    pub registry: Option<Vec<RegisteredAgent>>,
    /// The executable the agent launches as its stdio MCP server, with `--mcp-relay ADDR`: this one.
    pub relay_exe: PathBuf,
    /// Connect to agents in-process instead of spawning them (tests).
    pub connect: Option<StreamConnector>,
}

impl AgentsSetup {
    pub fn from_env() -> Self {
        Self {
            registry: None,
            relay_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("eludite")),
            connect: None,
        }
    }
}

/// The registry from the environment and the user's `agents.json` (with the settings error, if any).
fn search_registry() -> (Vec<RegisteredAgent>, Option<String>) {
    let path =
        eludite_docking::eludite_config_dir().map(|d| d.join(eludite_acp::settings::SETTINGS_FILE));
    let (settings, error) = match path.as_deref().map(AgentSettings::load) {
        Some(Ok(s)) => (s, None),
        Some(Err(e)) => (AgentSettings::default(), Some(e)),
        None => (AgentSettings::default(), None),
    };
    (
        eludite_acp::settings::registry(&AdapterSearch::from_env(), &settings),
        error,
    )
}

/// What reaches the UI thread from the agents' and the endpoint's threads.
pub enum HostMsg {
    Session(u64, Box<SessionEvent>),
    Mcp(Box<ToolCallRecord>),
    Registry(Vec<RegisteredAgent>, Option<String>),
}

pub struct Agents {
    pub window: Entity<AgentsWindow>,
    setup: AgentsSetup,
    pub registry: Vec<RegisteredAgent>,
    pub selected: usize,
    session: Option<AgentSession>,
    pub generation: u64,
    pub state: StateKind,
    detail: String,
    login: Vec<LoginMethod>,
    pub session_id: Option<String>,
    pub agent_info: Option<String>,
    pub protocol: Option<u16>,
    pub last_stop: Option<String>,
    endpoint: Option<McpEndpoint>,
    tx: UnboundedSender<HostMsg>,
    /// The running agent's name, for the endpoint's audit records.
    current: Arc<Mutex<String>>,
    /// When the current session was started, and its milestones (ms).
    pub started: Option<Instant>,
    pub timings: Vec<(&'static str, f64)>,
    /// When the session became ready, since `started`.
    pub ready_ms: Option<f64>,
}

impl Agents {
    /// The window and the channel; `rx` goes to the shell's pump.
    pub fn new(
        setup: AgentsSetup,
        theme: eludite_ui::Theme,
        cx: &mut Context<Shell>,
    ) -> (Self, UnboundedReceiver<HostMsg>) {
        let (tx, rx) = unbounded();
        let window = cx.new(|cx| AgentsWindow::new(theme, cx));
        let registry = setup.registry.clone().unwrap_or_default();
        window.update(cx, |w, _| {
            w.header.agents = registry.iter().map(|a| a.name().to_owned()).collect()
        });
        if setup.registry.is_none() {
            let tx = tx.clone();
            // PATH and the settings file are read off the UI thread.
            cx.background_spawn(async move {
                let (r, e) = search_registry();
                let _ = tx.unbounded_send(HostMsg::Registry(r, e));
            })
            .detach();
        }
        (
            Self {
                window,
                setup,
                registry,
                selected: 0,
                session: None,
                generation: 0,
                state: StateKind::Stopped,
                detail: String::new(),
                login: Vec::new(),
                session_id: None,
                agent_info: None,
                protocol: None,
                last_stop: None,
                endpoint: None,
                tx,
                current: Arc::default(),
                started: None,
                timings: Vec::new(),
                ready_ms: None,
            },
            rx,
        )
    }

    pub fn selected_agent(&self) -> Option<&RegisteredAgent> {
        self.registry.get(self.selected)
    }

    #[allow(dead_code)]
    pub fn session(&self) -> Option<&AgentSession> {
        self.session.as_ref()
    }

    fn header(&self) -> HeaderState {
        HeaderState {
            agents: self.registry.iter().map(|a| a.name().to_owned()).collect(),
            selected: self.selected,
            state: self.state,
            detail: self.detail.clone(),
            login: self.login.clone(),
        }
    }

    /// The status bar text.
    pub fn status_text(&self) -> String {
        match (self.selected_agent(), self.state) {
            (_, StateKind::Stopped) | (None, _) => String::new(),
            (Some(a), s) => format!("{}: {}", a.name(), s.as_str().replace('_', " ")),
        }
    }
}

/// `mcp__eludite__<tool>` → the command id, when the request is for one of Eludite's own MCP tools (and the adapter,
/// if it names the server, names ours).
pub fn eludite_tool(request: &RequestPermissionRequest) -> Option<eludite_commands::CommandId> {
    let tc = &request.tool_call;
    let name = tc.agent_tool_name().or(tc.title.as_deref())?;
    let tool = name
        .strip_prefix("mcp__")?
        .strip_prefix(MCP_SERVER_NAME)?
        .strip_prefix("__")?;
    if let Some(server) = tc
        .meta
        .as_ref()
        .and_then(|m| m.pointer("/claudeCode/mcpServer/name"))
        .and_then(|v| v.as_str())
        && server != MCP_SERVER_NAME
    {
        return None;
    }
    command_id_from_tool_name(tool)
}

/// The ACP-side policy (runs on the agent's reader thread): Eludite's own tools are allowed here, because the MCP
/// boundary applies the command's class to the call itself (read runs, edits are reviewed, the rest may prompt), so
/// the user is never asked twice; everything else is asked.
fn acp_policy(commands: Arc<CommandRegistry>) -> PermissionPolicy {
    Arc::new(move |request| {
        let Some(id) = eludite_tool(request) else {
            return PolicyAnswer::Ask;
        };
        match commands.lookup(id.as_str()) {
            Some(spec) if spec.agent_visible && spec.permission == PermissionClass::Read => {
                PolicyAnswer::Allow(format!("{id} is class read"))
            }
            Some(spec) if spec.agent_visible => PolicyAnswer::Allow(format!(
                "Eludite checks {id} (class {}) at its MCP boundary",
                spec.permission.as_str()
            )),
            _ => PolicyAnswer::Ask,
        }
    })
}

impl Shell {
    #[allow(dead_code)]
    pub fn agents(&self) -> &Agents {
        &self.agents
    }

    /// Push the header and the status bar slot.
    pub(super) fn sync_agents_header(&mut self, cx: &mut Context<Self>) {
        let header = self.agents.header();
        self.agents
            .window
            .update(cx, |w, cx| w.set_header(header, cx));
        self.status.set(AGENTS_SLOT, self.agents.status_text());
        cx.notify();
    }

    /// Start the selected agent (or `agent`), unless one is running and `restart` is false.
    pub fn agents_start(
        &mut self,
        agent: Option<&str>,
        restart: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if let Some(name) = agent {
            let ix = self
                .agents
                .registry
                .iter()
                .position(|a| a.name() == name)
                .ok_or_else(|| format!("no agent named `{name}`"))?;
            if ix != self.agents.selected {
                self.agents.selected = ix;
                self.stop_session(cx);
            }
        }
        let live = self.agents.session.is_some()
            && !matches!(self.agents.state, StateKind::Error | StateKind::Stopped);
        if live && !restart {
            return Ok(());
        }
        self.stop_session(cx);
        let Some(agent) = self.agents.selected_agent().cloned() else {
            return Err("no agent is configured".into());
        };
        if self.agents.endpoint.is_none() {
            let endpoint = McpEndpoint::start(self.commands.clone(), self.endpoint_hooks())
                .map_err(|e| format!("Eludite's MCP endpoint: {e}"))?;
            self.agents.endpoint = Some(endpoint);
        }
        let endpoint = self.agents.endpoint.as_ref().expect("started above");
        let cwd = self
            .solution_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        self.agents.generation += 1;
        let generation = self.agents.generation;
        *self
            .agents
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = agent.name().to_owned();
        let config = SessionConfig {
            agent: agent.descriptor.clone(),
            connect: self.agents.setup.connect.clone(),
            cwd,
            mcp_servers: vec![endpoint.acp_server(&self.agents.setup.relay_exe)],
            client_info: Implementation {
                name: "eludite".into(),
                title: Some("Eludite".into()),
                version: eludite_commands::builtins::VERSION.into(),
            },
            handshake_timeout: HANDSHAKE_TIMEOUT,
            policy: acp_policy(self.commands.clone()),
        };
        let tx = self.agents.tx.clone();
        let sink = Arc::new(move |g, e| {
            let _ = tx.unbounded_send(HostMsg::Session(g, Box::new(e)));
        });
        self.agents.started = Some(Instant::now());
        self.agents.timings.clear();
        self.agents.ready_ms = None;
        self.agents.state = StateKind::Starting;
        self.agents.detail = format!("Starting {}\u{2026}", agent.command_line());
        self.agents.login.clear();
        self.agents.session = Some(AgentSession::start(config, generation, sink));
        let name = agent.name().to_owned();
        self.agents.window.update(cx, |w, cx| {
            w.transcript.notice(format!("Starting {name}"));
            w.sync(cx);
        });
        self.sync_agents_header(cx);
        Ok(())
    }

    /// Stop the session (the agent process is killed) and forget its state.
    fn stop_session(&mut self, cx: &mut Context<Self>) {
        if let Some(s) = self.agents.session.take() {
            s.cancel();
            s.shutdown();
        }
        self.agents.state = StateKind::Stopped;
        self.agents.session_id = None;
        self.agents.detail.clear();
        self.sync_agents_header(cx);
    }

    /// Send a prompt, starting the agent first when needed.
    pub fn agents_prompt(&mut self, text: &str, cx: &mut Context<Self>) -> Result<(), String> {
        if self.agents.state == StateKind::Running {
            return Err("the agent is still answering; cancel the turn first".into());
        }
        if self.agents.session.is_none()
            || matches!(self.agents.state, StateKind::Error | StateKind::Stopped)
        {
            self.agents_start(None, false, cx)?;
        }
        let session = self.agents.session.as_ref().expect("started");
        session.prompt(text);
        self.agents.state = StateKind::Running;
        let text = text.to_owned();
        self.agents.window.update(cx, |w, cx| {
            w.transcript.user(&text);
            w.sync(cx);
        });
        self.sync_agents_header(cx);
        Ok(())
    }

    /// Cancel the turn: `session/cancel`, and every pending request answered `cancelled`.
    pub fn agents_cancel(&mut self, cx: &mut Context<Self>) {
        let Some(session) = &self.agents.session else {
            return;
        };
        let keys = session.cancel();
        self.agents.window.update(cx, |w, cx| {
            for key in keys {
                w.transcript.answer(
                    key,
                    Permission::Denied {
                        auto: false,
                        reason: "the turn was cancelled".into(),
                    },
                );
            }
            w.sync(cx);
        });
        self.after_permission_change(cx);
    }

    /// Answer permission request `key` of the agent.
    pub fn agents_answer(
        &mut self,
        key: u64,
        decision: Decision,
        cx: &mut Context<Self>,
    ) -> Result<String, String> {
        let session = self.agents.session.as_ref().ok_or("no agent is running")?;
        // Always Allow is Eludite's own rule (the policy file); the agent is told to allow once, so it keeps asking.
        let kind = match decision {
            Decision::Allow | Decision::AlwaysAllow => PermissionOptionKind::AllowOnce,
            Decision::Deny => PermissionOptionKind::RejectOnce,
        };
        let option = session
            .answer(key, kind)
            .ok_or_else(|| format!("no pending permission request {key}"))?;
        let allowed = decision != Decision::Deny;
        self.agents.window.update(cx, |w, cx| {
            w.transcript.answer(
                key,
                if allowed {
                    Permission::Allowed {
                        auto: false,
                        reason: option.name.clone(),
                    }
                } else {
                    Permission::Denied {
                        auto: false,
                        reason: option.name.clone(),
                    }
                },
            );
            w.sync(cx);
        });
        self.after_permission_change(cx);
        Ok(option.name)
    }

    /// Show the oldest pending request in the window's prompt.
    pub(super) fn after_permission_change(&mut self, cx: &mut Context<Self>) {
        let window = self.agents.window.clone();
        window.update(cx, |w, cx| {
            let oldest = w.transcript.asked().first().copied();
            w.prompt = oldest.and_then(|key| {
                let row = w.transcript.tools().find(
                    |t| matches!(t.permission, Some(Permission::Asked { key: k, .. }) if k == key),
                )?;
                let class = match row.permission {
                    Some(Permission::Asked { class, .. }) => class,
                    _ => PermissionClass::Execute,
                };
                Some(window::Prompt {
                    request: key,
                    tool: row.name(),
                    class: class.as_str().to_owned(),
                    detail: row
                        .call
                        .raw_input
                        .as_ref()
                        .map(|v| v.to_string())
                        .unwrap_or_default(),
                    can_persist: false,
                })
            });
            cx.notify();
        });
    }

    fn endpoint_hooks(&self) -> EndpointHooks {
        let current = self.agents.current.clone();
        let tx = self.agents.tx.clone();
        EndpointHooks {
            agent: Arc::new(move || current.lock().map(|c| c.clone()).unwrap_or_default()),
            gate: Arc::new(|_, _, _| GateDecision::Deny("no one is there to allow it".into())),
            invoker: None,
            observer: Some(Arc::new(move |record| {
                let _ = tx.unbounded_send(HostMsg::Mcp(Box::new(record.clone())));
            })),
        }
    }

    /// Window events: the user's actions.
    pub(super) fn on_agents_window_event(
        &mut self,
        _: &Entity<AgentsWindow>,
        event: &AgentsWindowEvent,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let result = match event {
            AgentsWindowEvent::Start { agent, restart } => {
                self.agents_start(agent.as_deref(), *restart, cx)
            }
            AgentsWindowEvent::Prompt(text) => self.agents_prompt(text, cx),
            AgentsWindowEvent::Cancel => {
                self.agents_cancel(cx);
                Ok(())
            }
            AgentsWindowEvent::Answer { request, decision } => {
                self.agents_answer(*request, *decision, cx).map(|_| ())
            }
            AgentsWindowEvent::Review { .. } | AgentsWindowEvent::OpenChange(_) => Ok(()),
        };
        if let Err(e) = result {
            self.status.set(eludite_ui::slots::STATE, e);
            cx.notify();
        }
    }

    /// Apply a batch of agent events (everything queued since the last frame).
    pub(super) fn on_agent_batch(&mut self, batch: Vec<HostMsg>, cx: &mut Context<Self>) {
        let generation = self.agents.generation;
        let mut header = false;
        let mut permissions = false;
        let window = self.agents.window.clone();
        for msg in batch {
            match msg {
                HostMsg::Registry(registry, error) => {
                    self.agents.registry = registry;
                    self.agents.selected = 0;
                    if let Some(e) = error {
                        window.update(cx, |w, _| w.transcript.error(e));
                    }
                    header = true;
                }
                HostMsg::Mcp(record) => {
                    let audit = self
                        .commands
                        .audit_log()
                        .for_call(record.call)
                        .last()
                        .map_or(0, |e| e.seq);
                    let link = McpLink {
                        command: record
                            .command
                            .as_ref()
                            .map(|c| c.to_string())
                            .unwrap_or_default(),
                        class: record.permission.unwrap_or(PermissionClass::Read),
                        ok: record.outcome.is_ok(),
                        ms: record.elapsed.as_secs_f64() * 1e3,
                        audit,
                    };
                    window.update(cx, |w, _| {
                        w.transcript
                            .link_mcp(record.tool_call.as_deref(), &record.tool, link)
                    });
                }
                HostMsg::Session(g, _) if g != generation => {}
                HostMsg::Session(_, event) => match *event {
                    SessionEvent::State(s) => {
                        header = true;
                        self.on_agent_state(s, cx);
                    }
                    SessionEvent::Ready {
                        session_id,
                        agent_info,
                        protocol_version,
                    } => {
                        header = true;
                        self.agents.state = StateKind::Ready;
                        self.agents.ready_ms =
                            self.agents.started.map(|t| t.elapsed().as_secs_f64() * 1e3);
                        let info = agent_info
                            .map(|a| format!("{} {}", a.name, a.version))
                            .unwrap_or_default();
                        self.agents.detail = format!(
                            "{info} (ACP v{protocol_version}). MCP: {}",
                            self.agents
                                .endpoint
                                .as_ref()
                                .map(McpEndpoint::describe)
                                .unwrap_or_default()
                        );
                        self.agents.session_id = Some(session_id);
                        self.agents.agent_info = Some(info);
                        self.agents.protocol = Some(protocol_version);
                    }
                    SessionEvent::Timing { name, ms } => self.agents.timings.push((name, ms)),
                    SessionEvent::Update(u) => window.update(cx, |w, _| w.transcript.apply(&u)),
                    SessionEvent::Permission { key, request } => {
                        permissions = true;
                        let class = self.classify(&request);
                        window.update(cx, |w, _| {
                            w.transcript
                                .permission(&request, Permission::Asked { key, class })
                        });
                    }
                    SessionEvent::PermissionAnswered {
                        request,
                        allowed,
                        reason,
                        ..
                    } => {
                        let state = if allowed {
                            Permission::Allowed { auto: true, reason }
                        } else {
                            Permission::Denied { auto: true, reason }
                        };
                        window.update(cx, |w, _| w.transcript.permission(&request, state));
                    }
                    SessionEvent::TurnEnded(r) => {
                        let text = match &r {
                            Ok(stop) => {
                                let s = serde_json::to_value(stop)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_owned))
                                    .unwrap_or_default();
                                self.agents.last_stop = Some(s.clone());
                                format!("Turn ended: {s}")
                            }
                            Err(e) => {
                                self.agents.last_stop = Some("error".into());
                                format!("The turn failed: {e}")
                            }
                        };
                        window.update(cx, |w, _| match r {
                            Ok(_) => w.transcript.notice(text),
                            Err(_) => w.transcript.error(text),
                        });
                    }
                    SessionEvent::Stderr(line) => {
                        if std::env::var_os("ELUDITE_AGENT_STDERR").is_some() {
                            eprintln!("[agent] {line}");
                        }
                    }
                    SessionEvent::Notification { .. } => {}
                },
            }
        }
        window.update(cx, |w, cx| w.sync(cx));
        if permissions {
            self.after_permission_change(cx);
        }
        if header {
            self.sync_agents_header(cx);
        }
    }

    fn on_agent_state(&mut self, s: AgentState, cx: &mut Context<Self>) {
        let window = self.agents.window.clone();
        match s {
            AgentState::Starting => self.agents.state = StateKind::Starting,
            AgentState::Ready => self.agents.state = StateKind::Ready,
            AgentState::Running => self.agents.state = StateKind::Running,
            AgentState::NeedsLogin { label, methods } => {
                self.agents.state = StateKind::NeedsLogin;
                self.agents.detail = label;
                self.agents.login = methods;
            }
            AgentState::Error(e) => {
                self.agents.state = StateKind::Error;
                self.agents.detail = e.clone();
                window.update(cx, |w, _| w.transcript.error(e));
            }
            AgentState::Exited => {
                if self.agents.state != StateKind::Stopped {
                    self.agents.state = StateKind::Error;
                    self.agents.detail = "The agent exited. Restart it to continue.".into();
                    window.update(cx, |w, _| w.transcript.error("The agent process exited."));
                }
            }
        }
    }

    /// The permission class of an agent's request (PLAN.md 5.3).
    fn classify(&self, request: &RequestPermissionRequest) -> PermissionClass {
        if let Some(id) = eludite_tool(request)
            && let Some(spec) = self.commands.lookup(id.as_str())
        {
            return spec.permission;
        }
        PermissionClass::Execute
    }
}
