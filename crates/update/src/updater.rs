//! The updater: one worker thread that checks the channel, downloads and stages, with a [`Status`] snapshot any
//! thread can read and a listener told of every change. Nothing runs until asked ([`Updater::check`],
//! [`Updater::download`], or [`Updater::due`] answered by the shell's timer), so a start makes no network call.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use serde::{Deserialize, Serialize};

use crate::build::{Build, Channel, Platform};
use crate::download::{self, ReleaseList, Source};
use crate::error::Error;
use crate::http::{Cancel, Transport};
use crate::release::{self, Candidate, Nothing};
use crate::stage::{Stage, Staged};

/// What the updater does on its own (the setting `updates.mode`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Mode {
    /// Not decided yet: the shell asks once, on the first start of a packaged build; nothing runs until answered.
    Ask,
    /// Check on a timer and say when a build is available; download only when asked.
    Notify,
    /// Check on a timer, download and stage; install when the person restarts.
    Download,
    /// Never check. Help > Check for Updates still works.
    Off,
}

impl Mode {
    pub const ALL: [Mode; 4] = [Mode::Ask, Mode::Notify, Mode::Download, Mode::Off];

    pub fn as_str(self) -> &'static str {
        match self {
            Mode::Ask => "ask",
            Mode::Notify => "notify",
            Mode::Download => "download",
            Mode::Off => "off",
        }
    }

    pub fn parse(s: &str) -> Option<Mode> {
        Mode::ALL.into_iter().find(|m| m.as_str() == s)
    }

    /// Whether the timer checks.
    pub fn checks(self) -> bool {
        matches!(self, Mode::Notify | Mode::Download)
    }
}

/// The settings the updater follows.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Config {
    pub channel: Channel,
    pub mode: Mode,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            channel: Channel::Unstable,
            mode: Mode::Ask,
        }
    }
}

/// Where this Eludite is installed and where releases come from.
pub struct Setup {
    /// The folder holding the `eludite` executable.
    pub install_dir: PathBuf,
    pub platform: Platform,
    /// This build (`build.json` beside the executable), `Ok(None)` for a development build.
    pub build: Result<Option<Build>, String>,
    /// Where the release list cache and the last check time are kept (`<config dir>/eludite/updates.json`).
    pub state_file: Option<PathBuf>,
    pub source: Source,
    pub transport: Arc<dyn Transport>,
}

impl std::fmt::Debug for Setup {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Setup")
            .field("install_dir", &self.install_dir)
            .field("platform", &self.platform)
            .field("build", &self.build)
            .field("state_file", &self.state_file)
            .field("source", &self.source)
            .finish()
    }
}

impl Setup {
    /// This executable's folder and build, GitHub as the source, the real transport.
    pub fn detect(repository: &str, state_file: Option<PathBuf>) -> Setup {
        let install_dir = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(Path::to_owned))
            .unwrap_or_else(|| PathBuf::from("."));
        let build = Build::read(&install_dir);
        Setup {
            install_dir,
            platform: Platform::current(),
            build,
            state_file,
            source: Source::github(repository),
            transport: Arc::new(crate::http::UreqTransport::default()),
        }
    }
}

/// A build on offer, as the status shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub tag: String,
    pub channel: Channel,
    pub build: crate::build::BuildId,
    pub version: String,
    pub published_at: Option<String>,
    pub html_url: Option<String>,
    /// The archive's name and size.
    pub archive: String,
    pub size: u64,
}

impl From<&Candidate> for Summary {
    fn from(c: &Candidate) -> Self {
        Summary {
            tag: c.tag.clone(),
            channel: c.channel,
            build: c.build.clone(),
            version: c.version.clone(),
            published_at: c.published_at.clone(),
            html_url: c.html_url.clone(),
            archive: c.archive.asset.name.clone(),
            size: c.archive.asset.size,
        }
    }
}

/// Where the updater is.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum State {
    /// Never checked since the start.
    Idle,
    UpToDate {
        tag: String,
    },
    NoRelease,
    NoArchive {
        tag: String,
    },
    Available {
        update: Summary,
    },
    Downloading {
        update: Summary,
        received: u64,
        total: Option<u64>,
    },
    Unpacking {
        update: Summary,
    },
    Ready {
        update: Summary,
        staged: Staged,
    },
    Failed {
        update: Option<Summary>,
        error: String,
        message: String,
        at: String,
    },
}

