//! The Workspace window's watcher (brief 0073): keeps the tree current when files are added, removed or renamed
//! outside Eludite, and has the host re-evaluate when a project, `.props`, `.targets` or solution file changes.
//!
//! Polling, as brief 0040's Git watcher and the settings store poll (the `notify` crate is not in the build). A
//! thread of its own stats every folder under the roots each [`WatchOptions::poll`]: a folder's modification time
//! changes when an entry is added, removed or renamed in it, so only a folder whose time moved is listed again, and
//! files are stat'ed only when they are project files (whose content matters). The cost of a tick is one `stat` per
//! folder and per project file, not per file. A folder modified within the last two seconds is listed on every tick
//! until it is older (file systems with coarse times). Folders never walked: `bin`, `obj`, `node_modules`, `target`,
//! `packages` and hidden ones (`.git`, `.vs`). A change is reported once [`WatchOptions::debounce`] passes with no
//! further change, on the listener's side of a channel; the UI thread never waits on a scan.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant, SystemTime};

/// Folders never walked.
pub const SKIPPED: [&str; 5] = ["bin", "obj", "node_modules", "target", "packages"];

/// How often to look, and how long a change must settle before it is reported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WatchOptions {
    pub poll: Duration,
    pub debounce: Duration,
}

impl Default for WatchOptions {
    fn default() -> Self {
        Self {
            poll: Duration::from_secs(1),
            debounce: Duration::from_millis(300),
        }
    }
}

/// What changed since the last report.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct WatchReport {
    /// Files and folders that appeared, sorted.
    pub added: Vec<PathBuf>,
    /// Files and folders that went away, sorted.
    pub removed: Vec<PathBuf>,
    /// A project, `.props`, `.targets`, `.sln` or `.slnx` file was added, removed or written.
    pub project_files: bool,
    /// An added or removed path lies under one of the project folders.
    pub under_projects: bool,
}

impl WatchReport {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty() && !self.project_files
    }
}

/// Whether `path` is a file whose writes make the host re-evaluate.
pub fn is_project_file(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .map(str::to_ascii_lowercase)
        .is_some_and(|e| {
            matches!(
                e.as_str(),
                "csproj" | "vbproj" | "fsproj" | "props" | "targets" | "sln" | "slnx"
            )
        })
}

fn skipped(name: &str) -> bool {
    name.starts_with('.') || SKIPPED.iter().any(|s| s.eq_ignore_ascii_case(name))
}

struct Dir {
    mtime: Option<SystemTime>,
    /// When it was last listed.
    listed: SystemTime,
    files: BTreeSet<String>,
    dirs: BTreeSet<String>,
}

/// The folders under the roots and the project files in them: what one tick compares with.
#[derive(Default)]
pub struct Snapshot {
    dirs: HashMap<PathBuf, Dir>,
    projects: HashMap<PathBuf, (u64, Option<SystemTime>)>,
    /// Folders and files found, for the numbers.
    pub files: usize,
}

impl Snapshot {
    /// Walk `roots` (a root inside another is walked once).
    pub fn scan(roots: &[PathBuf]) -> Self {
        let mut s = Snapshot::default();
        for r in roots {
            if !s.dirs.contains_key(r) && !roots.iter().any(|o| o != r && r.starts_with(o)) {
                s.add_dir(r, &mut Vec::new());
            }
        }
        s
    }

    pub fn folders(&self) -> usize {
        self.dirs.len()
    }

