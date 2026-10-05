//! Self-update in the shell (brief 0055, ADR-0011): `eludite.update.*` over `eludite-update`, Help > Check for
//! Updates, the status bar's update slot, the Output window's Updates source, the first-start question and the
//! restart that installs a staged build.
//!
//! - **One service.** [`UpdateService`] answers `eludite.update.*` on the invoking thread: `status`, `check` and
//!   `download` ask the [`Updater`] (its own worker thread) and wait only as long as the caller asked; `apply`
//!   needs the UI thread (it quits the IDE), so from another thread it is a job the UI applies, and from the UI it
//!   is staged here first ([`stage`]) as the project commands are.
//! - **Nothing at startup.** The updater is started with the shell but asks the network only on a command or on the
//!   timer, which runs [`Updater::due`] about 15 s after the first frame and every minute after, checking at most
//!   every 4 hours, and only when the mode says so. A packaged build whose mode is still `ask` asks once, after its
//!   first window, with a prompt whose answer is written through `eludite.settings.set`.
//! - **The status bar** (`update` slot, right): "Update available", "Downloading…", "Restart to update", "Update
//!   failed"; clicking it runs the next command. The Output window's Updates source has the log.
//! - **The restart** (`eludite.update.apply`): after the person's confirmation the staged executable's copy is started
//!   with the plan and the IDE quits; the applier swaps the files once the shell has exited and starts the new
//!   Eludite with the same arguments and `--updated-from` (`eludite_update::apply`).

use std::cell::RefCell;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, mpsc};
use std::time::Duration;

use eludite_commands::build::OutputSource;
use eludite_commands::update::{self as cmds, UpdateRequest, UpdateTarget};
use eludite_commands::{CommandError, CommandRegistry};
use eludite_ui::{SlotAlign, slots};
use eludite_update::apply::{self, Plan, Relaunch};
use eludite_update::stage::Stage;
use eludite_update::updater::{Config, Mode, Setup, State, Status, Updater};
use eludite_update::{Channel, REPOSITORY};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{Context, PromptLevel, Window};
use serde_json::{Value, json};

use super::Shell;

/// The status bar slot of the update's state.
pub const UPDATE_SLOT: &str = "update";
/// How often the timer checks the channel at most.
pub const CHECK_INTERVAL: Duration = Duration::from_secs(4 * 3600);
/// How often the timer asks whether a check is due.
pub const TICK: Duration = Duration::from_secs(60);
/// The settings keys.
pub const CHANNEL_KEY: &str = "updates.channel";
pub const MODE_KEY: &str = "updates.mode";
/// The environment variable naming another releases API (a mirror, a test server).
pub const API_ENV: &str = "ELUDITE_UPDATE_API";
/// The state file beside the settings: the last check and the release list's `ETag`.
pub const STATE_FILE: &str = "updates.json";

/// What `eludite.update.apply` does with a plan: starts the applier (the real one), or records it (tests).
pub type Restarter = Arc<dyn Fn(&Plan) -> Result<PathBuf, String> + Send + Sync>;

/// How the shell's updater is set up.
pub struct UpdateSetup {
    pub setup: Setup,
    /// The arguments the new Eludite starts with after an update.
    pub relaunch: Relaunch,
    pub restarter: Restarter,
    /// Quit after starting the applier (false in tests).
    pub quit_on_apply: bool,
    /// Ask the first-start question when the mode is `ask` (false for measurement runs and tests).
    pub ask: bool,
    pub ask_delay: Duration,
    /// How long after the first frame the timer starts.
    pub first_check_delay: Duration,
    /// `--updated-from`: the build this start replaced.
    pub updated_from: Option<String>,
}

impl std::fmt::Debug for UpdateSetup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("UpdateSetup")
            .field("setup", &self.setup)
            .field("relaunch", &self.relaunch)
            .field("ask", &self.ask)
            .finish()
    }
}