impl State {
    pub fn kind(&self) -> &'static str {
        match self {
            State::Idle => "idle",
            State::UpToDate { .. } => "up_to_date",
            State::NoRelease => "no_release",
            State::NoArchive { .. } => "no_archive",
            State::Available { .. } => "available",
            State::Downloading { .. } => "downloading",
            State::Unpacking { .. } => "unpacking",
            State::Ready { .. } => "ready",
            State::Failed { .. } => "failed",
        }
    }

    /// The build on offer or staged, if any.
    pub fn update(&self) -> Option<&Summary> {
        match self {
            State::Available { update }
            | State::Downloading { update, .. }
            | State::Unpacking { update }
            | State::Ready { update, .. } => Some(update),
            State::Failed { update, .. } => update.as_ref(),
            _ => None,
        }
    }
}

/// A snapshot of the updater.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Status {
    /// Whether this Eludite can update itself: a packaged build (`build.json` beside the executable).
    pub enabled: bool,
    /// Why not, when it cannot.
    pub reason: Option<String>,
    pub build: Option<Build>,
    pub install_dir: PathBuf,
    pub channel: Channel,
    pub mode: Mode,
    pub state: State,
    /// The last check's time, RFC 3339 (kept across starts).
    pub last_check: Option<String>,
    /// A check or a download is running.
    pub busy: bool,
    /// Whether the install folder could be written, once a download tried (`None` before).
    pub writable: Option<bool>,
}

/// What the state file keeps.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
struct Persisted {
    last_check: Option<String>,
    #[serde(default)]
    releases: ReleaseList,
}

enum Job {
    Check { then_download: bool },
    Download,
}

struct Inner {
    setup: Setup,
    stage: Stage,
    config: Mutex<Config>,
    status: Mutex<Status>,
    candidate: Mutex<Option<Candidate>>,
    persisted: Mutex<Persisted>,
    cancel: Mutex<Cancel>,
    /// Jobs queued and finished; waited on by `wait`.
    done: (Mutex<Counts>, Condvar),
    listener: Mutex<Option<Listener>>,
}

/// Told after every status change, on the worker thread.
pub type Listener = Arc<dyn Fn(&Status) + Send + Sync>;

/// The updater. Cheap to clone; one worker thread per [`Updater::new`].
#[derive(Clone)]
pub struct Updater {
    inner: Arc<Inner>,
    jobs: Arc<Mutex<Sender<Job>>>,
}

