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
//! - **Policy** (brief 0024, ADR-0009). The browser commands' escalation hooks read the open solution's
//!   `agents-policy.json` (its `browser` object) through the command registry's policy source, which the Agents
//!   window sets; the MCP gate decides on the class they give each call. Commands that need no engine
//!   (`open_external` with a url) still run on the worker, so nothing here ever runs on the UI thread.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use eludite_browser::embedded::{FrameSource, TabControl};
use eludite_browser::{
    Browser, ChromeSearch, ChromiumSearch, EmbeddedChromium, Engine, EngineChoice, EngineConfig,
    EngineEvent, ExternalChrome, Interrupt, LogSink,
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
    /// The command tabs after a command: `(t1, engine tab id)` in order, and the selected one.
    Tabs {
        tabs: Vec<(String, String)>,
        active: Option<String>,
    },
    /// The agents whose calls are in flight now (empty: nobody drives).
    Driving(Vec<String>),
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
            WindowEvent::Tabs { tabs, active } => write!(f, "Tabs({tabs:?}, {active:?})"),
            WindowEvent::Driving(a) => write!(f, "Driving({a:?})"),
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
        mpsc::SyncSender<Result<BrowserOutput, CommandError>>,
    ),
    /// Close the browser if it runs (the workspace changed or closed, or the shell exits).
    Shutdown(Option<mpsc::SyncSender<()>>),
}

struct Inner {
    ui_thread: std::thread::ThreadId,
    /// A test's engines; `None`: by the setting `browser.engine`.
    factory: Mutex<Option<EngineFactory>>,
    /// Where the embedded engine and CEF are searched for.
    search: Mutex<ChromiumSearch>,
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
        let (kind, why) = eludite_browser::select_engine(choice, &lock(&self.search));
        let message = match (choice, kind) {
            (EngineChoice::External, _) => Some(
                "The Web Browser window draws Eludite's embedded Chromium, and the setting browser.engine is                  `external`: the browser tools use a Chrome with its own window. Set browser.engine to `embedded`                  (Tools > Options > Web Browser) to browse here."
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

    /// The engine of `status`, telling the window what the embedded one says.
    fn make_engine(&self, embedded: bool, config: EngineConfig, log: LogSink) -> Box<dyn Engine> {
        if let Some(f) = lock(&self.factory).clone() {
            return f(config, log);
        }
        if !embedded {
            return Box::new(ExternalChrome::new(config, ChromeSearch::defaults(), log));
        }
        let mut engine = EmbeddedChromium::new(config, lock(&self.search).clone(), log);
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
        EngineConfig {
            executable: s.chrome_path,
            profile_dir: profile_dir(lock(&self.workspace).as_deref()),
            headless: s.headless,
            viewport: s.viewport,
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
            b
        };
        let mut browser = make(embedded, config.clone());
        let mut shown: Option<WindowEvent> = None;
        for job in rx {
            match job {
                Job::Apply(request, reply) => {
                    let now = self.config();
                    let kind = self.status().embedded;
                    if kind != embedded {
                        // browser.engine changed: the other kind of browser from now on.
                        browser.shutdown();
                        embedded = kind;
                        config = now.clone();
                        browser = make(embedded, now);
                    } else if now != config {
                        browser.configure(now.clone());
                        config = now;
                    }
                    browser.set_opener(lock(&self.opener).clone());
                    let _ = reply.send(browser.apply(request));
                }
                Job::Shutdown(reply) => {
                    if browser.is_running() {
                        browser.shutdown();
                    }
                    if let Some(reply) = reply {
                        let _ = reply.send(());
                    }
                }
            }
            self.announce_tabs(&browser, &mut shown);
        }
        browser.shutdown();
    }

    /// Tell the window the command tabs, when they changed.
    fn announce_tabs(&self, browser: &Browser, shown: &mut Option<WindowEvent>) {
        let (tabs, active) = if browser.is_running() {
            (browser.targets_of_tabs(), browser.active_tab())
        } else {
            (Vec::new(), None)
        };
        let changed = !matches!(shown, Some(WindowEvent::Tabs { tabs: t, active: a }) if *t == tabs && *a == active);
        if changed {
            let e = WindowEvent::Tabs { tabs, active };
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
        let agent = match current_caller() {
            Caller::Agent { agent, call, .. } => Some((call, agent)),
            Caller::User => None,
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
                tx.send(Job::Apply(request, reply))
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
        *lock(&self.inner.search) = search;
    }

    /// Which engine runs, and why the window cannot draw tabs when it cannot.
    pub fn status(&self) -> EngineStatus {
        self.inner.status()
    }

    /// The Web Browser window's events, once (the shell takes them at startup).
    pub fn window_events(&self) -> Option<UnboundedReceiver<WindowEvent>> {
        lock(&self.inner.window_rx).take()
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

    /// The settings of the next launch.
    pub fn set_settings(&self, settings: BrowserSettings) {
        *lock(&self.inner.settings) = settings;
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
            search: Mutex::new(ChromiumSearch::defaults()),
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
        let events_task = cx.spawn_in(window, async move |_, cx| {
            let Some(mut events) = events else { return };
            while let Some(e) = events.next().await {
                if cx
                    .update(|window, cx| w.update(cx, |w, cx| w.on_event(e, window, cx)))
                    .is_err()
                {
                    break;
                }
            }
        });
        let mut changes = controller.subscribe();
        let controller = controller.clone();
        let w = browser_window.clone();
        let layout_task = cx.spawn_in(window, async move |_, cx| {
            while changes.next().await.is_some() {
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
