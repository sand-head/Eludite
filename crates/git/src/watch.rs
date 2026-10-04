//! The status a repository's readers share ([`StatusCache`]) and the watcher that keeps it current ([`Watcher`]).
//!
//! The cache holds the last status and its generation: a number that grows by one whenever a recomputed status
//! differs from the one before, so a reader can tell a stale answer from a current one. Readers never wait on
//! libgit2 ([`StatusCache::snapshot`]); [`StatusCache::refresh`] recomputes on the caller's thread (a command after
//! it changed the repository, or the watcher).
//!
//! The watcher is a thread of its own. Every [`WatchOptions::poll`] it stats `.git/index`, `.git/HEAD`, the files of
//! an operation in progress, `packed-refs`, `config` (for `http.sslVerify`) and the files under `refs/`; a change there, or [`Watcher::touch`] (the
//! shell saved a file), is followed by a recompute once [`WatchOptions::debounce`] (200 ms) passes with no further
//! change. Working-tree changes made outside Eludite are found by a recompute every [`WatchOptions::rescan`] (1 s,
//! longer when a status takes long: ten times its cost). Polling, as the settings store polls its files: the `notify`
//! crate is not in the build, and the brief allows no new dependency.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime};

use crate::{Repo, Result, Status};

/// Listens for a new generation (on the thread that computed it).
pub type Listener = Box<dyn Fn(u64) + Send + Sync>;

struct CacheState {
    status: Option<Arc<Status>>,
    generation: u64,
    error: Option<String>,
    last_duration: Duration,
}

struct Inner {
    repo: Repo,
    include_ignored: bool,
    state: Mutex<CacheState>,
    /// One recompute at a time, so generations follow the order the statuses were computed in.
    refresh: Mutex<()>,
    listeners: Mutex<Vec<Listener>>,
}

/// A repository's last status and its generation. Cheap to clone; shared by every reader.
#[derive(Clone)]
pub struct StatusCache {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for StatusCache {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("StatusCache")
            .field("repo", &self.inner.repo.workdir())
            .field("generation", &self.generation())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl StatusCache {
    /// An empty cache for `repo` (generation 0: nothing computed yet); statuses include ignored files when
    /// `include_ignored`.
    pub fn new(repo: Repo, include_ignored: bool) -> Self {
        Self {
            inner: Arc::new(Inner {
                repo,
                include_ignored,
                state: Mutex::new(CacheState {
                    status: None,
                    generation: 0,
                    error: None,
                    last_duration: Duration::ZERO,
                }),
                refresh: Mutex::new(()),
                listeners: Mutex::new(Vec::new()),
            }),
        }
    }

    pub fn repo(&self) -> &Repo {
        &self.inner.repo
    }

    /// The last status, its generation and the last error, without waiting.
    pub fn snapshot(&self) -> (u64, Option<Arc<Status>>, Option<String>) {
        let s = lock(&self.inner.state);
        (s.generation, s.status.clone(), s.error.clone())
    }

    pub fn generation(&self) -> u64 {
        lock(&self.inner.state).generation
    }

    /// How long the last recompute took.
    pub fn last_duration(&self) -> Duration {
        lock(&self.inner.state).last_duration
    }

    /// Call `f` with each new generation, on the thread that computed it.
    pub fn on_change(&self, f: impl Fn(u64) + Send + Sync + 'static) {
        lock(&self.inner.listeners).push(Box::new(f));
    }

    /// Recompute the status on this thread; the generation grows when it differs from the last. Returns the
    /// generation and the status now.
    pub fn refresh(&self) -> Result<(u64, Arc<Status>)> {
        let _one = lock(&self.inner.refresh);
        let started = Instant::now();
        let result = self.inner.repo.status(self.inner.include_ignored);
        let took = started.elapsed();
        let (generation, status, changed) = {
            let mut s = lock(&self.inner.state);
            s.last_duration = took;
            match result {
                Ok(status) => {
                    let changed = s.status.as_deref() != Some(&status) || s.error.is_some();
                    if changed {
                        s.generation += 1;
                        s.status = Some(Arc::new(status));
                        s.error = None;
                    }
                    (s.generation, s.status.clone().expect("set"), changed)
                }
                Err(e) => {
                    if s.error.as_deref() != Some(e.message.as_str()) {
                        s.error = Some(e.message.clone());
                        s.generation += 1;
                        let g = s.generation;
                        drop(s);
                        for l in lock(&self.inner.listeners).iter() {
                            l(g);
                        }
                    }
                    return Err(e);
                }
            }
        };
        if changed {
            for l in lock(&self.inner.listeners).iter() {
                l(generation);
            }
        }
        Ok((generation, status))
    }
}

/// How the watcher polls.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    pub poll: Duration,
    pub debounce: Duration,
    pub rescan: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            poll: Duration::from_millis(100),
            debounce: Duration::from_millis(200),
            rescan: Duration::from_secs(1),
        }
    }
}

