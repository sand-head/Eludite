//! The browser of `eludite.browser.*` (brief 0023): one browser per workspace, through `eludite-browser`.
//!
//! - **The `browser` worker.** Every browser command runs on one thread named `browser`, which owns the
//!   [`eludite_browser::Browser`] (the engine and the tabs). An agent's call arrives on the MCP thread, is handed to
//!   the worker and blocks there, as other commands block their caller. The UI thread never waits on Chrome
//!   (invariant 1): a browser command invoked on it fails at once; the Web Browser window (brief 0032,
//!   `browser_window`) invokes the same commands from the background executor.
//! - **Which engine** (brief 0032). The setting `browser.engine`: `embedded` (the default) is `eludite-chromium` when
//!   it and CEF are found ([`eludite_browser::select_engine`]), drawn in the Web Browser window; else, or with
//!   `external`, the external Chrome of brief 0023. The worker makes a new engine when the choice changes. The
//!   embedded engine's notifications reach the window as [`WindowEvent`]s through [`BrowserBus::window_events`];
//!   a download's end also writes a line to the Output window.
//! - **Who drives** (brief 0032). An agent's call in flight is announced to the window ([`WindowEvent::Driving`]), whose
//!   "Agent is driving" strip offers Stop. The person's hand ([`BrowserBus::person_acted`], [`BrowserBus::stop`])
//!   ends the agent's `wait` (`interrupted_by: "user"`) through the shared [`Interrupt`] and marks the agent stale:
//!   its next action command is refused until it reads `eludite.browser.tabs` again.
//! - **The sandbox** (brief 0039). The embedded engine decides how Chromium's sandbox runs and refuses where it cannot
//!   start; its refusal reaches the window as [`WindowEvent::SandboxRefused`], whose dialog offers the person's
//!   opt-in for the workspace once. The next launch passes `--allow-no-sandbox` only for that opt-in (the setting
//!   `browser.allowNoSandbox`, read from the person's state for the workspace and never from the workspace's
//!   `.eludite/settings.json` (brief 0047), or [`BrowserBus::opt_in_no_sandbox`] until the setting applies) or
//!   `ELUDITE_CHROME_NO_SANDBOX=1`; turning the setting off again removes it at the next start. What allowed it
//!   (`dialog`, `options`, `variable`) is in `tabs`' `engine.allowed_by` and the audit entry.
//! - **Discovery on first use** (brief 0039). The engine and CEF are searched for ([`ChromiumSearch`], with the
//!   setting `browser.enginePath`) the first time the window opens or a command needs the engine, never at startup.
//! - **Lifetime with the window.** Closing the Web Browser window keeps the engine for [`LINGER`] (a reopen is
//!   instant), then closes it; closing the workspace closes it at once.
//! - **Nothing until the first command.** The worker thread and the engine are created by the first browser command,
//!   so a cold start costs nothing; Chrome itself starts with the first `tab_open` (or `navigate` with no tab).
//! - **Configuration.** The settings `browser.chromePath` (`ELUDITE_CHROME`), `browser.headless` and
//!   `browser.viewport` and the workspace's profile (`<workspace>/.eludite/browser/profile`, or a folder in
//!   Eludite's cache with no workspace open) configure the next launch. When the workspace changes or closes, the
//!   running browser is closed; on shell exit it is closed and its process waited for.
//! - **The Output window.** The engine's lifecycle lines (launch, tabs opened and closed, navigation failures, console
//!   errors, exit) go to the Output window's Browser source through a channel the shell drains in batches.
//! - **Tabs of debugging sessions** (brief 0037). A tab a session's launch opened (`tab_open`, or a restart's
//!   `navigate`, with the caller [`Caller::Session`]) is that session's: `tabs` names it in `session`, and the window
//!   draws it with the project's name in its tooltip and a debug glyph. Closing the tab forgets it; the session's end
//!   does not close it.
//! - **Policy** (brief 0024, ADR-0009). The browser commands' escalation hooks read the open solution's
//!   `agents-policy.json` (its `browser` object) through the command registry's policy source, which the Agents
//!   window sets; the MCP gate decides on the class they give each call. Commands that need no engine
//!   (`open_external` with a url) still run on the worker, so nothing here ever runs on the UI thread.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use eludite_browser::embedded::{FrameSource, TabControl};
use eludite_browser::{
    Browser, ChromeSearch, ChromiumSearch, DebugTarget, EmbeddedChromium, Engine, EngineChoice,
    EngineConfig, EngineEvent, ExternalChrome, Interrupt, LogSink,
};
use eludite_commands::browser::{self as cmds, BrowserOutput, BrowserRequest, BrowserTarget};
use eludite_commands::{Caller, CommandError, current_caller};
use eludite_commands::{CommandRegistry, build::OutputSource};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{Context, Entity, Window};
use serde_json::Value;

use super::Shell;

