//! The git service: `eludite.git.*` over `eludite-git`, on whichever thread invokes the command (an agent's, or the
//! background thread the UI invokes from; never the UI thread). It owns the workspace's repository: found off the UI
//! thread when the workspace opens ([`GitService::set_workspace`]), its [`StatusCache`] and the [`Watcher`] that
//! keeps it current, and it tells the UI what changed through [`GitEvent`]s. Changes are serialized; a long transfer
//! (fetch, pull, push, sync) can be canceled with `eludite.git.cancel`.
//!
//! Credentials (brief 0045): the service keeps the session's [`SessionCredentials`] (what the person typed in the
//! credential prompt, in memory only, forgotten when the workspace closes) and hands them to the repository's
//! transfers. A transfer nothing answered fails with `credentials_required` and the host; for an agent the answer
//! adds that agents cannot answer the prompt, which only the person sees.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::git::{
    BlameCommitOut, BlameLineOut, BlameOutput, BranchOut, BranchesOutput, CancelOutput, ChangeOut,
    CheckoutOutput, CommitOutput, DiffHunkOut, DiffLineOut, DiffOutput, DiscardOutput, FetchOutput,
    GitCommands, GitOutput, GitRequest, GraphOut, InitOutput, LogEntryOut, LogOutput, MergeOutput,
    PushOutput, RebaseAction, RebaseOutput, ResetMode, ResetOutput, StageOutput, StashAction,
    StashOut, StashOutput, StatusOutput, StatusTotals, SyncOutput, TagOut, UnstageOutput,
    WorktreeAction, WorktreeOut, WorktreesOutput,
};
use eludite_git::branches::MergeOutcome;
use eludite_git::commit::{CommitOptions, Identity};
use eludite_git::diff::{Against, DiffTexts};
use eludite_git::log::{LogOptions, rfc3339};
use eludite_git::remote::Progress;
use eludite_git::{
    Cancel, ErrorKind, GitError, GlobalConfig, Repo, SessionCredentials, Status, StatusCache,
    WatchOptions, Watcher,
};
use eludite_ui::diff::{DiffKind, DiffLine, diff_lines};
use futures::channel::mpsc::UnboundedSender;

/// What the UI hears from the service.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitEvent {
    /// The workspace's repository (its working tree), or none, after a workspace change or `init`.
    Repository(Option<PathBuf>),
    /// A new status generation.
    Status(u64),
    /// A transfer's progress text, or `None` when it ended.
    Progress(Option<String>),
}

/// How the service is set up (tests: an isolated global config and a fast watcher).
#[derive(Debug, Clone, Default)]
pub struct GitSetup {
    pub global: GlobalConfig,
    pub watch: WatchOptions,
    /// Where the commit message drafts are kept (`<config dir>/eludite/git/`); `None` keeps none.
    pub state_dir: Option<PathBuf>,
}

impl GitSetup {
    /// The user's global git config and Eludite's config directory.
    pub fn from_env() -> Self {
        Self {
            global: GlobalConfig::User,
            watch: WatchOptions::default(),
            state_dir: eludite_docking::eludite_config_dir().map(|d| d.join("git")),
        }
    }
}

/// The settings the service applies (`git.*`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitSettings {
    pub enabled: bool,
    /// `git.userName` and `git.userEmail`, the identity's last resort.
    pub fallback: Option<Identity>,
}

impl Default for GitSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            fallback: None,
        }
    }
}

struct Active {
    repo: Repo,
    cache: StatusCache,
    watcher: Watcher,
}

#[derive(Default)]
struct Inner {
    workspace: Option<PathBuf>,
    active: Option<Arc<Active>>,
    /// The workspace is being looked at for a repository.
    discovering: bool,
}

