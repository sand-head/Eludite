//! The Agents window (brief 0016): hosting ACP agents in the shell, Eludite's MCP endpoint inside this process, the
//! permission policy and the agents' edits held as pending changes.
//!
//! - **Registry.** The native Claude Code adapter when found (beside the executable, the setting
//!   `agents.claudeCodeAdapterPath` or `ELUDITE_CLAUDE_ACP`, `PATH`), the npx adapter as the fallback, then the
//!   agents the user adds in the setting `agents.custom` (brief 0020; the older `agents.json` when no settings file
//!   has that key). The search runs off the UI thread, at startup and again when those settings change.
//! - **Sessions.** `eludite_acp::AgentSession` runs each agent on its own threads; its events reach the UI through
//!   one channel, are applied in batches (a burst of streamed chunks costs one frame), and carry the session's
//!   generation so a restarted session never shows the old one's events (CLAUDE.md invariant 12).
//! - **MCP.** The endpoint starts with the first session ([`endpoint`]).
//! - **Permissions** (PLAN.md 5.3). Every tool call is classed read, edit_buffer, execute or dangerous: an Eludite
//!   tool by its command's class, an agent's own tool by its ACP kind ([`class_of_kind`]). The solution's committable
//!   policy file (`eludite_commands::policy`) decides; what it leaves open is asked in the window's permission
//!   prompt, whose Always Allow writes a rule into that file. Eludite's own tools are judged once, at the MCP
//!   boundary (the gate), never twice; the agent's own tools at its `session/request_permission`.
//! - **Escalation** (brief 0024, ADR-0009). The gate decides on a call's effective class, which a command's
//!   escalation hook may raise from the call's input and the policy (the browser's origin rule); the prompt shows
//!   why, and Always Allow remembers what the hook names (an origin added to `browser.origins`, or nothing). The
//!   registry reads the policy through a source this module sets when an agent starts.
//! - **Thumbnails** (brief 0024). Images in a tool call's result (Eludite's screenshot, or image content the agent
//!   forwards) are decoded and scaled off the UI thread and shown in its transcript row; clicking one saves the image
//!   beside the transcript (or in Eludite's cache) and opens it with `eludite.browser.open_external`.
//! - **Slash commands** (brief 0056). The agent's `available_commands_update` is kept in the transcript model, listed
//!   in the state output's `commands` and offered by the prompt box's slash menu; a new session starts with none.
//! - **Model, effort and mode** (brief 0057). The session's ACP modes and config options are kept here, listed in the
//!   state output's `mode` and `options`, shown by the pickers under the prompt box and named in the status bar.
//!   `eludite.agents.configure` checks the choice, has the session send `session/set_mode` or
//!   `session/set_config_option` on its own thread, and answers once the agent has (from another thread the caller
//!   waits for that answer: [`JobReply`]; from the UI thread it returns at once and the picker shows the pick muted
//!   until then). A pick made in the window is remembered in the settings `agents.model` and `agents.effort` (user
//!   scope), which the next `session/new` passes in `_meta.claudeCode.options`; the mode is not remembered.

pub mod endpoint;
pub mod review;
#[cfg(test)]
pub(in crate::shell) mod scenario;
pub mod transcript;
pub mod window;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use eludite_acp::protocol::{
    Implementation, PermissionOptionKind, RequestPermissionRequest, SessionConfigOption,
    SessionModeState,
};
use eludite_acp::session::StreamConnector;
use eludite_acp::{
    AdapterSearch, AgentSession, AgentSettings, AgentState, LoginMethod, PermissionPolicy,
    PolicyAnswer, RegisteredAgent, SessionConfig, SessionEvent,
};
use eludite_commands::policy::{AgentPolicy, AlwaysAllow, PolicySnapshot, Verdict};
use eludite_commands::{CallClass, CommandRegistry, PermissionClass};
use eludite_mcp::{GateDecision, ToolCallRecord, command_id_from_tool_name, tool_name};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AppContext as _, Context, Entity, Window};
use serde_json::{Value, json};

use self::endpoint::{EndpointHooks, MCP_SERVER_NAME, McpEndpoint};
use self::review::{DiffView, GutterMarkers, PendingChange, ReviewBoard};
use self::transcript::{ImageData, McpLink, Permission, Thumb};
use self::window::{AgentsWindow, AgentsWindowEvent, Decision, HeaderState, StateKind};
use super::Shell;

/// `eludite.agents.*` from another thread (an outer agent), for the UI thread to apply.
pub struct AgentsJob {
    pub request: eludite_commands::agents::AgentsRequest,
    pub reply: JobReply,
}

pub type AgentsOutcome =
    Result<eludite_commands::agents::AgentsOutput, eludite_commands::CommandError>;

/// What the UI thread answers a job: the outcome, or (`eludite.agents.configure`, brief 0057) where the outcome will
/// arrive once the agent has answered.
pub enum JobAnswer {
    Now(Box<AgentsOutcome>),
    Later(mpsc::Receiver<AgentsOutcome>),
}

/// Where the UI thread answers an [`AgentsJob`]. The pump sends the outcome of `Shell::apply_agents` right after it
/// returns, on the UI thread; a configure that waits for the agent left its receiver in [`DEFERRED`], and the caller's
/// thread waits on that instead, so the UI thread never waits for the agent.
pub struct JobReply {
    tx: mpsc::SyncSender<JobAnswer>,
    configure: bool,
}

impl JobReply {
    /// Fails when the caller stopped waiting.
    pub fn send(&self, outcome: AgentsOutcome) -> Result<(), ()> {
        let later = DEFERRED.with(|d| d.borrow_mut().take());
        let answer = match later {
            Some(rx) if self.configure && outcome.is_ok() => JobAnswer::Later(rx),
            _ => JobAnswer::Now(Box::new(outcome)),
        };
        self.tx.send(answer).map_err(|_| ())
    }
}

/// How long a caller of `eludite.agents.configure` waits for the agent (its session gives the agent 30 s).
const CONFIGURE_WAIT: Duration = Duration::from_secs(35);

thread_local! {
    static STAGED: RefCell<Option<AgentsOutcome>> = const { RefCell::new(None) };
    /// The receiver of the configure `Shell::apply_agents` just started (see [`JobReply`]).
    static DEFERRED: RefCell<Option<mpsc::Receiver<AgentsOutcome>>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: AgentsOutcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// The shell's `AgentsTarget`: on the UI thread the shell has applied the request already (keys, buttons, the prompt
/// box); from another thread the request is posted to the UI and the caller waits for its answer.
pub struct AgentsBus {
    pub ui_thread: std::thread::ThreadId,
    pub jobs: UnboundedSender<AgentsJob>,
}

impl eludite_commands::agents::AgentsTarget for AgentsBus {
    fn apply(&self, request: eludite_commands::agents::AgentsRequest) -> AgentsOutcome {
        use eludite_commands::CommandError;
        if std::thread::current().id() == self.ui_thread {
            // On the UI thread a configure returns at once; the window shows the pick until the agent answers.
            DEFERRED.with(|d| d.borrow_mut().take());
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let configure = matches!(
            request,
            eludite_commands::agents::AgentsRequest::Configure { .. }
        );
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(AgentsJob {
                request,
                reply: JobReply { tx, configure },
            })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        match rx
            .recv_timeout(Duration::from_secs(30))
            .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
        {
            JobAnswer::Now(outcome) => *outcome,
            JobAnswer::Later(rx) => rx
                .recv_timeout(CONFIGURE_WAIT)
                .map_err(|_| CommandError::Failed("the agent did not answer".into()))?,
        }
    }
}

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
    /// The agent to select once the registry is known (`--agent`).
    pub preferred: Option<String>,
    /// Write the transcript as JSON here whenever a turn ends (`--transcript-out`).
    pub transcript_out: Option<PathBuf>,
}

impl AgentsSetup {
    pub fn from_env() -> Self {
        Self {
            registry: None,
            relay_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("eludite")),
            connect: None,
            preferred: None,
            transcript_out: None,
        }
    }
}

/// What the agents settings say (brief 0020): the registry is searched again when it changes.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RegistryConfig {
    /// `agents.default` (empty: none).
    pub default: Option<String>,
    /// `agents.claudeCodeAdapterPath`, or `ELUDITE_CLAUDE_ACP` (the store resolves the override).
    pub adapter: Option<PathBuf>,
    /// `agents.custom` when a settings file sets it; `None`: read the older `agents.json`.
    pub custom: Option<Vec<eludite_acp::settings::ConfiguredAgent>>,
}