/// The worker thread's name.
pub const THREAD: &str = "browser";
/// What let the engine run without the sandbox (brief 0047, `tabs`' `engine.allowed_by` and the audit entry): the
/// Web Browser window's dialog this session, the person's stored answer (Tools > Options, `eludite.settings.set`, or
/// the dialog in an earlier session), or `ELUDITE_CHROME_NO_SANDBOX=1`.
pub const ALLOWED_BY_DIALOG: &str = "dialog";
pub const ALLOWED_BY_OPTIONS: &str = "options";
pub const ALLOWED_BY_VARIABLE: &str = "variable";
/// The longest a caller waits for the worker: above every command's own bound (`navigate` waits up to 120 s).
const REPLY_TIMEOUT: Duration = Duration::from_secs(300);
/// How long shell exit waits for the browser to close.
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(5);
/// How long the engine outlives the Web Browser window (proposal 0002: a reopen is instant).
pub const LINGER: Duration = Duration::from_secs(60);

/// Makes the engine of the worker (the external Chrome; tests make fakes).
pub type EngineFactory = Arc<dyn Fn(EngineConfig, LogSink) -> Box<dyn Engine> + Send + Sync>;

/// What the settings say about the next launch.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserSettings {
    pub chrome_path: Option<PathBuf>,
    pub headless: bool,
    pub viewport: (u32, u32),
    /// `browser.engine`.
    pub engine: EngineChoice,
    /// `browser.homePage`: new tabs of the Web Browser window and its Home button.
    pub home_page: String,
    /// `browser.showDevToolsTab`.
    pub show_devtools_tab: bool,
    /// `browser.enginePath` (brief 0039).
    pub engine_path: Option<PathBuf>,
    /// `browser.allowNoSandbox`, the person's opt-in for the workspace (brief 0039; read from their state for the
    /// workspace only, brief 0047).
    pub allow_no_sandbox: bool,
    /// The workspace's `.eludite/settings.json` carries `browser.allowNoSandbox`, which is ignored (brief 0047).
    pub opt_in_ignored: bool,
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: false,
            viewport: EngineConfig::VIEWPORT,
            engine: EngineChoice::Embedded,
            home_page: "about:blank".into(),
            show_devtools_tab: false,
            engine_path: None,
            allow_no_sandbox: false,
            opt_in_ignored: false,
        }
    }
}

/// What the Web Browser window draws and drives a tab with: the embedded engine's handle ([`TabControl`]), or a
/// test's fake. Every method returns at once.
pub trait PageDriver: Send + Sync {
    /// The tab's frames (engine tab id).
    fn frames(&self, target: &str) -> Option<Arc<dyn FrameSource>>;
    /// A `tab/input` event.
    fn input(&self, target: &str, event: Value);
    /// Fit the tab to the view: CSS size and scale factor.
    fn resize(&self, target: &str, width: u32, height: u32, scale: f32);
    /// A shell-to-engine notification (`tab/action`, `tab/dialogAnswer`, `tab/permissionAnswer`).
    fn notify(&self, method: &str, params: Value);
    /// Close a tab no command closes (DevTools).
    fn close(&self, target: &str);
    /// Whether the engine runs with Chromium's sandbox (brief 0039; false: `--no-sandbox`).
    fn sandboxed(&self) -> bool {
        true
    }
}

impl PageDriver for TabControl {
    fn frames(&self, target: &str) -> Option<Arc<dyn FrameSource>> {
        let f: Arc<dyn FrameSource> = TabControl::frames(self, target)?;
        Some(f)
    }
    fn input(&self, target: &str, event: Value) {
        TabControl::input(self, target, event)
    }
    fn resize(&self, target: &str, width: u32, height: u32, scale: f32) {
        TabControl::resize(self, target, width, height, scale)
    }
    fn notify(&self, method: &str, params: Value) {
        TabControl::notify(self, method, params)
    }
    fn close(&self, target: &str) {
        TabControl::close(self, target)
    }
    fn sandboxed(&self) -> bool {
        TabControl::sandboxed(self)
    }
}

/// What the Web Browser window hears from the browser, on the UI thread.
#[derive(Clone)]
pub enum WindowEvent {
    /// The embedded engine started.
    Started(Arc<dyn PageDriver>),
    /// The engine exited or was closed.
    Stopped,
    /// An engine notification for the window (`tab/state`, `tab/cursor`, `tab/popup`, `tab/dialog`,
    /// `tab/permission`, `tab/dialogClosed`, `tab/download`, `tab/contextMenu`, `tab/closed`).
    Notification { method: String, params: Value },
    /// DevTools opened for page `page` as tab `devtools` (engine tab ids).
    DevtoolsOpened { page: String, devtools: String },
    /// The command tabs after a command: `(t1, engine tab id)` in order, the selected one, and the debugging
    /// sessions that opened tabs (by `t1`; brief 0037).
    Tabs {
        tabs: Vec<(String, String)>,
        active: Option<String>,
        sessions: BTreeMap<String, cmds::TabSession>,
    },
    /// The agents whose calls are in flight now (empty: nobody drives).
    Driving(Vec<String>),
    /// The embedded engine refused to start without Chromium's sandbox (brief 0039), with its message.
    SandboxRefused(String),
}