/// `eludite.git.*` over libgit2. `Send + Sync`: every method runs on its caller's thread.
pub struct GitService {
    inner: Mutex<Inner>,
    events: UnboundedSender<GitEvent>,
    setup: Mutex<GitSetup>,
    settings: Mutex<GitSettings>,
    /// One change at a time.
    op: Mutex<()>,
    /// The transfer in flight and its token.
    running: Mutex<Option<(&'static str, Cancel)>>,
    /// Grows with each workspace change; a discovery for an older one is dropped.
    epoch: AtomicU64,
    /// The credential prompt's answers, for this session (in memory only).
    credentials: SessionCredentials,
}

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A git error as the command bus reports it (with the paths it names).
pub fn command_error(e: GitError) -> CommandError {
    if e.paths.is_empty() || e.message.contains(&e.paths[0]) {
        CommandError::Failed(e.message)
    } else {
        CommandError::Failed(format!("{} ({})", e.message, e.paths.join(", ")))
    }
}

/// What an agent's answer adds to `credentials_required`: the prompt is the person's.
pub const AGENT_CANNOT_ANSWER: &str = "Agents cannot answer the credential prompt: ask the person to run this in \
                                       Eludite (Git > Fetch, Pull or Push) and sign in there, or to set up a \
                                       credential helper or the ssh agent; do not retry it another way.";

/// The host a `credentials_required` message names (`credentials_required: <host> ...`).
pub fn credentials_required_host(message: &str) -> Option<&str> {
    let rest = message
        .strip_prefix(eludite_git::credentials::CREDENTIALS_REQUIRED)?
        .strip_prefix(": ")?;
    rest.split(' ').next().filter(|h| !h.is_empty())
}

const NO_REPOSITORY: &str = "The workspace is in no Git repository: create one with eludite.git.init (Create Git \
                             Repository in the Git Changes window)";

impl GitService {
    pub fn new(setup: GitSetup, events: UnboundedSender<GitEvent>) -> Self {
        Self {
            inner: Mutex::new(Inner::default()),
            events,
            setup: Mutex::new(setup),
            settings: Mutex::new(GitSettings::default()),
            op: Mutex::new(()),
            running: Mutex::new(None),
            epoch: AtomicU64::new(0),
            credentials: SessionCredentials::new(),
        }
    }

    /// The credentials the prompt supplied this session.
    pub fn credentials(&self) -> &SessionCredentials {
        &self.credentials
    }

    pub fn setup(&self) -> GitSetup {
        lock(&self.setup).clone()
    }

    /// Replace the setup (tests, before the workspace opens).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_setup(&self, setup: GitSetup) {
        *lock(&self.setup) = setup;
    }

    pub fn set_settings(self: &Arc<Self>, settings: GitSettings) {
        let was_enabled = lock(&self.settings).enabled;
        *lock(&self.settings) = settings.clone();
        if was_enabled != settings.enabled {
            let ws = lock(&self.inner).workspace.clone();
            self.set_workspace(ws.as_deref());
        }
    }

    pub fn settings(&self) -> GitSettings {
        lock(&self.settings).clone()
    }

    /// The workspace's folder changed (opened, closed): look for its repository on a thread of its own. Nothing
    /// else runs until the workspace opens.
    pub fn set_workspace(self: &Arc<Self>, folder: Option<&Path>) {
        let epoch = self.epoch.fetch_add(1, Ordering::SeqCst) + 1;
        // The session's credentials go with the workspace that asked for them.
        self.credentials.clear();
        {
            let mut inner = lock(&self.inner);
            inner.workspace = folder.map(Path::to_path_buf);
            inner.active = None;
            inner.discovering = folder.is_some();
        }
        let enabled = self.settings().enabled;
        let Some(folder) = folder.map(Path::to_path_buf).filter(|_| enabled) else {
            lock(&self.inner).discovering = false;
            let _ = self.events.unbounded_send(GitEvent::Repository(None));
            return;
        };
        let this = self.clone();
        let spawned = std::thread::Builder::new()
            .name("eludite-git-open".into())
            .spawn(move || {
                let found = Repo::discover(&folder).ok().flatten();
                if this.epoch.load(Ordering::SeqCst) != epoch {
                    return;
                }
                let workdir = found.as_ref().map(|r| r.workdir().to_path_buf());
                match found {
                    Some(repo) => {
                        this.activate(repo, epoch);
                    }
                    None => lock(&this.inner).discovering = false,
                }
                let _ = this.events.unbounded_send(GitEvent::Repository(workdir));
            });
        if let Err(e) = spawned {
            eprintln!("eludite: git: {e}");
        }
    }

    /// Watch `repo` as the workspace's repository.
    fn activate(&self, repo: Repo, epoch: u64) -> Arc<Active> {
        let setup = self.setup();
        let repo = repo
            .with_global_config(setup.global.clone())
            .with_credentials(self.credentials.clone());
        let cache = StatusCache::new(repo.clone(), true);
        let events = self.events.clone();
        cache.on_change(move |g| {
            let _ = events.unbounded_send(GitEvent::Status(g));
        });
        let watcher = Watcher::start(cache.clone(), setup.watch);
        let active = Arc::new(Active {
            repo,
            cache,
            watcher,
        });
        let mut inner = lock(&self.inner);
        if self.epoch.load(Ordering::SeqCst) == epoch {
            inner.active = Some(active.clone());
            inner.discovering = false;
        }
        active
    }