impl RegistryConfig {
    pub fn from_store(s: &crate::settings::SettingsStore) -> Self {
        let custom = s
            .is_set_in_a_file("agents.custom")
            .then(|| serde_json::from_value(s.effective("agents.custom").0).unwrap_or_default());
        Self {
            default: Some(s.string("agents.default")).filter(|d| !d.is_empty()),
            adapter: s.path("agents.claudeCodeAdapterPath"),
            custom,
        }
    }
}

/// The registry from the settings, the environment and (without `agents.custom`) the user's `agents.json`, with
/// the `agents.json` error, if any.
fn search_registry(config: &RegistryConfig) -> (Vec<RegisteredAgent>, Option<String>) {
    let (mut settings, error) = match &config.custom {
        Some(agents) => (
            AgentSettings {
                default: None,
                agents: agents.clone(),
            },
            None,
        ),
        None => {
            let path = eludite_docking::eludite_config_dir()
                .map(|d| d.join(eludite_acp::settings::SETTINGS_FILE));
            match path.as_deref().map(AgentSettings::load) {
                Some(Ok(s)) => (s, None),
                Some(Err(e)) => (AgentSettings::default(), Some(e)),
                None => (AgentSettings::default(), None),
            }
        }
    };
    if config.default.is_some() {
        settings.default = config.default.clone();
    }
    let mut search = AdapterSearch::from_env();
    search.configured = config.adapter.clone();
    (eludite_acp::settings::registry(&search, &settings), error)
}

/// The solution's permission policy, read lazily off the UI thread (by the first agent request that needs it).
#[derive(Debug, Default)]
pub struct PolicyStore {
    /// `None` without a solution: the defaults, and Always Allow cannot persist.
    path: Option<PathBuf>,
    policy: Mutex<Option<AgentPolicy>>,
    /// The workspace's launch urls (`browser.origins`' `$launch_urls`), read on first use.
    launch_urls: std::sync::OnceLock<Vec<String>>,
    /// What "Allow for this session" granted this agent session (brief 0041); a new session starts a new store.
    grants: Mutex<std::collections::BTreeSet<String>>,
}

impl PolicyStore {
    pub fn new(path: Option<PathBuf>) -> Self {
        Self {
            path,
            policy: Mutex::new(None),
            launch_urls: std::sync::OnceLock::new(),
            grants: Mutex::new(Default::default()),
        }
    }

    /// The solution's folder, which holds `.eludite/agents-policy.json`.
    pub fn workspace(&self) -> Option<PathBuf> {
        self.path
            .as_deref()
            .and_then(Path::parent)
            .and_then(Path::parent)
            .map(Path::to_path_buf)
    }

    /// What escalation hooks see (ADR-0009): the policy, the folder and its launch urls. Reads files the first
    /// time; called on the MCP call's thread.
    pub fn snapshot(&self) -> PolicySnapshot {
        let workspace = self.workspace();
        let launch_urls = self
            .launch_urls
            .get_or_init(|| {
                workspace
                    .as_deref()
                    .map(eludite_commands::policy::launch_urls)
                    .unwrap_or_default()
            })
            .clone();
        PolicySnapshot {
            policy: self.get(),
            workspace,
            launch_urls,
            session_grants: self
                .grants
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .iter()
                .cloned()
                .collect(),
            ..Default::default()
        }
    }

    /// "Allow for this session": grant `key` until the agent session ends (brief 0041).
    pub fn grant(&self, key: &str) {
        self.grants
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key.to_owned());
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// The policy (a malformed file is reported on stderr and the defaults apply).
    pub fn get(&self) -> AgentPolicy {
        let mut p = self.policy.lock().unwrap_or_else(|e| e.into_inner());
        p.get_or_insert_with(|| match self.path.as_deref().map(AgentPolicy::load) {
            Some(Ok(p)) => p,
            Some(Err(e)) => {
                eprintln!("eludite: agents policy: {e}");
                AgentPolicy::default()
            }
            None => AgentPolicy::default(),
        })
        .clone()
    }

    /// Change the policy; returns what to write and where.
    fn update(&self, f: impl FnOnce(&mut AgentPolicy)) -> Option<(PathBuf, AgentPolicy)> {
        let mut current = self.get();
        f(&mut current);
        *self.policy.lock().unwrap_or_else(|e| e.into_inner()) = Some(current.clone());
        self.path.clone().map(|p| (p, current))
    }
}

/// Review views by document tab id, drawn by the document area.
pub type Reviews = Rc<RefCell<HashMap<String, Entity<DiffView>>>>;
/// Pending-change gutter marks by document id.
pub type Gutters = Rc<RefCell<HashMap<String, Entity<GutterMarkers>>>>;

/// The current solution's store, shared with the agents' and the endpoint's threads.
pub type SharedPolicy = Arc<Mutex<Arc<PolicyStore>>>;

fn current_policy(shared: &SharedPolicy) -> Arc<PolicyStore> {
    shared.lock().unwrap_or_else(|e| e.into_inner()).clone()
}

/// The class of an agent's own tool by its ACP `kind` (PLAN.md 5.3): reading and searching are read; edits and
/// moves are edit_buffer; deleting and fetching from the network are dangerous; running commands, switching modes and
/// anything unknown are execute.
pub fn class_of_kind(kind: Option<&str>) -> PermissionClass {
    match kind {
        Some("read" | "search" | "think") => PermissionClass::Read,
        Some("edit" | "move") => PermissionClass::EditBuffer,
        Some("delete" | "fetch") => PermissionClass::Dangerous,
        _ => PermissionClass::Execute,
    }
}

/// The Eludite commands whose edits go through the workspace-edit applier, so they can be held as pending changes.
pub const REVIEWED_COMMANDS: [&str; 3] = [
    eludite_commands::workspace::WORKSPACE_APPLY_EDIT,
    eludite_commands::workspace::EDITOR_RENAME,
    eludite_commands::workspace::EDITOR_APPLY_CODE_ACTION,
];

/// Keys of the endpoint's own permission requests (Eludite commands of class execute and dangerous), kept apart from
/// the agent's.
static NEXT_ASK: AtomicU64 = AtomicU64::new(1 << 40);

/// What reaches the UI thread from the agents' and the endpoint's threads.
pub enum HostMsg {
    Session(u64, Box<SessionEvent>),
    Mcp(Box<ToolCallRecord>),
    /// A searched registry, its `agents.json` error, and the agent to select (`agents.default` when it changed).
    Registry(Vec<RegisteredAgent>, Option<String>, Option<String>),
    /// The MCP gate asks the user about an Eludite command; the answer goes to `reply`.
    Ask(Box<GateAsk>),
    /// A tool call's images, decoded off the UI thread: (tool call id, thumbnails).
    Images(String, Vec<Thumb>),
}

pub struct GateAsk {
    pub key: u64,
    /// `mcp__eludite__<tool>`.
    pub tool: String,
    /// The call's effective class (ADR-0009).
    pub call: CallClass,
    pub input: Value,
    pub tool_call: Option<String>,
    pub reply: mpsc::Sender<bool>,
}

/// An `eludite.agents.configure` waiting for the agent (brief 0057).
struct Configuring {
    /// [`window::MODE_KEY`] or the option's id.
    key: String,
    value: String,
    /// The setting a pick in the window is remembered in (`agents.model`, `agents.effort`).
    remember: Option<&'static str>,
    /// Callers on other threads, answered with the state once the agent answered.
    waiters: Vec<mpsc::SyncSender<AgentsOutcome>>,
}

/// The setting that remembers a pick of `option` in the window: the model's and the effort's (by ACP category, else
/// by the native adapter's ids).
fn remembered_in(option: &SessionConfigOption) -> Option<&'static str> {
    match (option.category.as_deref(), option.id.as_str()) {
        (Some("model"), _) | (None, "model") => Some("agents.model"),
        (Some("thought_level"), _) | (None, "effort") => Some("agents.effort"),
        _ => None,
    }
}

/// A permission request waiting in the window.
struct Waiting {
    tool: String,
    /// Its effective class, why, and what Always Allow remembers.
    call: CallClass,
    input: Value,
    /// The MCP gate's reply; `None` for the agent's own requests (answered through ACP).
    reply: Option<mpsc::Sender<bool>>,
}

/// What `eludite.agents.permission` answered.
#[derive(Debug, Clone, PartialEq)]
pub struct Answered {
    pub request: u64,
    pub tool: String,
    pub class: PermissionClass,
    pub persisted: bool,
    pub policy_path: Option<PathBuf>,
}