impl std::fmt::Debug for WindowEvent {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            WindowEvent::Started(_) => write!(f, "Started"),
            WindowEvent::Stopped => write!(f, "Stopped"),
            WindowEvent::Notification { method, .. } => write!(f, "Notification({method})"),
            WindowEvent::DevtoolsOpened { page, devtools } => {
                write!(f, "DevtoolsOpened({page} -> {devtools})")
            }
            WindowEvent::Tabs {
                tabs,
                active,
                sessions,
            } => write!(f, "Tabs({tabs:?}, {active:?}, {sessions:?})"),
            WindowEvent::Driving(a) => write!(f, "Driving({a:?})"),
            WindowEvent::SandboxRefused(_) => write!(f, "SandboxRefused"),
        }
    }
}

/// Where the engine's events for the window go (from any thread; never blocks).
pub type WindowSink = Arc<dyn Fn(WindowEvent) + Send + Sync>;

/// Which engine runs, and why not the embedded one when it was chosen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngineStatus {
    /// The window draws tabs (the embedded engine, or a test's engine factory).
    pub embedded: bool,
    /// Why the Web Browser window cannot draw tabs, with what to run.
    pub message: Option<String>,
}

enum Job {
    Apply(
        BrowserRequest,
        Box<Caller>,
        mpsc::SyncSender<Result<BrowserOutput, CommandError>>,
    ),
    /// Close the browser if it runs (the workspace changed or closed, or the shell exits).
    Shutdown(Option<mpsc::SyncSender<()>>),
    /// Where a debugger reaches a tab, by its id or a page url (brief 0038); never starts the browser.
    DebugTarget(
        DebugTab,
        mpsc::SyncSender<Result<DebugTarget, CommandError>>,
    ),
    /// The tabs as (id, title, url), when the browser runs (brief 0038: the Attach to Process dialog's).
    TabList(mpsc::SyncSender<Option<Vec<(String, String, String)>>>),
}

/// A tab a debugger names (brief 0038).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugTab {
    Id(String),
    Url(String),
}

struct Inner {
    ui_thread: std::thread::ThreadId,
    /// A test's engines; `None`: by the setting `browser.engine`.
    factory: Mutex<Option<EngineFactory>>,
    /// Where the embedded engine and CEF are searched for: made on first use (brief 0039: nothing at startup), or a
    /// test's.
    search: Mutex<Option<ChromiumSearch>>,
    /// The workspace whose opt-in the window's dialog took this session (brief 0039): it counts until the setting
    /// it stored applies, and is forgotten when the setting is turned off.
    opted_in: Mutex<Option<Option<PathBuf>>>,
    /// `ELUDITE_CHROME_NO_SANDBOX=1` in the shell's environment (read once; tests replace it).
    no_sandbox_env: AtomicBool,
    /// What allowed the engine's launch to drop the sandbox (brief 0047): set by the worker before each command that
    /// may launch it, so it holds the launch's answer when the window hears of the start.
    launch_allowed_by: Mutex<Option<&'static str>>,
    /// Where the window's events go, and the receiver until the window takes it.
    window: futures::channel::mpsc::UnboundedSender<WindowEvent>,
    window_rx: Mutex<Option<UnboundedReceiver<WindowEvent>>>,
    /// The person's hand, shared with every engine's [`Browser`].
    interrupt: Arc<Interrupt>,
    /// Agents' calls in flight: (call id, agent name).
    driving: Mutex<Vec<(u64, String)>>,
    /// Agents the person interrupted, refused action commands until they read `tabs`.
    stale: Mutex<BTreeSet<String>>,
    /// The window is open, and the count of its opens and closes (a linger checks it did not reopen).
    window_open: AtomicBool,
    window_generation: AtomicU64,
    linger: Mutex<Duration>,
    settings: Mutex<BrowserSettings>,
    /// The workspace folder whose profile the next launch uses.
    workspace: Mutex<Option<PathBuf>>,
    jobs: Mutex<Option<mpsc::Sender<Job>>>,
    log: UnboundedSender<String>,
    /// The program `open_external` runs instead of the system's opener (tests).
    opener: Mutex<Option<String>>,
    /// The tabs debugging sessions opened, by `t1` (brief 0037).
    sessions: Mutex<BTreeMap<String, cmds::TabSession>>,
    /// How many tabs the browser has, as last announced (before the command that changed them answers).
    tab_count: AtomicUsize,
    /// A debugging session's launch opened or navigated the window's page last (brief 0038): the window opens and
    /// shows it without taking the keyboard focus from the editor; the person's next command clears it.
    quiet: AtomicBool,
    /// The tabs stopped in the debugger, shared with every engine's [`Browser`] (brief 0038).
    pauses: Arc<eludite_browser::DebuggerPauses>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Inner {
    /// Which engine the setting and what is found give.
    fn status(&self) -> EngineStatus {
        if lock(&self.factory).is_some() {
            return EngineStatus {
                embedded: true,
                message: None,
            };
        }
        let choice = lock(&self.settings).engine;
        let (kind, why) = eludite_browser::select_engine(choice, &self.search());
        let message = match (choice, kind) {
            (EngineChoice::External, _) => Some(
                "The Web Browser window draws Eludite's embedded Chromium, and the setting browser.engine is \
                 `external`: the browser tools use a Chrome with its own window. Set browser.engine to `embedded` \
                 (Tools > Options > Web Browser) to browse here."
                    .to_owned(),
            ),
            (_, EngineChoice::External) => why,
            _ => None,
        };
        EngineStatus {
            embedded: kind == EngineChoice::Embedded,
            message,
        }
    }

    /// Where to look for the engine: the search (made on first use) with the setting `browser.enginePath`.
    fn search(&self) -> ChromiumSearch {
        let mut search = lock(&self.search)
            .get_or_insert_with(ChromiumSearch::defaults)
            .clone();
        if let Some(p) = lock(&self.settings).engine_path.clone() {
            search.engine.setting = Some(p);
        }
        search
    }

    /// What lets the next launch drop the sandbox where it cannot start, if anything (brief 0039), as `tabs`'
    /// `engine.allowed_by` names it (brief 0047): `variable` (`ELUDITE_CHROME_NO_SANDBOX=1`), `dialog` (the window's
    /// opt-in taken this session for this workspace) or `options` (the person's stored answer, `browser.allowNoSandbox`).
    fn allows_no_sandbox(&self) -> Option<&'static str> {
        if self.no_sandbox_env.load(Ordering::SeqCst) {
            return Some(ALLOWED_BY_VARIABLE);
        }
        let allow_setting = lock(&self.settings).allow_no_sandbox;
        if lock(&self.opted_in).as_ref() == Some(&*lock(&self.workspace)) {
            return Some(ALLOWED_BY_DIALOG);
        }
        allow_setting.then_some(ALLOWED_BY_OPTIONS)
    }

