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
//! - **Slash commands** (brief 0057). The agent's `available_commands_update` is kept in the transcript model, listed
//!   in the state output's `commands` and offered by the prompt box's slash menu; a new session starts with none.
//! - **Model, effort and mode** (brief 0058). The session's ACP modes and config options are kept here, listed in the
//!   state output's `mode` and `options`, shown by the pickers under the prompt box and named in the status bar.
//!   `eludite.agents.configure` checks the choice, has the session send `session/set_mode` or
//!   `session/set_config_option` on its own thread, and answers once the agent has (from another thread the caller
//!   waits for that answer: [`JobReply`]; from the UI thread it returns at once and the picker shows the pick muted
//!   until then). A pick made in the window is remembered in the settings `agents.model` and `agents.effort` (user
//!   scope), which the next `session/new` passes in `_meta.claudeCode.options`; the mode is not remembered.
//! - **The log** (brief 0059). The Output window's Agents source has one line per start (with the command line),
//!   ready (the agent's version, the ACP version and the MCP endpoint, which the window shows only as the state's
//!   tooltip), login state, prompt, turn end (its stop reason and duration), error and exit, and every line the agent
//!   writes to stderr (also to Eludite's stderr while `ELUDITE_AGENT_STDERR` is set). The transcript says how a turn
//!   ended only when it did not end normally (`end_turn`): "Stopped" after a cancel, the other stop reasons in words.
//! - **OpenAI-compatible servers** (brief 0060, [`providers`]). `agents.json`'s `providers` are registry entries
//!   (source `provider`) run through `eludite-openai-acp`, kept whether or not `agents.custom` is set; their keys are
//!   read from the credential store off the UI thread with each registry search and handed to the agent's
//!   environment at start; without the adapter an entry starts in the `error` state ("eludite-openai-acp was not
//!   found"). `eludite.agents.provider_set`, `provider_remove` and `provider_models` are registered here
//!   ([`providers::ProviderStore`]), as are `eludite.file.read` (an open document's unsaved text, else the file) and
//!   `eludite.file.edit` (one `eludite.workspace.apply_edit` under the calling agent, so it is a pending change
//!   reviewed like any other), both answered off the UI thread. The agent picker ends with "Add server…", which opens
//!   [`providers::ProviderDialog`]; Tools > Options > Agents lists the servers with Edit and Remove.
//! - **Sessions** (brief 0061). `Agents` keeps every session at once: the shown one's state is its [`Slot`] (the
//!   fields above are reached through it), the others wait in `parked` with what the window shows of them
//!   ([`window::SessionView`]). Each live session has its own agent process, generation, MCP endpoint (from a pool, so
//!   its calls are audited under its own agent and its prompts reach its own transcript), policy grants, waiting
//!   permissions, modes and options. Events of a session that is not shown are applied with it swapped in for the
//!   moment ([`Shell::with_parked`]), so they keep filling its transcript off screen. At most
//!   [`sessions::LIVE_LIMIT`] are live; a new one past that stops the oldest idle one. Every session with a prompt is
//!   kept in the person's per-workspace state ([`sessions`]), written when a turn ends, a permission is answered or
//!   the session stops, and at most every 2 s otherwise; the history list (the clock in the header) and
//!   `eludite.agents.sessions` list them, `eludite.agents.switch` shows one (a stored one rebuilt from its record and,
//!   when its agent advertises `loadSession`, resumed with ACP `session/load`, the agent's replay counted and
//!   discarded), and `eludite.agents.new_session` starts another while the shown one keeps running.

pub mod endpoint;
pub mod providers;
pub mod review;
#[cfg(test)]
pub(in crate::shell) mod scenario;
pub mod sessions;
pub mod transcript;
pub mod window;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak, mpsc};
use std::time::{Duration, Instant};

use eludite_acp::protocol::{
    Implementation, PermissionOptionKind, RequestPermissionRequest, SessionConfigOption,
    SessionModeState,
};
use eludite_acp::session::StreamConnector;
use eludite_acp::{
    AdapterSearch, AgentSession, AgentSettings, AgentSource, AgentState, LoginMethod,
    PermissionPolicy, PolicyAnswer, RegisteredAgent, SessionConfig, SessionEvent,
};
use eludite_commands::agents::ProviderRow;
use eludite_commands::files::{self, FILE_EDIT, FILE_READ, FilesTarget};
use eludite_commands::policy::{AgentPolicy, AlwaysAllow, PolicySnapshot, Verdict};
use eludite_commands::{CallClass, CommandRegistry, PermissionClass};
use eludite_forge::credentials::Credentials;
use eludite_mcp::{GateDecision, ToolCallRecord, command_id_from_tool_name, tool_name};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AppContext as _, Context, Entity, Window};
use serde_json::{Value, json};

use self::endpoint::{EndpointHooks, MCP_SERVER_NAME, McpEndpoint};
use self::providers::{ProviderKeys, ProviderStore};
use self::review::{DiffView, GutterMarkers, PendingChange, ReviewBoard};
use self::sessions::{SessionId, SessionMeta, SessionStore, StoreEvent};
use self::transcript::{ImageData, McpLink, Permission, Thumb};
use self::window::{
    AgentsWindow, AgentsWindowEvent, Decision, HeaderState, HistoryRow, SessionView, StateKind,
};
use super::Shell;

/// `eludite.agents.*` from another thread (an outer agent), for the UI thread to apply.
pub struct AgentsJob {
    pub request: eludite_commands::agents::AgentsRequest,
    pub reply: JobReply,
}

pub type AgentsOutcome =
    Result<eludite_commands::agents::AgentsOutput, eludite_commands::CommandError>;

/// What the UI thread answers a job: the outcome, or (`eludite.agents.configure`, brief 0058) where the outcome will
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

/// What the transcript says, and the prompt box in place of the text, when a stored session's agent cannot resume it.
pub const CANNOT_RESUME: &str = "This agent cannot resume a session; start a new one to continue";

/// How often a session's record is written while it changes (brief 0061), besides at a turn's end, a permission
/// answered and the session's stop.
const WRITE_EVERY: Duration = Duration::from_secs(2);

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
    /// `agents.json`, where the OpenAI-compatible servers are kept (brief 0060); `None`: no servers.
    pub agents_file: Option<PathBuf>,
    /// Where the servers' keys are kept (the operating system's store and the consented file; tests: memory).
    pub credentials: Arc<Credentials>,
    /// `eludite-openai-acp` when known up front (tests), else found like the Claude adapter
    /// (`ELUDITE_OPENAI_ACP`, beside the executable, `PATH`).
    pub openai_adapter: Option<PathBuf>,
    /// Where the per-workspace state folders are (`<config dir>/eludite/workspaces`, brief 0047), whose
    /// `agents/sessions` keep the sessions (brief 0061); `None`: nothing is kept (the shell's tests other than the
    /// Agents window's).
    pub sessions_root: Option<PathBuf>,
}

impl AgentsSetup {
    pub fn from_env() -> Self {
        Self {
            registry: None,
            relay_exe: std::env::current_exe().unwrap_or_else(|_| PathBuf::from("eludite")),
            connect: None,
            preferred: None,
            transcript_out: None,
            agents_file: eludite_docking::eludite_config_dir()
                .map(|d| d.join(eludite_acp::settings::SETTINGS_FILE)),
            credentials: Arc::new(Credentials::system()),
            openai_adapter: None,
            // A test never writes into the person's own state.
            sessions_root: if cfg!(test) {
                None
            } else {
                eludite_docking::eludite_config_dir()
                    .map(|d| d.join(crate::settings::WORKSPACES_DIR))
            },
        }
    }