pub struct Agents {
    pub window: Entity<AgentsWindow>,
    setup: AgentsSetup,
    /// The agents settings the registry was last searched with.
    registry_config: Option<RegistryConfig>,
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
    /// The solution's policy.
    policy: SharedPolicy,
    /// `--bench-agent-stream`: per batch, the UI time to apply it, its size, and each chunk's send-to-apply time.
    probe: Option<(Vec<f64>, Vec<usize>, Vec<f64>)>,
    /// Permission requests waiting for the user, by key.
    waiting: HashMap<u64, Waiting>,
    /// Pending changes by id (decided ones stay, for the transcript's links).
    pub changes: BTreeMap<u64, PendingChange>,
    next_change: u64,
    /// Where the endpoint's call threads wait for the review.
    pub board: Arc<ReviewBoard>,
    pub reviews: Reviews,
    pub gutters: Gutters,
    /// Changes whose review view was opened once by itself.
    pub opened_once: Vec<u64>,
    /// The session's modes and config options (brief 0057).
    pub modes: Option<SessionModeState>,
    pub config_options: Vec<SessionConfigOption>,
    /// Configure requests waiting for the agent.
    configuring: Vec<Configuring>,
    /// The configure about to run came from a picker in the window (remembered in the settings).
    picking: bool,
}