    /// The engine of `status`, telling the window what the embedded one says.
    fn make_engine(&self, embedded: bool, config: EngineConfig, log: LogSink) -> Box<dyn Engine> {
        if let Some(f) = lock(&self.factory).clone() {
            return f(config, log);
        }
        if !embedded {
            return Box::new(ExternalChrome::new(config, ChromeSearch::defaults(), log));
        }
        let mut engine = EmbeddedChromium::new(config, self.search(), log);
        let sink = self.sink();
        engine.set_observer(Some(Arc::new(move |e: EngineEvent| {
            sink(match e {
                EngineEvent::Started(control) => WindowEvent::Started(Arc::new(control)),
                EngineEvent::Stopped => WindowEvent::Stopped,
                EngineEvent::Notification { method, params } => {
                    WindowEvent::Notification { method, params }
                }
                EngineEvent::DevtoolsOpened { page, devtools } => {
                    WindowEvent::DevtoolsOpened { page, devtools }
                }
                EngineEvent::SandboxRefused { message } => WindowEvent::SandboxRefused(message),
            })
        })));
        Box::new(engine)
    }

    /// Where the engine's events go: the window, and a download's end to the Output window too.
    fn sink(&self) -> WindowSink {
        let (window, log) = (self.window.clone(), self.log.clone());
        Arc::new(move |e: WindowEvent| {
            if let WindowEvent::Notification { method, params } = &e
                && method == "tab/download"
                && let Some(line) = eludite_browser::embedded::download_line(params)
            {
                let _ = log.unbounded_send(line);
            }
            let _ = window.unbounded_send(e);
        })
    }

    fn config(&self) -> EngineConfig {
        let s = lock(&self.settings).clone();
        // One lock at a time: `allows_no_sandbox` takes the settings, the opt-in and the workspace in that order.
        let allow_no_sandbox = self.allows_no_sandbox().is_some();
        let workspace = lock(&self.workspace).clone();
        EngineConfig {
            executable: s.chrome_path,
            profile_dir: profile_dir(workspace.as_deref()),
            headless: s.headless,
            viewport: s.viewport,
            allow_no_sandbox,
        }
    }

    /// The worker's queue, starting the worker on first use.
    fn sender(self: &Arc<Self>) -> Result<mpsc::Sender<Job>, CommandError> {
        let mut jobs = lock(&self.jobs);
        if let Some(tx) = jobs.as_ref() {
            return Ok(tx.clone());
        }
        let (tx, rx) = mpsc::channel();
        let inner = self.clone();
        std::thread::Builder::new()
            .name(THREAD.into())
            .spawn(move || inner.run(rx))
            .map_err(|e| CommandError::Failed(format!("cannot start the browser worker: {e}")))?;
        *jobs = Some(tx.clone());
        Ok(tx)
    }