    /// List `dir` and everything under it; every path found goes to `added`.
    fn add_dir(&mut self, dir: &Path, added: &mut Vec<PathBuf>) {
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Some(listing) = list(&d) else { continue };
            for name in &listing.dirs {
                let p = d.join(name);
                added.push(p.clone());
                stack.push(p);
            }
            for name in &listing.files {
                let p = d.join(name);
                if is_project_file(&p) {
                    self.projects.insert(p.clone(), stamp(&p));
                }
                added.push(p);
            }
            self.files += listing.files.len();
            self.dirs.insert(d, listing);
        }
    }

    /// Forget `dir` and everything under it; every path forgotten goes to `removed`.
    fn remove_dir(&mut self, dir: &Path, removed: &mut Vec<PathBuf>) {
        let gone: Vec<PathBuf> = self
            .dirs
            .keys()
            .filter(|k| k.starts_with(dir))
            .cloned()
            .collect();
        for d in gone {
            if let Some(entry) = self.dirs.remove(&d) {
                removed.extend(entry.files.iter().map(|f| d.join(f)));
                if d != dir {
                    removed.push(d.clone());
                }
            }
        }
        self.projects.retain(|p, _| !p.starts_with(dir));
    }

    /// Look again; what changed since this snapshot was taken (or last ticked).
    pub fn tick(&mut self) -> WatchReport {
        let mut added = Vec::new();
        let mut removed = Vec::new();
        let mut project_files = false;
        let now = SystemTime::now();
        let recent = Duration::from_secs(2);
        let mut dirs: Vec<PathBuf> = self.dirs.keys().cloned().collect();
        dirs.sort();
        for d in dirs {
            let Some(entry) = self.dirs.get(&d) else {
                continue; // under a folder that went away earlier in this tick
            };
            let mtime = std::fs::metadata(&d).ok().map(|m| m.modified().ok());
            let Some(mtime) = mtime else {
                // The folder is gone (its parent's listing reports the folder itself).
                let mut gone = Vec::new();
                self.remove_dir(&d, &mut gone);
                removed.extend(gone);
                continue;
            };
            let settling = entry
                .mtime
                .is_some_and(|t| entry.listed.duration_since(t).unwrap_or_default() < recent);
            if mtime == entry.mtime && !settling {
                continue;
            }
            let Some(listing) = list(&d) else { continue };
            let entry = self.dirs.get_mut(&d).expect("looked up above");
            let new_files: Vec<String> = listing.files.difference(&entry.files).cloned().collect();
            let old_files: Vec<String> = entry.files.difference(&listing.files).cloned().collect();
            let new_dirs: Vec<String> = listing.dirs.difference(&entry.dirs).cloned().collect();
            let old_dirs: Vec<String> = entry.dirs.difference(&listing.dirs).cloned().collect();
            *entry = Dir {
                mtime: listing.mtime,
                listed: now,
                files: listing.files,
                dirs: listing.dirs,
            };
            for f in new_files {
                let p = d.join(f);
                if is_project_file(&p) {
                    project_files = true;
                    self.projects.insert(p.clone(), stamp(&p));
                }
                added.push(p);
            }
            for f in old_files {
                let p = d.join(f);
                if self.projects.remove(&p).is_some() {
                    project_files = true;
                }
                removed.push(p);
            }
            for name in old_dirs {
                let p = d.join(name);
                let mut gone = Vec::new();
                self.remove_dir(&p, &mut gone);
                removed.push(p);
                removed.extend(gone);
            }
            for name in new_dirs {
                let p = d.join(name);
                added.push(p.clone());
                self.add_dir(&p, &mut added);
            }
        }
        for (p, old) in self.projects.iter_mut() {
            let now = stamp(p);
            if now != *old {
                *old = now;
                project_files = true;
            }
        }
        added.sort();
        added.dedup();
        removed.sort();
        removed.dedup();
        if added.iter().chain(&removed).any(|p| is_project_file(p)) {
            project_files = true;
        }
        WatchReport {
            added,
            removed,
            project_files,
            under_projects: false,
        }
    }
}

fn stamp(p: &Path) -> (u64, Option<SystemTime>) {
    std::fs::metadata(p)
        .map(|m| (m.len(), m.modified().ok()))
        .unwrap_or((u64::MAX, None))
}

fn list(dir: &Path) -> Option<Dir> {
    let mtime = std::fs::metadata(dir).ok()?.modified().ok();
    let mut files = BTreeSet::new();
    let mut dirs = BTreeSet::new();
    for e in std::fs::read_dir(dir).ok()?.flatten() {
        let name = e.file_name().to_string_lossy().into_owned();
        match e.file_type() {
            Ok(t) if t.is_dir() => {
                if !skipped(&name) {
                    dirs.insert(name);
                }
            }
            Ok(_) => {
                files.insert(name);
            }
            Err(_) => {}
        }
    }
    Some(Dir {
        mtime,
        listed: SystemTime::now(),
        files,
        dirs,
    })
}