impl UpdateSetup {
    /// This executable, GitHub (or `ELUDITE_UPDATE_API`), the real applier, this process's arguments.
    pub fn detect() -> Self {
        let state_file = eludite_docking::eludite_config_dir().map(|d| d.join(STATE_FILE));
        let mut setup = Setup::detect(REPOSITORY, state_file);
        if let Ok(api) = std::env::var(API_ENV)
            && !api.trim().is_empty()
        {
            setup.source.api = api.trim().to_owned();
        }
        Self {
            setup,
            relaunch: Relaunch {
                args: relaunch_args(std::env::args().skip(1)),
                cwd: std::env::current_dir().ok(),
            },
            restarter: Arc::new(|plan: &Plan| {
                let stage = Stage::new(&plan.install_dir);
                let launched = apply::launch(&stage, plan).map_err(|e| e.to_string())?;
                // The applier swaps once its stdin pipe closes, which happens when this process exits.
                std::mem::forget(launched.child);
                Ok(launched.plan_path)
            }),
            quit_on_apply: true,
            ask: true,
            ask_delay: Duration::from_secs(2),
            first_check_delay: Duration::from_secs(15),
            updated_from: None,
        }
    }
}

/// The arguments worth carrying across a restart: what the person opened and how the window looks, never a
/// measurement or one-shot flag.
pub fn relaunch_args(args: impl IntoIterator<Item = String>) -> Vec<String> {
    const WITH_VALUE: [&str; 5] = [
        "--solution",
        "--folder",
        "--theme",
        "--open-file",
        "--agent",
    ];
    const ALONE: [&str; 1] = ["--no-persist"];
    let mut out = Vec::new();
    let mut it = args.into_iter();
    while let Some(a) = it.next() {
        if WITH_VALUE.contains(&a.as_str()) {
            if let Some(v) = it.next() {
                out.push(a);
                out.push(v);
            }
        } else if ALONE.contains(&a.as_str()) {
            out.push(a);
        } else if a == "--updated-from"
            || a == "--mcp-relay"
            || a == "--transcript-out"
            || a == "--timings-out"
            || a == "--bounds-out"
            || a == "--exit-after-ms"
            || a.starts_with("--bench-")
            || a == "--apply-update"
            || a == "--spike-browser"
        {
            it.next();
        }
    }
    out
}

/// `eludite.update.apply` from another thread, for the UI thread.
pub struct UpdateJob {
    pub reply: mpsc::SyncSender<Result<Value, CommandError>>,
}

/// The shell's [`UpdateTarget`].
pub struct UpdateService {
    updater: Mutex<Option<Updater>>,
    ui_thread: std::thread::ThreadId,
    jobs: UnboundedSender<UpdateJob>,
}

thread_local! {
    /// The UI thread's `apply` outcome, staged before the bus handler runs (as the project commands do).
    static STAGED: RefCell<Option<Result<Value, CommandError>>> = const { RefCell::new(None) };
}

/// Stage the UI thread's `apply` outcome for the handler.
pub fn stage(outcome: Result<Value, CommandError>) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