    fn run(self: Arc<Self>, rx: mpsc::Receiver<Job>) {
        let out = self.log.clone();
        let log: LogSink = Arc::new(move |line: &str| {
            let _ = out.unbounded_send(line.to_owned());
        });
        let mut config = self.config();
        let mut embedded = self.status().embedded;
        let make = |embedded: bool, config: EngineConfig| {
            let mut b = Browser::new(self.make_engine(embedded, config, log.clone()), log.clone());
            b.set_interrupt(self.interrupt.clone());
            b.set_pauses(self.pauses.clone());
            b
        };
        let mut browser = make(embedded, config.clone());
        let mut shown: Option<WindowEvent> = None;
        for job in rx {
            match job {
                Job::Apply(request, caller, reply) => {
                    let now = self.config();
                    let kind = self.status().embedded;
                    if kind != embedded {
                        // browser.engine changed: the other kind of browser from now on.
                        browser.shutdown();
                        lock(&self.sessions).clear();
                        embedded = kind;
                        config = now.clone();
                        browser = make(embedded, now);
                    } else if now != config {
                        browser.configure(now.clone());
                        config = now;
                    }
                    browser.set_opener(lock(&self.opener).clone());
                    if !browser.is_running() {
                        // This command may launch the engine with `config`: remember what allowed it.
                        let allowed = self.allows_no_sandbox();
                        *lock(&self.launch_allowed_by) = allowed;
                    }
                    // A session's page opens without taking the keys (brief 0038); anyone else's command may.
                    self.quiet
                        .store(matches!(*caller, Caller::Session { .. }), Ordering::SeqCst);
                    let mut answer = browser.apply(request);
                    self.follow_sessions(&caller, &mut answer);
                    // The window hears of the tabs before the caller gets its answer (a session's launch shows the
                    // window right after its tab opened; brief 0037).
                    self.announce_tabs(&browser, &mut shown);
                    let _ = reply.send(answer);
                    continue;
                }
                Job::DebugTarget(tab, reply) => {
                    let found = match tab {
                        DebugTab::Id(t) => browser.debug_target(&t),
                        DebugTab::Url(u) => browser
                            .tab_with_url(&u)
                            .and_then(|t| browser.debug_target(&t)),
                    };
                    let _ = reply.send(found);
                    continue;
                }
                Job::TabList(reply) => {
                    let rows = browser.is_running().then(|| {
                        browser
                            .apply(BrowserRequest::Tabs)
                            .ok()
                            .and_then(|o| match o {
                                BrowserOutput::Tabs(t) => Some(
                                    t.tabs.into_iter().map(|r| (r.id, r.title, r.url)).collect(),
                                ),
                                _ => None,
                            })
                            .unwrap_or_default()
                    });
                    let _ = reply.send(rows);
                    continue;
                }
                Job::Shutdown(reply) => {
                    if browser.is_running() {
                        browser.shutdown();
                    }
                    lock(&self.sessions).clear();
                    if let Some(reply) = reply {
                        let _ = reply.send(());
                    }
                }
            }
            self.announce_tabs(&browser, &mut shown);
        }
        browser.shutdown();
    }

    /// Which tabs are debugging sessions' (brief 0037): a session's `tab_open` or `navigate` makes the tab its own,
    /// closing a tab forgets it, tabs that are gone are forgotten, and `tabs` names each tab's session.
    fn follow_sessions(&self, caller: &Caller, answer: &mut Result<BrowserOutput, CommandError>) {
        let mut sessions = lock(&self.sessions);
        match (&mut *answer, caller) {
            (Ok(BrowserOutput::TabOpen(o)), Caller::Session { session, name }) => {
                sessions.insert(
                    o.tab.id.clone(),
                    cmds::TabSession {
                        id: *session,
                        name: name.clone(),
                    },
                );
            }
            (Ok(BrowserOutput::Navigate(o)), Caller::Session { session, name }) => {
                sessions.insert(
                    o.tab.clone(),
                    cmds::TabSession {
                        id: *session,
                        name: name.clone(),
                    },
                );
            }
            (Ok(BrowserOutput::TabClose(o)), _) => {
                sessions.remove(&o.closed);
            }
            (Ok(BrowserOutput::Tabs(o)), _) => {
                // Brief 0047: what let the engine run without the sandbox.
                if o.engine.sandbox.as_deref() == Some("none") {
                    o.engine.allowed_by = lock(&self.launch_allowed_by).map(str::to_owned);
                }
                sessions.retain(|id, _| o.tabs.iter().any(|t| t.id == *id));
                for t in &mut o.tabs {
                    t.session = sessions.get(&t.id).cloned();
                }
            }
            _ => {}
        }
    }

    /// Tell the window the command tabs, when they changed.
    fn announce_tabs(&self, browser: &Browser, shown: &mut Option<WindowEvent>) {
        let (tabs, active) = if browser.is_running() {
            (browser.targets_of_tabs(), browser.active_tab())
        } else {
            (Vec::new(), None)
        };
        self.tab_count.store(tabs.len(), Ordering::SeqCst);
        let sessions: BTreeMap<String, cmds::TabSession> = lock(&self.sessions)
            .iter()
            .filter(|(id, _)| tabs.iter().any(|(t, _)| t == *id))
            .map(|(id, s)| (id.clone(), s.clone()))
            .collect();
        let changed = !matches!(shown, Some(WindowEvent::Tabs { tabs: t, active: a, sessions: s })
            if *t == tabs && *a == active && *s == sessions);
        if changed {
            let e = WindowEvent::Tabs {
                tabs,
                active,
                sessions,
            };
            *shown = Some(e.clone());
            let _ = self.window.unbounded_send(e);
        }
    }