struct Control {
    stop: bool,
    rebaseline: bool,
}

/// The watcher's thread; stops when dropped.
pub struct WorkspaceWatcher {
    roots: Vec<PathBuf>,
    control: Arc<(Mutex<Control>, Condvar)>,
    started: Arc<AtomicBool>,
}

impl std::fmt::Debug for WorkspaceWatcher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WorkspaceWatcher")
            .field("roots", &self.roots)
            .finish()
    }
}

impl WorkspaceWatcher {
    /// Watch `roots` (the workspace folder and project folders outside it); `projects` are the project folders, for
    /// [`WatchReport::under_projects`]. `on_report` runs on the watcher's thread.
    pub fn start(
        roots: Vec<PathBuf>,
        projects: Vec<PathBuf>,
        options: WatchOptions,
        on_report: impl Fn(WatchReport) + Send + 'static,
    ) -> Self {
        let control = Arc::new((
            Mutex::new(Control {
                stop: false,
                rebaseline: false,
            }),
            Condvar::new(),
        ));
        let started = Arc::new(AtomicBool::new(false));
        let thread_roots = roots.clone();
        let thread_control = control.clone();
        let thread_started = started.clone();
        let spawned = std::thread::Builder::new()
            .name("eludite-workspace-watch".into())
            .spawn(move || {
                let mut snapshot = Snapshot::scan(&thread_roots);
                thread_started.store(true, Ordering::SeqCst);
                let mut pending = WatchReport::default();
                let mut changed_at: Option<Instant> = None;
                let (lock, wake) = &*thread_control;
                loop {
                    let wait = if changed_at.is_some() {
                        options.debounce.min(options.poll)
                    } else {
                        options.poll
                    };
                    {
                        let mut c = lock.lock().unwrap_or_else(|e| e.into_inner());
                        if !c.stop && !c.rebaseline {
                            c = wake
                                .wait_timeout(c, wait)
                                .unwrap_or_else(|e| e.into_inner())
                                .0;
                        }
                        if c.stop {
                            return;
                        }
                        if std::mem::take(&mut c.rebaseline) {
                            drop(c);
                            snapshot = Snapshot::scan(&thread_roots);
                            pending = WatchReport::default();
                            changed_at = None;
                            continue;
                        }
                    }
                    let r = snapshot.tick();
                    if !r.is_empty() {
                        pending.added.extend(r.added);
                        pending.removed.extend(r.removed);
                        pending.project_files |= r.project_files;
                        changed_at = Some(Instant::now());
                        continue;
                    }
                    if changed_at.is_some_and(|t| t.elapsed() >= options.debounce) {
                        changed_at = None;
                        let mut report = std::mem::take(&mut pending);
                        report.added.sort();
                        report.added.dedup();
                        report.removed.sort();
                        report.removed.dedup();
                        report.under_projects = report
                            .added
                            .iter()
                            .chain(&report.removed)
                            .any(|p| projects.iter().any(|d| p.starts_with(d)));
                        on_report(report);
                    }
                }
            });
        if spawned.is_err() {
            eprintln!("eludite: cannot start the workspace watcher");
        }
        Self {
            roots,
            control,
            started,
        }
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    /// Whether the first scan finished (changes before it are not reported).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn ready(&self) -> bool {
        self.started.load(Ordering::SeqCst)
    }

    /// Take the disk as it is now as the baseline, without reporting what changed (the window's own operations
    /// refresh the tree themselves).
    pub fn rebaseline(&self) {
        let (lock, wake) = &*self.control;
        lock.lock().unwrap_or_else(|e| e.into_inner()).rebaseline = true;
        wake.notify_all();
    }
}

impl Drop for WorkspaceWatcher {
    fn drop(&mut self) {
        let (lock, wake) = &*self.control;
        lock.lock().unwrap_or_else(|e| e.into_inner()).stop = true;
        wake.notify_all();
    }
}

#[cfg(test)]
#[path = "workspace_watch_tests.rs"]
mod tests;