impl UpdateService {
    pub fn updater(&self) -> Option<Updater> {
        self.updater
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    fn attach(&self, updater: Updater) {
        *self.updater.lock().unwrap_or_else(|e| e.into_inner()) = Some(updater);
    }

    /// `check` (then `download` when asked) or `download`, waiting `wait_ms` off the UI thread.
    fn check_or_download(
        &self,
        download_job: bool,
        then_download: bool,
        wait_ms: u64,
        on_ui: bool,
    ) -> Result<Value, CommandError> {
        let Some(updater) = self.updater() else {
            return Err(CommandError::Failed("the updater has not started".into()));
        };
        if !updater.status().enabled {
            // Nothing to check: the output says why (`enabled: false`, `reason`).
            return Ok(status_json(&updater.status()));
        }
        if download_job {
            updater.download();
        } else {
            updater.check(then_download);
        }
        // The UI thread never waits (invariant 1); the menu passes no wait anyway.
        let wait = if on_ui { 0 } else { wait_ms };
        if wait > 0 {
            Ok(status_json(&updater.wait(Duration::from_millis(wait))))
        } else {
            Ok(status_json(&updater.status()))
        }
    }

    /// The status as JSON; before the shell starts the updater, a disabled one.
    pub fn status_json(&self) -> Value {
        match self.updater() {
            Some(u) => status_json(&u.status()),
            None => status_json(&Status {
                enabled: false,
                reason: Some("the updater has not started".into()),
                build: None,
                install_dir: PathBuf::new(),
                channel: Channel::Unstable,
                mode: Mode::Ask,
                state: State::Idle,
                last_check: None,
                busy: false,
                writable: None,
            }),
        }
    }
}

/// `Status` as the schema `update-status.output.json` has it.
pub fn status_json(status: &Status) -> Value {
    serde_json::to_value(status).expect("a status serializes")
}

impl UpdateTarget for UpdateService {
    fn apply(&self, request: UpdateRequest) -> Result<Value, CommandError> {
        let on_ui = std::thread::current().id() == self.ui_thread;
        match request {
            UpdateRequest::Status => Ok(self.status_json()),
            UpdateRequest::Check { wait_ms, download } => {
                self.check_or_download(false, download, wait_ms, on_ui)
            }
            UpdateRequest::Download { wait_ms } => {
                self.check_or_download(true, true, wait_ms, on_ui)
            }
            UpdateRequest::Apply => {
                if on_ui {
                    return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                        Err(CommandError::Failed(
                            "eludite.update.apply runs on the UI thread through the shell (Help > Check for \
                             Updates, or the status bar's Restart to update)"
                                .into(),
                        ))
                    });
                }
                let (reply, rx) = mpsc::sync_channel(1);
                self.jobs
                    .unbounded_send(UpdateJob { reply })
                    .map_err(|_| CommandError::Failed("the window is closed".into()))?;
                rx.recv_timeout(Duration::from_secs(30))
                    .map_err(|_| CommandError::Failed("the UI did not answer".into()))?
            }
        }
    }
}

/// Register `eludite.update.*` on `commands`. The updater itself starts with the shell ([`Shell::update_install`]).
pub fn register(commands: &CommandRegistry) -> (Arc<UpdateService>, UnboundedReceiver<UpdateJob>) {
    let (jobs, rx) = unbounded();
    let service = Arc::new(UpdateService {
        updater: Mutex::new(None),
        ui_thread: std::thread::current().id(),
        jobs,
    });
    cmds::register(commands, service.clone());
    (service, rx)
}

/// The update half of the shell.
pub struct UpdateUi {
    pub service: Arc<UpdateService>,
    pub updater: Option<Updater>,
    restarter: Option<Restarter>,
    relaunch: Option<Relaunch>,
    quit_on_apply: bool,
    /// The last state kind written to the Output window, so a repeated snapshot (a progress tick) adds no line.
    last_kind: Option<&'static str>,
    /// The person ran a check or a download: the next finished snapshot goes to the status bar's text too.
    feedback_pending: bool,
    /// The plan `eludite.update.apply` last wrote (tests read it).
    pub last_plan: Option<PathBuf>,
    pub asked: bool,
}

impl UpdateUi {
    pub fn new(service: Arc<UpdateService>) -> Self {
        Self {
            service,
            updater: None,
            restarter: None,
            relaunch: None,
            quit_on_apply: true,
            last_kind: None,
            feedback_pending: false,
            last_plan: None,
            asked: false,
        }
    }
}