    /// An agent's call begins or ends: tell the window who drives.
    fn set_driving(&self, f: impl FnOnce(&mut Vec<(u64, String)>)) {
        let names = {
            let mut d = lock(&self.driving);
            f(&mut d);
            let mut names: Vec<String> = d.iter().map(|(_, a)| a.clone()).collect();
            names.dedup();
            names
        };
        let _ = self.window.unbounded_send(WindowEvent::Driving(names));
    }
}

/// The profile directory of a workspace folder; with none open, one in Eludite's cache.
pub fn profile_dir(workspace: Option<&Path>) -> PathBuf {
    match workspace {
        Some(w) => EngineConfig::profile_for(w),
        None => eludite_browser::chrome::fallback_profile(),
    }
}

/// The browser commands' target, and the shell's handle on the worker. Cheap to clone.
#[derive(Clone)]
pub struct BrowserBus {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for BrowserBus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("BrowserBus")
    }
}

impl BrowserTarget for BrowserBus {
    fn apply(&self, request: BrowserRequest) -> Result<BrowserOutput, CommandError> {
        if std::thread::current().id() == self.inner.ui_thread {
            return Err(CommandError::Failed(
                "the eludite.browser.* commands run on the browser worker and wait on the browser, so the UI thread \
                 does not call them; invoke them from an agent (MCP) or another thread"
                    .into(),
            ));
        }
        let id = request.command();
        let caller = current_caller();
        let agent = match &caller {
            Caller::Agent { agent, call, .. } => Some((*call, agent.clone())),
            Caller::User | Caller::Session { .. } => None,
        };
        if let Some((_, name)) = &agent {
            let mut stale = lock(&self.inner.stale);
            if id == cmds::TABS {
                stale.remove(name);
            } else if cmds::ACTIONS.contains(&id) && stale.contains(name) {
                return Err(CommandError::Failed(STALE.into()));
            }
        }
        if let Some((call, name)) = &agent {
            self.inner.set_driving(|d| d.push((*call, name.clone())));
        }
        let (reply, rx) = mpsc::sync_channel(1);
        let answer = self
            .inner
            .sender()
            .and_then(|tx| {
                tx.send(Job::Apply(request, Box::new(caller), reply))
                    .map_err(|_| CommandError::Failed("the browser worker is gone".into()))
            })
            .and_then(|()| {
                rx.recv_timeout(REPLY_TIMEOUT)
                    .map_err(|_| CommandError::Failed("the browser worker did not answer".into()))?
            });
        if let Some((call, _)) = &agent {
            self.inner.set_driving(|d| d.retain(|(c, _)| c != call));
        }
        answer
    }
}

/// What an interrupted agent's next action command answers.
pub const STALE: &str = "the person took over the Web Browser window while your call ran, so the page may have \
    changed; read eludite.browser.tabs (and the page's page_generation) before acting on it again";