    fn active(&self) -> Result<Arc<Active>, CommandError> {
        if !self.settings().enabled {
            return Err(CommandError::Failed(
                "Git integration is off (the setting git.enabled)".into(),
            ));
        }
        lock(&self.inner)
            .active
            .clone()
            .ok_or_else(|| CommandError::Failed(NO_REPOSITORY.into()))
    }

    /// The repository's working tree, when there is one.
    pub fn repository(&self) -> Option<Repo> {
        lock(&self.inner).active.as_ref().map(|a| a.repo.clone())
    }

    /// The status the UI draws: the repository, the generation, the status and the last error. Never waits.
    pub fn snapshot(&self) -> Snapshot {
        let inner = lock(&self.inner);
        match &inner.active {
            Some(a) => {
                let (generation, status, error) = a.cache.snapshot();
                Snapshot {
                    repository: Some(a.repo.workdir().to_path_buf()),
                    generation,
                    status,
                    error,
                    loading: generation == 0,
                }
            }
            None => Snapshot {
                loading: inner.discovering,
                ..Snapshot::default()
            },
        }
    }

    /// A file of the working tree changed (the editor saved it): the watcher recomputes after its debounce.
    pub fn touch(&self) {
        if let Some(a) = &lock(&self.inner).active {
            a.watcher.touch();
        }
    }

    /// The old and new texts of `path` (Compare with Unmodified, the change margin). Blocks: call off the UI thread.
    pub fn diff_texts(&self, path: &str, against: &str, staged: bool) -> Result<DiffTexts, String> {
        let a = self.active().map_err(|e| e.to_string())?;
        a.repo
            .diff_texts(path, &Against::parse(against), staged)
            .map_err(|e| e.message)
    }

    /// The index's text of `path` for the change margin (`None`: not in the index, binary, or no repository).
    /// Blocks: call off the UI thread.
    pub fn index_text(&self, path: &Path) -> Option<String> {
        let a = self.active().ok()?;
        let rel = a.repo.relative(&path.to_string_lossy()).ok()?;
        a.repo.index_text(&rel).ok().flatten()
    }

    fn progress(&self, text: Option<String>) {
        let _ = self.events.unbounded_send(GitEvent::Progress(text));
    }

    /// Run a transfer with a cancel token registered for `eludite.git.cancel` and its progress reported (at most
    /// every 100 ms).
    fn transfer<T>(
        &self,
        what: &'static str,
        label: &str,
        f: impl FnOnce(&Cancel, &mut dyn FnMut(Progress)) -> eludite_git::Result<T>,
    ) -> Result<T, CommandError> {
        let cancel = Cancel::new();
        {
            let mut running = lock(&self.running);
            if let Some((other, _)) = running.as_ref() {
                return Err(CommandError::Failed(format!(
                    "A {other} is running: wait for it, or cancel it (eludite.git.cancel)"
                )));
            }
            *running = Some((what, cancel.clone()));
        }
        self.progress(Some(format!("{label}\u{2026}")));
        let mut last = Instant::now() - Duration::from_secs(1);
        let label = label.to_owned();
        let mut report = |p: Progress| {
            if last.elapsed() >= Duration::from_millis(100) && p.total > 0 {
                last = Instant::now();
                self.progress(Some(format!(
                    "{label}\u{2026} {}/{} objects",
                    p.current, p.total
                )));
            }
        };
        let result = f(&cancel, &mut report);
        *lock(&self.running) = None;
        self.progress(None);
        result.map_err(|e| self.transfer_error(e))
    }

    /// A failed transfer as the caller gets it: a refused prompt answer is forgotten, and an agent's
    /// `credentials_required` says the prompt is the person's.
    fn transfer_error(&self, e: GitError) -> CommandError {
        if e.kind != ErrorKind::CredentialsRequired {
            return command_error(e);
        }
        if e.refused
            && let Some(host) = &e.host
        {
            self.credentials.forget(host);
        }
        if eludite_commands::current_caller().is_agent() {
            CommandError::Failed(format!("{} {AGENT_CANNOT_ANSWER}", e.message))
        } else {
            CommandError::Failed(e.message)
        }
    }

    fn identity(&self) -> Option<Identity> {
        self.settings().fallback
    }
}

