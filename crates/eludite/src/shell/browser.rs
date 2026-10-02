//! The browser of `eludite.browser.*` (brief 0023): one browser per workspace, through `eludite-browser`.
//!
//! - **The `browser` worker.** Every browser command runs on one thread named `browser`, which owns the
//!   [`eludite_browser::Browser`] (the engine and the tabs). An agent's call arrives on the MCP thread, is handed to
//!   the worker and blocks there, as other commands block their caller. The UI thread never waits on Chrome
//!   (invariant 1): a browser command invoked on it fails at once. There is no Web Browser window yet (brief B).
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

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use eludite_browser::{Browser, ChromeSearch, Engine, EngineConfig, ExternalChrome, LogSink};
use eludite_commands::CommandError;
use eludite_commands::browser::{self as cmds, BrowserOutput, BrowserRequest, BrowserTarget};
use eludite_commands::{CommandRegistry, build::OutputSource};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{Context, Window};

use super::Shell;

/// The worker thread's name.
pub const THREAD: &str = "browser";
/// The longest a caller waits for the worker: above every command's own bound (`navigate` waits up to 120 s).
const REPLY_TIMEOUT: Duration = Duration::from_secs(300);
/// How long shell exit waits for the browser to close.
pub const EXIT_TIMEOUT: Duration = Duration::from_secs(5);

/// Makes the engine of the worker (the external Chrome; tests make fakes).
pub type EngineFactory = Arc<dyn Fn(EngineConfig, LogSink) -> Box<dyn Engine> + Send + Sync>;

/// The external Chrome of brief 0023, searched for as `crates/browser`'s discovery says (the setting, which already
/// resolved `ELUDITE_CHROME`, comes in the configuration).
pub fn external_chrome() -> EngineFactory {
    Arc::new(|config, log| Box::new(ExternalChrome::new(config, ChromeSearch::defaults(), log)))
}

/// What the settings say about the next launch.
#[derive(Debug, Clone, PartialEq)]
pub struct BrowserSettings {
    pub chrome_path: Option<PathBuf>,
    pub headless: bool,
    pub viewport: (u32, u32),
}

impl Default for BrowserSettings {
    fn default() -> Self {
        Self {
            chrome_path: None,
            headless: false,
            viewport: EngineConfig::VIEWPORT,
        }
    }
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
    factory: Mutex<EngineFactory>,
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
        let factory = lock(&self.factory).clone();
        let mut browser = Browser::new(factory(config.clone(), log.clone()), log);
        for job in rx {
            match job {
                Job::Apply(request, reply) => {
                    let now = self.config();
                    if now != config {
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
        }
        browser.shutdown();
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
        let (reply, rx) = mpsc::sync_channel(1);
        self.inner
            .sender()?
            .send(Job::Apply(request, reply))
            .map_err(|_| CommandError::Failed("the browser worker is gone".into()))?;
        rx.recv_timeout(REPLY_TIMEOUT)
            .map_err(|_| CommandError::Failed("the browser worker did not answer".into()))?
    }
}

impl BrowserBus {
    /// Use `factory` for the engine (before the first browser command; tests make fakes).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_engine_factory(&self, factory: EngineFactory) {
        *lock(&self.inner.factory) = factory;
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
    let bus = BrowserBus {
        inner: Arc::new(Inner {
            ui_thread: std::thread::current().id(),
            factory: Mutex::new(external_chrome()),
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
}