impl BrowserBus {
    /// Use `factory` for the engine (before the first browser command; tests make fakes). The window then draws its
    /// tabs as the embedded engine's.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_engine_factory(&self, factory: EngineFactory) {
        *lock(&self.inner.factory) = Some(factory);
    }

    /// Search for the embedded engine and CEF here (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_chromium_search(&self, search: ChromiumSearch) {
        *lock(&self.inner.search) = Some(search);
    }

    /// Whether the engine and CEF were searched for yet (brief 0039: never at startup; tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn searched(&self) -> bool {
        lock(&self.inner.search).is_some()
    }

    /// The person took the opt-in of the Web Browser window's dialog for the current workspace (brief 0039): the next
    /// launch may drop the sandbox, before the setting the window stores applies.
    pub fn opt_in_no_sandbox(&self) {
        *lock(&self.inner.opted_in) = Some(lock(&self.inner.workspace).clone());
    }

    /// Whether `ELUDITE_CHROME_NO_SANDBOX=1` counts (tests that prove the opt-in run with it set).
    #[cfg(test)]
    pub fn set_no_sandbox_env(&self, on: bool) {
        self.inner.no_sandbox_env.store(on, Ordering::SeqCst);
    }

    /// What allowed the running engine's launch to drop the sandbox (`dialog`, `options` or `variable`), as the
    /// worker recorded it before the launch (brief 0047).
    pub fn launch_allowed_by(&self) -> Option<&'static str> {
        *lock(&self.inner.launch_allowed_by)
    }

    /// The workspace folder whose profile and settings the browser uses.
    pub fn workspace(&self) -> Option<PathBuf> {
        lock(&self.inner.workspace).clone()
    }

    /// Which engine runs, and why the window cannot draw tabs when it cannot.
    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }

    /// The Web Browser window's events, once (the shell takes them at startup).
    pub fn window_events(&self) -> Option<UnboundedReceiver<WindowEvent>> {
        lock(&self.inner.window_rx).take()
    }

    /// Where a debugger reaches tab `tab` (brief 0038): the browser's remote debugging endpoint and the page's target
    /// id, url and title. Waits on the browser worker: call it off the UI thread.
    pub fn debug_target(&self, tab: DebugTab) -> Result<DebugTarget, CommandError> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Job::DebugTarget(tab, reply))
            .map_err(|_| CommandError::Failed("the browser worker is gone".into()))?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| CommandError::Failed("the browser worker did not answer".into()))?
    }

    /// The browser's tabs as (id, title, url), `None` while it does not run (brief 0038). Waits on the browser
    /// worker: call it off the UI thread.
    pub fn tab_list(&self) -> Option<Vec<(String, String, String)>> {
        let (reply, rx) = mpsc::sync_channel(1);
        self.inner.sender().ok()?.send(Job::TabList(reply)).ok()?;
        rx.recv_timeout(REPLY_TIMEOUT).ok().flatten()
    }

    /// Whether a debugging session's launch opened the page last (brief 0038): the window then does not take the
    /// keyboard focus.
    pub fn quiet(&self) -> bool {
        self.inner.quiet.load(Ordering::SeqCst)
    }

    /// The tabs whose page is stopped in the debugger from now on (brief 0038): `eludite.browser.input` stops
    /// waiting on such a page and answers `paused`. Whether they changed.
    pub fn set_debugger_pauses(&self, tabs: Vec<String>) -> bool {
        self.inner.pauses.set(tabs)
    }

    /// Whether `tab` is marked stopped in the debugger (tests).
    #[cfg(test)]
    pub fn debugger_paused(&self, tab: &str) -> bool {
        self.inner.pauses.is_paused(tab)
    }

    /// Where a test's fake engine sends what the embedded engine would.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn window_sink(&self) -> WindowSink {
        self.inner.sink()
    }

    /// The person used the page or the window's toolbar: an agent's call in flight stops waiting
    /// (`interrupted_by: "user"`) and that agent must read `tabs` before acting again. False when no agent drove.
    pub fn person_acted(&self) -> bool {
        self.take_over(false)
    }

    /// The "Agent is driving" strip's Stop: the agents' calls in flight end (a wait as interrupted, anything else
    /// failed) and they must read `tabs` before acting again.
    pub fn stop(&self) -> bool {
        self.take_over(true)
    }

    fn take_over(&self, stop: bool) -> bool {
        let agents: Vec<String> = lock(&self.inner.driving)
            .iter()
            .map(|(_, a)| a.clone())
            .collect();
        if agents.is_empty() {
            return false;
        }
        lock(&self.inner.stale).extend(agents);
        if stop {
            self.inner.interrupt.stop();
        } else {
            self.inner.interrupt.took_over();
        }
        true
    }

    /// The Web Browser window opened.
    pub fn window_opened(&self) {
        self.inner.window_open.store(true, Ordering::SeqCst);
        self.inner.window_generation.fetch_add(1, Ordering::SeqCst);
    }

    /// The Web Browser window closed: the engine closes after [`LINGER`] unless the window reopens meanwhile.
    pub fn window_closed(&self) {
        self.inner.window_open.store(false, Ordering::SeqCst);
        let generation = self.inner.window_generation.fetch_add(1, Ordering::SeqCst) + 1;
        let linger = *lock(&self.inner.linger);
        let inner = self.inner.clone();
        let _ = std::thread::Builder::new()
            .name("browser-linger".into())
            .spawn(move || {
                std::thread::sleep(linger);
                if !inner.window_open.load(Ordering::SeqCst)
                    && inner.window_generation.load(Ordering::SeqCst) == generation
                    && let Some(tx) = lock(&inner.jobs).as_ref()
                {
                    let _ = tx.send(Job::Shutdown(None));
                }
            });
    }

    /// How long the engine outlives the window (tests shorten it).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_linger(&self, linger: Duration) {
        *lock(&self.inner.linger) = linger;
    }

    /// Run `program url` for `eludite.browser.open_external` instead of the system's opener (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_opener(&self, program: Option<String>) {
        *lock(&self.inner.opener) = program;
    }

    /// The settings of the next launch. `browser.allowNoSandbox` turned off forgets the dialog's opt-in too.
    pub fn set_settings(&self, settings: BrowserSettings) {
        let mut s = lock(&self.inner.settings);
        if s.allow_no_sandbox && !settings.allow_no_sandbox {
            *lock(&self.inner.opted_in) = None;
        }
        *s = settings;
    }

    /// The settings of the next launch, as last applied.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings(&self) -> BrowserSettings {
        lock(&self.inner.settings).clone()
    }

    /// The workspace changed (or closed, `None`): the next launch uses its profile, and a running browser, which
    /// has the old workspace's profile, is closed. Never waits.
    pub fn set_workspace(&self, root: Option<&Path>) {
        let mut w = lock(&self.inner.workspace);
        if w.as_deref() == root {
            return;
        }
        *w = root.map(Path::to_path_buf);
        drop(w);
        if let Some(tx) = lock(&self.inner.jobs).as_ref() {
            let _ = tx.send(Job::Shutdown(None));
        }
    }

    /// The configuration the next launch gets.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn config(&self) -> EngineConfig {
        self.inner.config()
    }

    /// Close the browser (shell exit). The receiver gets `()` once it is closed; immediately when it never started.
    pub fn shutdown(&self) -> mpsc::Receiver<()> {
        let (tx, rx) = mpsc::sync_channel(1);
        match lock(&self.inner.jobs).as_ref() {
            Some(jobs) => {
                if let Err(mpsc::SendError(Job::Shutdown(Some(tx)))) =
                    jobs.send(Job::Shutdown(Some(tx)))
                {
                    let _ = tx.send(());
                }
            }
            None => {
                let _ = tx.send(());
            }
        }
        rx
    }

    /// Whether the browser has tabs, as the worker last announced them (the window opening does not add a blank tab
    /// to a browser that has some, as a session's launch's; brief 0037).
    pub fn has_tabs(&self) -> bool {
        self.inner.tab_count.load(Ordering::SeqCst) > 0
    }

    /// Whether the worker was started (by a browser command).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn started(&self) -> bool {
        lock(&self.inner.jobs).is_some()
    }
}