/// What the UI draws from.
#[derive(Debug, Clone, Default)]
pub struct Snapshot {
    pub repository: Option<PathBuf>,
    pub generation: u64,
    pub status: Option<Arc<Status>>,
    pub error: Option<String>,
    /// The repository is being looked for, or its first status computed.
    pub loading: bool,
}

/// The status as `eludite.git.status` answers it.
pub fn status_output(s: &Snapshot, max_items: usize, include_ignored: bool) -> StatusOutput {
    let Some(repo) = &s.repository else {
        return StatusOutput {
            state: if s.loading { "loading" } else { "none" }.into(),
            generation: s.generation,
            ..Default::default()
        };
    };
    let Some(st) = &s.status else {
        return StatusOutput {
            state: "loading".into(),
            repository: Some(repo.to_string_lossy().into_owned()),
            generation: s.generation,
            message: s.error.clone(),
            ..Default::default()
        };
    };
    let mut truncated = false;
    let mut cap = |n: usize| {
        if n > max_items {
            truncated = true;
        }
        n.min(max_items)
    };
    let change = |c: &eludite_git::Change| ChangeOut {
        path: c.path.clone(),
        kind: c.kind.as_str().into(),
        old_path: c.old_path.clone(),
    };
    let staged = st.staged[..cap(st.staged.len())]
        .iter()
        .map(change)
        .collect();
    let unstaged = st.unstaged[..cap(st.unstaged.len())]
        .iter()
        .map(change)
        .collect();
    let untracked = st.untracked[..cap(st.untracked.len())].to_vec();
    let conflicted = st.conflicted[..cap(st.conflicted.len())].to_vec();
    let ignored = include_ignored.then(|| st.ignored[..cap(st.ignored.len())].to_vec());
    let has_upstream = st.upstream.is_some();
    StatusOutput {
        state: "ready".into(),
        repository: Some(repo.to_string_lossy().into_owned()),
        generation: s.generation,
        branch: st.branch.clone(),
        detached: st.detached,
        head: st.head.map(|h| h.to_string()),
        upstream: st.upstream.clone(),
        ahead: has_upstream.then_some(st.ahead as u32),
        behind: has_upstream.then_some(st.behind as u32),
        operation: st.operation.map(|o| o.as_str().to_owned()),
        staged,
        unstaged,
        untracked,
        conflicted,
        ignored,
        stashes: st.stashes as u32,
        totals: StatusTotals {
            staged: st.staged.len() as u32,
            unstaged: st.unstaged.len() as u32,
            untracked: st.untracked.len() as u32,
            conflicted: st.conflicted.len() as u32,
        },
        truncated,
        message: s.error.clone(),
    }
}

/// The hunks of `lines` with `context` lines around each change, at most `max` changed lines (the rest counted).
pub fn diff_output(
    texts: &DiffTexts,
    lines: &[DiffLine],
    context: usize,
    max: usize,
) -> DiffOutput {
    let (added, removed) = eludite_ui::diff::counts(lines);
    let changed: Vec<usize> = lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.kind != DiffKind::Same)
        .map(|(i, _)| i)
        .collect();
    // Runs of changes with their context, merged when they touch.
    let mut groups: Vec<(usize, usize)> = Vec::new();
    for &i in &changed {
        let start = i.saturating_sub(context);
        let end = (i + context).min(lines.len() - 1);
        match groups.last_mut() {
            Some((_, e)) if start <= *e + 1 => *e = end.max(*e),
            _ => groups.push((start, end)),
        }
    }
    let mut hunks = Vec::new();
    let mut shown = 0usize;
    let mut truncated = false;
    'groups: for (start, end) in groups {
        let mut out = Vec::new();
        for l in &lines[start..=end] {
            if l.kind != DiffKind::Same {
                if shown == max {
                    truncated = true;
                    if !out.is_empty() {
                        hunks.push(hunk_of(lines, start, out));
                    }
                    break 'groups;
                }
                shown += 1;
            }
            out.push(l.clone());
        }
        hunks.push(hunk_of(lines, start, out));
    }
    let count = |t: &str| {
        if t.is_empty() {
            0
        } else {
            t.strip_suffix('\n').unwrap_or(t).split('\n').count() as u32
        }
    };
    DiffOutput {
        path: texts.path.clone(),
        old_label: texts.old_label.clone(),
        new_label: texts.new_label.clone(),
        binary: texts.binary,
        old_lines: count(&texts.old),
        new_lines: count(&texts.new),
        added: added as u32,
        removed: removed as u32,
        hunks,
        omitted: (changed.len() - shown) as u32,
        truncated,
    }
}