impl Agents {
    /// Search the registry again with the agents settings (off the UI thread). A registry fixed up front (tests,
    /// the harness) stays.
    pub fn set_registry_config(&mut self, config: RegistryConfig, cx: &mut Context<Shell>) {
        if self.setup.registry.is_some() {
            return;
        }
        let select = config.default.clone().filter(|d| {
            self.registry_config
                .as_ref()
                .is_none_or(|old| old.default.as_ref() != Some(d))
        });
        self.registry_config = Some(config.clone());
        let tx = self.tx.clone();
        cx.background_spawn(async move {
            let (r, e) = search_registry(&config);
            let _ = tx.unbounded_send(HostMsg::Registry(r, e, select));
        })
        .detach();
    }

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
        // Without a fixed registry, the shell's first settings pass searches it (`set_registry_config`).
        (
            Self {
                window,
                setup,
                registry_config: None,
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
                policy: Arc::new(Mutex::new(Arc::new(PolicyStore::default()))),
                probe: None,
                waiting: HashMap::new(),
                changes: BTreeMap::new(),
                next_change: 1,
                board: Arc::default(),
                reviews: Rc::default(),
                gutters: Rc::default(),
                opened_once: Vec::new(),
                modes: None,
                config_options: Vec::new(),
                configuring: Vec::new(),
                picking: false,
            },
            rx,
        )
    }

    pub fn next_change(&mut self) -> u64 {
        let id = self.next_change;
        self.next_change += 1;
        id
    }

    /// The running agent's name.
    pub fn current_name(&self) -> String {
        self.current.lock().map(|c| c.clone()).unwrap_or_default()
    }

    pub fn selected_agent(&self) -> Option<&RegisteredAgent> {
        self.registry.get(self.selected)
    }

    pub fn session(&self) -> Option<&AgentSession> {
        self.session.as_ref()
    }

    #[cfg(test)]
    pub fn endpoint(&self) -> Option<&McpEndpoint> {
        self.endpoint.as_ref()
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

    /// The footer's pickers (brief 0057).
    pub fn pickers(&self) -> Vec<window::Picker> {
        window::pickers(self.modes.as_ref(), &self.config_options)
    }

    /// The status bar text: the agent, its state and (brief 0057) the model's name.
    pub fn status_text(&self) -> String {
        match (self.selected_agent(), self.state) {
            (_, StateKind::Stopped) | (None, _) => String::new(),
            (Some(a), s) => {
                let mut text = format!("{}: {}", a.name(), s.as_str().replace('_', " "));
                if let Some(model) = self
                    .pickers()
                    .into_iter()
                    .find(|p| p.category.as_deref() == Some("model") || p.key == "model")
                {
                    text.push_str(&format!(" \u{b7} {}", model.name_of(&model.current)));
                }
                text
            }
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

/// The agent's name for a tool call (`Bash`, `mcp__eludite__diagnostics-list`), else its title.
fn tool_of(request: &RequestPermissionRequest) -> String {
    let tc = &request.tool_call;
    tc.agent_tool_name()
        .map(str::to_owned)
        .or_else(|| tc.title.clone())
        .unwrap_or_else(|| tc.tool_call_id.clone())
}

/// The class of an agent's permission request: an Eludite tool's command class, else by the call's ACP kind.
fn class_of(commands: &CommandRegistry, request: &RequestPermissionRequest) -> PermissionClass {
    if let Some(id) = eludite_tool(request)
        && let Some(spec) = commands.lookup(id.as_str())
    {
        return spec.permission;
    }
    class_of_kind(request.tool_call.kind.as_deref())
}

/// The ACP-side policy (runs on the agent's reader thread). Eludite's own tools are allowed here, because the MCP
/// boundary applies the command's class to the call itself (read runs, edits are reviewed, the rest may prompt), so
/// the user is never asked twice. The agent's own tools are judged by the solution's policy.
fn acp_policy(commands: Arc<CommandRegistry>, policy: SharedPolicy) -> PermissionPolicy {
    Arc::new(move |request| {
        if let Some(id) = eludite_tool(request) {
            return match commands.lookup(id.as_str()) {
                Some(spec) if spec.agent_visible && spec.permission == PermissionClass::Read => {
                    PolicyAnswer::Allow(format!("{id} is class read"))
                }
                Some(spec) if spec.agent_visible => PolicyAnswer::Allow(format!(
                    "Eludite checks {id} (class {}) at its MCP boundary",
                    spec.permission.as_str()
                )),
                _ => PolicyAnswer::Ask,
            };
        }
        let class = class_of(&commands, request);
        let input = request.tool_call.raw_input.clone().unwrap_or(Value::Null);
        match current_policy(&policy)
            .get()
            .decide(class, &tool_of(request), &input)
        {
            Verdict::Allow(r) => PolicyAnswer::Allow(r),
            Verdict::Deny(r) => PolicyAnswer::Deny(r),
            // An edit is reviewed in the window: the Agents window holds it as a pending change.
            Verdict::Ask | Verdict::Review => PolicyAnswer::Ask,
        }
    })
}

impl Shell {
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
        // The policy of the solution open now; read when the first request needs it, off the UI thread.
        *self.agents.policy.lock().unwrap_or_else(|e| e.into_inner()) = Arc::new(PolicyStore::new(
            self.solution_dir().map(|d| AgentPolicy::path_for(&d)),
        ));
        // Escalation hooks read the same policy (ADR-0009), on the calling thread, when a hook needs it.
        let shared = self.agents.policy.clone();
        // With which processes this shell started, for `eludite.debug.attach`'s hook (brief 0027).
        let launched = self.debug.launched_processes();
        self.commands
            .set_policy_source(Arc::new(move || PolicySnapshot {
                launched: launched.clone(),
                ..current_policy(&shared).snapshot()
            }));
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
            policy: acp_policy(self.commands.clone(), self.agents.policy.clone()),
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
        // The model and effort last picked in the window (brief 0057), for the agent to start with.
        let meta = {
            let s = self.settings.lock();
            let mut options = serde_json::Map::new();
            for (key, setting) in [("model", "agents.model"), ("effort", "agents.effort")] {
                let v = s.string(setting);
                if !v.is_empty() {
                    options.insert(key.into(), json!(v));
                }
            }
            (!options.is_empty()).then(|| json!({"claudeCode": {"options": options}}))
        };
        self.agents.session = Some(AgentSession::start_with_meta(
            config, meta, generation, sink,
        ));
        let name = agent.name().to_owned();
        // The guides the session can read from Eludite's MCP server (brief 0027): MCP clients list them at start.
        let guides = eludite_mcp::resources::GUIDES
            .iter()
            .map(|g| format!("{} ({})", g.uri, g.title))
            .collect::<Vec<_>>()
            .join(", ");
        self.agents.window.update(cx, |w, cx| {
            w.transcript.notice(format!(
                "Starting {name} (Eludite's MCP resources for the agent: {guides})"
            ));
            // The new session's agent sends its own slash commands, if it has any (brief 0056).
            w.transcript.commands.clear();
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
        // The next session reports its own modes and options; a configure still waiting fails.
        for c in std::mem::take(&mut self.agents.configuring) {
            for w in c.waiters {
                let _ = w.send(Err(eludite_commands::CommandError::Failed(
                    "the agent session ended".into(),
                )));
            }
        }
        self.agents.modes = None;
        self.agents.config_options.clear();
        self.agents.window.update(cx, |w, cx| {
            w.pending.clear();
            w.set_options(None, Vec::new(), cx);
        });
        self.sync_agents_header(cx);
    }

    /// `eludite.agents.configure` (brief 0057): check the choice and have the session ask the agent. Returns the
    /// receiver its answer reaches (the state, or the agent's message).
    fn agents_configure(
        &mut self,
        option: &str,
        value: &str,
        from_window: bool,
    ) -> Result<mpsc::Receiver<AgentsOutcome>, String> {
        let a = &self.agents;
        let session = a
            .session
            .as_ref()
            .filter(|_| matches!(a.state, StateKind::Ready | StateKind::Running))
            .ok_or("not supported: no agent session is running; start the agent first")?;
        if a.state == StateKind::Running {
            return Err("busy: a turn is in progress; wait for the turn to end".into());
        }
        let pickers = a.pickers();
        if pickers.is_empty() {
            return Err(format!(
                "not supported: {} offers no modes or config options",
                a.current_name()
            ));
        }
        let picker = pickers.iter().find(|p| p.key == option).ok_or_else(|| {
            let known: Vec<&str> = pickers.iter().map(|p| p.key.as_str()).collect();
            if option == window::MODE_KEY {
                format!(
                    "not supported: the agent offers no modes (its options: {})",
                    known.join(", ")
                )
            } else {
                format!(
                    "unknown option `{option}` (the agent offers {})",
                    known.join(", ")
                )
            }
        })?;
        if !picker.has(value) {
            let values: Vec<&str> = picker.choices.iter().map(|c| c.value.as_str()).collect();
            return Err(format!(
                "unknown value `{value}` for {option} (one of {})",
                values.join(", ")
            ));
        }
        if picker.is_mode {
            session.set_mode(value);
        } else {
            session.set_option(option, value);
        }
        let remember = if from_window && !picker.is_mode {
            a.config_options
                .iter()
                .find(|o| o.id == option)
                .and_then(remembered_in)
        } else {
            None
        };
        let (tx, rx) = mpsc::sync_channel(1);
        self.agents.configuring.push(Configuring {
            key: option.to_owned(),
            value: value.to_owned(),
            remember,
            waiters: vec![tx],
        });
        Ok(rx)
    }

    /// The agent's modes and options changed: answer the configures they satisfy, remember the window's picks.
    fn on_agent_options(
        &mut self,
        modes: Option<SessionModeState>,
        config_options: Vec<SessionConfigOption>,
        cx: &mut Context<Self>,
    ) {
        self.agents.modes = modes.clone();
        self.agents.config_options = config_options.clone();
        let pickers = self.agents.pickers();
        // The screenshot driver waits for the pickers (brief 0057).
        super::documents::trace(format_args!(
            "agents options {}",
            pickers
                .iter()
                .map(|p| format!("{}={}", p.key, p.current))
                .collect::<Vec<_>>()
                .join(" ")
        ));
        let (done, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agents.configuring)
            .into_iter()
            .partition(|c| {
                pickers
                    .iter()
                    .any(|p| p.key == c.key && p.current == c.value)
            });
        self.agents.configuring = waiting;
        self.agents.window.update(cx, |w, cx| {
            w.set_options(modes, config_options, cx);
        });
        for c in done {
            super::documents::trace(format_args!("agents option {} = {}", c.key, c.value));
            if let Some(setting) = c.remember
                && let Err(e) = self.settings.set(
                    setting,
                    json!(c.value),
                    eludite_commands::settings::SettingScope::User,
                )
            {
                eprintln!("eludite: cannot remember {setting}: {e}");
            }
            let state = self.agents_state(cx);
            for w in c.waiters {
                let _ = w.send(Ok(eludite_commands::agents::AgentsOutput::State(
                    state.clone(),
                )));
            }
        }
        self.sync_agents_header(cx);
    }

    /// The agent refused a change of `id`: the picker reverts and the transcript says why.
    fn on_agent_option_failed(&mut self, id: &str, error: &str, cx: &mut Context<Self>) {
        let (failed, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.agents.configuring)
            .into_iter()
            .partition(|c| c.key == id);
        self.agents.configuring = waiting;
        let title = self
            .agents
            .pickers()
            .into_iter()
            .find(|p| p.key == id)
            .map_or_else(|| id.to_owned(), |p| p.title);
        for c in failed {
            for w in c.waiters {
                let _ = w.send(Err(eludite_commands::CommandError::Failed(
                    error.to_owned(),
                )));
            }
        }
        let text = format!("Could not change the {}: {error}", title.to_lowercase());
        self.agents.window.update(cx, |w, cx| {
            w.revert(id, cx);
            w.transcript.error(text);
            w.sync(cx);
        });
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

    /// Cancel the turn: `session/cancel`, every pending request answered `cancelled`, every pending change rejected.
    pub fn agents_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.reject_pending(window, cx);
        let mut keys = self
            .agents
            .session
            .as_ref()
            .map(AgentSession::cancel)
            .unwrap_or_default();
        // The endpoint's own requests are denied too, so the agent's tool calls return.
        for (key, w) in &self.agents.waiting {
            if let Some(reply) = &w.reply {
                let _ = reply.send(false);
                keys.push(*key);
            }
        }
        self.agents.waiting.clear();
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

    /// Answer permission request `key`: the agent's (through ACP) or the MCP gate's. Always Allow adds a rule to the
    /// solution's policy file (written off the UI thread) and allows this call.
    pub fn agents_answer(
        &mut self,
        key: u64,
        decision: Decision,
        cx: &mut Context<Self>,
    ) -> Result<Answered, String> {
        let waiting = self
            .agents
            .waiting
            .remove(&key)
            .ok_or_else(|| format!("no pending permission request {key}"))?;
        let allowed = decision != Decision::Deny;
        let label = match &waiting.reply {
            Some(reply) => {
                let _ = reply.send(allowed);
                if allowed { "Allowed" } else { "Denied" }.to_owned()
            }
            None => {
                let session = self.agents.session.as_ref().ok_or("no agent is running")?;
                // Always Allow is Eludite's own rule (the policy file); the agent is told to allow once, so it
                // keeps asking and the policy keeps deciding.
                let kind = if allowed {
                    PermissionOptionKind::AllowOnce
                } else {
                    PermissionOptionKind::RejectOnce
                };
                session
                    .answer(key, kind)
                    .map(|o| o.name)
                    .ok_or_else(|| format!("no pending permission request {key}"))?
            }
        };
        let mut persisted = false;
        let mut remembered = None;
        let store = current_policy(&self.agents.policy);
        // "Allow for this session" (brief 0041): the grant holds until the agent session ends; nothing is written.
        if decision == Decision::AlwaysAllow
            && let AlwaysAllow::Session(key) = &waiting.call.always_allow
        {
            store.grant(key);
            remembered = Some(format!("{key} for this session"));
        }
        // Always Allow remembers what the call's escalation names (a rule, an origin), or nothing (ADR-0009).
        if decision == Decision::AlwaysAllow
            && waiting.call.always_allow != AlwaysAllow::Never
            && !matches!(waiting.call.always_allow, AlwaysAllow::Session(_))
            && let Some((path, policy)) = store
                .update(|p| remembered = p.remember(&waiting.call, &waiting.tool, &waiting.input))
        {
            persisted = true;
            cx.background_spawn(async move {
                if let Err(e) = policy.save(&path) {
                    eprintln!("eludite: {}: {e}", path.display());
                }
            })
            .detach();
        }
        let reason = match (decision, &remembered) {
            (Decision::AlwaysAllow, Some(what)) if persisted => {
                format!("{label}, and always for this solution ({what})")
            }
            (Decision::AlwaysAllow, Some(what)) => format!("{label} ({what})"),
            _ => label,
        };
        self.agents.window.update(cx, |w, cx| {
            w.transcript.answer(
                key,
                if allowed {
                    Permission::Allowed {
                        auto: false,
                        reason,
                    }
                } else {
                    Permission::Denied {
                        auto: false,
                        reason,
                    }
                },
            );
            w.sync(cx);
        });
        self.after_permission_change(cx);
        Ok(Answered {
            request: key,
            tool: waiting.tool,
            class: waiting.call.class,
            persisted,
            policy_path: store.path().map(Path::to_path_buf).filter(|_| persisted),
        })
    }

    /// The oldest waiting request, for `eludite.agents.permission` without a request id.
    pub fn agents_oldest_request(&self, cx: &gpui::App) -> Option<u64> {
        self.agents
            .window
            .read(cx)
            .transcript
            .asked()
            .first()
            .copied()
    }

    /// Show the oldest pending request in the window's prompt.
    pub(super) fn after_permission_change(&mut self, cx: &mut Context<Self>) {
        let window = self.agents.window.clone();
        let can_persist = current_policy(&self.agents.policy).path().is_some();
        let calls: HashMap<u64, CallClass> = self
            .agents
            .waiting
            .iter()
            .map(|(k, w)| (*k, w.call.clone()))
            .collect();
        window.update(cx, |w, cx| {
            let oldest = w.transcript.asked().first().copied();
            w.prompt = oldest.and_then(|key| {
                let call = calls.get(&key);
                let row = w.transcript.tools().find(
                    |t| matches!(t.permission, Some(Permission::Asked { key: k, .. }) if k == key),
                )?;
                let class = match row.permission {
                    Some(Permission::Asked { class, .. }) => class,
                    _ => PermissionClass::Execute,
                };
                // "Allow for this session" (brief 0041) needs no policy file.
                let session =
                    call.is_some_and(|c| matches!(c.always_allow, AlwaysAllow::Session(_)));
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
                    can_persist: (can_persist
                        && call.is_none_or(|c| c.always_allow != AlwaysAllow::Never))
                        || session,
                    session,
                    reason: call.and_then(|c| c.reason.clone()),
                })
            });
            cx.notify();
        });
    }

    pub(super) fn endpoint_hooks(&self) -> EndpointHooks {
        let current = self.agents.current.clone();
        let tx = self.agents.tx.clone();
        let gate_tx = tx.clone();
        let policy = self.agents.policy.clone();
        EndpointHooks {
            agent: Arc::new(move || current.lock().map(|c| c.clone()).unwrap_or_default()),
            // On the endpoint's call thread: the policy, else ask the user and wait for the answer.
            gate: Arc::new(move |spec, args, ctx| {
                let tool = format!("mcp__{MCP_SERVER_NAME}__{}", tool_name(&spec.id));
                // The call's effective class (ADR-0009): an escalated call is not allowed by the tool's rules.
                let verdict = current_policy(&policy)
                    .get()
                    .decide_call(&ctx.class, &tool, args);
                match verdict {
                    Verdict::Allow(_) => return GateDecision::Allow,
                    Verdict::Deny(r) => return GateDecision::Deny(r),
                    // Edits through the applier are held as pending changes, reviewed after the call runs.
                    Verdict::Review if REVIEWED_COMMANDS.contains(&spec.id.as_str()) => {
                        return GateDecision::Allow;
                    }
                    Verdict::Review | Verdict::Ask => {}
                }
                let (reply, answer) = mpsc::channel();
                let ask = GateAsk {
                    key: NEXT_ASK.fetch_add(1, Ordering::Relaxed),
                    tool,
                    call: ctx.class.clone(),
                    input: args.clone(),
                    tool_call: ctx.tool_call.clone(),
                    reply,
                };
                if gate_tx.unbounded_send(HostMsg::Ask(Box::new(ask))).is_err() {
                    return GateDecision::Deny("the window is closed".into());
                }
                match answer.recv() {
                    Ok(true) => GateDecision::Allow,
                    _ => GateDecision::Deny("the user denied it".into()),
                }
            }),
            // The call runs as the agent; an edit it proposed is answered once the user has reviewed it.
            invoker: Some({
                let commands = self.commands.clone();
                let board = self.agents.board.clone();
                Arc::new(move |spec, args, ctx| {
                    let out = eludite_commands::with_caller(ctx.caller(), || {
                        commands.invoke_as(spec.id.as_str(), args, &ctx.class).1
                    })?;
                    if !REVIEWED_COMMANDS.contains(&spec.id.as_str()) {
                        return Ok(out);
                    }
                    let decided = board.wait_decided(ctx.call);
                    if decided.is_empty() {
                        return Ok(out);
                    }
                    Ok(review::amend_output(spec.id.as_str(), out, &decided))
                })
            }),
            observer: Some(Arc::new(move |record| {
                let _ = tx.unbounded_send(HostMsg::Mcp(Box::new(record.clone())));
            })),
        }
    }

    /// `eludite.agents.*`, on the UI thread.
    pub fn apply_agents(
        &mut self,
        request: eludite_commands::agents::AgentsRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AgentsOutcome {
        use eludite_commands::CommandError;
        use eludite_commands::agents::{
            AgentsOutput, AgentsRequest, ChangeRow, PermissionDecision, PermissionOutput,
            ReviewOutput, ReviewTarget,
        };
        let failed = CommandError::Failed;
        DEFERRED.with(|d| d.borrow_mut().take());
        match request {
            AgentsRequest::Start { agent, restart } => {
                self.agents_start(agent.as_deref(), restart, cx)
                    .map_err(failed)?;
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            AgentsRequest::Configure { option, value } => {
                let from_window = std::mem::take(&mut self.agents.picking);
                let rx = self
                    .agents_configure(&option, &value, from_window)
                    .map_err(failed)?;
                // A caller on another thread waits on `rx` (see `JobReply`).
                DEFERRED.with(|d| *d.borrow_mut() = Some(rx));
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            AgentsRequest::Prompt { text } => {
                self.agents_prompt(&text, cx).map_err(failed)?;
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            AgentsRequest::Cancel => {
                self.agents_cancel(window, cx);
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            AgentsRequest::Permission { request, decision } => {
                let key = request
                    .or_else(|| self.agents_oldest_request(cx))
                    .ok_or_else(|| failed("no permission request is pending".into()))?;
                let d = match decision {
                    PermissionDecision::Allow => Decision::Allow,
                    PermissionDecision::AlwaysAllow => Decision::AlwaysAllow,
                    PermissionDecision::Deny => Decision::Deny,
                };
                let a = self.agents_answer(key, d, cx).map_err(failed)?;
                Ok(AgentsOutput::Permission(PermissionOutput {
                    request: a.request,
                    decision,
                    tool: a.tool,
                    class: a.class,
                    persisted: a.persisted,
                    policy_path: a.policy_path.map(|p| p.to_string_lossy().into_owned()),
                }))
            }
            AgentsRequest::Review { target, accept } => {
                let ids = match &target {
                    ReviewTarget::Change(id) => self.changes_for(Some(*id), None),
                    ReviewTarget::Path(p) => self.changes_for(None, Some(p)),
                    ReviewTarget::All => self.changes_for(None, None),
                };
                let message = self.decide(&ids, accept, window, cx).err();
                let changes = ids
                    .iter()
                    .filter_map(|id| self.agents.changes.get(id))
                    .map(|c| ChangeRow {
                        id: c.id,
                        path: c.path.to_string_lossy().into_owned(),
                        state: c.state.label().into(),
                        edits: c.edits,
                        tool_call: c.tool_call.clone(),
                        message: match &c.state {
                            review::ChangeState::Failed(why) => Some(why.clone()),
                            _ => None,
                        },
                    })
                    .collect();
                Ok(AgentsOutput::Review(ReviewOutput {
                    decision: if accept { "accept" } else { "reject" }.into(),
                    changes,
                    message,
                }))
            }
        }
    }

    /// `agents-state.output.json`.
    pub fn agents_state(&self, cx: &gpui::App) -> eludite_commands::agents::AgentsStateOutput {
        use eludite_commands::agents::{
            AgentRow, AgentsStateOutput, ChoiceRow, CommandRow, LoginRow, ModeOutput, ModeRow,
            OptionRow,
        };
        let a = &self.agents;
        AgentsStateOutput {
            agent: a
                .selected_agent()
                .map(|x| x.name().to_owned())
                .unwrap_or_default(),
            state: a.state.as_str().into(),
            generation: a.generation,
            session_id: a.session_id.clone(),
            agent_info: a.agent_info.clone(),
            protocol_version: a.protocol,
            message: (!a.detail.is_empty()).then(|| a.detail.clone()),
            login: a
                .login
                .iter()
                .map(|m| LoginRow {
                    name: m.name.clone(),
                    description: (!m.description.is_empty()).then(|| m.description.clone()),
                    command: m.command.clone(),
                })
                .collect(),
            last_stop_reason: a.last_stop.clone(),
            agents: a
                .registry
                .iter()
                .map(|r| AgentRow {
                    name: r.name().to_owned(),
                    command: r.command_line(),
                    source: r.source.as_str().into(),
                })
                .collect(),
            pending_permissions: a.window.read(cx).transcript.asked().len() as u64,
            pending_changes: a
                .changes
                .values()
                .filter(|c| c.state == review::ChangeState::Pending)
                .count() as u64,
            // What the slash menu offers (brief 0056).
            commands: a
                .window
                .read(cx)
                .transcript
                .commands
                .iter()
                .map(|c| CommandRow {
                    name: c.name.clone(),
                    description: c.description.clone(),
                    hint: c.hint().map(str::to_owned),
                })
                .collect(),
            // The session's mode and select options (brief 0057).
            mode: a.modes.as_ref().map(|m| ModeOutput {
                current: m.current_mode_id.clone(),
                available: m
                    .available_modes
                    .iter()
                    .map(|m| ModeRow {
                        id: m.id.clone(),
                        name: m.name.clone(),
                        description: m.description.clone(),
                    })
                    .collect(),
            }),
            options: a
                .config_options
                .iter()
                .filter_map(|o| {
                    let s = o.as_select()?;
                    Some(OptionRow {
                        id: o.id.clone(),
                        name: o.name.clone(),
                        description: o.description.clone(),
                        category: o.category.clone(),
                        current: s.current_value.clone(),
                        choices: s
                            .options
                            .iter()
                            .map(|c| ChoiceRow {
                                value: c.value.clone(),
                                name: c.name.clone(),
                                description: c.description.clone(),
                            })
                            .collect(),
                    })
                })
                .collect(),
        }
    }

    /// Stop the agent (the process is killed).
    pub fn agents_stop(&mut self, cx: &mut Context<Self>) {
        self.stop_session(cx);
    }

    /// `--bench-agent-stream`: start or stop recording frame work and batch costs.
    pub fn agents_probe(&mut self, on: bool, cx: &mut Context<Self>) {
        if on {
            self.agents.probe = Some(Default::default());
        }
        self.agents.window.update(cx, |w, _| {
            let mut p = w.probes.borrow_mut();
            p.recording = on;
            if on {
                p.frame_work_ms.clear();
            }
        });
    }

    /// (frame work, apply per batch, batch sizes, chunk send to apply), in ms.
    pub fn agents_probe_results(
        &mut self,
        cx: &mut Context<Self>,
    ) -> (Vec<f64>, Vec<f64>, Vec<usize>, Vec<f64>) {
        let frames = self
            .agents
            .window
            .read(cx)
            .probes
            .borrow()
            .frame_work_ms
            .clone();
        let (apply, sizes, chunks) = self.agents.probe.take().unwrap_or_default();
        (frames, apply, sizes, chunks)
    }

    /// `eludite.view.show` for a tool window.
    pub fn commands_invoke_view_show(
        &self,
        id: &str,
    ) -> Result<Value, eludite_commands::CommandError> {
        self.commands
            .invoke("eludite.view.show", json!({ "id": id }))
    }

    /// `--bench-diff`: hold a 20-edit change of `file` (2000 lines) as an agent's pending change. Returns its id.
    pub fn bench_capture_big_edit(
        &mut self,
        file: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Option<u64> {
        let uri = super::documents::path_to_uri(file);
        let edits: Vec<Value> = (0..20)
            .map(|i| {
                let line = i * 100;
                json!({"range": {"start": {"line": line, "character": 4}, "end": {"line": line, "character": 7}}, "newText": "long"})
            })
            .collect();
        let edit: eludite_lsp::lsp::WorkspaceEdit =
            serde_json::from_value(json!({"changes": {uri: edits}})).ok()?;
        let caller = eludite_commands::Caller::Agent {
            agent: "bench".into(),
            call: eludite_commands::next_call_id(),
            tool_call: None,
        };
        let ids = self
            .capture_edit(&edit, &Default::default(), &caller, window, cx)
            .ok()?;
        ids.first().copied()
    }

    /// `--bench-diff`: reject every pending change and close the review views.
    pub fn bench_reject_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.changes_for(None, None);
        let _ = self.decide(&ids, false, window, cx);
        let tabs: Vec<String> = self.agents.reviews.borrow().keys().cloned().collect();
        for t in tabs {
            self.controller.close_document(&t);
        }
        self.agents.reviews.borrow_mut().clear();
    }

    /// Whether `request` from `caller` is an agent's edit to hold for review (the solution's policy says review).
    pub(super) fn reviews_edit(
        &self,
        request: &eludite_commands::workspace::WorkspaceRequest,
        caller: &eludite_commands::Caller,
    ) -> bool {
        use eludite_commands::workspace::WorkspaceRequest as R;
        let edit = matches!(
            request,
            R::ApplyEdit { .. } | R::ApplyCodeAction { .. } | R::Rename { apply: true, .. }
        );
        edit && caller.is_agent()
            && current_policy(&self.agents.policy)
                .get()
                .edit_buffer
                .unwrap_or_default()
                == eludite_commands::policy::EditPolicy::Review
    }

    /// Window events: the user's actions.
    pub(super) fn on_agents_window_event(
        &mut self,
        _: &Entity<AgentsWindow>,
        event: &AgentsWindowEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use eludite_commands::agents::{CANCEL, CONFIGURE, PERMISSION, PROMPT, REVIEW, START};
        // Every action in the window is a command on the bus (CLAUDE.md invariant 3).
        match event {
            AgentsWindowEvent::Start { agent, restart } => {
                let mut args = json!({ "restart": restart });
                if let Some(a) = agent {
                    args["agent"] = json!(a);
                }
                self.run(START, args, window, cx);
            }
            AgentsWindowEvent::Prompt(text) => {
                self.run(PROMPT, json!({ "text": text }), window, cx)
            }
            AgentsWindowEvent::Cancel => self.run(CANCEL, json!({}), window, cx),
            AgentsWindowEvent::Answer { request, decision } => self.run(
                PERMISSION,
                json!({ "request": request, "decision": decision.as_str() }),
                window,
                cx,
            ),
            AgentsWindowEvent::Review { change, accept } => {
                let mut args = json!({ "decision": if *accept { "accept" } else { "reject" } });
                match change {
                    Some(c) => args["change"] = json!(c),
                    None => args["all"] = json!(true),
                }
                self.run(REVIEW, args, window, cx);
            }
            // A picker's choice (brief 0057): remembered in the settings once the agent took it.
            AgentsWindowEvent::Configure { option, value } => {
                self.agents.picking = true;
                let result = self.invoke(
                    CONFIGURE,
                    json!({ "option": option, "value": value }),
                    window,
                    cx,
                );
                self.agents.picking = false;
                if let Err(e) = result {
                    let text = format!("Could not change the {option}: {e}");
                    self.status.set(eludite_ui::slots::STATE, text.clone());
                    self.agents.window.update(cx, |w, cx| {
                        w.revert(option, cx);
                        w.transcript.error(text);
                        w.sync(cx);
                    });
                }
            }
            AgentsWindowEvent::OpenChange(id) => self.open_change(*id, window, cx),
            AgentsWindowEvent::OpenImage { tool_call, index } => {
                self.open_image(tool_call, *index, cx)
            }
            // A debug row's stop location opens the file at the line, as an Error List row does (brief 0027).
            AgentsWindowEvent::OpenLocation { path, line } => {
                if let Err(e) = self.open_at(path, *line, 1, window, cx) {
                    self.status.set(eludite_ui::slots::STATE, e.to_string());
                }
            }
            // A link in the agent's message: a web page in the person's browser, a file in the editor at its line.
            AgentsWindowEvent::OpenLink(url) => {
                match resolve_link(url, self.workspace_root().as_deref()) {
                    Link::Web(url) => cx.open_url(&url),
                    Link::File { path, line } => {
                        let path = path.to_string_lossy().into_owned();
                        if let Err(e) = self.open_at(&path, line, 1, window, cx) {
                            self.status.set(eludite_ui::slots::STATE, e.to_string());
                        }
                    }
                    Link::Unsupported => {
                        self.status.set(
                            eludite_ui::slots::STATE,
                            format!("Cannot open the link {url}"),
                        );
                    }
                }
            }
        }
    }

    /// Apply a batch of agent events (everything queued since the last frame).
    pub(super) fn on_agent_batch(
        &mut self,
        batch: Vec<HostMsg>,
        window_: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let applied = Instant::now();
        let batch_len = batch.len();
        if let Some((_, _, chunks)) = &mut self.agents.probe {
            let now = eludite_acp::fake_agent::wall_ns();
            for m in &batch {
                if let HostMsg::Session(_, e) = m
                    && let SessionEvent::Update(
                        eludite_acp::protocol::SessionUpdate::AgentMessageChunk(c),
                    ) = &**e
                    && let Some(sent) = c
                        .meta()
                        .and_then(|m| m.get(eludite_acp::fake_agent::SENT_AT_META))
                        .and_then(|v| v.as_str())
                        .and_then(|v| v.parse::<u128>().ok())
                {
                    chunks.push(now.saturating_sub(sent) as f64 / 1e6);
                }
            }
        }
        self.apply_agent_batch(batch, window_, cx);
        if let Some((apply, sizes, _)) = &mut self.agents.probe {
            apply.push(applied.elapsed().as_secs_f64() * 1e3);
            sizes.push(batch_len);
        }
    }

    fn apply_agent_batch(
        &mut self,
        batch: Vec<HostMsg>,
        window_: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let generation = self.agents.generation;
        let mut header = false;
        let mut permissions = false;
        let window = self.agents.window.clone();
        for msg in batch {
            match msg {
                HostMsg::Registry(registry, error, select) => {
                    // `--agent` first, then a newly chosen `agents.default`, else keep the selected agent across a
                    // new search (a settings change).
                    let was = self
                        .agents
                        .registry
                        .get(self.agents.selected)
                        .map(|a| a.name().to_owned());
                    self.agents.registry = registry;
                    self.agents.selected = self
                        .agents
                        .setup
                        .preferred
                        .as_ref()
                        .or(select.as_ref())
                        .or(was.as_ref())
                        .and_then(|p| self.agents.registry.iter().position(|a| a.name() == p))
                        .unwrap_or(0);
                    if let Some(e) = error {
                        window.update(cx, |w, _| w.transcript.error(e));
                    }
                    header = true;
                }
                HostMsg::Mcp(record) => {
                    super::documents::trace(format_args!(
                        "agents mcp {} ok={} {:.2} ms on {}",
                        record.tool,
                        record.outcome.is_ok(),
                        record.elapsed.as_secs_f64() * 1e3,
                        record.thread
                    ));
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
                        // A debug command reads as the Debug toolbar and the status bar would say it (brief 0027).
                        debug: record.command.as_ref().and_then(|c| {
                            transcript::debug_line(c.as_str(), &record.arguments, &record.outcome)
                        }),
                    };
                    let linked = window.update(cx, |w, _| {
                        w.transcript
                            .link_mcp(record.tool_call.as_deref(), &record.tool, link)
                    });
                    // The images the command answered (a screenshot), as thumbnails.
                    if let (Some(row), Some(spec), Ok(output)) = (
                        linked,
                        record
                            .command
                            .as_ref()
                            .and_then(|c| self.commands.lookup(c.as_str())),
                        &record.outcome,
                    ) {
                        let images: Vec<ImageData> =
                            eludite_mcp::take_image_content(&spec, &mut output.clone())
                                .into_iter()
                                .filter_map(|i| {
                                    Some(ImageData {
                                        data: i["data"].as_str()?.to_owned(),
                                        mime: i["mimeType"].as_str()?.to_owned(),
                                    })
                                })
                                .collect();
                        self.decode_images(row, images, cx);
                    }
                }
                HostMsg::Images(row, thumbs) => {
                    window.update(cx, |w, _| w.transcript.add_thumbs(&row, thumbs));
                }
                HostMsg::Ask(ask) => {
                    permissions = true;
                    super::documents::trace(format_args!(
                        "agents permission asked {} ({})",
                        ask.tool,
                        ask.call.class.as_str()
                    ));
                    let GateAsk {
                        key,
                        tool,
                        call,
                        input,
                        tool_call,
                        reply,
                    } = *ask;
                    let class = call.class;
                    let bare = tool.rsplit("__").next().unwrap_or_default().to_owned();
                    window.update(cx, |w, _| {
                        w.transcript.ask_mcp(
                            tool_call.as_deref(),
                            &bare,
                            &input,
                            Permission::Asked { key, class },
                        )
                    });
                    self.agents.waiting.insert(
                        key,
                        Waiting {
                            tool,
                            call,
                            input,
                            reply: Some(reply),
                        },
                    );
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
                        super::documents::trace(format_args!("agents ready {session_id}"));
                        // The login state can arrive (on the reader thread) before the handshake's end: keep it.
                        if self.agents.state != StateKind::NeedsLogin {
                            self.agents.state = StateKind::Ready;
                        }
                        self.agents.ready_ms =
                            self.agents.started.map(|t| t.elapsed().as_secs_f64() * 1e3);
                        let info = agent_info
                            .map(|a| format!("{} {}", a.name, a.version))
                            .unwrap_or_default();
                        let detail = format!(
                            "{info} (ACP v{protocol_version}). MCP: {}",
                            self.agents
                                .endpoint
                                .as_ref()
                                .map(McpEndpoint::describe)
                                .unwrap_or_default()
                        );
                        // The login state's detail is the label the agent gave.
                        if self.agents.state != StateKind::NeedsLogin {
                            self.agents.detail = detail;
                        }
                        self.agents.session_id = Some(session_id);
                        self.agents.agent_info = Some(info);
                        self.agents.protocol = Some(protocol_version);
                    }
                    SessionEvent::Timing { name, ms } => {
                        super::documents::trace(format_args!("agents {name} {ms:.1} ms"));
                        self.agents.timings.push((name, ms))
                    }
                    SessionEvent::Update(u) => {
                        // Image content in a tool call's result (the agent forwarding a screenshot), as thumbnails.
                        if let eludite_acp::protocol::SessionUpdate::ToolCall(t)
                        | eludite_acp::protocol::SessionUpdate::ToolCallUpdate(t) = &u
                            && let Some(content) = &t.content
                        {
                            let images = transcript::content_images(content);
                            self.decode_images(t.tool_call_id.clone(), images, cx);
                        }
                        if let Some(commands) = u.available_commands() {
                            super::documents::trace(format_args!(
                                "agents commands {}",
                                commands.len()
                            ));
                        }
                        window.update(cx, |w, _| w.transcript.apply(&u))
                    }
                    SessionEvent::Permission { key, request } => {
                        let class = class_of(&self.commands, &request);
                        // The agent's own file tool with a diff: a pending change, reviewed like Eludite's edits.
                        if class == PermissionClass::EditBuffer
                            && current_policy(&self.agents.policy)
                                .get()
                                .edit_buffer
                                .unwrap_or_default()
                                == eludite_commands::policy::EditPolicy::Review
                        {
                            window.update(cx, |w, _| w.transcript.ensure_tool(&request.tool_call));
                            if self.capture_tool_edit(key, &request, window_, cx) {
                                continue;
                            }
                        }
                        permissions = true;
                        super::documents::trace(format_args!(
                            "agents permission asked {} ({})",
                            tool_of(&request),
                            class.as_str()
                        ));
                        self.agents.waiting.insert(
                            key,
                            Waiting {
                                tool: tool_of(&request),
                                call: CallClass::declared(class),
                                input: request.tool_call.raw_input.clone().unwrap_or(json!({})),
                                reply: None,
                            },
                        );
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
                        super::documents::trace(format_args!(
                            "agents permission {} {}: {reason}",
                            tool_of(&request),
                            if allowed { "allowed" } else { "denied" }
                        ));
                        let state = if allowed {
                            Permission::Allowed { auto: true, reason }
                        } else {
                            Permission::Denied { auto: true, reason }
                        };
                        window.update(cx, |w, _| w.transcript.permission(&request, state));
                    }
                    SessionEvent::TurnEnded(r) => {
                        super::documents::trace(format_args!("agents turn ended {r:?}"));
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
                        if let Some(path) = self.agents.setup.transcript_out.clone() {
                            let json = window.read(cx).transcript.to_json();
                            cx.background_spawn(async move {
                                let _ = std::fs::write(
                                    &path,
                                    serde_json::to_string_pretty(&json).unwrap_or_default(),
                                );
                            })
                            .detach();
                        }
                    }
                    SessionEvent::Stderr(line) => {
                        if std::env::var_os("ELUDITE_AGENT_STDERR").is_some() {
                            eprintln!("[agent] {line}");
                        }
                    }
                    SessionEvent::Notification { .. } => {}
                    SessionEvent::Options {
                        modes,
                        config_options,
                    } => self.on_agent_options(modes, config_options, cx),
                    SessionEvent::OptionFailed { id, error } => {
                        self.on_agent_option_failed(&id, &error, cx)
                    }
                },
            }
        }
        self.audit_agent_tools(cx);
        window.update(cx, |w, cx| w.sync(cx));
        if permissions {
            self.after_permission_change(cx);
        }
        if header {
            self.sync_agents_header(cx);
        }
    }

    /// Decode `images` of tool call `row` off the UI thread; their thumbnails reach the row through the pump.
    fn decode_images(&self, row: String, images: Vec<ImageData>, cx: &mut Context<Self>) {
        if images.is_empty() {
            return;
        }
        let tx = self.agents.tx.clone();
        cx.background_spawn(async move {
            let thumbs: Vec<Thumb> = images.iter().filter_map(transcript::decode_thumb).collect();
            if !thumbs.is_empty() {
                let _ = tx.unbounded_send(HostMsg::Images(row, thumbs));
            }
        })
        .detach();
    }

    /// Where opened images are saved: beside the transcript (`--transcript-out`), else in Eludite's cache.
    pub fn image_dir(&self) -> PathBuf {
        match &self.agents.setup.transcript_out {
            Some(t) => {
                let stem = t
                    .file_stem()
                    .map_or_else(|| "transcript".into(), |s| s.to_string_lossy().into_owned());
                t.parent()
                    .unwrap_or(Path::new("."))
                    .join(format!("{stem}-images"))
            }
            None => eludite_browser::discovery::default_cache_root()
                .and_then(|c| c.parent().map(Path::to_path_buf))
                .unwrap_or_else(std::env::temp_dir)
                .join("agents")
                .join("images"),
        }
    }

    /// Open image `index` of tool call `tool_call` in full: the editor has no image view yet, so it is saved
    /// ([`Shell::image_dir`]) and opened with the system viewer through `eludite.browser.open_external`, off the UI
    /// thread.
    pub fn open_image(&mut self, tool_call: &str, index: usize, cx: &mut Context<Self>) {
        let image = self
            .agents
            .window
            .read(cx)
            .transcript
            .tool(tool_call)
            .and_then(|t| t.images.get(index))
            .cloned();
        let Some(image) = image else {
            return;
        };
        let ext = match image.mime.as_str() {
            "image/jpeg" => "jpg",
            "image/gif" => "gif",
            "image/webp" => "webp",
            _ => "png",
        };
        let name: String = tool_call
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                    c
                } else {
                    '_'
                }
            })
            .collect();
        let path = self.image_dir().join(format!("{name}-{index}.{ext}"));
        let commands = self.commands.clone();
        // A thread of its own: the browser worker refuses the UI thread, and the write may block.
        let spawned = std::thread::Builder::new()
            .name("agents-open-image".into())
            .spawn(move || {
                let saved = path
                    .parent()
                    .map_or(Ok(()), std::fs::create_dir_all)
                    .and_then(|()| std::fs::write(&path, image.bytes.as_slice()));
                if let Err(e) = saved {
                    eprintln!("eludite: {}: {e}", path.display());
                    return;
                }
                let url = super::documents::path_to_uri(&path);
                if let Err(e) = commands.invoke(
                    eludite_commands::browser::OPEN_EXTERNAL,
                    json!({ "url": url }),
                ) {
                    eprintln!("eludite: opening {url}: {e}");
                }
            });
        if let Err(e) = spawned {
            eprintln!("eludite: opening an image: {e}");
        }
    }

    /// Every tool call is audited: Eludite's at its MCP boundary; the agent's own once they end, with their arguments
    /// and how they ended.
    fn audit_agent_tools(&mut self, cx: &mut Context<Self>) {
        let window = self.agents.window.clone();
        let ended = window.read(cx).transcript.unaudited();
        if ended.is_empty() {
            return;
        }
        let agent = self.agents.current_name();
        window.update(cx, |w, _| {
            for super::agents::transcript::EndedTool {
                id,
                name,
                kind,
                input,
                ok,
            } in ended
            {
                let seq = self.commands.audit_log().record_call(
                    &name,
                    Some(class_of_kind(kind.as_deref())),
                    if ok {
                        eludite_commands::Outcome::Ok
                    } else {
                        eludite_commands::Outcome::Err("the tool call failed".into())
                    },
                    eludite_commands::Caller::Agent {
                        agent: agent.clone(),
                        call: eludite_commands::next_call_id(),
                        tool_call: Some(id.clone()),
                    },
                    input,
                );
                w.transcript.set_audit(&id, seq);
            }
        });
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
}

#[cfg(test)]
pub(in crate::shell) mod tests;

/// Where a link in an agent's message leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Link {
    /// `http`, `https` or `mailto`: the person's browser or mail client.
    Web(String),
    /// A file, at `line` (1 when the link names none).
    File { path: PathBuf, line: u32 },
    /// Any other scheme: not followed.
    Unsupported,
}

/// Resolve a link's target as an agent writes it: a URL, `file://`, or a path (absolute, or relative to the workspace
/// `root`) with the line as `#L42`, `#L42-L50` or `:42` (`:42:7`).
pub(crate) fn resolve_link(url: &str, root: Option<&Path>) -> Link {
    let url = url.trim();
    let lower = url.to_ascii_lowercase();
    if ["http://", "https://", "mailto:"]
        .iter()
        .any(|s| lower.starts_with(s))
    {
        return Link::Web(url.to_owned());
    }
    let path = match url.get(..7) {
        Some(p) if p.eq_ignore_ascii_case("file://") => &url[7..],
        _ => {
            // Another scheme (`javascript:`, `vscode:`), but not a Windows drive (`C:\`).
            let scheme = url.split_once(':').map(|(s, _)| s);
            if scheme.is_some_and(|s| {
                s.len() > 1
                    && s.chars()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
            }) {
                return Link::Unsupported;
            }
            url
        }
    };
    let (path, fragment) = path.split_once('#').unwrap_or((path, ""));
    let mut line = fragment
        .strip_prefix('L')
        .and_then(|l| l.split('-').next()?.parse().ok());
    let mut path = percent_decode(path);
    // `a.rs:42` or `a.rs:42:7`: the line is the first number after the path.
    for _ in 0..2 {
        if let Some((head, tail)) = path.rsplit_once(':')
            && !tail.is_empty()
            && tail.chars().all(|c| c.is_ascii_digit())
            && !head.is_empty()
        {
            line = tail.parse().ok().or(line);
            path = head.to_owned();
        }
    }
    if path.is_empty() {
        return Link::Unsupported;
    }
    let path = PathBuf::from(path);
    let path = match root {
        Some(root) if path.is_relative() => root.join(path),
        _ => path,
    };
    Link::File {
        path,
        line: line.unwrap_or(1).max(1),
    }
}

/// `%20` and the like in a link's path.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        let hex = |b: u8| (b as char).to_digit(16);
        if bytes[i] == b'%'
            && let (Some(Some(h)), Some(Some(l))) = (
                bytes.get(i + 1).map(|&b| hex(b)),
                bytes.get(i + 2).map(|&b| hex(b)),
            )
        {
            out.push((h * 16 + l) as u8);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod link_tests {
    use super::{Link, resolve_link};
    use std::path::{Path, PathBuf};

    fn file(path: &str, line: u32) -> Link {
        Link::File {
            path: PathBuf::from(path),
            line,
        }
    }

    #[test]
    fn links_resolve_to_web_pages_or_files_at_their_line() {
        let root = Some(Path::new("/ws"));
        assert_eq!(
            resolve_link("https://example.org/a#b", root),
            Link::Web("https://example.org/a#b".into())
        );
        assert_eq!(
            resolve_link("mailto:a@b.c", root),
            Link::Web("mailto:a@b.c".into())
        );
        assert_eq!(resolve_link("src/a.rs", root), file("/ws/src/a.rs", 1));
        assert_eq!(resolve_link("src/a.rs#L42", root), file("/ws/src/a.rs", 42));
        assert_eq!(
            resolve_link("src/a.rs#L42-L50", root),
            file("/ws/src/a.rs", 42)
        );
        assert_eq!(resolve_link("src/a.rs:42", root), file("/ws/src/a.rs", 42));
        assert_eq!(
            resolve_link("src/a.rs:42:7", root),
            file("/ws/src/a.rs", 42)
        );
        assert_eq!(
            resolve_link("/abs/My%20File.cs", root),
            file("/abs/My File.cs", 1)
        );
        assert_eq!(
            resolve_link("file:///abs/b.cs#L3", root),
            file("/abs/b.cs", 3)
        );
        assert_eq!(resolve_link("a.rs", None), file("a.rs", 1));
        assert_eq!(resolve_link("javascript:alert(1)", root), Link::Unsupported);
        assert_eq!(resolve_link("vscode://file/x", root), Link::Unsupported);
        assert_eq!(resolve_link("#anchor", root), Link::Unsupported);
    }
}