/// Register the browser commands; call on the UI thread. Answers the handle and the Output lines' receiver.
pub fn register(commands: &CommandRegistry) -> (BrowserBus, UnboundedReceiver<String>) {
    let (log, lines) = unbounded();
    let (window, window_rx) = unbounded();
    let bus = BrowserBus {
        inner: Arc::new(Inner {
            ui_thread: std::thread::current().id(),
            factory: Mutex::new(None),
            search: Mutex::new(None),
            opted_in: Mutex::new(None),
            no_sandbox_env: AtomicBool::new(eludite_browser::chrome::no_sandbox_from_env()),
            launch_allowed_by: Mutex::new(None),
            window,
            window_rx: Mutex::new(Some(window_rx)),
            interrupt: Arc::default(),
            driving: Mutex::default(),
            stale: Mutex::default(),
            window_open: AtomicBool::new(false),
            window_generation: AtomicU64::new(0),
            linger: Mutex::new(LINGER),
            settings: Mutex::new(BrowserSettings::default()),
            workspace: Mutex::new(None),
            jobs: Mutex::new(None),
            log,
            opener: Mutex::new(None),
            sessions: Mutex::default(),
            tab_count: AtomicUsize::new(0),
            quiet: AtomicBool::new(false),
            pauses: Arc::default(),
        }),
    };
    cmds::register(commands, Arc::new(bus.clone()));
    (bus, lines)
}

impl Shell {
    /// Drain the browser's lifecycle lines into the Output window's Browser source, a batch per frame.
    pub(super) fn browser_output_task(
        mut lines: UnboundedReceiver<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> gpui::Task<()> {
        use futures::StreamExt as _;
        cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = lines.next().await {
                let mut text = first + "\n";
                while let Ok(more) = lines.try_recv() {
                    text.push_str(&more);
                    text.push('\n');
                }
                if this
                    .update(cx, |shell, cx| {
                        shell
                            .output
                            .update(cx, |o, cx| o.append(OutputSource::Browser, &text, cx))
                    })
                    .is_err()
                {
                    break;
                }
            }
        })
    }

    /// The browser commands' handle: shell exit closes the browser through it; tests set the engine factory.
    pub fn browser(&self) -> &BrowserBus {
        &self.browser
    }

    /// The Web Browser window (brief 0032).
    pub fn browser_window(&self) -> &Entity<super::browser_window::BrowserWindow> {
        &self.browser_window
    }

    /// The Web Browser window's two tasks: the browser's events to the window, and the window's document tab opening
    /// and closing (the layout changes).
    pub(super) fn browser_window_tasks(
        bus: &BrowserBus,
        controller: &eludite_docking::DockController,
        browser_window: &Entity<super::browser_window::BrowserWindow>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> (gpui::Task<()>, gpui::Task<()>) {
        use futures::StreamExt as _;
        let events = bus.window_events();
        let w = browser_window.clone();
        let events_task = cx.spawn_in(window, async move |this, cx| {
            let Some(mut events) = events else { return };
            while let Some(e) = events.next().await {
                // The tabs, or a tab's title, changed: a browser session follows its tab (brief 0038).
                let tabs_changed = match &e {
                    WindowEvent::Tabs { .. } | WindowEvent::Stopped => true,
                    WindowEvent::Notification { method, .. } => {
                        method == "tab/state" || method == "tab/closed"
                    }
                    _ => false,
                };
                if cx
                    .update(|window, cx| w.update(cx, |w, cx| w.on_event(e, window, cx)))
                    .is_err()
                {
                    break;
                }
                if tabs_changed {
                    let w = w.clone();
                    let _ = this.update(cx, |shell, cx| {
                        let tabs = w.read(cx).tab_titles();
                        shell.debug_tabs_changed(&tabs, cx);
                    });
                }
            }
        });
        let mut changes = controller.subscribe();
        let controller = controller.clone();
        let w = browser_window.clone();
        let layout_task = cx.spawn_in(window, async move |_, cx| {
            // A restored layout may hold the window already; then every layout change.
            let mut first = true;
            while std::mem::take(&mut first) || changes.next().await.is_some() {
                while changes.try_recv().is_ok() {}
                let open = controller
                    .layout()
                    .documents
                    .get(eludite_docking::ids::WEB_BROWSER)
                    .is_some();
                if cx
                    .update(|window, cx| w.update(cx, |w, cx| w.set_open(open, window, cx)))
                    .is_err()
                {
                    break;
                }
            }
        });
        (events_task, layout_task)
    }
}