impl std::fmt::Debug for Updater {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Updater")
            .field("status", &self.status())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl Updater {
    /// Start the worker. A staged update left by an earlier run is reported as `ready` at once, with no network call.
    pub fn new(setup: Setup, config: Config, listener: Option<Listener>) -> Updater {
        let stage = Stage::new(&setup.install_dir);
        let (enabled, reason, build) = match &setup.build {
            Ok(Some(b)) => (true, None, Some(b.clone())),
            Ok(None) => (
                false,
                Some("a development build: no build.json beside the executable".to_owned()),
                None,
            ),
            Err(e) => (false, Some(e.clone()), None),
        };
        let persisted = setup
            .state_file
            .as_ref()
            .and_then(|p| std::fs::read_to_string(p).ok())
            .and_then(|t| serde_json::from_str::<Persisted>(&t).ok())
            .unwrap_or_default();
        let state = match (enabled, stage.staged()) {
            (true, Some(staged)) if build.as_ref().is_none_or(|b| staged.build != *b) => {
                State::Ready {
                    update: Summary {
                        tag: staged.tag.clone(),
                        channel: staged.build.channel,
                        build: staged.build.build.clone(),
                        version: staged.build.version.clone(),
                        published_at: staged.build.published.clone(),
                        html_url: None,
                        archive: staged
                            .archive
                            .file_name()
                            .map(|n| n.to_string_lossy().into_owned())
                            .unwrap_or_default(),
                        size: std::fs::metadata(&staged.archive)
                            .map(|m| m.len())
                            .unwrap_or(0),
                    },
                    staged,
                }
            }
            _ => State::Idle,
        };
        let status = Status {
            enabled,
            reason,
            build,
            install_dir: setup.install_dir.clone(),
            channel: config.channel,
            mode: config.mode,
            state,
            last_check: persisted.last_check.clone(),
            busy: false,
            writable: None,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let inner = Arc::new(Inner {
            setup,
            stage,
            config: Mutex::new(config),
            status: Mutex::new(status),
            candidate: Mutex::new(None),
            persisted: Mutex::new(persisted),
            cancel: Mutex::new(Cancel::new()),
            done: (Mutex::new(Counts::default()), Condvar::new()),
            listener: Mutex::new(listener),
        });
        let worker = inner.clone();
        std::thread::Builder::new()
            .name("eludite-update".into())
            .spawn(move || worker.run(rx))
            .expect("the update thread starts");
        Updater {
            inner,
            jobs: Arc::new(Mutex::new(tx)),
        }
    }

    pub fn status(&self) -> Status {
        lock(&self.inner.status).clone()
    }

    pub fn config(&self) -> Config {
        *lock(&self.inner.config)
    }

    pub fn install_dir(&self) -> &Path {
        &self.inner.setup.install_dir
    }

    pub fn stage(&self) -> &Stage {
        &self.inner.stage
    }

    pub fn platform(&self) -> &Platform {
        &self.inner.setup.platform
    }

    /// Apply new settings. A channel change forgets the last check's answer (the next check is of the new channel).
    pub fn set_config(&self, config: Config) {
        let changed_channel = {
            let mut c = lock(&self.inner.config);
            let changed = c.channel != config.channel;
            *c = config;
            changed
        };
        self.inner.update(|s| {
            s.channel = config.channel;
            s.mode = config.mode;
            if changed_channel
                && !matches!(s.state, State::Ready { .. } | State::Downloading { .. })
            {
                s.state = State::Idle;
                s.last_check = None;
            }
        });
        if changed_channel {
            *lock(&self.inner.candidate) = None;
            let mut p = lock(&self.inner.persisted);
            p.last_check = None;
        }
    }

    /// Check the channel now; `then_download` stages what it finds. Returns at once; [`Updater::wait`] blocks.
    pub fn check(&self, then_download: bool) {
        self.send(Job::Check { then_download });
    }

    /// Download and stage the available build (checking first when none is known). Returns at once.
    pub fn download(&self) {
        self.send(Job::Download);
    }

    /// Stop the running download; the state becomes `failed` with `canceled`.
    pub fn cancel(&self) {
        lock(&self.inner.cancel).cancel();
    }

    /// Whether the timer should check now: a checking mode, nothing running or staged, and the last check older
    /// than `interval` (or none).
    pub fn due(&self, interval: Duration) -> bool {
        let s = lock(&self.inner.status);
        if !s.enabled || !s.mode.checks() || s.busy {
            return false;
        }
        if matches!(
            s.state,
            State::Ready { .. } | State::Downloading { .. } | State::Unpacking { .. }
        ) {
            return false;
        }
        match s.last_check.as_deref().and_then(crate::time::parse_rfc3339) {
            None => true,
            Some(t) => SystemTime::now()
                .duration_since(t)
                .map(|d| d >= interval)
                .unwrap_or(true),
        }
    }

    /// Wait until the jobs queued before this call have run, at most `timeout`. Returns the status.
    pub fn wait(&self, timeout: Duration) -> Status {
        let deadline = Instant::now() + timeout;
        let (counts, cv) = &self.inner.done;
        let mut g = lock(counts);
        let target = g.queued;
        while g.finished < target && Instant::now() < deadline {
            let left = deadline.saturating_duration_since(Instant::now());
            let (ng, _) = cv.wait_timeout(g, left).unwrap_or_else(|e| e.into_inner());
            g = ng;
        }
        drop(g);
        self.status()
    }

    fn send(&self, job: Job) {
        {
            let (counts, _) = &self.inner.done;
            lock(counts).queued += 1;
        }
        let _ = lock(&self.jobs).send(job);
    }
}

/// Jobs queued and finished, for [`Updater::wait`].
#[derive(Debug, Default)]
struct Counts {
    queued: u64,
    finished: u64,
}

impl Inner {
    fn run(&self, rx: Receiver<Job>) {
        for job in rx {
            *lock(&self.cancel) = Cancel::new();
            self.update(|s| s.busy = true);
            match job {
                Job::Check { then_download } => {
                    let found = self.check();
                    if then_download && found {
                        self.download();
                    }
                }
                Job::Download => {
                    let known = lock(&self.candidate).is_some()
                        || matches!(lock(&self.status).state, State::Ready { .. });
                    if known || self.check() {
                        self.download();
                    }
                }
            }
            self.update(|s| s.busy = false);
            let (counts, cv) = &self.done;
            lock(counts).finished += 1;
            cv.notify_all();
        }
    }

    fn update(&self, f: impl FnOnce(&mut Status)) {
        let snapshot = {
            let mut s = lock(&self.status);
            f(&mut s);
            s.clone()
        };
        if let Some(l) = lock(&self.listener).as_ref() {
            l(&snapshot);
        }
    }

    fn fail(&self, update: Option<Summary>, e: &Error) {
        self.update(|s| {
            s.state = State::Failed {
                update,
                error: e.kind().to_owned(),
                message: e.to_string(),
                at: crate::time::now_rfc3339(),
            }
        });
    }

    fn persist(&self) {
        let Some(path) = &self.setup.state_file else {
            return;
        };
        let text = serde_json::to_string(&*lock(&self.persisted)).expect("serializes");
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(path, text);
    }

    /// Read the channel; true when a newer build is available (or already staged).
    fn check(&self) -> bool {
        if !lock(&self.status).enabled {
            return false;
        }
        if matches!(lock(&self.status).state, State::Ready { .. }) {
            return true;
        }
        let config = *lock(&self.config);
        let cancel = lock(&self.cancel).clone();
        let cached = lock(&self.persisted).releases.clone();
        let list = match download::fetch_releases(
            self.setup.transport.as_ref(),
            &self.setup.source,
            Some(&cached),
            &cancel,
        ) {
            Ok(l) => l,
            Err(e) => {
                self.fail(None, &e);
                return false;
            }
        };
        let now = crate::time::now_rfc3339();
        {
            let mut p = lock(&self.persisted);
            p.releases = list.clone();
            p.last_check = Some(now.clone());
        }
        self.persist();
        let installed = self.setup.build.as_ref().ok().and_then(|b| b.as_ref());
        let chosen = release::choose(
            &list.releases,
            config.channel,
            installed,
            &self.setup.platform,
        );
        let found = chosen.is_ok();
        let state = match &chosen {
            Ok(c) => State::Available { update: c.into() },
            Err(Nothing::UpToDate { tag }) => State::UpToDate { tag: tag.clone() },
            Err(Nothing::NoRelease) => State::NoRelease,
            Err(Nothing::NoArchive { tag, .. }) => State::NoArchive { tag: tag.clone() },
        };
        *lock(&self.candidate) = chosen.ok();
        self.update(|s| {
            s.last_check = Some(now);
            s.state = state;
        });
        found
    }

    fn download(&self) {
        if matches!(lock(&self.status).state, State::Ready { .. }) {
            return;
        }
        let Some(candidate) = lock(&self.candidate).clone() else {
            return;
        };
        let summary = Summary::from(&candidate);
        if let Err(reason) = self.stage.ensure_writable() {
            self.update(|s| s.writable = Some(false));
            self.fail(Some(summary), &Error::Apply(reason));
            return;
        }
        self.update(|s| s.writable = Some(true));
        let cancel = lock(&self.cancel).clone();
        let transport = self.setup.transport.as_ref();
        let result = (|| -> crate::error::Result<Staged> {
            let sums = download::fetch_sums(transport, &candidate.sums.url, &cancel)?;
            let name = &candidate.archive.asset.name;
            let expected = sums.get(name).ok_or_else(|| {
                Error::malformed(format!("{} does not list {name}", release::SUMS_ASSET))
            })?;
            let dest = self.stage.archive_path(&candidate);
            let total_hint = Some(candidate.archive.asset.size).filter(|s| *s > 0);
            self.update(|s| {
                s.state = State::Downloading {
                    update: summary.clone(),
                    received: 0,
                    total: total_hint,
                }
            });
            let last = Mutex::new((Instant::now(), 0u64));
            let progress = |received: u64, total: Option<u64>| {
                let mut l = lock(&last);
                // At most ten status changes a second, and one per megabyte.
                if l.0.elapsed() < Duration::from_millis(100) && received - l.1 < 1 << 20 {
                    return;
                }
                *l = (Instant::now(), received);
                self.update(|s| {
                    s.state = State::Downloading {
                        update: summary.clone(),
                        received,
                        total: total.or(total_hint),
                    }
                });
            };
            download::download_verified(
                transport,
                &candidate.archive.asset.url,
                &dest,
                expected,
                &progress,
                &cancel,
            )?;
            self.update(|s| {
                s.state = State::Unpacking {
                    update: summary.clone(),
                }
            });
            self.stage
                .stage(&candidate, &dest, &self.setup.platform, &cancel)
        })();
        match result {
            Ok(staged) => self.update(|s| {
                s.state = State::Ready {
                    update: summary.clone(),
                    staged,
                }
            }),
            Err(e) => {
                if matches!(e, Error::Canceled) {
                    self.stage.discard();
                    let _ = std::fs::remove_dir_all(self.stage.dir_of(&candidate.tag));
                }
                self.fail(Some(summary), &e);
            }
        }
    }
}