fn hunk_of(all: &[DiffLine], start: usize, lines: Vec<DiffLine>) -> DiffHunkOut {
    let old_start = all[..start].iter().filter(|l| l.old.is_some()).count() as u32;
    let new_start = all[..start].iter().filter(|l| l.new.is_some()).count() as u32;
    DiffHunkOut {
        old_start,
        old_len: lines.iter().filter(|l| l.old.is_some()).count() as u32,
        new_start,
        new_len: lines.iter().filter(|l| l.new.is_some()).count() as u32,
        lines: lines
            .into_iter()
            .map(|l| DiffLineOut {
                kind: match l.kind {
                    DiffKind::Same => "same",
                    DiffKind::Added => "added",
                    DiffKind::Removed => "removed",
                }
                .into(),
                old: l.old,
                new: l.new,
                text: l.text,
            })
            .collect(),
    }
}

fn merge_output(repo: &Repo, outcome: MergeOutcome, generation: u64) -> MergeOutput {
    let head = repo
        .repository()
        .ok()
        .and_then(|r| r.head().ok().and_then(|h| h.target()))
        .map(|o| o.to_string());
    let (result, conflicts) = match outcome {
        MergeOutcome::UpToDate => ("up_to_date", vec![]),
        MergeOutcome::FastForward(_) => ("fast_forward", vec![]),
        MergeOutcome::Merged(_) => ("merged", vec![]),
        MergeOutcome::Rebased(_) => ("rebased", vec![]),
        MergeOutcome::Picked(_) => ("picked", vec![]),
        MergeOutcome::Aborted(_) => ("aborted", vec![]),
        MergeOutcome::Conflicts { paths, .. } => ("conflicts", paths),
    };
    MergeOutput {
        result: result.into(),
        commit: head,
        conflicts,
        generation,
    }
}

impl GitService {
    fn refreshed(&self, a: &Active) -> Result<u64, CommandError> {
        a.cache.refresh().map(|(g, _)| g).map_err(command_error)
    }

    fn status_now(&self, a: &Active) -> Result<Arc<Status>, CommandError> {
        match a.cache.snapshot() {
            (g, Some(s), _) if g > 0 => Ok(s),
            _ => a.cache.refresh().map(|(_, s)| s).map_err(command_error),
        }
    }