impl Shell {
    /// Start the updater from `setup`, wire its events, the status slot, the timer and the first-start question.
    pub(super) fn update_install(
        &mut self,
        setup: UpdateSetup,
        mut jobs: UnboundedReceiver<UpdateJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt as _;
        let UpdateSetup {
            setup,
            relaunch,
            restarter,
            quit_on_apply,
            ask,
            ask_delay,
            first_check_delay,
            updated_from,
        } = setup;
        self.status.add_slot(UPDATE_SLOT, SlotAlign::Right);
        let (tx, mut events) = unbounded::<Status>();
        let listener: eludite_update::updater::Listener = {
            let tx = Mutex::new(tx);
            Arc::new(move |s: &Status| {
                let _ = tx
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .unbounded_send(s.clone());
            })
        };
        let updater = Updater::new(setup, self.update_config(), Some(listener));
        self.update.service.attach(updater.clone());
        self.update.updater = Some(updater.clone());
        self.update.restarter = Some(restarter);
        self.update.relaunch = Some(relaunch);
        self.update.quit_on_apply = quit_on_apply;
        let status = updater.status();
        if let Some(from) = updated_from {
            let to = status
                .build
                .as_ref()
                .map(|b| b.tag())
                .unwrap_or_else(|| "this build".into());
            let line = format!("Eludite updated from {from} to {to}.");
            self.update_log(&line, cx);
            self.status.set(slots::STATE, line);
        }
        self.update_on_status(&status, cx);

        let events_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                // A burst of snapshots costs one update per state change: progress ticks of one state collapse into
                // the last of them, but no state is skipped, so the Output window's lines are the same whether the
                // download took a second or a millisecond.
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    if batch
                        .last()
                        .is_some_and(|b: &Status| b.state.kind() == more.state.kind())
                    {
                        batch.pop();
                    }
                    batch.push(more);
                }
                if this
                    .update(cx, |shell, cx| {
                        for status in &batch {
                            shell.update_on_status(status, cx);
                        }
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let jobs_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(UpdateJob { reply }) = jobs.next().await {
                let outcome = this
                    .update(cx, |shell, cx| shell.update_apply_now(cx))
                    .unwrap_or_else(|_| Err(CommandError::Failed("the window is closed".into())));
                let quit = outcome.is_ok();
                let _ = reply.send(outcome);
                if quit {
                    let _ = this.update(cx, |shell, cx| {
                        if shell.update.quit_on_apply {
                            cx.quit();
                        }
                    });
                }
            }
        });
        // The timer: the previous build's leftovers go first (a packaged build only), then the checks the mode asks
        // for, at most every CHECK_INTERVAL.
        let timer_updater = updater.clone();
        let timer_task = cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(first_check_delay).await;
            if timer_updater.status().enabled {
                let stage = timer_updater.stage().clone();
                cx.background_executor()
                    .spawn(async move {
                        for removed in stage.clean_after_start() {
                            eprintln!("eludite: removed {}", removed.display());
                        }
                    })
                    .detach();
            }
            loop {
                if timer_updater.due(CHECK_INTERVAL) {
                    let mode = timer_updater.config().mode;
                    timer_updater.check(mode == Mode::Download);
                }
                if this.update(cx, |_, _| ()).is_err() {
                    break;
                }
                cx.background_executor().timer(TICK).await;
            }
        });
        let mut tasks = vec![events_task, jobs_task, timer_task];
        if ask && status.enabled && status.mode == Mode::Ask {
            tasks.push(cx.spawn_in(window, async move |this, cx| {
                cx.background_executor().timer(ask_delay).await;
                let _ = this.update_in(cx, |shell, window, cx| shell.update_ask(window, cx));
            }));
        }
        self._tasks.extend(tasks);
    }

    /// `updates.channel` and `updates.mode` as set.
    fn update_config(&self) -> Config {
        let s = self.settings.lock();
        Config {
            channel: Channel::parse(&s.string(CHANNEL_KEY)).unwrap_or(Channel::Unstable),
            mode: Mode::parse(&s.string(MODE_KEY)).unwrap_or(Mode::Ask),
        }
    }

    /// The settings changed.
    pub(super) fn update_apply_settings(&mut self) {
        let config = self.update_config();
        if let Some(u) = &self.update.updater
            && u.config() != config
        {
            u.set_config(config);
        }
    }

    /// The first-start question of a packaged build whose mode is still `ask`.
    fn update_ask(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(updater) = self.update.updater.clone() else {
            return;
        };
        if self.update.asked || updater.config().mode != Mode::Ask {
            return;
        }
        self.update.asked = true;
        let channel = updater.config().channel;
        let answer = window.prompt(
            PromptLevel::Info,
            "Check for new Eludite builds automatically?",
            Some(&format!(
                "Eludite can read the {channel} channel's releases on GitHub about 15 seconds after it opens and \
                 every 4 hours, and download a newer build in the background to install when you restart. \
                 Eludite makes no other network call on its own. You can change this under Tools > Options > \
                 Environment > Updates."
            )),
            &["Yes, download them", "Only tell me", "No"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            let mode = match answer.await {
                Ok(0) => Mode::Download,
                Ok(1) => Mode::Notify,
                Ok(2) => Mode::Off,
                _ => return,
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.run(
                    eludite_commands::settings::SET,
                    json!({ "key": MODE_KEY, "value": mode.as_str() }),
                    window,
                    cx,
                );
                shell.update_log(&format!("Automatic updates: {}.", mode.as_str()), cx);
                // The answer applies now, not at the next tick.
                if mode.checks()
                    && let Some(u) = &shell.update.updater
                {
                    u.set_config(Config {
                        channel: u.config().channel,
                        mode,
                    });
                    u.check(mode == Mode::Download);
                }
            });
        })
        .detach();
    }

    /// One line in the Output window's Updates source.
    fn update_log(&mut self, line: &str, cx: &mut Context<Self>) {
        self.output.update(cx, |o, cx| {
            o.append(OutputSource::Updates, &format!("{line}\n"), cx)
        });
    }

    /// A status snapshot: the slot, the Output window, the status text.
    fn update_on_status(&mut self, status: &Status, cx: &mut Context<Self>) {
        let (text, action): (String, Option<&str>) = match &status.state {
            State::Downloading {
                received, total, ..
            } => {
                let mb = *received as f64 / 1_048_576.0;
                match total {
                    Some(t) if *t > 0 => (
                        format!(
                            "Downloading update… {}%",
                            (*received as f64 / *t as f64 * 100.0).min(100.0) as u32
                        ),
                        None,
                    ),
                    _ => (format!("Downloading update… {mb:.0} MB"), None),
                }
            }
            State::Unpacking { .. } => ("Unpacking update…".into(), None),
            _ if status.busy => ("Checking for updates…".into(), None),
            State::Available { update } => (
                format!("Update available: {}", update.tag),
                Some(cmds::DOWNLOAD),
            ),
            State::Ready { update, .. } => (
                format!("Restart to update ({})", update.tag),
                Some(cmds::APPLY),
            ),
            State::Failed { .. } => ("Update failed".into(), Some(cmds::CHECK)),
            _ => (String::new(), None),
        };
        self.status.set(UPDATE_SLOT, text);
        if let Some(command) = action {
            self.status.set_action(UPDATE_SLOT, command, json!({}));
        }
        let kind = status.state.kind();
        if self.update.last_kind != Some(kind) {
            let line = match &status.state {
                State::Idle => None,
                State::UpToDate { tag } => Some(format!(
                    "Eludite is up to date: {} is the newest build of the {} channel.",
                    tag, status.channel
                )),
                State::NoRelease => Some(format!(
                    "The {} channel has no release yet.",
                    status.channel
                )),
                State::NoArchive { tag } => Some(format!(
                    "{tag} has no archive for this platform; nothing to install."
                )),
                State::Available { update } => Some(format!(
                    "A newer build is available: {} ({}, {:.0} MB){}",
                    update.tag,
                    update.archive,
                    update.size as f64 / 1_048_576.0,
                    match (&update.html_url, status.mode) {
                        (Some(u), Mode::Download) => format!("; downloading. {u}"),
                        (Some(u), _) => format!(". Click the status bar to download it. {u}"),
                        (None, Mode::Download) => "; downloading.".into(),
                        (None, _) => ". Click the status bar to download it.".into(),
                    }
                )),
                State::Downloading { update, .. } => {
                    Some(format!("Downloading {}…", update.archive))
                }
                State::Unpacking { update } => Some(format!(
                    "{} verified against SHA256SUMS; unpacking.",
                    update.archive
                )),
                State::Ready { update, staged } => Some(format!(
                    "{} verified against SHA256SUMS and staged in {}. Restart Eludite to install it (the status \
                     bar, or eludite.update.apply).",
                    update.tag,
                    staged.layout.display()
                )),
                State::Failed { error, message, .. } => {
                    Some(format!("Update failed ({error}): {message}"))
                }
            };
            if let Some(line) = line {
                self.update_log(&line, cx);
            }
        }
        self.update.last_kind = Some(kind);
        // The person asked (Help > Check for Updates, the status bar): the answer goes where they are looking.
        if self.update.feedback_pending && !status.busy {
            self.update.feedback_pending = false;
            let short = match &status.state {
                State::UpToDate { tag } => Some(format!("Eludite is up to date ({tag})")),
                State::NoRelease => Some(format!("No {} release yet", status.channel)),
                State::NoArchive { .. } => Some("No build for this platform".to_owned()),
                State::Available { update } => Some(format!("Update available: {}", update.tag)),
                State::Ready { update, .. } => {
                    Some(format!("{} is ready; restart to install it", update.tag))
                }
                State::Failed { error, .. } => {
                    Some(format!("Update failed ({error}); see Output > Updates"))
                }
                State::Idle | State::Downloading { .. } | State::Unpacking { .. } => None,
            };
            if let Some(short) = short {
                self.status.set(slots::STATE, short);
            }
        }
        cx.notify();
    }

    /// Visual Studio's confirmations for `eludite.update.*` from the UI: `apply` asks before quitting. The other
    /// commands go to the bus as they are.
    pub(super) fn run_update(
        &mut self,
        command: &str,
        _args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if command == cmds::CHECK || command == cmds::DOWNLOAD {
            // Runs on the bus as it is; the result is shown when the updater answers.
            self.update.feedback_pending = self
                .update
                .updater
                .as_ref()
                .is_some_and(|u| u.status().enabled);
            return false;
        }
        if command != cmds::APPLY {
            return false;
        }
        let Some(updater) = self.update.updater.clone() else {
            return false;
        };
        let State::Ready { update, .. } = updater.status().state else {
            self.status
                .set(slots::STATE, "No update is staged; check for updates first");
            cx.notify();
            return true;
        };
        let answer = window.prompt(
            PromptLevel::Warning,
            &format!("Restart Eludite now to install {}?", update.tag),
            Some(
                "Eludite quits, every agent session with it, replaces its files and starts again with the same \
                 workspace. Save your work first: open files are not saved.",
            ),
            &["Restart Now", "Later"],
            cx,
        );
        cx.spawn_in(window, async move |this, cx| {
            if answer.await != Ok(0) {
                return;
            }
            let _ = this.update_in(cx, |shell, window, cx| {
                let outcome = shell.update_apply_now(cx);
                let ok = outcome.is_ok();
                stage(outcome);
                let result = shell.invoke(cmds::APPLY, json!({}), window, cx);
                match &result {
                    Ok(_) if ok && shell.update.quit_on_apply => cx.quit(),
                    Ok(_) => {}
                    Err(e) => {
                        shell.status.set(slots::STATE, e.to_string());
                        cx.notify();
                    }
                }
            });
        })
        .detach();
        true
    }

    /// Start the applier for the staged build (the UI-thread half of `apply`); the caller quits on `Ok`.
    fn update_apply_now(&mut self, cx: &mut Context<Self>) -> Result<Value, CommandError> {
        let updater = self
            .update
            .updater
            .clone()
            .ok_or_else(|| CommandError::Failed("the updater has not started".into()))?;
        let status = updater.status();
        let State::Ready { staged, .. } = &status.state else {
            return Err(CommandError::Failed(
                "no update is staged: run eludite.update.download first (its state must be `ready`)".into(),
            ));
        };
        let plan = apply::plan(
            updater.stage(),
            staged,
            updater.platform(),
            status.build.clone(),
            self.update.relaunch.clone(),
        );
        let restarter = self
            .update
            .restarter
            .clone()
            .ok_or_else(|| CommandError::Failed("the updater has not started".into()))?;
        let path = restarter(&plan).map_err(CommandError::Failed)?;
        self.update.last_plan = Some(path.clone());
        self.update_log(
            &format!(
                "Restarting to install {}: the applier's plan is {} (its log is beside it).",
                staged.tag,
                path.display()
            ),
            cx,
        );
        Ok(json!({
            "applying": true,
            "tag": staged.tag,
            "plan": path.to_string_lossy(),
        }))
    }
}