type Stamp = Vec<(PathBuf, Option<(SystemTime, u64)>)>;

/// Refs files stat-ed at most per poll.
const MAX_REF_FILES: usize = 2_000;

fn stamp_of(p: &Path) -> Option<(SystemTime, u64)> {
    let m = std::fs::metadata(p).ok()?;
    Some((m.modified().ok()?, m.len()))
}

/// What the watcher compares between polls.
fn stamps(repo: &Repo) -> Stamp {
    let g = repo.git_dir();
    let c = repo.common_dir();
    let mut out: Stamp = [
        g.join("index"),
        g.join("HEAD"),
        g.join("MERGE_HEAD"),
        g.join("CHERRY_PICK_HEAD"),
        g.join("REVERT_HEAD"),
        g.join("rebase-merge"),
        g.join("rebase-apply"),
        c.join("packed-refs"),
        c.join("FETCH_HEAD"),
        c.join("config"),
    ]
    .into_iter()
    .map(|p| {
        let s = stamp_of(&p);
        (p, s)
    })
    .collect();
    let mut dirs = vec![c.join("refs")];
    let mut n = 0;
    while let Some(d) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&d) else {
            continue;
        };
        for e in entries.flatten() {
            n += 1;
            if n > MAX_REF_FILES {
                return out;
            }
            let p = e.path();
            if e.file_type().is_ok_and(|t| t.is_dir()) {
                dirs.push(p);
            } else {
                let s = stamp_of(&p);
                out.push((p, s));
            }
        }
    }
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

struct Signal {
    touched: Mutex<bool>,
    wake: Condvar,
}

/// Keeps a [`StatusCache`] current from its own thread; stops when dropped.
pub struct Watcher {
    stop: Arc<AtomicBool>,
    signal: Arc<Signal>,
}

impl std::fmt::Debug for Watcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Watcher").finish_non_exhaustive()
    }
}

impl Watcher {
    /// Start watching `cache`'s repository; the first status is computed at once, on the watcher's thread.
    pub fn start(cache: StatusCache, opts: WatchOptions) -> Watcher {
        let stop = Arc::new(AtomicBool::new(false));
        let signal = Arc::new(Signal {
            touched: Mutex::new(false),
            wake: Condvar::new(),
        });
        let (stop2, signal2) = (stop.clone(), signal.clone());
        let spawned = std::thread::Builder::new()
            .name("eludite-git-watch".into())
            .spawn(move || run(cache, opts, &stop2, &signal2));
        if let Err(e) = spawned {
            eprintln!("eludite: the git watcher did not start: {e}");
        }
        Watcher { stop, signal }
    }

    /// Something changed in the working tree (a save): recompute after the debounce.
    pub fn touch(&self) {
        *lock(&self.signal.touched) = true;
        self.signal.wake.notify_all();
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        self.signal.wake.notify_all();
    }
}