    fn apply_request(&self, request: GitRequest) -> Result<GitOutput, CommandError> {
        if let GitRequest::Status {
            max_items,
            include_ignored,
        } = request
        {
            if !self.settings().enabled {
                return Err(CommandError::Failed(
                    "Git integration is off (the setting git.enabled)".into(),
                ));
            }
            return Ok(GitOutput::Status(Box::new(status_output(
                &self.snapshot(),
                max_items,
                include_ignored,
            ))));
        }
        if let GitRequest::Cancel = request {
            let running = lock(&self.running).clone();
            return Ok(GitOutput::Cancel(match running {
                Some((what, c)) => {
                    c.cancel();
                    CancelOutput {
                        canceled: true,
                        operation: Some(what.into()),
                    }
                }
                None => CancelOutput {
                    canceled: false,
                    operation: None,
                },
            }));
        }
        if let GitRequest::Init { path } = &request {
            if !self.settings().enabled {
                return Err(CommandError::Failed(
                    "Git integration is off (the setting git.enabled)".into(),
                ));
            }
            let folder = match path {
                Some(p) => PathBuf::from(p),
                None => lock(&self.inner)
                    .workspace
                    .clone()
                    .ok_or_else(|| CommandError::Failed("No workspace is open".into()))?,
            };
            let _one = lock(&self.op);
            let repo = Repo::init(&folder).map_err(command_error)?;
            let epoch = self.epoch.load(Ordering::SeqCst);
            let a = self.activate(repo, epoch);
            let generation = self.refreshed(&a)?;
            let _ = self
                .events
                .unbounded_send(GitEvent::Repository(Some(a.repo.workdir().to_path_buf())));
            return Ok(GitOutput::Init(InitOutput {
                repository: a.repo.workdir().to_string_lossy().into_owned(),
                branch: "main".into(),
                generation,
            }));
        }
        let a = self.active()?;
        let repo = &a.repo;
        let id = self.identity();
        let id = id.as_ref();
        let err = command_error;
        // Reads.
        match &request {
            GitRequest::Diff {
                path,
                against,
                staged,
                context,
                max_lines,
            } => {
                let texts = repo
                    .diff_texts(path, &Against::parse(against), *staged)
                    .map_err(err)?;
                let lines = if texts.binary {
                    Vec::new()
                } else {
                    diff_lines(&texts.old, &texts.new)
                };
                return Ok(GitOutput::Diff(Box::new(diff_output(
                    &texts, &lines, *context, *max_lines,
                ))));
            }
            GitRequest::Log {
                revision,
                all,
                path,
                max,
                skip,
            } => {
                let (entries, more) = repo
                    .log(
                        &LogOptions {
                            revision: revision.clone(),
                            all: *all,
                            path: path.clone(),
                            max: *max,
                            skip: *skip,
                        },
                        &Cancel::new(),
                    )
                    .map_err(err)?;
                return Ok(GitOutput::Log(Box::new(LogOutput {
                    entries: entries
                        .into_iter()
                        .map(|e| LogEntryOut {
                            commit: e.oid.to_string(),
                            short: e.short,
                            parents: e.parents.iter().map(|p| p.to_string()).collect(),
                            author: e.author,
                            email: e.email,
                            time: e.time,
                            date: Some(rfc3339(e.time, e.offset_minutes)),
                            summary: e.summary,
                            refs: e.refs,
                            graph: GraphOut {
                                lane: e.graph.lane as u32,
                                edges: e
                                    .graph
                                    .edges
                                    .iter()
                                    .map(|&(a, b)| [a as u32, b as u32])
                                    .collect(),
                                overflow: e.graph.overflow,
                            },
                        })
                        .collect(),
                    truncated: more,
                })));
            }
            GitRequest::Blame { path, max_lines } => {
                let b = repo.blame(path, *max_lines).map_err(err)?;
                return Ok(GitOutput::Blame(Box::new(BlameOutput {
                    path: b.path,
                    lines: b
                        .lines
                        .iter()
                        .map(|&(line, commit)| BlameLineOut {
                            line,
                            commit: commit as u32,
                        })
                        .collect(),
                    commits: b
                        .commits
                        .into_iter()
                        .map(|c| BlameCommitOut {
                            commit: c.oid.to_string(),
                            short: c.short,
                            author: c.author,
                            time: c.time,
                            date: Some(rfc3339(c.time, c.offset_minutes)),
                            summary: c.summary,
                        })
                        .collect(),
                    truncated: b.truncated,
                })));
            }
            GitRequest::Branches {
                delete: None,
                remote,
                ..
            } => return self.branches(&a, *remote, None),
            GitRequest::Stash {
                action: StashAction::List,
                ..
            } => return self.stash_output(&a, "listed", vec![]),
            GitRequest::Worktrees {
                action: WorktreeAction::List,
                ..
            } => return self.worktrees_output(&a, None, None),
            _ => {}
        }
        // Transfers: their own lock, so a long fetch does not hold up staging.
        match &request {
            GitRequest::Fetch { remote, prune } => {
                let f = self.transfer("fetch", "Fetching", |c, p| {
                    repo.fetch(remote.as_deref(), *prune, c, p)
                })?;
                let generation = self.refreshed(&a)?;
                let behind = self.status_now(&a)?;
                return Ok(GitOutput::Fetch(FetchOutput {
                    remote: f.remote,
                    updated: f.updated,
                    received_objects: f.received_objects as u64,
                    behind: behind.upstream.is_some().then_some(behind.behind as u32),
                    generation,
                }));
            }
            GitRequest::Push {
                remote,
                branch,
                set_upstream,
                force,
            } => {
                let p = self.transfer("push", "Pushing", |c, pr| {
                    repo.push(
                        remote.as_deref(),
                        branch.as_deref(),
                        *set_upstream,
                        *force,
                        c,
                        pr,
                    )
                })?;
                let generation = self.refreshed(&a)?;
                return Ok(GitOutput::Push(PushOutput {
                    remote: p.remote,
                    branch: p.branch,
                    upstream: p.upstream,
                    commit: p.oid.to_string(),
                    generation,
                }));
            }
            GitRequest::Pull { remote, rebase } => {
                let out = {
                    let _one = lock(&self.op);
                    self.transfer("pull", "Pulling", |c, p| {
                        repo.pull(remote.as_deref(), *rebase, id, c, p)
                    })?
                };
                let generation = self.refreshed(&a)?;
                return Ok(GitOutput::Merge(merge_output(repo, out, generation)));
            }
            GitRequest::Sync { remote } => {
                let out = {
                    let _one = lock(&self.op);
                    self.transfer("pull", "Pulling", |c, p| {
                        repo.pull(remote.as_deref(), false, id, c, p)
                    })?
                };
                let pulled = merge_output(repo, out, 0);
                let pushed = if pulled.result == "conflicts" {
                    false
                } else {
                    self.transfer("push", "Pushing", |c, p| {
                        repo.push(remote.as_deref(), None, false, false, c, p)
                    })?;
                    true
                };
                let generation = self.refreshed(&a)?;
                return Ok(GitOutput::Sync(SyncOutput {
                    pull: pulled.result,
                    pushed,
                    commit: pulled.commit,
                    conflicts: pulled.conflicts,
                    generation,
                }));
            }
            _ => {}
        }
        // Local changes, one at a time.
        let _one = lock(&self.op);
        let out = match request {
            GitRequest::Stage { paths } => {
                let s = repo.stage(paths.as_deref()).map_err(err)?;
                GitOutput::Stage(StageOutput {
                    staged: s.staged,
                    resolved: s.resolved,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Unstage { paths } => {
                let u = repo.unstage(paths.as_deref()).map_err(err)?;
                GitOutput::Unstage(UnstageOutput {
                    unstaged: u,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Discard { paths, staged } => {
                let d = repo.discard(paths.as_deref(), staged).map_err(err)?;
                GitOutput::Discard(DiscardOutput {
                    restored: d.restored,
                    deleted: d.deleted,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Commit {
                message,
                all,
                amend,
                author,
            } => {
                let c = repo
                    .commit(&CommitOptions {
                        message,
                        all,
                        amend,
                        author: author.map(|a| Identity {
                            name: a.name,
                            email: a.email,
                        }),
                        fallback: id.cloned(),
                    })
                    .map_err(err)?;
                GitOutput::Commit(CommitOutput {
                    commit: c.oid.to_string(),
                    summary: c.summary,
                    branch: c.branch,
                    amended: c.amended,
                    files: c.files as u32,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Branches {
                delete: Some(name),
                force,
                remote,
            } => {
                repo.delete_branch(&name, force).map_err(err)?;
                self.refreshed(&a)?;
                return self.branches(&a, remote, Some(name));
            }
            GitRequest::Checkout {
                name,
                create,
                start_point,
                force,
            } => {
                let c = repo
                    .checkout(&name, create, start_point.as_deref(), force)
                    .map_err(err)?;
                GitOutput::Checkout(CheckoutOutput {
                    detached: c.branch.is_none(),
                    branch: c.branch,
                    commit: c.oid.to_string(),
                    created: c.created,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Merge {
                branch,
                no_ff,
                message,
                abort,
            } => {
                let out = if abort {
                    repo.abort_merge()
                } else {
                    repo.merge(
                        branch.as_deref().unwrap_or_default(),
                        no_ff,
                        message.as_deref(),
                        id,
                    )
                }
                .map_err(err)?;
                GitOutput::Merge(merge_output(repo, out, self.refreshed(&a)?))
            }
            GitRequest::Rebase { onto, action } => {
                let out = match action {
                    RebaseAction::Start => repo.rebase(onto.as_deref().unwrap_or_default(), id),
                    RebaseAction::Continue => repo.rebase_continue(id),
                    RebaseAction::Abort => repo.rebase_abort(),
                }
                .map_err(err)?;
                let (step, steps) = match &out {
                    MergeOutcome::Conflicts { step, steps, .. } => {
                        (Some(*step as u32), Some(*steps as u32))
                    }
                    _ => (None, None),
                };
                let m = merge_output(repo, out, self.refreshed(&a)?);
                GitOutput::Rebase(RebaseOutput {
                    result: m.result,
                    commit: m.commit,
                    step,
                    steps,
                    conflicts: m.conflicts,
                    generation: m.generation,
                })
            }
            GitRequest::CherryPick { commit } => {
                let out = repo.cherry_pick(&commit, id).map_err(err)?;
                GitOutput::Merge(merge_output(repo, out, self.refreshed(&a)?))
            }
            GitRequest::Reset { revision, mode } => {
                let m = match mode {
                    ResetMode::Soft => eludite_git::branches::ResetMode::Soft,
                    ResetMode::Mixed => eludite_git::branches::ResetMode::Mixed,
                    ResetMode::Hard => eludite_git::branches::ResetMode::Hard,
                };
                let oid = repo.reset(&revision, m).map_err(err)?;
                GitOutput::Reset(ResetOutput {
                    commit: oid.to_string(),
                    mode,
                    generation: self.refreshed(&a)?,
                })
            }
            GitRequest::Stash {
                action,
                message,
                include_untracked,
                index,
            } => {
                let (result, conflicts) = match action {
                    StashAction::List => ("listed", vec![]),
                    StashAction::Push => {
                        repo.stash_push(message.as_deref(), include_untracked, id)
                            .map_err(err)?;
                        ("pushed", vec![])
                    }
                    StashAction::Apply | StashAction::Pop => {
                        let pop = action == StashAction::Pop;
                        match repo.stash_apply(index, pop).map_err(err)? {
                            MergeOutcome::Conflicts { paths, .. } => ("conflicts", paths),
                            _ if pop => ("popped", vec![]),
                            _ => ("applied", vec![]),
                        }
                    }
                    StashAction::Drop => {
                        repo.stash_drop(index).map_err(err)?;
                        ("dropped", vec![])
                    }
                };
                self.refreshed(&a)?;
                return self.stash_output(&a, result, conflicts);
            }
            GitRequest::Worktrees {
                action,
                name,
                path,
                branch,
                force,
            } => {
                let name = name.unwrap_or_default();
                return match action {
                    WorktreeAction::List => self.worktrees_output(&a, None, None),
                    WorktreeAction::Add => {
                        repo.add_worktree(&name, path.as_deref().map(Path::new), branch.as_deref())
                            .map_err(err)?;
                        self.refreshed(&a)?;
                        self.worktrees_output(&a, Some(name), None)
                    }
                    WorktreeAction::Remove => {
                        repo.remove_worktree(&name, force).map_err(err)?;
                        self.refreshed(&a)?;
                        self.worktrees_output(&a, None, Some(name))
                    }
                };
            }
            other => {
                return Err(CommandError::Failed(format!(
                    "{} is not handled here",
                    other.command()
                )));
            }
        };
        Ok(out)
    }

    fn branches(
        &self,
        a: &Active,
        remote: bool,
        deleted: Option<String>,
    ) -> Result<GitOutput, CommandError> {
        let (branches, tags) = a.repo.branches(remote).map_err(command_error)?;
        let current = branches.iter().find(|b| b.head).map(|b| b.name.clone());
        Ok(GitOutput::Branches(Box::new(BranchesOutput {
            current,
            branches: branches
                .into_iter()
                .map(|b| BranchOut {
                    ahead: b.upstream.is_some().then_some(b.ahead as u32),
                    behind: b.upstream.is_some().then_some(b.behind as u32),
                    name: b.name,
                    remote: b.remote,
                    head: b.head,
                    commit: b.oid.to_string(),
                    summary: b.summary,
                    upstream: b.upstream,
                })
                .collect(),
            tags: tags
                .into_iter()
                .map(|t| TagOut {
                    name: t.name,
                    commit: t.oid.to_string(),
                })
                .collect(),
            deleted,
            generation: a.cache.generation(),
        })))
    }

    fn stash_output(
        &self,
        a: &Active,
        result: &str,
        conflicts: Vec<String>,
    ) -> Result<GitOutput, CommandError> {
        let stashes = a.repo.stashes().map_err(command_error)?;
        Ok(GitOutput::Stash(StashOutput {
            result: result.into(),
            stashes: stashes
                .into_iter()
                .map(|s| StashOut {
                    index: s.index as u32,
                    message: s.message,
                    commit: s.oid.to_string(),
                })
                .collect(),
            conflicts,
            generation: a.cache.generation(),
        }))
    }

    fn worktrees_output(
        &self,
        a: &Active,
        added: Option<String>,
        removed: Option<String>,
    ) -> Result<GitOutput, CommandError> {
        let list = a.repo.worktrees().map_err(command_error)?;
        Ok(GitOutput::Worktrees(WorktreesOutput {
            worktrees: list
                .into_iter()
                .map(|w| WorktreeOut {
                    name: w.name,
                    path: w.path.to_string_lossy().into_owned(),
                    branch: w.branch,
                    commit: w.head.map(|h| h.to_string()),
                    main: w.main,
                    locked: w.locked,
                })
                .collect(),
            added,
            removed,
        }))
    }
}

impl GitCommands for GitService {
    fn apply(&self, request: GitRequest) -> Result<GitOutput, CommandError> {
        self.apply_request(request)
    }
}