    /// Where `eludite-openai-acp` is looked for: the given one; with a fixed registry (tests) nowhere else, so a
    /// test never finds one installed on the machine.
    fn openai_search(&self) -> AdapterSearch {
        let mut search = if self.registry.is_some() {
            AdapterSearch::default()
        } else {
            AdapterSearch::from_env()
        };
        if let Some(p) = &self.openai_adapter {
            search.openai_configured = Some(p.clone());
        }
        search
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

/// A registry search, run off the UI thread.
struct RegistryJob {
    /// The agents settings (`None`: not applied yet, the defaults).
    config: RegistryConfig,
    /// The registry fixed up front (tests, the harness): kept, with the servers added after it.
    fixed: Option<Vec<RegisteredAgent>>,
    file: Option<PathBuf>,
    openai: AdapterSearch,
    credentials: Arc<Credentials>,
    select: Option<String>,
    seq: u64,
}

/// What a registry search found.
pub struct Searched {
    pub registry: Vec<RegisteredAgent>,
    /// The `agents.json` error, if any.
    pub error: Option<String>,
    /// The agent to select (`agents.default` when it changed).
    pub select: Option<String>,
    /// The servers' keys and rows (brief 0060).
    pub keys: ProviderKeys,
    pub providers: Vec<ProviderRow>,
    /// Which search this is: an older one arriving late is dropped.
    pub seq: u64,
}

impl RegistryJob {
    fn run(self) -> Searched {
        let (registry, error, settings) = match &self.fixed {
            Some(fixed) => fixed_registry(fixed, self.file.as_deref(), &self.openai),
            None => search_registry(&self.config, self.file.as_deref(), &self.openai),
        };
        let keys = providers::read_keys(&self.credentials, &settings.providers);
        Searched {
            providers: providers::rows(&settings, &keys),
            registry,
            error,
            select: self.select,
            keys,
            seq: self.seq,
        }
    }
}

/// The registry from the settings, the environment and the user's `agents.json` (its `providers` always; its
/// `agents` and `default` only without `agents.custom`), with the `agents.json` error, if any, and the settings read.
fn search_registry(
    config: &RegistryConfig,
    file: Option<&Path>,
    openai: &AdapterSearch,
) -> (Vec<RegisteredAgent>, Option<String>, AgentSettings) {
    let (mut settings, error) = providers::load_settings(file);
    if let Some(agents) = &config.custom {
        // `agents.custom` replaces the file's agents; its servers stay (brief 0060).
        settings.agents = agents.clone();
        settings.default = None;
    }
    if config.default.is_some() {
        settings.default = config.default.clone();
    }
    let mut search = AdapterSearch::from_env();
    search.configured = config.adapter.clone();
    if openai.openai_configured.is_some() {
        search.openai_configured = openai.openai_configured.clone();
    }
    (
        eludite_acp::settings::registry(&search, &settings),
        error,
        settings,
    )
}

/// A fixed registry with the servers of `file` after it (a server with an entry's name replaces it).
fn fixed_registry(
    fixed: &[RegisteredAgent],
    file: Option<&Path>,
    openai: &AdapterSearch,
) -> (Vec<RegisteredAgent>, Option<String>, AgentSettings) {
    let (settings, error) = providers::load_settings(file);
    let servers = AgentSettings {
        providers: settings.providers.clone(),
        ..AgentSettings::default()
    };
    let mut out = fixed.to_vec();
    for entry in eludite_acp::settings::registry(openai, &servers)
        .into_iter()
        .filter(|a| a.source == AgentSource::Provider)
    {
        match out.iter_mut().find(|a| a.name() == entry.name()) {
            Some(existing) => *existing = entry,
            None => out.push(entry),
        }
    }
    (out, error, settings)
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
pub const REVIEWED_COMMANDS: [&str; 4] = [
    eludite_commands::workspace::WORKSPACE_APPLY_EDIT,
    eludite_commands::workspace::EDITOR_RENAME,
    eludite_commands::workspace::EDITOR_APPLY_CODE_ACTION,
    // Brief 0060: one `workspace.apply_edit`, answered as it is.
    FILE_EDIT,
];

/// Keys of the endpoint's own permission requests (Eludite commands of class execute and dangerous), kept apart from
/// the agent's.
static NEXT_ASK: AtomicU64 = AtomicU64::new(1 << 40);

/// What reaches the UI thread from the agents' and the endpoint's threads.
pub enum HostMsg {
    Session(u64, Box<SessionEvent>),
    /// An MCP call of session `.0` (its endpoint's owner, brief 0061).
    Mcp(SessionId, Box<ToolCallRecord>),
    /// A searched registry.
    Registry(Box<Searched>),
    /// The servers in `agents.json` or their keys changed (brief 0060): search the registry again.
    Providers,
    /// `eludite.file.*` asks the UI for the workspace folder or an open document's text (brief 0060).
    Files(FilesJob),
    /// The MCP gate asks the user about an Eludite command of session `.0`; the answer goes to `reply`.
    Ask(SessionId, Box<GateAsk>),
    /// A tool call's images, decoded off the UI thread: (session, tool call id, thumbnails).
    Images(SessionId, String, Vec<Thumb>),
    /// The session store found or did something (brief 0061).
    Store(StoreEvent),
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

/// What `eludite.file.read` and `eludite.file.edit` need from the UI thread.
pub enum FilesJob {
    Root(mpsc::SyncSender<Option<PathBuf>>),
    Text(PathBuf, mpsc::SyncSender<Option<String>>),
}

/// The shell's [`FilesTarget`]: asks the UI for what lives there, applies through `eludite.workspace.apply_edit`
/// under the calling agent (so the edit is a pending change).
struct ShellFiles {
    tx: UnboundedSender<HostMsg>,
    commands: Weak<CommandRegistry>,
}

impl ShellFiles {
    fn ask<T>(&self, job: impl FnOnce(mpsc::SyncSender<T>) -> FilesJob) -> Option<T> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.tx.unbounded_send(HostMsg::Files(job(tx))).ok()?;
        rx.recv_timeout(Duration::from_secs(30)).ok()
    }
}

impl FilesTarget for ShellFiles {
    fn workspace_root(&self) -> Option<PathBuf> {
        self.ask(FilesJob::Root).flatten()
    }

    fn open_text(&self, path: &Path) -> Option<String> {
        let path = path.to_path_buf();
        self.ask(|tx| FilesJob::Text(path, tx)).flatten()
    }

    fn apply_edit(&self, input: Value) -> Result<Value, eludite_commands::CommandError> {
        let commands = self
            .commands
            .upgrade()
            .ok_or_else(|| eludite_commands::CommandError::Failed("the window is closed".into()))?;
        commands.invoke(eludite_commands::workspace::WORKSPACE_APPLY_EDIT, input)
    }
}

/// Register `eludite.file.read` and `eludite.file.edit` on `target`. They wait for the UI thread, so on it they
/// refuse (agents reach them through the MCP endpoint, off the UI thread).
fn register_files(registry: &CommandRegistry, target: Arc<ShellFiles>) {
    let ui_thread = std::thread::current().id();
    for id in files::ALL {
        let t = target.clone();
        registry.replace(files::spec(id), move |input| {
            if std::thread::current().id() == ui_thread {
                return Err(eludite_commands::CommandError::Failed(format!(
                    "{id} waits for the window, so it runs off the UI thread (agents call it through Eludite's MCP \
                     endpoint)"
                )));
            }
            if id == FILE_READ {
                let req = files::parse_read(input)?;
                Ok(serde_json::to_value(files::read(t.as_ref(), &req)?).expect("outputs serialize"))
            } else {
                files::edit(t.as_ref(), &files::parse_edit(input)?)
            }
        });
    }
}

/// An `eludite.agents.configure` waiting for the agent (brief 0058).
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

/// Who an MCP endpoint of the pool serves (brief 0061): its calls are that session's, audited under its agent and
/// judged by its policy.
struct EndpointOwner {
    session: SessionId,
    agent: Arc<Mutex<String>>,
    policy: SharedPolicy,
}

type Owner = Arc<Mutex<EndpointOwner>>;

/// One session's state in the shell (brief 0061): the shown session's is `Agents::slot`, reached through `Agents`'
/// `Deref`, so `self.agents.state` is the shown session's state.
pub struct Slot {
    /// `None` only for the empty slot shown before the first session.
    pub meta: Option<SessionMeta>,
    session: Option<AgentSession>,
    pub generation: u64,
    pub state: StateKind,
    detail: String,
    login: Vec<LoginMethod>,
    pub session_id: Option<String>,
    pub agent_info: Option<String>,
    pub protocol: Option<u16>,
    pub last_stop: Option<String>,
    /// The session's agent's name, for the endpoint's audit records.
    current: Arc<Mutex<String>>,
    /// When the session's agent was started, and its milestones (ms).
    pub started: Option<Instant>,
    pub timings: Vec<(&'static str, f64)>,
    /// When the session became ready, since `started`.
    pub ready_ms: Option<f64>,
    /// When the running turn's prompt was sent (brief 0059: its duration goes to the log).
    turn_started: Option<Instant>,
    /// The solution's policy, with this session's "Allow for this session" grants.
    policy: SharedPolicy,
    /// Permission requests waiting for the user, by key.
    waiting: HashMap<u64, Waiting>,
    /// The session's modes and config options (brief 0058).
    pub modes: Option<SessionModeState>,
    pub config_options: Vec<SessionConfigOption>,
    /// Configure requests waiting for the agent.
    configuring: Vec<Configuring>,
    /// Its MCP endpoint, from the pool, while its agent runs.
    endpoint: Option<(McpEndpoint, Owner)>,
    /// What the window shows of it while it is not shown.
    view: SessionView,
    /// The store folder its record goes to (the workspace's when it started).
    store_dir: Option<PathBuf>,
    /// When its record was last written, and whether it changed since.
    last_write: Option<Instant>,
    dirty: bool,
    /// Its transcript came from Eludite's record (a stored session shown again): an agent's replay is not needed.
    pub from_record: bool,
    /// Waiting for its record to load before its agent resumes it.
    awaiting_record: bool,
    /// What the agent replayed on `session/load` (counted; discarded when there is a record).
    pub replayed: usize,
    /// The agent is resuming it (`session/load` in flight).
    resuming: bool,
}

impl Slot {
    fn new(meta: Option<SessionMeta>, store_dir: Option<PathBuf>, view: SessionView) -> Self {
        Self {
            meta,
            session: None,
            generation: 0,
            state: StateKind::Stopped,
            detail: String::new(),
            login: Vec::new(),
            session_id: None,
            agent_info: None,
            protocol: None,
            last_stop: None,
            current: Arc::default(),
            started: None,
            timings: Vec::new(),
            ready_ms: None,
            turn_started: None,
            policy: Arc::new(Mutex::new(Arc::new(PolicyStore::default()))),
            waiting: HashMap::new(),
            modes: None,
            config_options: Vec::new(),
            configuring: Vec::new(),
            endpoint: None,
            view,
            store_dir,
            last_write: None,
            dirty: false,
            from_record: false,
            awaiting_record: false,
            replayed: 0,
            resuming: false,
        }
    }

    /// Its agent process runs.
    pub fn live(&self) -> bool {
        self.session.is_some() && !matches!(self.state, StateKind::Stopped | StateKind::Error)
    }

    /// Note activity now (the history list sorts by it).
    fn touch(&mut self) {
        if let Some(m) = &mut self.meta {
            m.last_activity = sessions::now();
        }
        self.dirty = true;
    }
}

pub struct Agents {
    pub window: Entity<AgentsWindow>,
    setup: AgentsSetup,
    /// The agents settings the registry was last searched with.
    registry_config: Option<RegistryConfig>,
    pub registry: Vec<RegisteredAgent>,
    pub selected: usize,
    /// The shown session's state (brief 0061); `Deref` reaches it.
    slot: Slot,
    /// Its id (`None` before the first session).
    pub shown: Option<SessionId>,
    /// The other sessions in memory (live, or stopped this run), with what the window shows of them.
    parked: BTreeMap<SessionId, Slot>,
    /// Which session each live generation belongs to.
    gens: HashMap<u64, SessionId>,
    next_generation: u64,
    /// The store, the workspace's folder in it, and the records it holds (newest first).
    store: SessionStore,
    store_dir: Option<PathBuf>,
    stored: Vec<SessionMeta>,
    /// MCP endpoints no live session uses (brief 0061).
    free_endpoints: Vec<(McpEndpoint, Owner)>,
    _watch: Option<gpui::Subscription>,
    tx: UnboundedSender<HostMsg>,
    /// `--bench-agent-stream`: per batch, the UI time to apply it, its size, and each chunk's send-to-apply time.
    probe: Option<(Vec<f64>, Vec<usize>, Vec<f64>)>,
    /// Pending changes by id (decided ones stay, for the transcript's links).
    pub changes: BTreeMap<u64, PendingChange>,
    next_change: u64,
    /// Where the endpoint's call threads wait for the review.
    pub board: Arc<ReviewBoard>,
    pub reviews: Reviews,
    pub gutters: Gutters,
    /// Changes whose review view was opened once by itself.
    pub opened_once: Vec<u64>,
    /// The configure about to run came from a picker in the window (remembered in the settings).
    picking: bool,
    /// The servers' keys, read with the last registry search (brief 0060), and the servers.
    keys: ProviderKeys,
    pub providers: Vec<ProviderRow>,
    /// The last registry search started, and the last one applied.
    search_seq: u64,
    applied_seq: u64,
    /// The servers' list on Tools > Options > Agents, while the dialog has it.
    pub providers_page: Option<Entity<providers::ProvidersPage>>,
}

impl std::ops::Deref for Agents {
    type Target = Slot;
    fn deref(&self) -> &Slot {
        &self.slot
    }
}

impl std::ops::DerefMut for Agents {
    fn deref_mut(&mut self) -> &mut Slot {
        &mut self.slot
    }
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
        self.registry_config = Some(config);
        self.search(select, cx);
    }

    /// Search the registry again off the UI thread (the settings or the servers changed). A fixed registry keeps
    /// its entries and gains the servers of `agents.json`.
    pub fn search(&mut self, select: Option<String>, cx: &mut Context<Shell>) {
        self.search_seq += 1;
        let job = RegistryJob {
            config: self.registry_config.clone().unwrap_or_default(),
            fixed: self.setup.registry.clone(),
            file: self.setup.agents_file.clone(),
            openai: self.setup.openai_search(),
            credentials: self.setup.credentials.clone(),
            select,
            seq: self.search_seq,
        };
        let tx = self.tx.clone();
        cx.background_spawn(async move {
            let _ = tx.unbounded_send(HostMsg::Registry(Box::new(job.run())));
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
        // `eludite.file.*` and `eludite.agents.provider_*` (brief 0060) answer through this module; registered once
        // the shell exists (right after this returns), before any agent can call them.
        let shell = cx.weak_entity();
        let (files_tx, store) = (
            tx.clone(),
            ProviderStore {
                file: setup.agents_file.clone(),
                credentials: setup.credentials.clone(),
                search: setup.openai_search(),
                notify: tx.clone(),
                lock: Mutex::new(()),
            },
        );
        cx.defer(move |cx| {
            let _ = shell.update(cx, |s, _| {
                register_files(
                    &s.commands,
                    Arc::new(ShellFiles {
                        tx: files_tx,
                        commands: Arc::downgrade(&s.commands),
                    }),
                );
                eludite_commands::agents::register_providers(&s.commands, Arc::new(store));
            });
        });
        // Brief 0061: the session store's thread reports through the pump; the store folder follows the workspace.
        let store = {
            let tx = tx.clone();
            SessionStore::start(Arc::new(move |e| {
                let _ = tx.unbounded_send(HostMsg::Store(e));
            }))
        };
        let watch = setup
            .sessions_root
            .is_some()
            .then(|| cx.observe_self(|shell, cx| shell.agents_watch_workspace(cx)));
        // Without a fixed registry, the shell's first settings pass searches it (`set_registry_config`).
        (
            Self {
                window,
                setup,
                registry_config: None,
                registry,
                selected: 0,
                slot: Slot::new(None, None, SessionView::default()),
                shown: None,
                parked: BTreeMap::new(),
                gens: HashMap::new(),
                next_generation: 0,
                store,
                store_dir: None,
                stored: Vec::new(),
                free_endpoints: Vec::new(),
                _watch: watch,
                tx,
                probe: None,
                changes: BTreeMap::new(),
                next_change: 1,
                board: Arc::default(),
                reviews: Rc::default(),
                gutters: Rc::default(),
                opened_once: Vec::new(),
                picking: false,
                keys: ProviderKeys::default(),
                providers: Vec::new(),
                search_seq: 0,
                applied_seq: 0,
                providers_page: None,
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
        self.slot.endpoint.as_ref().map(|(e, _)| e)
    }

    fn header(&self) -> HeaderState {
        HeaderState {
            agents: self.registry.iter().map(|a| a.name().to_owned()).collect(),
            selected: self.selected,
            state: self.state,
            detail: self.detail.clone(),
            login: self.login.clone(),
            session: self.meta.as_ref().map(|m| (m.id.clone(), m.title.clone())),
        }
    }

    /// A new generation, unique across sessions (events carry it).
    fn next_generation(&mut self) -> u64 {
        self.next_generation += 1;
        self.next_generation
    }

    /// The sessions in memory: the shown one first, then the parked ones.
    fn slots(&self) -> impl Iterator<Item = &Slot> {
        std::iter::once(&self.slot)
            .filter(|s| s.meta.is_some())
            .chain(self.parked.values())
    }

    /// The session holding permission request `key`.
    fn session_waiting_for(&self, key: u64) -> Option<SessionId> {
        self.slots()
            .find(|s| s.waiting.contains_key(&key))
            .and_then(|s| s.meta.as_ref().map(|m| m.id.clone()))
    }

    /// Every session, in memory or stored, newest first (by `last_activity`), for `eludite.agents.sessions` and the
    /// history list: the meta, and whether it is live, running a turn, and waiting for a permission answer.
    pub fn session_list(&self, cx: &gpui::App) -> Vec<(SessionMeta, bool, bool, bool)> {
        let shown_waiting = !self.window.read(cx).transcript.asked().is_empty();
        let mut rows: Vec<(SessionMeta, bool, bool, bool)> = self
            .slots()
            .filter_map(|s| {
                let meta = s.meta.clone()?;
                let shown = self.shown.as_ref() == Some(&meta.id);
                let waiting = !s.waiting.is_empty()
                    || if shown {
                        shown_waiting
                    } else {
                        !s.view.transcript.asked().is_empty()
                    };
                Some((meta, s.live(), s.state == StateKind::Running, waiting))
            })
            .collect();
        let known: HashSet<String> = rows.iter().map(|r| r.0.id.clone()).collect();
        rows.extend(
            self.stored
                .iter()
                .filter(|m| !known.contains(&m.id))
                .map(|m| (m.clone(), false, false, false)),
        );
        rows.sort_by(|a, b| b.0.last_activity.cmp(&a.0.last_activity));
        rows
    }

    /// The footer's pickers (brief 0058).
    pub fn pickers(&self) -> Vec<window::Picker> {
        window::pickers(self.modes.as_ref(), &self.config_options)
    }

    /// The model's name as the model picker shows it (brief 0058).
    fn model_name(&self) -> Option<String> {
        self.pickers()
            .into_iter()
            .find(|p| p.category.as_deref() == Some("model") || p.key == "model")
            .map(|model| model.name_of(&model.current))
    }

    /// The status bar text: the agent, its state, (brief 0058) the model's name and (brief 0061) the session's title
    /// once a prompt named it: `Claude Code: ready · Sonnet 5.5 · Fix the failing test`.
    pub fn status_text(&self) -> String {
        match (self.selected_agent(), self.state) {
            (_, StateKind::Stopped) | (None, _) => String::new(),
            (Some(a), s) => {
                let mut text = format!("{}: {}", a.name(), s.as_str().replace('_', " "));
                if let Some(model) = self.model_name() {
                    text.push_str(&format!(" \u{b7} {model}"));
                }
                if let Some(m) = self.meta.as_ref().filter(|m| m.prompted()) {
                    text.push_str(&format!(" \u{b7} {}", m.title));
                }
                text
            }
        }
    }
}

/// What the tests read of the sessions (brief 0061).
#[cfg(test)]
impl Agents {
    /// The workspace's store folder.
    pub fn store_dir(&self) -> Option<&Path> {
        self.store_dir.as_deref()
    }

    /// Session `id`'s state, shown or not.
    pub fn session_slot(&self, id: &str) -> Option<&Slot> {
        if self.shown.as_deref() == Some(id) {
            return Some(&self.slot);
        }
        self.parked.get(id)
    }

    /// Session `id`'s agent text, shown or not.
    pub fn session_text(&self, id: &str, cx: &gpui::App) -> Option<String> {
        if self.shown.as_deref() == Some(id) {
            return Some(self.window.read(cx).transcript.agent_message());
        }
        self.parked
            .get(id)
            .map(|s| s.view.transcript.agent_message())
    }

    /// How many rows session `id`'s transcript has, shown or not.
    pub fn session_rows(&self, id: &str, cx: &gpui::App) -> Option<usize> {
        if self.shown.as_deref() == Some(id) {
            return Some(self.window.read(cx).transcript.rows.len());
        }
        self.parked.get(id).map(|s| s.view.transcript.rows.len())
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

    /// One line in the Output window's Agents source (brief 0059).
    pub(super) fn agents_log(&mut self, line: &str, cx: &mut Context<Self>) {
        let text = format!("{line}\n");
        self.output.update(cx, |o, cx| {
            o.append(eludite_commands::build::OutputSource::Agents, &text, cx)
        });
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

    /// Start or restart the shown session's agent (or `agent`), unless it runs and `restart` is false (brief 0061's
    /// meaning of `eludite.agents.start`): with no session shown, a new one; with `agent` another than the shown
    /// session's, that session stops and a new one starts with `agent`; a stored session its agent cannot resume is
    /// replaced by a new session with the same agent.
    pub fn agents_start(
        &mut self,
        agent: Option<&str>,
        restart: bool,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let index = |agents: &Agents, name: &str| {
            agents
                .registry
                .iter()
                .position(|a| a.name() == name)
                .ok_or_else(|| format!("no agent named `{name}`"))
        };
        if let Some(name) = agent {
            index(&self.agents, name)?;
        }
        let Some(meta) = self.agents.meta.clone() else {
            return self.agents_new_session(agent, cx);
        };
        if let Some(name) = agent.filter(|n| *n != meta.agent) {
            self.stop_session(cx);
            return self.agents_new_session(Some(name), cx);
        }
        if self.agents.window.read(cx).disabled.is_some() {
            if !restart {
                return Ok(());
            }
            return self.agents_new_session(Some(&meta.agent), cx);
        }
        if let Ok(ix) = index(&self.agents, &meta.agent) {
            self.agents.selected = ix;
        }
        if self.agents.live() && !restart {
            return Ok(());
        }
        self.stop_session(cx);
        let Some(agent) = self.agents.selected_agent().cloned() else {
            return Err("no agent is configured".into());
        };
        self.launch(agent, None, cx)
    }

    /// A new session with `agent` (else the selected one), shown at once; the one shown before keeps running
    /// (brief 0061). Past [`sessions::LIVE_LIMIT`] live sessions the oldest idle one stops first.
    pub fn agents_new_session(
        &mut self,
        agent: Option<&str>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let ix = match agent {
            Some(name) => self
                .agents
                .registry
                .iter()
                .position(|a| a.name() == name)
                .ok_or_else(|| format!("no agent named `{name}`"))?,
            None => self.agents.selected,
        };
        let Some(agent) = self.agents.registry.get(ix).cloned() else {
            return Err("no agent is configured".into());
        };
        self.make_room(cx)?;
        let meta = SessionMeta::new(agent.name());
        let id = meta.id.clone();
        let dir = self.agents_store_dir();
        self.agents.parked.insert(
            id.clone(),
            Slot::new(Some(meta), dir, SessionView::default()),
        );
        self.show_session(&id, cx);
        self.agents.selected = ix;
        self.launch(agent, None, cx)
    }

    /// Stop the oldest idle live session when [`sessions::LIVE_LIMIT`] are live (its record stays).
    fn make_room(&mut self, cx: &mut Context<Self>) -> Result<(), String> {
        let live: Vec<&Slot> = self.agents.slots().filter(|s| s.live()).collect();
        if live.len() < sessions::LIVE_LIMIT {
            return Ok(());
        }
        let idle = live
            .iter()
            .filter(|s| s.state != StateKind::Running && s.waiting.is_empty())
            .filter_map(|s| s.meta.as_ref())
            .min_by(|a, b| a.last_activity.cmp(&b.last_activity))
            .map(|m| m.id.clone())
            .ok_or_else(|| {
                format!(
                    "{} sessions are running a turn or waiting for an answer; stop one first",
                    sessions::LIVE_LIMIT
                )
            })?;
        super::documents::trace(format_args!("agents session {idle} stopped to make room"));
        if self.agents.shown.as_ref() == Some(&idle) {
            self.stop_session(cx);
        } else {
            self.with_parked(&idle, cx, |shell, cx| shell.stop_session(cx));
        }
        Ok(())
    }

    /// The store folder of the workspace open now (brief 0061), when sessions are kept.
    fn agents_store_dir(&self) -> Option<PathBuf> {
        let root = self.agents.setup.sessions_root.as_ref()?;
        let workspace = self
            .workspace_root()
            .or_else(|| std::env::current_dir().ok())?;
        Some(sessions::dir_for(root, &workspace))
    }

    /// The shell changed (anything): when the workspace did, read its stored sessions (brief 0061).
    fn agents_watch_workspace(&mut self, _cx: &mut Context<Self>) {
        let dir = self.agents_store_dir();
        if dir != self.agents.store_dir {
            self.agents.store_dir = dir.clone();
            self.agents.stored.clear();
            if let Some(dir) = dir {
                self.agents.store.scan(dir);
            }
        }
    }

    /// Show session `id` (in memory) in place of the shown one, which keeps running off screen (brief 0061). Returns
    /// whether `id` is in memory.
    fn show_session(&mut self, id: &SessionId, cx: &mut Context<Self>) -> bool {
        if self.agents.shown.as_ref() == Some(id) {
            self.agents.window.update(cx, |w, cx| w.scroll_to_end(cx));
            return true;
        }
        let Some(mut incoming) = self.agents.parked.remove(id) else {
            return false;
        };
        std::mem::swap(&mut self.agents.slot, &mut incoming);
        let mut view = std::mem::take(&mut self.agents.slot.view);
        self.agents
            .window
            .update(cx, |w, cx| w.show_view(&mut view, cx));
        incoming.view = view;
        if let Some(previous) = self.agents.shown.replace(id.clone()) {
            self.agents.parked.insert(previous, incoming);
        }
        if let Some(ix) = self.agents.meta.as_ref().and_then(|m| {
            self.agents
                .registry
                .iter()
                .position(|a| a.name() == m.agent)
        }) {
            self.agents.selected = ix;
        }
        self.reconcile_changes(cx);
        super::documents::trace(format_args!("agents session shown {id}"));
        self.sync_agents_header(cx);
        self.after_permission_change(cx);
        self.refresh_history(cx);
        true
    }

    /// Apply `f` to parked session `id` as if it were shown (brief 0061): its state and what the window shows of it
    /// are swapped in for the moment, so its events fill its own transcript off screen; then the shown session comes
    /// back. `None` when `id` is not parked.
    pub(super) fn with_parked<R>(
        &mut self,
        id: &SessionId,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Self, &mut Context<Self>) -> R,
    ) -> Option<R> {
        let mut other = self.agents.parked.remove(id)?;
        std::mem::swap(&mut self.agents.slot, &mut other);
        let mut view = std::mem::take(&mut self.agents.slot.view);
        self.agents
            .window
            .update(cx, |w, cx| w.swap_view(&mut view, cx));
        other.view = view;
        let shown = self.agents.shown.replace(id.clone());
        let selected = self.agents.selected;
        if let Some(ix) = self.agents.meta.as_ref().and_then(|m| {
            self.agents
                .registry
                .iter()
                .position(|a| a.name() == m.agent)
        }) {
            self.agents.selected = ix;
        }
        self.reconcile_changes(cx);
        let out = f(self, cx);
        let mut view = std::mem::take(&mut other.view);
        self.agents
            .window
            .update(cx, |w, cx| w.swap_view(&mut view, cx));
        std::mem::swap(&mut self.agents.slot, &mut other);
        other.view = view;
        self.agents.shown = shown;
        self.agents.selected = selected;
        self.agents.parked.insert(id.clone(), other);
        // The status bar may have shown the other session's state meanwhile.
        self.status.set(AGENTS_SLOT, self.agents.status_text());
        Some(out)
    }

    /// Bring the shown transcript's change links up to date: a change decided while its session was off screen.
    fn reconcile_changes(&mut self, cx: &mut Context<Self>) {
        let changes: Vec<(String, u64, String, &'static str)> = self
            .agents
            .changes
            .values()
            .filter_map(|c| {
                Some((
                    c.tool_call.clone()?,
                    c.id,
                    c.path.to_string_lossy().into_owned(),
                    c.state.label(),
                ))
            })
            .collect();
        if changes.is_empty() {
            return;
        }
        self.agents.window.update(cx, |w, cx| {
            let mut touched = false;
            for (tc, id, path, label) in changes {
                let stale = w.transcript.tool(&tc).is_some_and(|row| {
                    row.restored.is_none()
                        && !row.changes.iter().any(|(i, _, s)| *i == id && s == label)
                });
                if stale {
                    w.transcript.change(&tc, id, &path, label);
                    touched = true;
                }
            }
            if touched {
                w.sync(cx);
            }
        });
    }

    /// `eludite.agents.switch` (brief 0061): show session `id`. A stored one is shown from its record (loaded off the
    /// UI thread) and then resumed by its agent when it can.
    pub fn agents_switch(&mut self, id: &str, cx: &mut Context<Self>) -> Result<(), String> {
        let id = id.to_owned();
        if self.show_session(&id, cx) {
            return Ok(());
        }
        let meta = self
            .agents
            .stored
            .iter()
            .find(|m| m.id == id)
            .cloned()
            .ok_or_else(|| format!("no session `{id}` (see eludite.agents.sessions)"))?;
        let dir = self.agents.store_dir.clone();
        let mut slot = Slot::new(Some(meta.clone()), dir.clone(), SessionView::default());
        slot.awaiting_record = true;
        slot.from_record = true;
        self.agents.parked.insert(id.clone(), slot);
        self.show_session(&id, cx);
        self.agents.window.update(cx, |w, cx| {
            w.transcript
                .notice(format!("Loading \u{201c}{}\u{201d}\u{2026}", meta.title));
            w.sync(cx);
        });
        if let Some(dir) = dir {
            self.agents.store.load(dir, id);
        }
        Ok(())
    }

    /// A stored session's record arrived: its transcript replaces the loading notice, then its agent resumes it with
    /// `session/load` (or the window says it cannot).
    fn on_record_loaded(
        &mut self,
        id: &SessionId,
        loaded: Option<Box<(sessions::Record, transcript::Transcript)>>,
        cx: &mut Context<Self>,
    ) {
        if !self.agents.awaiting_record {
            return;
        }
        self.agents.awaiting_record = false;
        let Some(loaded) = loaded else {
            self.cannot_resume(
                "The session's record could not be read; start a new session to continue".into(),
                cx,
            );
            return;
        };
        let (record, transcript) = *loaded;
        let rows = transcript.rows.len();
        self.agents
            .window
            .update(cx, |w, cx| w.replace_transcript(transcript, cx));
        super::documents::trace(format_args!("agents session {id} rebuilt {rows} rows"));
        let agent = self
            .agents
            .registry
            .iter()
            .find(|a| a.name() == record.meta.agent)
            .cloned();
        match (agent, record.meta.acp_session_id.clone()) {
            (Some(agent), Some(acp)) => {
                if let Some(ix) = self
                    .agents
                    .registry
                    .iter()
                    .position(|a| a.name() == agent.name())
                {
                    self.agents.selected = ix;
                }
                if let Err(e) = self
                    .make_room(cx)
                    .and_then(|()| self.launch(agent, Some(acp), cx))
                {
                    self.cannot_resume(e, cx);
                }
            }
            (None, _) => self.cannot_resume(
                format!(
                    "{} is not in the agent list; start a new session to continue",
                    record.meta.agent
                ),
                cx,
            ),
            (Some(_), None) => self.cannot_resume(CANNOT_RESUME.into(), cx),
        }
        self.sync_agents_header(cx);
    }

    /// The shown session cannot continue (brief 0061): the transcript says why and the prompt box is disabled.
    fn cannot_resume(&mut self, why: String, cx: &mut Context<Self>) {
        self.agents_log(&format!("Cannot resume: {why}"), cx);
        if let Some(s) = self.agents.slot.session.take() {
            s.shutdown();
        }
        self.agents.state = StateKind::Stopped;
        self.agents.resuming = false;
        self.release_endpoint();
        self.agents.window.update(cx, |w, cx| {
            w.transcript.notice(why.clone());
            w.disabled = Some(why);
            w.sync(cx);
        });
        self.sync_agents_header(cx);
    }

    /// Give the shown session's endpoint back to the pool and forget its generation.
    fn release_endpoint(&mut self) {
        let g = self.agents.generation;
        self.agents.gens.remove(&g);
        if let Some(e) = self.agents.slot.endpoint.take() {
            self.agents.free_endpoints.push(e);
        }
    }

    /// Start `agent` in the shown session: `session/new`, or with `resume` its `session/load` of that ACP session
    /// (brief 0061).
    fn launch(
        &mut self,
        agent: RegisteredAgent,
        resume: Option<String>,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        // A server whose adapter is not installed (brief 0060): the error state says so, nothing is launched.
        if let Some(e) = agent.launch_error() {
            self.agents.state = StateKind::Error;
            self.agents.detail = e.to_owned();
            self.agents_log(&format!("Cannot start {}: {e}", agent.name()), cx);
            self.sync_agents_header(cx);
            return Err(e.to_owned());
        }
        let id = self.agents.shown.clone().ok_or("no session is shown")?;
        let cwd = self
            .solution_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        // The policy of the solution open now; read when the first request needs it, off the UI thread. Each session
        // has its own store, so its "Allow for this session" grants are its own.
        let policy: SharedPolicy = Arc::new(Mutex::new(Arc::new(PolicyStore::new(
            self.solution_dir().map(|d| AgentPolicy::path_for(&d)),
        ))));
        self.agents.policy = policy.clone();
        // Escalation hooks read the same policy (ADR-0009), on the calling thread, when a hook needs it.
        let shared = policy.clone();
        // With which processes this shell started, for `eludite.debug.attach`'s hook (brief 0027).
        let launched = self.debug.launched_processes();
        self.commands
            .set_policy_source(Arc::new(move || PolicySnapshot {
                launched: launched.clone(),
                ..current_policy(&shared).snapshot()
            }));
        *self
            .agents
            .current
            .lock()
            .unwrap_or_else(|e| e.into_inner()) = agent.name().to_owned();
        // The session's MCP endpoint, from the pool (brief 0061): its calls are this session's.
        let owner = EndpointOwner {
            session: id.clone(),
            agent: self.agents.current.clone(),
            policy: policy.clone(),
        };
        let endpoint = match self.agents.free_endpoints.pop() {
            Some((e, o)) => {
                *o.lock().unwrap_or_else(|e| e.into_inner()) = owner;
                (e, o)
            }
            None => {
                let o: Owner = Arc::new(Mutex::new(owner));
                let e = McpEndpoint::start(self.commands.clone(), self.endpoint_hooks(o.clone()))
                    .map_err(|e| format!("Eludite's MCP endpoint: {e}"))?;
                (e, o)
            }
        };
        let mcp = endpoint.0.acp_server(&self.agents.setup.relay_exe);
        self.agents.slot.endpoint = Some(endpoint);
        let generation = self.agents.next_generation();
        self.agents.generation = generation;
        self.agents.gens.insert(generation, id);
        // A server's key from the credential store, in the agent's environment for this launch only (brief 0060).
        let descriptor = match agent.source {
            AgentSource::Provider => eludite_acp::with_provider_key(
                agent.descriptor.clone(),
                self.agents.keys.get(agent.name()),
            ),
            _ => agent.descriptor.clone(),
        };
        let config = SessionConfig {
            agent: descriptor,
            connect: self.agents.setup.connect.clone(),
            cwd,
            mcp_servers: vec![mcp],
            client_info: Implementation {
                name: "eludite".into(),
                title: Some("Eludite".into()),
                version: eludite_commands::builtins::VERSION.into(),
            },
            handshake_timeout: HANDSHAKE_TIMEOUT,
            policy: acp_policy(self.commands.clone(), policy),
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
        self.agents.turn_started = None;
        self.agents.replayed = 0;
        self.agents_log(
            &format!("Starting {}: {}", agent.name(), agent.command_line()),
            cx,
        );
        let name = agent.name().to_owned();
        let resuming = resume.is_some();
        self.agents.resuming = resuming;
        if let Some(acp) = resume {
            // Brief 0061: the agent resumes its own session; its replay is counted (and discarded with a record).
            self.agents.slot.session = Some(AgentSession::resume(config, acp, generation, sink));
        } else {
            // The model and effort last picked in the window (brief 0058), for the agent to start with; not for a
            // server (brief 0060), whose models are its own and whose first is `defaultModel`.
            let meta = if agent.source == AgentSource::Provider {
                None
            } else {
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
            self.agents.slot.session = Some(AgentSession::start_with_meta(
                config, meta, generation, sink,
            ));
        }
        // The guides the session can read from Eludite's MCP server (brief 0027): MCP clients list them at start.
        let guides = eludite_mcp::resources::GUIDES
            .iter()
            .map(|g| format!("{} ({})", g.uri, g.title))
            .collect::<Vec<_>>()
            .join(", ");
        self.agents.window.update(cx, |w, cx| {
            if resuming {
                // The session's usage stays until the agent reports its own; its commands are the agent's to send.
                w.transcript.notice(format!(
                    "Resuming the session with {name} (Eludite's MCP resources for the agent: {guides})"
                ));
                w.transcript.commands.clear();
            } else {
                w.transcript.notice(format!(
                    "Starting {name} (Eludite's MCP resources for the agent: {guides})"
                ));
                // The new session's agent sends its own slash commands (brief 0057) and usage (brief 0059).
                w.transcript.new_session();
            }
            w.sync(cx);
        });
        self.sync_agents_header(cx);
        self.refresh_history(cx);
        Ok(())
    }

    /// Stop the shown session's agent (the process is killed) and forget its state; its record is written.
    fn stop_session(&mut self, cx: &mut Context<Self>) {
        let was_live = self.agents.slot.session.is_some();
        if let Some(s) = self.agents.slot.session.take() {
            s.cancel();
            s.shutdown();
        }
        self.release_endpoint();
        self.agents.state = StateKind::Stopped;
        self.agents.session_id = None;
        self.agents.detail.clear();
        self.agents.resuming = false;
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
        if was_live {
            self.agents.touch();
            self.persist(true, cx);
        }
        self.sync_agents_header(cx);
        self.refresh_history(cx);
    }

    /// Write the shown session's record (brief 0061): `now`, or when it changed and the last write is 2 s old.
    /// Nothing is written before a prompt; the record is serialized here and written by the store's thread.
    fn persist(&mut self, now: bool, cx: &mut Context<Self>) {
        let Some(meta) = self.agents.meta.clone().filter(|m| m.prompted()) else {
            return;
        };
        let Some(dir) = self.agents.slot.store_dir.clone() else {
            return;
        };
        if self.agents.awaiting_record {
            return;
        }
        let due = self
            .agents
            .last_write
            .is_none_or(|t| t.elapsed() >= WRITE_EVERY);
        if !now && !(self.agents.dirty && due) {
            return;
        }
        let w = self.agents.window.read(cx);
        let usage = w.usage_strip().map(|u| sessions::RecordUsage {
            used: u.used,
            size: u.size,
            cost: u
                .cost
                .map(|(amount, currency)| sessions::RecordCost { amount, currency }),
        });
        let record = sessions::Record {
            version: sessions::RECORD_VERSION,
            meta: meta.clone(),
            ended: !self.agents.live(),
            usage,
            transcript: w.transcript.to_json(),
        };
        self.agents.store.write(dir.clone(), record);
        self.agents.last_write = Some(Instant::now());
        self.agents.dirty = false;
        if self.agents.store_dir.as_ref() == Some(&dir) {
            match self.agents.stored.iter_mut().find(|m| m.id == meta.id) {
                Some(m) => *m = meta,
                None => self.agents.stored.push(meta),
            }
        }
    }

    /// The history list's rows and the state's `session` (brief 0061).
    pub(super) fn refresh_history(&mut self, cx: &mut Context<Self>) {
        let now = sessions::now();
        let offset = sessions::local_offset_minutes();
        let list = self.agents.session_list(cx);
        let more = list.len() > sessions::HISTORY_ROWS;
        let rows: Vec<HistoryRow> = list
            .into_iter()
            .take(sessions::HISTORY_ROWS)
            .map(|(m, _, running, waiting)| HistoryRow {
                current: self.agents.shown.as_ref() == Some(&m.id),
                when: sessions::relative(&m.last_activity, &now, offset),
                id: m.id,
                title: m.title,
                agent: m.agent,
                running,
                waiting,
            })
            .collect();
        self.agents
            .window
            .update(cx, |w, cx| w.set_history(rows, more, cx));
    }

    /// `eludite.agents.configure` (brief 0058): check the choice and have the session ask the agent. Returns the
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
        // A server's models are its own (brief 0060): its pick is not remembered for Claude Code's next start.
        let provider = a
            .selected_agent()
            .is_some_and(|x| x.source == AgentSource::Provider);
        let remember = if from_window && !picker.is_mode && !provider {
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
        // The history list names the session's model (brief 0061).
        let model = self.agents.model_name();
        if let Some(m) = self.agents.slot.meta.as_mut() {
            m.model = model;
        }
        let pickers = self.agents.pickers();
        // The screenshot driver waits for the pickers (brief 0058).
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
        // Brief 0061: a stored session its agent cannot resume takes no prompt; one still loading waits.
        if let Some(why) = self.agents.window.read(cx).disabled.clone() {
            return Err(why);
        }
        if self.agents.awaiting_record {
            return Err("the session is still loading; send the prompt when it is shown".into());
        }
        if self.agents.slot.session.is_none()
            || matches!(self.agents.state, StateKind::Error | StateKind::Stopped)
        {
            self.agents_start(None, false, cx)?;
        }
        let session = self.agents.slot.session.as_ref().expect("started");
        session.prompt(text);
        self.agents.state = StateKind::Running;
        self.agents.turn_started = Some(Instant::now());
        // The first prompt names the session (brief 0061).
        if let Some(m) = self.agents.slot.meta.as_mut()
            && !m.prompted()
        {
            m.title = sessions::title_of(text);
        }
        self.agents.touch();
        let first: String = text
            .chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .take(80)
            .collect();
        self.agents_log(&format!("Prompt: {first}"), cx);
        let text = text.to_owned();
        self.agents.window.update(cx, |w, cx| {
            w.transcript.user(&text);
            w.sync(cx);
        });
        self.persist(true, cx);
        self.sync_agents_header(cx);
        self.refresh_history(cx);
        Ok(())
    }

    /// Cancel the turn: `session/cancel`, every pending request answered `cancelled`, every pending change rejected.
    pub fn agents_cancel(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // The shown session's pending changes only (brief 0061): another session's turn goes on.
        let ids = self.session_changes(cx);
        if !ids.is_empty() {
            let _ = self.decide(&ids, false, window, cx);
        }
        let mut keys = self
            .agents
            .slot
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

    /// The pending changes of the shown session: its agent's own tool edits (by generation) and the edits its tool
    /// calls made through Eludite (by tool call id).
    fn session_changes(&self, cx: &gpui::App) -> Vec<u64> {
        let generation = self.agents.generation;
        let w = self.agents.window.read(cx);
        self.agents
            .changes
            .values()
            .filter(|c| c.state == review::ChangeState::Pending)
            .filter(|c| match &c.source {
                review::ChangeSource::AgentTool { generation: g, .. } => *g == generation,
                review::ChangeSource::Edit { .. } => c
                    .tool_call
                    .as_deref()
                    .is_some_and(|tc| w.transcript.tool(tc).is_some()),
            })
            .map(|c| c.id)
            .collect()
    }

    /// The session pending change `id` belongs to, when it is not the shown one: its agent's own tool edit by
    /// generation, an Eludite edit by the transcript holding its tool call.
    fn change_session(&self, id: u64) -> Option<SessionId> {
        let c = self.agents.changes.get(&id)?;
        match &c.source {
            review::ChangeSource::AgentTool { generation, .. } => self
                .agents
                .gens
                .get(generation)
                .filter(|s| self.agents.shown.as_ref() != Some(*s))
                .cloned(),
            review::ChangeSource::Edit { .. } => {
                let tc = c.tool_call.as_deref()?;
                self.agents
                    .parked
                    .iter()
                    .find(|(_, s)| s.view.transcript.tool(tc).is_some())
                    .map(|(id, _)| id.clone())
            }
        }
    }

    /// Accept or reject changes `ids` (`eludite.agents.review`), each with its own session swapped in (brief 0061),
    /// so an off-screen session's agent hears the answer and its transcript the outcome.
    fn decide_in_sessions(
        &mut self,
        ids: &[u64],
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let mut shown = Vec::new();
        let mut others: BTreeMap<SessionId, Vec<u64>> = BTreeMap::new();
        for &id in ids {
            match self.change_session(id) {
                Some(s) => others.entry(s).or_default().push(id),
                None => shown.push(id),
            }
        }
        let mut result = if shown.is_empty() && !others.is_empty() {
            Ok(())
        } else {
            self.decide(&shown, accept, window, cx)
        };
        for (session, ids) in others {
            let r = self
                .with_parked(&session, cx, |shell, cx| {
                    let r = shell.decide(&ids, accept, window, cx);
                    shell.persist(true, cx);
                    r
                })
                .unwrap_or_else(|| self.decide(&ids, accept, window, cx));
            if result.is_ok() {
                result = r;
            }
        }
        result
    }

    /// Answer permission request `key`: the agent's (through ACP) or the MCP gate's. Always Allow adds a rule to the
    /// solution's policy file (written off the UI thread) and allows this call. A request of a session that is not
    /// shown is answered in that session (brief 0061: keys are unique across sessions).
    pub fn agents_answer(
        &mut self,
        key: u64,
        decision: Decision,
        cx: &mut Context<Self>,
    ) -> Result<Answered, String> {
        if !self.agents.waiting.contains_key(&key)
            && let Some(other) = self.agents.session_waiting_for(key)
            && self.agents.shown.as_ref() != Some(&other)
        {
            return self
                .with_parked(&other, cx, |shell, cx| {
                    shell.agents_answer(key, decision, cx)
                })
                .unwrap_or_else(|| Err(format!("no pending permission request {key}")));
        }
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
                let session = self
                    .agents
                    .slot
                    .session
                    .as_ref()
                    .ok_or("no agent is running")?;
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
        // Brief 0061: an answer is written to the session's record at once.
        self.agents.touch();
        self.persist(true, cx);
        self.refresh_history(cx);
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
                    tool: row.tool_name(),
                    title: row.name(),
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

    /// The hooks of an MCP endpoint serving `owner`'s session (brief 0061: endpoints come from a pool, so the owner is
    /// read at each call).
    fn endpoint_hooks(&self, owner: Owner) -> EndpointHooks {
        let tx = self.agents.tx.clone();
        let gate_tx = tx.clone();
        let (name_of, gate_owner, record_owner) = (owner.clone(), owner.clone(), owner);
        let read = |o: &Owner| {
            let o = o.lock().unwrap_or_else(|e| e.into_inner());
            (o.session.clone(), o.agent.clone(), o.policy.clone())
        };
        EndpointHooks {
            agent: Arc::new(move || {
                let (_, agent, _) = read(&name_of);
                agent.lock().map(|c| c.clone()).unwrap_or_default()
            }),
            // On the endpoint's call thread: the policy, else ask the user and wait for the answer.
            gate: Arc::new(move |spec, args, ctx| {
                let (session, _, policy) = read(&gate_owner);
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
                if gate_tx
                    .unbounded_send(HostMsg::Ask(session, Box::new(ask)))
                    .is_err()
                {
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
                    // `file.edit` answers as the `workspace.apply_edit` it ran (brief 0060).
                    let id = match spec.id.as_str() {
                        FILE_EDIT => eludite_commands::workspace::WORKSPACE_APPLY_EDIT,
                        other => other,
                    };
                    Ok(review::amend_output(id, out, &decided))
                })
            }),
            observer: Some(Arc::new(move |record| {
                let (session, _, _) = read(&record_owner);
                let _ = tx.unbounded_send(HostMsg::Mcp(session, Box::new(record.clone())));
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
            ReviewOutput, ReviewTarget, SessionRow, SessionsOutput,
        };
        let failed = CommandError::Failed;
        DEFERRED.with(|d| d.borrow_mut().take());
        match request {
            AgentsRequest::Start {
                agent,
                restart,
                session,
            } => {
                if let Some(id) = session {
                    self.agents_switch(&id, cx).map_err(failed)?;
                }
                self.agents_start(agent.as_deref(), restart, cx)
                    .map_err(failed)?;
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            // Brief 0061: the history list, switching and a new session.
            AgentsRequest::Sessions => Ok(AgentsOutput::Sessions(SessionsOutput {
                current: self.agents.shown.clone(),
                sessions: self
                    .agents
                    .session_list(cx)
                    .into_iter()
                    .map(|(m, live, running, waiting)| SessionRow {
                        id: m.id,
                        agent: m.agent,
                        title: m.title,
                        started: m.started,
                        last_activity: m.last_activity,
                        model: m.model,
                        live,
                        running,
                        waiting,
                    })
                    .collect(),
            })),
            AgentsRequest::Switch { session } => {
                self.agents_switch(&session, cx).map_err(failed)?;
                Ok(AgentsOutput::State(self.agents_state(cx)))
            }
            AgentsRequest::NewSession { agent } => {
                self.agents_new_session(agent.as_deref(), cx)
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
                let message = self.decide_in_sessions(&ids, accept, window, cx).err();
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
            AgentRow, AgentsStateOutput, ChoiceRow, CommandRow, CostOutput, LoginRow, ModeOutput,
            ModeRow, OptionRow, SessionOutput, UsageOutput,
        };
        let a = &self.agents;
        AgentsStateOutput {
            agent: a
                .selected_agent()
                .map(|x| x.name().to_owned())
                .unwrap_or_default(),
            // The session shown (brief 0061).
            session: a.meta.as_ref().map(|m| SessionOutput {
                id: m.id.clone(),
                title: m.title.clone(),
                started: m.started.clone(),
                last_activity: m.last_activity.clone(),
                live: a.live(),
            }),
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
            // What the slash menu offers (brief 0057).
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
            // The session's mode and select options (brief 0058).
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
            // What the usage strip shows (brief 0059).
            usage: a.window.read(cx).usage_strip().map(|u| UsageOutput {
                used: u.used,
                size: u.size,
                cost: u
                    .cost
                    .map(|(amount, currency)| CostOutput { amount, currency }),
            }),
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
        self.reject_pending(window, cx);
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
            // A picker's choice (brief 0058): remembered in the settings once the agent took it.
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
            AgentsWindowEvent::AddServer => self.open_provider_dialog(None, window, cx),
            // Brief 0061: the history list and New session.
            AgentsWindowEvent::Switch(id) => self.run(
                eludite_commands::agents::SWITCH,
                json!({ "session": id }),
                window,
                cx,
            ),
            AgentsWindowEvent::NewSession { agent } => {
                let mut args = json!({});
                if let Some(a) = agent {
                    args["agent"] = json!(a);
                }
                self.run(eludite_commands::agents::NEW_SESSION, args, window, cx);
            }
            AgentsWindowEvent::HistoryOpened => self.refresh_history(cx),
            // A link opens a change this run knows only when the shown transcript's call made it (a restored record's
            // link names a change of an earlier run, whose number a new change may reuse).
            AgentsWindowEvent::OpenChange(id) => {
                let known = self.agents.changes.get(id).is_some_and(|c| {
                    c.tool_call.as_deref().is_some_and(|tc| {
                        self.agents
                            .window
                            .read(cx)
                            .transcript
                            .tool(tc)
                            .is_some_and(|row| {
                                row.restored.is_none()
                                    && row.changes.iter().any(|(i, _, _)| i == id)
                            })
                    })
                });
                if known {
                    self.open_change(*id, window, cx)
                }
            }
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
        let mut header = false;
        let window = self.agents.window.clone();
        // Brief 0061: each session's messages, in order, applied with that session swapped in when it is not shown.
        let mut groups: Vec<(SessionId, Vec<HostMsg>)> = Vec::new();
        for msg in batch {
            let session = match &msg {
                // A generation no session holds is a stopped or replaced agent's (CLAUDE.md invariant 12).
                HostMsg::Session(g, _) => match self.agents.gens.get(g) {
                    Some(s) => s.clone(),
                    None => continue,
                },
                HostMsg::Mcp(s, _) | HostMsg::Ask(s, _) | HostMsg::Images(s, _, _) => s.clone(),
                _ => {
                    match msg {
                        HostMsg::Registry(found) => {
                            if found.seq < self.agents.applied_seq {
                                continue;
                            }
                            self.agents.applied_seq = found.seq;
                            let Searched {
                                registry,
                                error,
                                select,
                                keys,
                                providers,
                                ..
                            } = *found;
                            self.agents.keys = keys;
                            // The screenshot driver waits for a saved server (brief 0060).
                            super::documents::trace(format_args!(
                                "agents registry {}",
                                registry
                                    .iter()
                                    .map(|a| a.name())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                            if self.agents.providers != providers {
                                self.agents.providers = providers.clone();
                                if let Some(page) = &self.agents.providers_page {
                                    page.update(cx, |p, cx| p.set_rows(providers, cx));
                                }
                            }
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
                                .and_then(|p| {
                                    self.agents.registry.iter().position(|a| a.name() == p)
                                })
                                .unwrap_or(0);
                            if let Some(e) = error {
                                window.update(cx, |w, _| w.transcript.error(e));
                            }
                            header = true;
                        }
                        HostMsg::Providers => self.agents.search(None, cx),
                        HostMsg::Files(job) => self.on_files_job(job, cx),
                        HostMsg::Store(e) => self.on_store_event(e, cx),
                        _ => {}
                    }
                    continue;
                }
            };
            match groups.iter_mut().find(|(s, _)| *s == session) {
                Some(g) => g.1.push(msg),
                None => groups.push((session, vec![msg])),
            }
        }
        for (session, msgs) in groups {
            if self.agents.shown.as_ref() == Some(&session) {
                self.apply_session_msgs(msgs, window_, cx);
            } else {
                self.with_parked(&session, cx, |shell, cx| {
                    shell.apply_session_msgs(msgs, window_, cx)
                });
            }
        }
        if header {
            self.sync_agents_header(cx);
        }
        self.refresh_history(cx);
    }

    /// The messages of the shown session (or one swapped in by [`Shell::with_parked`]).
    fn apply_session_msgs(
        &mut self,
        batch: Vec<HostMsg>,
        window_: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut header = false;
        let mut permissions = false;
        // Brief 0061: the record is written at once when a turn ends or the agent stops, else at most every 2 s.
        let mut now = false;
        let window = self.agents.window.clone();
        for msg in batch {
            match msg {
                HostMsg::Mcp(_, record) => {
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
                HostMsg::Images(_, row, thumbs) => {
                    window.update(cx, |w, _| w.transcript.add_thumbs(&row, thumbs));
                }
                HostMsg::Ask(_, ask) => {
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
                HostMsg::Session(_, event) => match *event {
                    SessionEvent::State(s) => {
                        header = true;
                        if matches!(s, AgentState::Exited | AgentState::Error(_)) {
                            self.agents.touch();
                            now = true;
                        }
                        self.on_agent_state(s, cx);
                    }
                    // Brief 0061: what the agent replays on `session/load`: counted, and discarded when Eludite's
                    // record of the session (richer: review links, audit numbers) is shown; else it builds the
                    // transcript.
                    SessionEvent::Replay(u) => {
                        self.agents.replayed += 1;
                        if !self.agents.from_record {
                            window.update(cx, |w, _| w.transcript.replay(&u));
                        }
                    }
                    SessionEvent::LoadUnsupported => {
                        header = true;
                        self.cannot_resume(CANNOT_RESUME.into(), cx);
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
                                .slot
                                .endpoint
                                .as_ref()
                                .map(|(e, _)| e.describe())
                                .unwrap_or_default()
                        );
                        // The login state's detail is the label the agent gave.
                        if self.agents.state != StateKind::NeedsLogin {
                            let name = self.agents.current_name();
                            self.agents_log(&format!("{name} is ready: {detail}"), cx);
                            self.agents.detail = detail;
                        }
                        // Brief 0061: the agent's id for the session, which a later `session/load` resumes.
                        if let Some(m) = self.agents.slot.meta.as_mut() {
                            m.acp_session_id = Some(session_id.clone());
                        }
                        if std::mem::take(&mut self.agents.resuming) {
                            let replayed = self.agents.replayed;
                            let what = if self.agents.from_record {
                                "discarded: Eludite's record is shown"
                            } else {
                                "shown: Eludite has no record of it"
                            };
                            super::documents::trace(format_args!(
                                "agents resumed {session_id}: replay {replayed} ({what})"
                            ));
                            self.agents_log(
                                &format!(
                                    "Resumed session {session_id}: the agent replayed {replayed} updates ({what})"
                                ),
                                cx,
                            );
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
                        self.agents.dirty = true;
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
                        self.agents.touch();
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
                        self.agents.touch();
                        now = true;
                        let took = self
                            .agents
                            .turn_started
                            .take()
                            .map_or(0., |t| t.elapsed().as_secs_f64());
                        // The transcript says how the turn ended only when it did not end normally (brief 0059).
                        let (log, row) = match &r {
                            Ok(stop) => {
                                let s = serde_json::to_value(stop)
                                    .ok()
                                    .and_then(|v| v.as_str().map(str::to_owned))
                                    .unwrap_or_default();
                                self.agents.last_stop = Some(s.clone());
                                (
                                    format!("Turn ended: {s} in {took:.1} s"),
                                    window::stop_notice(&s).map(Ok),
                                )
                            }
                            Err(e) => {
                                self.agents.last_stop = Some("error".into());
                                (
                                    format!("Turn ended: error in {took:.1} s: {e}"),
                                    Some(Err(format!("The turn failed: {e}"))),
                                )
                            }
                        };
                        self.agents_log(&log, cx);
                        window.update(cx, |w, _| {
                            w.transcript.end_turn();
                            match row {
                                Some(Ok(text)) => w.transcript.notice(text),
                                Some(Err(text)) => w.transcript.error(text),
                                None => {}
                            }
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
                        self.agents_log(&format!("[stderr] {line}"), cx);
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
                _ => {}
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
        if matches!(self.agents.state, StateKind::Error | StateKind::Stopped) && self.agents.dirty {
            now = true;
        }
        self.persist(now, cx);
    }

    /// Open the Add server dialog (brief 0060): for a new server, or to edit saved server `name`.
    pub fn open_provider_dialog(
        &mut self,
        name: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let theme = self.theme;
        let editing =
            name.and_then(|n| self.agents.providers.iter().find(|r| r.name == n).cloned());
        let probe = self.ui_bounds.clone();
        let dialog = cx.new(|cx| {
            let mut d = providers::ProviderDialog::new(theme, editing, cx);
            d.probe = probe;
            d
        });
        cx.subscribe_in(&dialog, window, Self::on_provider_dialog_event)
            .detach();
        dialog.update(cx, |d, cx| d.focus_first(window, cx));
        self.agents.window.update(cx, |w, cx| {
            w.provider_dialog = Some(dialog);
            cx.notify();
        });
        cx.notify();
    }

    /// The Add server dialog, while it is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn provider_dialog(&self, cx: &gpui::App) -> Option<Entity<providers::ProviderDialog>> {
        self.agents.window.read(cx).provider_dialog.clone()
    }

    fn close_provider_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.agents.window.update(cx, |w, cx| {
            w.provider_dialog = None;
            cx.notify();
        });
        self.focus.focus(window, cx);
        cx.notify();
    }

    /// Run provider command `id` through the bus off the UI thread (it touches the credential store or the network),
    /// then `done` with its answer on the UI thread.
    fn provider_command(
        &mut self,
        id: &'static str,
        args: Value,
        done: impl FnOnce(&mut Shell, Result<Value, String>, &mut Window, &mut Context<Shell>) + 'static,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.commands.clone();
        let task = cx.background_spawn(async move {
            commands.invoke(id, args).map_err(|e| {
                e.to_string()
                    .trim_start_matches("command failed: ")
                    .to_owned()
            })
        });
        cx.spawn_in(window, async move |this, cx| {
            let result = task.await;
            let _ = this.update_in(cx, |shell, window, cx| done(shell, result, window, cx));
        })
        .detach();
    }

    fn on_provider_dialog_event(
        &mut self,
        dialog: &Entity<providers::ProviderDialog>,
        event: &providers::ProviderDialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use eludite_commands::agents::{PROVIDER_MODELS, PROVIDER_SET};
        use providers::ProviderDialogEvent as E;
        let dialog = dialog.clone();
        match event {
            E::Test(args) => self.provider_command(
                PROVIDER_MODELS,
                args.clone(),
                move |_, result, _, cx| {
                    super::documents::trace(format_args!(
                        "agents provider test {}",
                        match &result {
                            Ok(v) => providers::test_summary(v).0,
                            Err(e) => e.clone(),
                        }
                    ));
                    dialog.update(cx, |d, cx| d.tested(result, cx));
                },
                window,
                cx,
            ),
            E::Save(args) => {
                let name = args["name"].as_str().unwrap_or_default().to_owned();
                let previous = dialog.read(cx).editing.clone();
                self.provider_command(
                    PROVIDER_SET,
                    args.clone(),
                    move |shell, result, window, cx| match result {
                        Ok(_) => {
                            // A rename keeps one entry: the old name goes, with its key.
                            if let Some(old) = previous.filter(|o| *o != name) {
                                shell.provider_command(
                                    eludite_commands::agents::PROVIDER_REMOVE,
                                    json!({ "name": old }),
                                    |_, _, _, _| {},
                                    window,
                                    cx,
                                );
                            }
                            shell.close_provider_dialog(window, cx);
                            shell.agents.search(Some(name.clone()), cx);
                            shell.status.set(
                                eludite_ui::slots::STATE,
                                format!("Saved the server {name}: start it from the Agents window's agent list"),
                            );
                        }
                        Err(e) => dialog.update(cx, |d, cx| d.save_failed(e, cx)),
                    },
                    window,
                    cx,
                );
            }
            E::Cancel => self.close_provider_dialog(window, cx),
        }
    }

    /// The servers' list for Tools > Options > Agents (brief 0060).
    pub(super) fn providers_options_page(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::AnyView {
        let theme = self.theme;
        let rows = self.agents.providers.clone();
        let page = cx.new(|_| providers::ProvidersPage::new(theme, rows));
        cx.subscribe_in(&page, window, Self::on_providers_page_event)
            .detach();
        self.agents.providers_page = Some(page.clone());
        page.into()
    }

    fn on_providers_page_event(
        &mut self,
        page: &Entity<providers::ProvidersPage>,
        event: &providers::ProvidersPageEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            // The dialog lives in the Agents window: Options closes and the window shows it.
            providers::ProvidersPageEvent::Edit(name) => {
                self.close_options(window, cx);
                let _ = self.commands.invoke(
                    "eludite.view.show",
                    json!({ "id": eludite_docking::ids::AGENTS }),
                );
                self.open_provider_dialog(name.as_deref(), window, cx);
            }
            providers::ProvidersPageEvent::Remove(name) => {
                let page = page.clone();
                self.provider_command(
                    eludite_commands::agents::PROVIDER_REMOVE,
                    json!({ "name": name }),
                    move |_, result, _, cx| {
                        page.update(cx, |p, cx| {
                            p.message = result.err();
                            cx.notify();
                        })
                    },
                    window,
                    cx,
                );
            }
        }
    }

    /// `eludite.file.*`'s questions (brief 0060): the workspace folder, or an open document's text (unsaved edits
    /// included).
    fn on_files_job(&mut self, job: FilesJob, cx: &mut Context<Self>) {
        match job {
            FilesJob::Root(reply) => {
                let _ = reply.send(self.workspace_root());
            }
            FilesJob::Text(path, reply) => {
                let path = super::documents::normalize_path(&path);
                let text = self
                    .documents
                    .values()
                    .find(|d| super::documents::normalize_path(&d.path) == path)
                    .map(|d| d.view.read(cx).editor().text());
                let _ = reply.send(text);
            }
        }
    }

    /// Decode `images` of tool call `row` off the UI thread; their thumbnails reach the row through the pump.
    fn decode_images(&self, row: String, images: Vec<ImageData>, cx: &mut Context<Self>) {
        if images.is_empty() {
            return;
        }
        // The session whose events are being applied (brief 0061).
        let Some(session) = self.agents.shown.clone() else {
            return;
        };
        let tx = self.agents.tx.clone();
        cx.background_spawn(async move {
            let thumbs: Vec<Thumb> = images.iter().filter_map(transcript::decode_thumb).collect();
            if !thumbs.is_empty() {
                let _ = tx.unbounded_send(HostMsg::Images(session, row, thumbs));
            }
        })
        .detach();
    }

    /// What the session store's thread found or did (brief 0061).
    fn on_store_event(&mut self, e: StoreEvent, cx: &mut Context<Self>) {
        match e {
            StoreEvent::Scanned(dir, metas) => {
                if self.agents.store_dir.as_ref() == Some(&dir) {
                    super::documents::trace(format_args!("agents sessions stored {}", metas.len()));
                    self.agents.stored = metas;
                    self.refresh_history(cx);
                }
            }
            StoreEvent::Loaded(id, loaded) => {
                if self.agents.shown.as_ref() == Some(&id) {
                    self.on_record_loaded(&id, loaded, cx);
                } else {
                    self.with_parked(&id, cx, |shell, cx| shell.on_record_loaded(&id, loaded, cx));
                }
            }
            StoreEvent::Pruned(dir, ids) => {
                if self.agents.store_dir.as_ref() == Some(&dir) {
                    self.agents.stored.retain(|m| !ids.contains(&m.id));
                    self.refresh_history(cx);
                }
            }
            StoreEvent::Failed(e) => self.agents_log(&format!("Cannot keep a session: {e}"), cx),
        }
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
        let name = self.agents.current_name();
        match s {
            AgentState::Starting => self.agents.state = StateKind::Starting,
            AgentState::Ready => self.agents.state = StateKind::Ready,
            AgentState::Running => self.agents.state = StateKind::Running,
            AgentState::NeedsLogin { label, methods } => {
                self.agents.state = StateKind::NeedsLogin;
                self.agents_log(&format!("{name} needs login: {label}"), cx);
                for m in &methods {
                    self.agents_log(&format!("  {}: {}", m.name, m.command), cx);
                }
                self.agents.detail = label;
                self.agents.login = methods;
            }
            AgentState::Error(e) => {
                self.agents.state = StateKind::Error;
                self.agents_log(&format!("{name} error: {e}"), cx);
                self.agents.detail = e.clone();
                window.update(cx, |w, _| w.transcript.error(e));
            }
            AgentState::Exited => {
                if self.agents.state != StateKind::Stopped {
                    self.agents.state = StateKind::Error;
                    self.agents_log(&format!("{name} exited"), cx);
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