fn run(cache: StatusCache, opts: WatchOptions, stop: &AtomicBool, signal: &Signal) {
    let _ = cache.refresh();
    let mut seen = stamps(cache.repo());
    let mut last_event: Option<Instant> = None;
    let mut last_scan = Instant::now();
    while !stop.load(Ordering::SeqCst) {
        {
            let touched = lock(&signal.touched);
            let (mut touched, _) = signal
                .wake
                .wait_timeout(touched, opts.poll)
                .unwrap_or_else(|e| e.into_inner());
            if *touched {
                *touched = false;
                last_event = Some(Instant::now());
            }
        }
        if stop.load(Ordering::SeqCst) {
            break;
        }
        let now = stamps(cache.repo());
        if now != seen {
            seen = now;
            last_event = Some(Instant::now());
        }
        let rescan = opts.rescan.max(cache.last_duration() * 10);
        let due = match last_event {
            Some(at) => at.elapsed() >= opts.debounce,
            None => last_scan.elapsed() >= rescan,
        };
        if due {
            let _ = cache.refresh();
            last_event = None;
            last_scan = Instant::now();
            // The recompute itself may refresh the index's stat cache: do not take that for a change.
            seen = stamps(cache.repo());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::TestRepo;

    fn wait_for(what: &str, mut f: impl FnMut() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !f() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn the_generation_grows_only_when_the_status_changes() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        t.commit_all("init");
        let cache = StatusCache::new(t.repo.clone(), true);
        assert_eq!(cache.snapshot().0, 0);
        let heard = Arc::new(Mutex::new(Vec::new()));
        let h = heard.clone();
        cache.on_change(move |g| h.lock().unwrap().push(g));
        assert_eq!(cache.refresh().unwrap().0, 1);
        assert_eq!(cache.refresh().unwrap().0, 1, "unchanged");
        t.write("a.cs", "b\n");
        let (g, s) = cache.refresh().unwrap();
        assert_eq!((g, s.unstaged.len()), (2, 1));
        assert_eq!(*heard.lock().unwrap(), [1, 2]);
    }

    #[test]
    fn the_watcher_bumps_the_generation_on_an_index_write_and_a_touch() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        t.commit_all("init");
        let cache = StatusCache::new(t.repo.clone(), false);
        let opts = WatchOptions {
            poll: Duration::from_millis(10),
            debounce: Duration::from_millis(30),
            rescan: Duration::from_secs(3600),
        };
        let w = Watcher::start(cache.clone(), opts);
        wait_for("the first status", || cache.generation() == 1);
        // An index write from outside the cache (as `git add` would do).
        t.write("a.cs", "changed\n");
        let repo = t.repo.repository().unwrap();
        let mut index = repo.index().unwrap();
        index.add_path(Path::new("a.cs")).unwrap();
        index.write().unwrap();
        wait_for("the index write", || cache.generation() == 2);
        assert_eq!(cache.snapshot().1.unwrap().staged.len(), 1);
        // A working-tree change is seen after a touch (the rescan is an hour away here).
        t.write("b.cs", "b\n");
        std::thread::sleep(Duration::from_millis(80));
        assert_eq!(cache.generation(), 2, "no poll of the working tree yet");
        w.touch();
        wait_for("the touch", || cache.generation() == 3);
        assert_eq!(cache.snapshot().1.unwrap().untracked, ["b.cs"]);
        drop(w);
    }

    #[test]
    fn the_rescan_finds_working_tree_changes() {
        let t = TestRepo::new();
        t.write("a.cs", "a\n");
        t.commit_all("init");
        let cache = StatusCache::new(t.repo.clone(), false);
        let _w = Watcher::start(
            cache.clone(),
            WatchOptions {
                poll: Duration::from_millis(10),
                debounce: Duration::from_millis(20),
                rescan: Duration::from_millis(50),
            },
        );
        wait_for("the first status", || cache.generation() == 1);
        t.write("a.cs", "edited elsewhere\n");
        wait_for("the rescan", || cache.generation() == 2);
    }
}
