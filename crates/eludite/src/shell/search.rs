//! Find in Files and Replace in Files (brief 0042; Visual Studio's Edit > Find and Replace > Find in Files,
//! Ctrl+Shift+F, and Replace in Files, Ctrl+Shift+H), the Find Results 1 and 2 windows, and `eludite.search.*`.
//!
//! - **One service.** [`SearchService`] implements `eludite.search.*` on the invoking thread with `eludite-search`:
//!   an agent's call searches on the agent's thread, the person's on a thread of its own, never the UI thread. It asks
//!   the UI thread only for what lives there ([`SearchJob`]): the workspace's folders and the open documents' text
//!   (the overlay, unsaved edits included), Replace All's pending changes or edits, and the results windows' answers.
//! - **Streaming.** A search shown in a Find Results window sends its files as they are found ([`SearchEvent`]); the
//!   shell applies everything queued in one update, so a burst of results costs one frame. A newer search in the same
//!   window cancels the older one, whose late files are dropped (invariant 12: the search id is its generation).
//! - **Replace All** checks every file against the text it searched (a file changed on disk or in its editor since
//!   is skipped, with a row), then holds the replacements as pending changes in brief 0016's review view (`preview`,
//!   the default) or applies them at once through the workspace-edit applier; either way an open document changes as
//!   one undo step. Accepting a change checks again, and with Keep modified files open opens an unopened file first
//!   so its replacements land in an editor (undoable, unsaved).
//! - **The dialog** ([`dialog::FindDialog`]) is modeless; its buttons, the windows' toolbars, F8 and Shift+F8 and a
//!   double-click on a result all run `eludite.search.*` commands ([`Shell::run_search`]).

pub mod dialog;
pub mod results;

use std::cell::RefCell;
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, mpsc};
use std::thread::{self, ThreadId};
use std::time::Duration;

use eludite_commands::search::{
    self as cmds, CancelOutput, FileOut, FindArgs, FindOutput, MatchOut, ReplaceFile,
    ReplaceOutput, ReplaceState, ResultsOutput, ScopeKind, SearchCommands, SearchOutput,
    SearchRequest, Skipped,
};
use eludite_commands::{Caller, CommandError, current_caller, with_caller, workspace};
use eludite_docking::ids;
use eludite_editor::EditorView;
use eludite_editor::SelectionRange;
use eludite_lsp::lsp;
use eludite_search::{
    CancelToken, FileMatches, Filters, Overlay, Query, Request, Scope, Summary, fingerprint,
};
use eludite_ui::{Theme, slots};
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender};
use gpui::{AppContext as _, Context, Entity, Focusable as _, Window};
use serde_json::{Value, json};

use self::dialog::{FindDialog, FindDialogEvent, Mode};
use self::results::{Counts, FindResults, Target, display_path};
use super::Shell;
use super::agents::review::ChangeState;
use super::documents::{normalize_path, offset_of, path_to_uri};
use super::workspace_edit::ApplyOptions;

/// The views that step through their own list with F8 and Shift+F8 (Compare with Unmodified's differences, brief
/// 0040): the keys stay theirs there instead of stepping through the Find Results.
const OWN_F8: [&str; 1] = ["GitCompare"];

/// Leave F8 and Shift+F8 to the views that use them for their own list. Call after the shell's keymap is bound.
pub fn bind_keys(cx: &mut gpui::App) {
    cx.bind_keys(OWN_F8.iter().flat_map(|context| {
        ["f8", "shift-f8"]
            .into_iter()
            .map(move |k| gpui::KeyBinding::new(k, gpui::NoAction, Some(context)))
    }));
}

/// How long a search waits for the UI thread.
const UI_TIMEOUT: Duration = Duration::from_secs(30);
/// Searches whose results `eludite.search.results` keeps by id.
const RECENT: usize = 16;
/// Queries the dialog remembers per workspace.
pub const HISTORY: usize = 20;

/// The `search.*` settings.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SearchSettings {
    pub excludes: Vec<String>,
    pub use_gitignore: bool,
    pub max_file_size_mb: u64,
    pub follow_symlinks: bool,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self {
            excludes: eludite_search::DEFAULT_EXCLUDES
                .iter()
                .map(|s| (*s).to_owned())
                .collect(),
            use_gitignore: true,
            max_file_size_mb: 4,
            follow_symlinks: false,
        }
    }
}

/// What a search needs from the UI thread.
#[derive(Debug, Clone, Default)]
pub struct SearchContext {
    pub root: Option<PathBuf>,
    /// The projects' names and folders.
    pub projects: Vec<(String, PathBuf)>,
    pub active_document: Option<PathBuf>,
    pub active_project: Option<String>,
    /// The open documents' text, by normalized path.
    pub overlay: HashMap<PathBuf, Arc<str>>,
}

/// What the service tells the UI.
#[derive(Debug, Clone)]
pub enum SearchEvent {
    Started {
        search_id: u64,
        window: u8,
        append: bool,
        title: String,
        root: Option<PathBuf>,
    },
    Files {
        search_id: u64,
        window: u8,
        files: Vec<FileMatches>,
    },
    Skipped {
        search_id: u64,
        window: u8,
        path: String,
        reason: String,
    },
    Finished {
        search_id: u64,
        window: u8,
        counts: Option<Counts>,
        error: Option<String>,
    },
}

/// One file of Replace All, checked against the text that was searched.
#[derive(Debug, Clone)]
pub struct PlanFile {
    pub path: PathBuf,
    /// Searched from an open document.
    pub open: bool,
    pub fingerprint: u64,
    pub edits: Vec<eludite_search::replace::Edit>,
}

/// Replace All's edits, for the UI thread to hold for review or apply.
#[derive(Debug, Clone)]
pub struct ReplacePlan {
    pub search_id: u64,
    pub window: Option<u8>,
    pub files: Vec<PlanFile>,
    pub skipped: Vec<Skipped>,
    pub preview: bool,
    pub keep_open: bool,
    pub label: String,
    pub caller: Caller,
    pub truncated: bool,
    pub elapsed_ms: u64,
    pub root: Option<PathBuf>,
}

/// What a search asks of the UI thread.
pub enum SearchJob {
    Context(mpsc::SyncSender<SearchContext>),
    Hold(
        Box<ReplacePlan>,
        mpsc::SyncSender<Result<ReplaceOutput, CommandError>>,
    ),
    Results(
        SearchRequest,
        mpsc::SyncSender<Result<ResultsOutput, CommandError>>,
    ),
}

type Outcome = Result<SearchOutput, CommandError>;

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The result the shell computed for the bus invocation it is about to make on the UI thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A finished search, kept for `eludite.search.results` by id.
struct Recent {
    id: u64,
    files: Arc<Vec<FileMatches>>,
    root: Option<PathBuf>,
}

#[derive(Default)]
struct State {
    next: u64,
    /// Running searches: their cancel token and the window that shows them.
    running: HashMap<u64, (CancelToken, Option<u8>)>,
    recent: VecDeque<Recent>,
    settings: SearchSettings,
    /// Tests: a delay per overlay lookup (a slow search to stop).
    slow: Option<Duration>,
}

/// `eludite.search.*` (any thread).
pub struct SearchService {
    state: Mutex<State>,
    events: UnboundedSender<SearchEvent>,
    jobs: UnboundedSender<SearchJob>,
    ui_thread: ThreadId,
    state_dir: Mutex<Option<PathBuf>>,
}

/// The overlay with a delay per lookup (tests).
struct Slow<'a> {
    inner: &'a HashMap<PathBuf, Arc<str>>,
    delay: Duration,
}

impl Overlay for Slow<'_> {
    fn text(&self, path: &Path) -> Option<Arc<str>> {
        std::thread::sleep(self.delay);
        self.inner.get(path).cloned()
    }

    fn paths(&self) -> Vec<PathBuf> {
        self.inner.paths()
    }
}

fn failed(message: impl Into<String>) -> CommandError {
    CommandError::Failed(message.into())
}

/// A finished search's numbers.
fn counts(s: &Summary) -> Counts {
    Counts {
        files_searched: s.files_searched,
        matching_files: s.matching_files,
        matching_lines: s.matching_lines,
        total: s.total,
        truncated: s.truncated,
        canceled: s.canceled,
        elapsed: s.elapsed,
    }
}

/// Visual Studio's title of a search's block: `Find all "x", Match case, Subfolders, Find Results 1, Entire
/// Solution, "*.cs"`.
pub fn block_title(
    args: &FindArgs,
    replacement: Option<(&str, bool)>,
    window: Option<u8>,
    scope: &str,
) -> String {
    let mut parts = vec![match replacement {
        Some((r, _)) => format!(
            "Replace all \"{}\", \"{r}\"",
            args.query.as_deref().unwrap_or_default()
        ),
        None => format!("Find all \"{}\"", args.query.as_deref().unwrap_or_default()),
    }];
    if args.case_sensitive {
        parts.push("Match case".into());
    }
    if args.whole_word {
        parts.push("Whole word".into());
    }
    if args.regex {
        parts.push("Regular expressions".into());
    }
    parts.push("Subfolders".into());
    if let Some((_, true)) = replacement {
        parts.push("Keep modified files open".into());
    }
    if let Some(w) = window {
        parts.push(format!("Find Results {w}"));
    }
    parts.push(scope.to_owned());
    let types = if args.include.is_empty() {
        "*.*".to_owned()
    } else {
        args.include.join(";")
    };
    parts.push(format!("\"{types}\""));
    parts.join(", ")
}

/// The JSON a dialog's search runs with.
pub fn args_json(a: &FindArgs) -> Value {
    let mut v = json!({
        "query": a.query,
        "regex": a.regex,
        "case_sensitive": a.case_sensitive,
        "whole_word": a.whole_word,
        "scope": a.scope,
        "append": a.append,
    });
    if let Some(p) = &a.project {
        v["project"] = json!(p);
    }
    if let Some(p) = &a.path {
        v["path"] = json!(p);
    }
    if !a.include.is_empty() {
        v["include"] = json!(a.include);
    }
    if let Some(w) = a.results_window {
        v["results_window"] = json!(w);
    }
    v
}

impl SearchService {
    pub fn new(
        ui_thread: ThreadId,
        events: UnboundedSender<SearchEvent>,
        jobs: UnboundedSender<SearchJob>,
    ) -> Self {
        Self {
            state: Mutex::new(State::default()),
            events,
            jobs,
            ui_thread,
            state_dir: Mutex::new(eludite_docking::eludite_config_dir().map(|d| d.join("search"))),
        }
    }

    fn state(&self) -> MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    pub fn set_settings(&self, settings: SearchSettings) {
        self.state().settings = settings;
    }

    pub fn settings(&self) -> SearchSettings {
        self.state().settings.clone()
    }

    /// Tests: every overlay lookup sleeps `delay` (a slow search).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_slow_overlay(&self, delay: Option<Duration>) {
        self.state().slow = delay;
    }

    /// Where the dialog's history is kept (`history.json`).
    pub fn state_dir(&self) -> Option<PathBuf> {
        self.state_dir
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_state_dir(&self, dir: Option<PathBuf>) {
        *self.state_dir.lock().unwrap_or_else(|e| e.into_inner()) = dir;
    }

    /// Stop `id` (every running search for `None`); the ids stopped.
    pub fn cancel(&self, id: Option<u64>) -> Vec<u64> {
        let s = self.state();
        let mut out: Vec<u64> = s
            .running
            .iter()
            .filter(|(i, _)| id.is_none_or(|x| x == **i))
            .map(|(i, (token, _))| {
                token.cancel();
                *i
            })
            .collect();
        out.sort_unstable();
        out
    }

    /// The running search shown in window `n`.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn running_in(&self, n: u8) -> Option<u64> {
        self.state()
            .running
            .iter()
            .find(|(_, (_, w))| *w == Some(n))
            .map(|(i, _)| *i)
    }

    fn ask<T>(
        &self,
        job: impl FnOnce(mpsc::SyncSender<T>) -> SearchJob,
    ) -> Result<T, CommandError> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(job(tx))
            .map_err(|_| failed("the window is closed"))?;
        rx.recv_timeout(UI_TIMEOUT)
            .map_err(|_| failed("the UI did not answer"))
    }

    /// The scope's search request and its label for the title.
    fn request(
        &self,
        args: &FindArgs,
        ctx: &SearchContext,
    ) -> Result<(Request, String), CommandError> {
        let text = args
            .query
            .clone()
            .ok_or_else(|| CommandError::InvalidInput("`query` is required".into()))?;
        let query = Query {
            text,
            regex: args.regex,
            case_sensitive: args.case_sensitive,
            whole_word: args.whole_word,
        };
        if let Err(e) = query.compile() {
            return Err(CommandError::InvalidInput(format!(
                "`query` is not valid: {e}"
            )));
        }
        let resolve = |p: &str| -> PathBuf {
            let path = Path::new(p);
            normalize_path(&if path.is_absolute() {
                path.to_path_buf()
            } else {
                ctx.root
                    .as_ref()
                    .map(|r| r.join(path))
                    .unwrap_or_else(|| path.to_path_buf())
            })
        };
        let no_workspace = || {
            failed(
                "no workspace is open: open one, or search a folder (`scope: folder` with its `path`)",
            )
        };
        let project_folder = |name: &str| {
            ctx.projects
                .iter()
                .find(|(n, _)| n == name)
                .or_else(|| {
                    ctx.projects
                        .iter()
                        .find(|(n, _)| n.eq_ignore_ascii_case(name))
                })
                .map(|(n, p)| (n.clone(), p.clone()))
                .ok_or_else(|| {
                    failed(format!(
                        "no project `{name}`: the projects are {}",
                        ctx.projects
                            .iter()
                            .map(|(n, _)| n.as_str())
                            .collect::<Vec<_>>()
                            .join(", ")
                    ))
                })
        };
        let (scope, label) = match args.scope {
            ScopeKind::Solution => {
                let root = ctx.root.clone().ok_or_else(no_workspace)?;
                let mut roots = vec![root];
                for (_, folder) in &ctx.projects {
                    if !roots.iter().any(|r| folder.starts_with(r)) && folder.is_dir() {
                        roots.push(folder.clone());
                    }
                }
                (Scope::Paths(roots), ScopeKind::Solution.label().to_owned())
            }
            ScopeKind::Project => {
                let name = match &args.project {
                    Some(n) => n.clone(),
                    None => ctx.active_project.clone().ok_or_else(|| {
                        failed("no project is current: open a document of a project, or name one in `project`")
                    })?,
                };
                let (name, folder) = project_folder(&name)?;
                let label = if args.project.is_some() {
                    format!("Project {name}")
                } else {
                    ScopeKind::Project.label().to_owned()
                };
                (Scope::Paths(vec![folder]), label)
            }
            ScopeKind::Document => {
                let path = match &args.path {
                    Some(p) => resolve(p),
                    None => ctx.active_document.clone().ok_or_else(|| {
                        failed("no document is active: open one, or give its `path`")
                    })?,
                };
                if !path.is_file() && !ctx.overlay.contains_key(&path) {
                    return Err(failed(format!("{} is not a file", path.display())));
                }
                (
                    Scope::Paths(vec![path]),
                    ScopeKind::Document.label().to_owned(),
                )
            }
            ScopeKind::OpenDocuments => (
                Scope::OpenDocuments,
                ScopeKind::OpenDocuments.label().to_owned(),
            ),
            ScopeKind::Folder => {
                let p = args.path.as_deref().ok_or_else(|| {
                    CommandError::InvalidInput("`scope: folder` needs `path`".into())
                })?;
                if !Path::new(p).is_absolute() && ctx.root.is_none() {
                    return Err(no_workspace());
                }
                let folder = resolve(p);
                if !folder.is_dir() {
                    return Err(failed(format!("{} is not a folder", folder.display())));
                }
                let label = folder.to_string_lossy().into_owned();
                (Scope::Paths(vec![folder]), label)
            }
        };
        let settings = self.settings();
        let mut request = Request::new(query, scope);
        request.filters = Filters {
            excludes: settings.excludes,
            include: args.include.clone(),
            exclude: args.exclude.clone(),
            use_gitignore: settings.use_gitignore,
            max_file_size: settings.max_file_size_mb.saturating_mul(1024 * 1024),
            follow_symlinks: settings.follow_symlinks,
        };
        request.max_results = args.max_results;
        request.context_lines = args.context_lines;
        Ok((request, label))
    }

    /// Run a search, streaming it to its window; the files sorted by path.
    fn run(
        &self,
        args: &FindArgs,
        replacement: Option<(&str, bool)>,
        caller: &Caller,
    ) -> Result<Ran, CommandError> {
        let ctx = self.ask(SearchJob::Context)?;
        let (request, label) = self.request(args, &ctx)?;
        // The person's searches always show; an agent's only in the window it names.
        let window = args
            .results_window
            .or_else(|| (!caller.is_agent()).then_some(1));
        let token = CancelToken::new();
        let (id, slow) = {
            let mut s = self.state();
            s.next += 1;
            let id = s.next;
            if let Some(w) = window {
                // A newer search in a window supersedes the one drawing there.
                for (token, win) in s.running.values() {
                    if *win == Some(w) {
                        token.cancel();
                    }
                }
            }
            s.running.insert(id, (token.clone(), window));
            (id, s.slow)
        };
        if let Some(w) = window {
            let _ = self.events.unbounded_send(SearchEvent::Started {
                search_id: id,
                window: w,
                append: args.append,
                title: block_title(args, replacement, window, &label),
                root: ctx.root.clone(),
            });
        }
        let collected: Mutex<Vec<FileMatches>> = Mutex::new(Vec::new());
        let events = self.events.clone();
        let sink = |f: FileMatches| {
            if let Some(w) = window {
                let _ = events.unbounded_send(SearchEvent::Files {
                    search_id: id,
                    window: w,
                    files: vec![f.clone()],
                });
            }
            collected.lock().unwrap_or_else(|e| e.into_inner()).push(f);
        };
        let result = match slow {
            Some(delay) => eludite_search::search(
                &request,
                &Slow {
                    inner: &ctx.overlay,
                    delay,
                },
                &token,
                &sink,
            ),
            None => eludite_search::search(&request, &ctx.overlay, &token, &sink),
        };
        self.state().running.remove(&id);
        let summary = match result {
            Ok(s) => s,
            Err(e) => {
                if let Some(w) = window {
                    let _ = self.events.unbounded_send(SearchEvent::Finished {
                        search_id: id,
                        window: w,
                        counts: None,
                        error: Some(e.to_string()),
                    });
                }
                return Err(CommandError::InvalidInput(e.to_string()));
            }
        };
        if let Some(w) = window {
            let _ = self.events.unbounded_send(SearchEvent::Finished {
                search_id: id,
                window: w,
                counts: Some(counts(&summary)),
                error: None,
            });
        }
        let mut files = collected.into_inner().unwrap_or_else(|e| e.into_inner());
        files.sort_by(|a, b| a.path.cmp(&b.path));
        let files = Arc::new(files);
        {
            let mut s = self.state();
            s.recent.push_back(Recent {
                id,
                files: files.clone(),
                root: ctx.root.clone(),
            });
            while s.recent.len() > RECENT {
                s.recent.pop_front();
            }
        }
        Ok(Ran {
            id,
            window,
            files,
            summary,
            root: ctx.root,
            request,
        })
    }

    fn find(&self, args: FindArgs, caller: &Caller) -> Result<FindOutput, CommandError> {
        let ran = self.run(&args, None, caller)?;
        let s = &ran.summary;
        Ok(FindOutput {
            search_id: Some(ran.id),
            files: ran
                .files
                .iter()
                .map(|f| file_out(f, ran.root.as_deref()))
                .collect(),
            total: s.total,
            matching_lines: s.matching_lines,
            matching_files: s.matching_files,
            files_searched: s.files_searched,
            truncated: s.truncated,
            canceled: s.canceled,
            elapsed_ms: s.elapsed.as_millis() as u64,
            results_window: ran.window,
            dialog: None,
        })
    }

    fn replace(
        &self,
        args: FindArgs,
        replacement: String,
        preview: bool,
        keep_open: bool,
        caller: &Caller,
    ) -> Result<ReplaceOutput, CommandError> {
        let ran = self.run(&args, Some((&replacement, keep_open)), caller)?;
        let compiled = ran
            .request
            .query
            .compile()
            .map_err(|e| CommandError::InvalidInput(e.to_string()))?;
        let root = ran.root.as_deref();
        let mut skipped = Vec::new();
        let mut files = Vec::new();
        for f in ran.files.iter() {
            let shown = display_path(&f.path, root);
            if f.encoding.is_utf16() {
                skipped.push(Skipped {
                    path: shown,
                    reason: "UTF-16 files are searched but not replaced in (the editor reads UTF-8 only)".into(),
                });
                continue;
            }
            if !f.open {
                // Checked against the text that was searched, never overwritten blindly.
                match std::fs::read(&f.path) {
                    Ok(bytes) if fingerprint(&bytes) != f.fingerprint => {
                        skipped.push(Skipped {
                            path: shown,
                            reason: "it changed on disk since the search".into(),
                        });
                        continue;
                    }
                    Ok(bytes) if std::str::from_utf8(&bytes).is_err() => {
                        skipped.push(Skipped {
                            path: shown,
                            reason: "it is not UTF-8 text".into(),
                        });
                        continue;
                    }
                    Ok(_) => {}
                    Err(e) => {
                        skipped.push(Skipped {
                            path: shown,
                            reason: format!("it cannot be read: {e}"),
                        });
                        continue;
                    }
                }
            }
            let edits = eludite_search::replace::replacements(&compiled, f, &replacement);
            if edits.is_empty() {
                continue;
            }
            files.push(PlanFile {
                path: f.path.clone(),
                open: f.open,
                fingerprint: f.fingerprint,
                edits,
            });
        }
        if let Some(w) = ran.window {
            for s in &skipped {
                let _ = self.events.unbounded_send(SearchEvent::Skipped {
                    search_id: ran.id,
                    window: w,
                    path: s.path.clone(),
                    reason: s.reason.clone(),
                });
            }
        }
        let elapsed_ms = ran.summary.elapsed.as_millis() as u64;
        if files.is_empty() {
            return Ok(ReplaceOutput {
                search_id: Some(ran.id),
                state: ReplaceState::Nothing,
                files: Vec::new(),
                replacements: 0,
                message: Some(if ran.summary.canceled {
                    "The search was stopped before anything was replaced.".into()
                } else if skipped.is_empty() {
                    "No match to replace.".into()
                } else {
                    "Every matching file was skipped (see `skipped`).".into()
                }),
                skipped,
                truncated: ran.summary.truncated,
                elapsed_ms,
            });
        }
        let label = format!(
            "Replace in Files: \"{}\" with \"{replacement}\"",
            args.query.as_deref().unwrap_or_default()
        );
        let plan = ReplacePlan {
            search_id: ran.id,
            window: ran.window,
            files,
            skipped,
            preview,
            keep_open,
            label,
            caller: caller.clone(),
            truncated: ran.summary.truncated,
            elapsed_ms,
            root: ran.root.clone(),
        };
        let (tx, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(SearchJob::Hold(Box::new(plan), tx))
            .map_err(|_| failed("the window is closed"))?;
        rx.recv_timeout(Duration::from_secs(120))
            .map_err(|_| failed("the UI did not answer"))?
    }

    /// `eludite.search.results` for a search by id (any thread).
    fn recent(&self, id: u64, offset: usize, limit: usize) -> Result<ResultsOutput, CommandError> {
        let s = self.state();
        let r = s
            .recent
            .iter()
            .find(|r| r.id == id)
            .ok_or_else(|| failed(format!("no search {id} is kept: the last {RECENT} are")))?;
        let mut rows = Vec::new();
        let mut total = 0u64;
        for f in r.files.iter() {
            let file = results::FileResult {
                display: display_path(&f.path, r.root.as_deref()),
                open: f.open,
                count: f.count(),
                lines: f
                    .lines
                    .iter()
                    .map(results::LineResult::from_match)
                    .collect(),
                path: f.path.clone(),
            };
            for line in &file.lines {
                if total as usize >= offset && rows.len() < limit {
                    rows.push(results::row_out(&file, line, 0, total));
                }
                total += 1;
            }
        }
        let end = offset.saturating_add(limit) as u64;
        Ok(ResultsOutput {
            results_window: None,
            blocks: Vec::new(),
            matches: rows,
            total,
            offset: offset as u64,
            next_offset: (end < total).then_some(end),
            selected: None,
        })
    }
}

/// A search that ran.
struct Ran {
    id: u64,
    window: Option<u8>,
    files: Arc<Vec<FileMatches>>,
    summary: Summary,
    root: Option<PathBuf>,
    request: Request,
}

/// A file of `search-find.output.json`.
fn file_out(f: &FileMatches, root: Option<&Path>) -> FileOut {
    FileOut {
        path: display_path(&f.path, root),
        open: f.open,
        matches: f
            .lines
            .iter()
            .map(|l| {
                let (text, ranges, column) = cmds::clip_line(&l.text, &l.ranges);
                MatchOut {
                    line: l.line,
                    column,
                    text,
                    ranges,
                    before: l.before.clone(),
                    after: l.after.clone(),
                }
            })
            .collect(),
    }
}

impl SearchCommands for SearchService {
    fn apply(&self, request: SearchRequest) -> Outcome {
        if thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(failed(format!(
                    "{} runs off the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let caller = current_caller();
        Ok(match request {
            SearchRequest::Find(args) => {
                if args.query.is_none() {
                    return Err(CommandError::InvalidInput(
                        "`query` is required (only the person's Ctrl+Shift+F opens the dialog)"
                            .into(),
                    ));
                }
                SearchOutput::Find(self.find(args, &caller)?)
            }
            SearchRequest::Replace {
                find,
                replacement,
                preview,
                keep_open,
            } => {
                let (Some(_), Some(replacement)) = (&find.query, replacement) else {
                    return Err(CommandError::InvalidInput(
                        "`query` and `replacement` are required (only the person's Ctrl+Shift+H opens the dialog)"
                            .into(),
                    ));
                };
                SearchOutput::Replace(self.replace(
                    find,
                    replacement,
                    preview,
                    keep_open,
                    &caller,
                )?)
            }
            SearchRequest::Cancel { search_id } => SearchOutput::Cancel(CancelOutput {
                canceled: self.cancel(search_id),
            }),
            SearchRequest::Results {
                search_id: Some(id),
                offset,
                limit,
                ..
            } => SearchOutput::Results(self.recent(id, offset, limit)?),
            r @ SearchRequest::Results { .. } => {
                SearchOutput::Results(self.ask(|tx| SearchJob::Results(r, tx))??)
            }
        })
    }
}

/// Register `eludite.search.*` over a new service; the events and jobs go to the UI. Call on the UI thread.
pub fn register(
    commands: &eludite_commands::CommandRegistry,
) -> (
    Arc<SearchService>,
    UnboundedReceiver<SearchEvent>,
    UnboundedReceiver<SearchJob>,
) {
    let (events_tx, events) = futures::channel::mpsc::unbounded();
    let (jobs_tx, jobs) = futures::channel::mpsc::unbounded();
    let service = Arc::new(SearchService::new(
        thread::current().id(),
        events_tx,
        jobs_tx,
    ));
    cmds::register(commands, service.clone());
    (service, events, jobs)
}

// ----- The shell's half -----

/// A pending change Replace All made, until it is decided.
#[derive(Debug, Clone)]
pub struct PendingSearch {
    pub path: PathBuf,
    /// What was searched: the file's bytes (or the open document's text).
    pub fingerprint: u64,
    pub keep_open: bool,
    /// The file was open in an editor when the change was made (the applier checks its version).
    pub open: bool,
    /// Checked again on Accept: it may be applied.
    pub verified: bool,
    pub search_id: u64,
    pub window: Option<u8>,
}

/// The search half of the shell.
pub struct SearchUi {
    pub service: Arc<SearchService>,
    pub windows: [Entity<FindResults>; 2],
    pub dialog: Option<Entity<FindDialog>>,
    /// The window of the last search: F8 and Shift+F8 step through it.
    pub active: u8,
    pub pending: HashMap<u64, PendingSearch>,
    /// The dialog's queries per workspace folder (the newest first).
    history: BTreeMap<String, Vec<String>>,
}

impl SearchUi {
    pub fn new(service: Arc<SearchService>, theme: Theme, cx: &mut Context<Shell>) -> Self {
        Self {
            service,
            windows: [
                cx.new(|_| FindResults::new(theme, 1)),
                cx.new(|_| FindResults::new(theme, 2)),
            ],
            dialog: None,
            active: 1,
            pending: HashMap::new(),
            history: BTreeMap::new(),
        }
    }

    pub fn window(&self, n: u8) -> &Entity<FindResults> {
        &self.windows[usize::from(n.clamp(1, 2) - 1)]
    }
}

fn window_id(n: u8) -> &'static str {
    if n == 2 {
        ids::FIND_RESULTS_2
    } else {
        ids::FIND_RESULTS_1
    }
}

impl Shell {
    /// Wire the search (called once, at the end of [`Shell::new`]).
    pub(super) fn search_install(
        &mut self,
        mut events: UnboundedReceiver<SearchEvent>,
        mut jobs: UnboundedReceiver<SearchJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        use futures::StreamExt as _;
        for w in self.search.windows.clone() {
            cx.observe(&w, |_, _, cx| cx.notify()).detach();
        }
        let events_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(first) = events.next().await {
                let mut batch = vec![first];
                while let Ok(more) = events.try_recv() {
                    batch.push(more);
                }
                if this
                    .update_in(cx, |shell, window, cx| {
                        shell.on_search_events(batch, window, cx)
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        let jobs_task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                if this
                    .update_in(cx, |shell, window, cx| shell.on_search_job(job, window, cx))
                    .is_err()
                {
                    break;
                }
            }
        });
        self._tasks.push(events_task);
        self._tasks.push(jobs_task);
        self.search_load_history(cx);
    }

    /// A batch of events in one update: consecutive files of a search go to its window together (one rebuild).
    fn on_search_events(
        &mut self,
        batch: Vec<SearchEvent>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut pending: Option<(u64, u8, Vec<FileMatches>)> = None;
        let flush = |shell: &mut Self,
                     p: &mut Option<(u64, u8, Vec<FileMatches>)>,
                     cx: &mut Context<Self>| {
            if let Some((id, w, files)) = p.take() {
                shell
                    .search
                    .window(w)
                    .clone()
                    .update(cx, |r, cx| r.add_files(id, files, cx));
            }
        };
        for e in batch {
            match e {
                SearchEvent::Files {
                    search_id,
                    window: w,
                    files,
                } => match &mut pending {
                    Some((id, _, acc)) if *id == search_id => acc.extend(files),
                    _ => {
                        flush(self, &mut pending, cx);
                        pending = Some((search_id, w, files));
                    }
                },
                other => {
                    flush(self, &mut pending, cx);
                    self.on_search_event(other, window, cx);
                }
            }
        }
        flush(self, &mut pending, cx);
        cx.notify();
    }

    fn on_search_event(&mut self, event: SearchEvent, window: &mut Window, cx: &mut Context<Self>) {
        match event {
            SearchEvent::Started {
                search_id,
                window: w,
                append,
                title,
                root,
            } => {
                self.search.active = w;
                self.search.window(w).clone().update(cx, |r, cx| {
                    r.set_root(root);
                    r.start(search_id, title, append, cx)
                });
                let _ = self.invoke(
                    "eludite.view.show",
                    json!({ "id": window_id(w) }),
                    window,
                    cx,
                );
                self.dock.update(cx, |_, cx| cx.notify());
            }
            SearchEvent::Skipped {
                search_id,
                window: w,
                path,
                reason,
            } => self
                .search
                .window(w)
                .clone()
                .update(cx, |r, cx| r.skip(search_id, path, reason, cx)),
            SearchEvent::Finished {
                search_id,
                window: w,
                counts,
                error,
            } => {
                let text = match (&counts, &error) {
                    (_, Some(e)) => e.clone(),
                    (Some(c), None) if c.canceled => "Find in Files: stopped".to_owned(),
                    (Some(c), None) => format!(
                        "Find in Files: {} matching lines in {} files ({} searched)",
                        c.matching_lines, c.matching_files, c.files_searched
                    ),
                    (None, None) => "Find in Files: done".to_owned(),
                };
                let shown = self
                    .search
                    .window(w)
                    .read(cx)
                    .blocks
                    .iter()
                    .any(|b| b.search_id == search_id);
                self.search
                    .window(w)
                    .clone()
                    .update(cx, |r, cx| r.finish(search_id, counts, error, cx));
                if shown {
                    self.status.set(slots::STATE, text);
                }
            }
            SearchEvent::Files { .. } => {}
        }
    }

    fn on_search_job(&mut self, job: SearchJob, window: &mut Window, cx: &mut Context<Self>) {
        match job {
            SearchJob::Context(reply) => {
                let _ = reply.send(self.search_context(cx));
            }
            SearchJob::Hold(plan, reply) => self.search_hold(*plan, reply, window, cx),
            SearchJob::Results(request, reply) => {
                let _ = reply.send(self.search_results(request, window, cx));
            }
        }
    }

    /// The workspace's folders, the active document and project, and the open documents' text.
    fn search_context(&self, cx: &Context<Self>) -> SearchContext {
        let root = self.workspace_root().map(|r| normalize_path(&r));
        let projects: Vec<(String, PathBuf)> = self
            .workspace_tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| {
                let path = Path::new(&p.path);
                let folder = if path.is_dir() {
                    path.to_path_buf()
                } else {
                    path.parent().map(Path::to_path_buf).unwrap_or_default()
                };
                (p.name.clone(), normalize_path(&folder))
            })
            .collect();
        let active_document = self
            .controller
            .active_document()
            .filter(|id| self.documents.contains_key(id))
            .map(PathBuf::from);
        let active_project = active_document.as_ref().and_then(|d| {
            projects
                .iter()
                .filter(|(_, f)| d.starts_with(f))
                .max_by_key(|(_, f)| f.components().count())
                .map(|(n, _)| n.clone())
        });
        let overlay = self
            .documents
            .values()
            .map(|d| {
                (
                    normalize_path(&d.path),
                    Arc::<str>::from(d.view.read(cx).editor().text()),
                )
            })
            .collect();
        SearchContext {
            root,
            projects,
            active_document,
            active_project,
            overlay,
        }
    }

    /// Replace All on the UI thread: check open documents against what was searched, then hold the edits as pending
    /// changes (preview) or apply them.
    fn search_hold(
        &mut self,
        plan: ReplacePlan,
        reply: mpsc::SyncSender<Result<ReplaceOutput, CommandError>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let root = plan.root.clone();
        let shown = |p: &Path| display_path(p, root.as_deref());
        let mut skipped = plan.skipped.clone();
        let mut changes: BTreeMap<String, Vec<lsp::TextEdit>> = BTreeMap::new();
        let mut files: Vec<(PathBuf, u64, bool, u64)> = Vec::new();
        for f in &plan.files {
            let doc = self.open_document_at(&f.path);
            let reason = match &doc {
                Some(id) => {
                    let view = self.documents[id].view.clone();
                    let now = if f.open {
                        fingerprint(view.read(cx).editor().text().as_bytes())
                    } else {
                        // Opened since the search: what it loaded must be what was searched.
                        fingerprint(&view.read(cx).editor().buffer().to_file_bytes())
                    };
                    (now != f.fingerprint).then_some(if f.open {
                        "it changed in its editor since the search"
                    } else {
                        "it was opened and changed since the search"
                    })
                }
                None if f.open => Some("it was closed since the search; search again"),
                None => None,
            };
            if let Some(reason) = reason {
                skipped.push(Skipped {
                    path: shown(&f.path),
                    reason: reason.into(),
                });
                if let Some(w) = plan.window {
                    self.search.window(w).clone().update(cx, |r, cx| {
                        r.skip(plan.search_id, shown(&f.path), reason.into(), cx)
                    });
                }
                continue;
            }
            if let Some(id) = &doc {
                // The edits are positions in the text searched, which the language server must have too.
                self.flush_change(id, cx);
            }
            let edits = f
                .edits
                .iter()
                .map(|e| {
                    let line = e.line.saturating_sub(1) as u32;
                    lsp::TextEdit::new(
                        lsp::Range {
                            start: lsp::Position {
                                line,
                                character: e.start_utf16,
                            },
                            end: lsp::Position {
                                line,
                                character: e.end_utf16,
                            },
                        },
                        e.text.clone(),
                    )
                })
                .collect();
            changes.insert(path_to_uri(&f.path), edits);
            files.push((
                f.path.clone(),
                f.fingerprint,
                doc.is_some(),
                f.edits.len() as u64,
            ));
        }
        let base = ReplaceOutput {
            search_id: Some(plan.search_id),
            state: ReplaceState::Nothing,
            files: files
                .iter()
                .map(|(p, _, open, n)| ReplaceFile {
                    path: shown(p),
                    replacements: *n,
                    change: None,
                    open: *open,
                })
                .collect(),
            replacements: files.iter().map(|f| f.3).sum(),
            skipped,
            truncated: plan.truncated,
            elapsed_ms: plan.elapsed_ms,
            message: None,
        };
        if changes.is_empty() {
            let _ = reply.send(Ok(ReplaceOutput {
                message: Some("Every matching file was skipped (see `skipped`).".into()),
                ..base
            }));
            return;
        }
        let edit = lsp::WorkspaceEdit {
            changes: Some(changes),
            ..Default::default()
        };
        let options = ApplyOptions {
            label: Some(plan.label.clone()),
            ..Default::default()
        };
        if !plan.preview {
            self.apply_workspace_edit(
                &edit,
                options,
                window,
                cx,
                Box::new(move |_, summary, _, _| {
                    let applied = summary.applied;
                    let _ = reply.send(Ok(ReplaceOutput {
                        state: if applied {
                            ReplaceState::Applied
                        } else {
                            ReplaceState::Failed
                        },
                        message: Some(if applied {
                            format!(
                                "{} replacements in {} files applied ({} open documents, {} files written)",
                                summary.edits,
                                summary.files,
                                summary.open_documents,
                                summary.files_on_disk
                            )
                        } else {
                            summary
                                .message
                                .clone()
                                .unwrap_or_else(|| "the edit was refused".into())
                        }),
                        ..base
                    }));
                }),
            );
            return;
        }
        let ids = match self.capture_edit(&edit, &options, &plan.caller, window, cx) {
            Ok(ids) => ids,
            Err(e) => {
                let _ = reply.send(Ok(ReplaceOutput {
                    state: ReplaceState::Failed,
                    message: Some(e),
                    ..base
                }));
                return;
            }
        };
        let mut out = base;
        for id in &ids {
            let Some(change) = self.agents.changes.get(id) else {
                continue;
            };
            let path = change.path.clone();
            if let Some((_, fp, open, _)) = files.iter().find(|(p, ..)| normalize_path(p) == path) {
                self.search.pending.insert(
                    *id,
                    PendingSearch {
                        path: path.clone(),
                        fingerprint: *fp,
                        keep_open: plan.keep_open,
                        open: *open,
                        verified: false,
                        search_id: plan.search_id,
                        window: plan.window,
                    },
                );
            }
            let shown_path = shown(&path);
            if let Some(f) = out.files.iter_mut().find(|f| f.path == shown_path) {
                f.change = Some(*id);
            }
        }
        self.search.pending.retain(|id, _| {
            self.agents
                .changes
                .get(id)
                .is_some_and(|c| !c.state.decided())
        });
        out.state = ReplaceState::Pending;
        out.message = Some(format!(
            "{} replacements in {} files held for review as pending changes {}: accept them with eludite.agents.review",
            out.replacements,
            out.files.len(),
            match ids.as_slice() {
                [first, .., last] if ids.len() > 5 => format!("#{first} to #{last}"),
                _ => ids
                    .iter()
                    .map(|i| format!("#{i}"))
                    .collect::<Vec<_>>()
                    .join(", "),
            }
        ));
        let _ = reply.send(Ok(out));
    }

    /// Accept's hook for Replace All's changes (brief 0042): the ones to accept now; the rest are checked against
    /// the searched text first (off the UI thread for closed files), and with Keep modified files open the closed
    /// ones are opened, then accepted.
    pub(super) fn search_before_accept(
        &mut self,
        ids: &[u64],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Vec<u64> {
        let mut now = Vec::new();
        let mut check: Vec<(u64, PathBuf, u64)> = Vec::new();
        let mut failed: Vec<(u64, &'static str)> = Vec::new();
        for &id in ids {
            let Some(p) = self.search.pending.get(&id).cloned() else {
                now.push(id);
                continue;
            };
            if p.verified || p.open {
                // An open document's version is checked by the applier.
                now.push(id);
                continue;
            }
            if let Some(doc) = self.open_document_at(&p.path) {
                // Opened since Replace All: it must still hold what was searched.
                let bytes = self.documents[&doc]
                    .view
                    .read(cx)
                    .editor()
                    .buffer()
                    .to_file_bytes();
                if fingerprint(&bytes) == p.fingerprint {
                    now.push(id);
                } else {
                    failed.push((id, "it changed since the search; search again"));
                }
                continue;
            }
            check.push((id, p.path.clone(), p.fingerprint));
        }
        for (id, reason) in failed {
            self.search_change_failed(id, reason, cx);
        }
        if check.is_empty() {
            return now;
        }
        for (id, ..) in &check {
            if let Some(c) = self.agents.changes.get_mut(id) {
                c.state = ChangeState::Applying;
            }
        }
        self.sync_changes(cx);
        let work = cx.background_spawn(async move {
            check
                .into_iter()
                .map(|(id, path, fp)| {
                    let same = std::fs::read(&path).ok().map(|b| fingerprint(&b)) == Some(fp);
                    (id, path, same)
                })
                .collect::<Vec<_>>()
        });
        cx.spawn_in(window, async move |this, cx| {
            let checked = work.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.search_checked(checked, window, cx)
            });
        })
        .detach();
        now
    }

    /// Closed files checked on disk: accept the unchanged ones (opening them first with Keep modified files open).
    fn search_checked(
        &mut self,
        checked: Vec<(u64, PathBuf, bool)>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut accept = Vec::new();
        let mut waits = Vec::new();
        for (id, path, same) in checked {
            if !same {
                self.search_change_failed(id, "it changed on disk since the search", cx);
                continue;
            }
            let keep_open = self.search.pending.get(&id).is_some_and(|p| p.keep_open);
            if let Some(p) = self.search.pending.get_mut(&id) {
                p.verified = true;
            }
            accept.push(id);
            if keep_open && self.open_document_at(&path).is_none() {
                let doc = path.to_string_lossy().into_owned();
                if self.open_file(&doc, None, window, cx).is_ok() {
                    let key = normalize_path(&path).to_string_lossy().into_owned();
                    if self.loading.contains_key(&key) {
                        let (tx, rx) = futures::channel::oneshot::channel();
                        self.load_waiters.entry(key).or_default().push(tx);
                        waits.push(rx);
                    }
                }
            }
        }
        if accept.is_empty() {
            return;
        }
        if waits.is_empty() {
            let _ = self.decide(&accept, true, window, cx);
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            for w in waits {
                let _ = w.await;
            }
            let _ = this.update_in(cx, |shell, window, cx| {
                let _ = shell.decide(&accept, true, window, cx);
            });
        })
        .detach();
    }

    fn search_change_failed(&mut self, id: u64, reason: &'static str, cx: &mut Context<Self>) {
        if let Some(p) = self.search.pending.remove(&id)
            && let Some(w) = p.window
        {
            let root = self.workspace_root();
            let shown = display_path(&p.path, root.as_deref());
            self.search
                .window(w)
                .clone()
                .update(cx, |r, cx| r.skip(p.search_id, shown, reason.into(), cx));
        }
        self.finish_change(id, ChangeState::Failed(reason.into()), None, cx);
    }

    /// `eludite.search.results` for a window: a page, after moving its selection (opening the editor there) or
    /// clearing it.
    fn search_results(
        &mut self,
        request: SearchRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<ResultsOutput, CommandError> {
        let SearchRequest::Results {
            results_window,
            offset,
            limit,
            navigate,
            select,
            clear,
            ..
        } = request
        else {
            return Err(failed("not a results request"));
        };
        let n = results_window.unwrap_or(self.search.active);
        let w = self.search.window(n).clone();
        if clear {
            if let Some(id) = w.read(cx).running() {
                self.search.service.cancel(Some(id));
            }
            w.update(cx, |r, cx| r.clear(cx));
        }
        let target = match (navigate, select) {
            (Some(nav), _) => w.update(cx, |r, cx| r.step(nav, cx)),
            (None, Some(m)) => {
                let t = w.update(cx, |r, cx| r.select(m, cx));
                if t.is_none() {
                    return Err(failed(format!(
                        "no match {m}: Find Results {n} has {}",
                        w.read(cx).match_count()
                    )));
                }
                t
            }
            _ => None,
        };
        if let Some(t) = target {
            self.search.active = n;
            self.search_open(t, window, cx);
        }
        Ok(w.read(cx).page(offset, limit))
    }

    /// Open the editor at a result with the match selected.
    fn search_open(&mut self, t: Target, window: &mut Window, cx: &mut Context<Self>) {
        let path = t.path.to_string_lossy().into_owned();
        if let Err(e) = self.open_file(&path, Some((t.line as u32, t.column as u32)), window, cx) {
            self.status.set(slots::STATE, e.to_string());
            return;
        }
        let key = normalize_path(&t.path).to_string_lossy().into_owned();
        if let Some(view) = self.views.borrow().get(&key).cloned() {
            select_match(&view, &t, cx);
            return;
        }
        if self.loading.contains_key(&key) {
            let (tx, rx) = futures::channel::oneshot::channel();
            self.load_waiters.entry(key.clone()).or_default().push(tx);
            cx.spawn_in(window, async move |this, cx| {
                let _ = rx.await;
                let _ = this.update(cx, |shell, cx| {
                    if let Some(view) = shell.views.borrow().get(&key).cloned() {
                        select_match(&view, &t, cx);
                    }
                });
            })
            .detach();
        }
    }

    /// `eludite.search.*` from the UI: the dialog opens without a query; a search runs on a thread of its own; Stop,
    /// Clear and the windows' navigation are answered here. Returns false for other commands.
    pub(super) fn run_search(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if !cmds::ALL.contains(&command) {
            return false;
        }
        let request = match cmds::parse(command, args.clone()) {
            Ok(r) => r,
            Err(e) => {
                self.status.set(slots::STATE, e.to_string());
                cx.notify();
                return true;
            }
        };
        let args = std::mem::take(args);
        let staged = match request {
            SearchRequest::Find(FindArgs { query: None, .. }) => {
                self.search_open_dialog(Mode::Find, window, cx);
                Ok(SearchOutput::Find(FindOutput {
                    dialog: Some(true),
                    ..Default::default()
                }))
            }
            SearchRequest::Replace {
                find: FindArgs { query: None, .. },
                ..
            } => {
                self.search_open_dialog(Mode::Replace, window, cx);
                Ok(SearchOutput::Replace(ReplaceOutput {
                    search_id: None,
                    state: ReplaceState::Dialog,
                    files: Vec::new(),
                    replacements: 0,
                    skipped: Vec::new(),
                    truncated: false,
                    elapsed_ms: 0,
                    message: None,
                }))
            }
            SearchRequest::Find(_) | SearchRequest::Replace { .. } => {
                self.search_spawn(command.to_owned(), args, window, cx);
                return true;
            }
            SearchRequest::Cancel { search_id } => Ok(SearchOutput::Cancel(CancelOutput {
                canceled: self.search.service.cancel(search_id),
            })),
            r @ SearchRequest::Results {
                search_id: None, ..
            } => self
                .search_results(r, window, cx)
                .map(SearchOutput::Results),
            SearchRequest::Results { .. } => {
                // A search by id is read off the UI thread like an agent's.
                self.search_spawn(command.to_owned(), args, window, cx);
                return true;
            }
        };
        if let Err(e) = &staged {
            self.status.set(slots::STATE, e.to_string());
        }
        stage(staged);
        let _ = self.commands.invoke(command, args);
        cx.notify();
        true
    }

    /// Invoke `command` on a thread of its own, as the person; a failure goes to the status bar.
    fn search_spawn(
        &mut self,
        command: String,
        args: Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let commands = self.commands.clone();
        let (tx, rx) = futures::channel::oneshot::channel();
        let name = command.clone();
        let spawned = thread::Builder::new()
            .name("eludite-search".into())
            .spawn(move || {
                let r = with_caller(Caller::User, || commands.invoke(&command, args));
                let _ = tx.send(r);
            });
        if let Err(e) = spawned {
            self.status.set(slots::STATE, format!("{name}: {e}"));
            return;
        }
        cx.spawn_in(window, async move |this, cx| {
            let Ok(r) = rx.await else {
                return;
            };
            let _ = this.update(cx, |shell, cx| {
                match r {
                    Ok(v) if name == cmds::REPLACE => {
                        if let Some(m) = v["message"].as_str() {
                            shell.status.set(slots::STATE, m.to_owned());
                        }
                    }
                    Ok(_) => {}
                    Err(e) => {
                        eprintln!("eludite: {name}: {e}");
                        let text = match e {
                            CommandError::Failed(m) | CommandError::InvalidInput(m) => m,
                            other => other.to_string(),
                        };
                        shell.status.set(slots::STATE, text);
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// The query the dialog opens with: the active editor's selection on one line, else the word at the caret.
    fn search_selection(&self, cx: &Context<Self>) -> Option<String> {
        let id = self.controller.active_document()?;
        let doc = self.documents.get(&id)?;
        let editor = doc.view.read(cx).editor();
        let sel = editor.primary_selection();
        let buffer = editor.buffer();
        if !sel.is_empty() {
            let text = buffer.text_for_range(sel.range());
            return (!text.contains('\n')).then_some(text);
        }
        let point = buffer.offset_to_point(sel.head);
        let line = buffer.line(point.row);
        let col = (point.column as usize).min(line.len());
        let is_word = |c: char| c.is_alphanumeric() || c == '_';
        let start = line[..col]
            .char_indices()
            .rev()
            .take_while(|(_, c)| is_word(*c))
            .last()
            .map_or(col, |(i, _)| i);
        let end = line[col..]
            .char_indices()
            .find(|(_, c)| !is_word(*c))
            .map_or(line.len(), |(i, _)| col + i);
        (end > start).then(|| line[start..end].to_owned())
    }

    fn search_open_dialog(&mut self, mode: Mode, window: &mut Window, cx: &mut Context<Self>) {
        let selection = self.search_selection(cx);
        let dialog = match self.search.dialog.clone() {
            Some(d) => d,
            None => {
                let theme = self.theme;
                let d = cx.new(|cx| FindDialog::new(theme, mode, cx));
                cx.subscribe_in(&d, window, |shell, _, e: &FindDialogEvent, window, cx| {
                    shell.on_search_dialog(*e, window, cx)
                })
                .detach();
                self.search.dialog = Some(d.clone());
                d
            }
        };
        let projects: Vec<String> = self
            .workspace_tree
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .projects
            .iter()
            .map(|p| p.name.clone())
            .collect();
        let history = self.search_history();
        dialog.update(cx, |d, cx| {
            d.set_projects(projects, cx);
            d.set_history(history, cx);
            d.show(mode, selection, cx);
        });
        dialog.focus_handle(cx).focus(window, cx);
        cx.notify();
    }

    fn search_close_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search.dialog.take().is_some() {
            if let Some(id) = self.controller.active_document()
                && let Some(doc) = self.documents.get(&id)
            {
                doc.view.focus_handle(cx).focus(window, cx);
            } else {
                gpui::Focusable::focus_handle(self, cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    fn on_search_dialog(
        &mut self,
        event: FindDialogEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(dialog) = self.search.dialog.clone() else {
            return;
        };
        let (args, replacement, keep_open, n) = {
            let d = dialog.read(cx);
            (d.args(), d.replacement.clone(), d.keep_open, d.window)
        };
        match event {
            FindDialogEvent::Close => self.search_close_dialog(window, cx),
            FindDialogEvent::FindAll => {
                if let Some(q) = &args.query {
                    self.search_remember(q.clone(), cx);
                    self.run(cmds::FIND, args_json(&args), window, cx);
                }
            }
            FindDialogEvent::ReplaceAll => {
                if let Some(q) = &args.query {
                    self.search_remember(q.clone(), cx);
                    let mut v = args_json(&args);
                    v["replacement"] = json!(replacement);
                    v["preview"] = json!(true);
                    v["keep_open"] = json!(keep_open);
                    self.run(cmds::REPLACE, v, window, cx);
                }
            }
            FindDialogEvent::SkipFile => self.run(
                cmds::RESULTS,
                json!({ "results_window": n, "navigate": "next_file" }),
                window,
                cx,
            ),
            FindDialogEvent::ReplaceNext => {
                self.search_replace_next(&args, &replacement, n, window, cx)
            }
        }
    }

    /// Replace Next: replace the match selected in the editor (through `eludite.workspace.apply_edit`), then step to
    /// the next result. Without results yet, Find All first.
    fn search_replace_next(
        &mut self,
        args: &FindArgs,
        replacement: &str,
        n: u8,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(q) = args.query.clone() else {
            return;
        };
        let compiled = match (Query {
            text: q,
            regex: args.regex,
            case_sensitive: args.case_sensitive,
            whole_word: args.whole_word,
        })
        .compile()
        {
            Ok(c) => c,
            Err(e) => {
                self.status.set(slots::STATE, e.to_string());
                cx.notify();
                return;
            }
        };
        if self.search.window(n).read(cx).match_count() == 0 {
            self.run(cmds::FIND, args_json(args), window, cx);
            return;
        }
        if let Some(id) = self.controller.active_document()
            && let Some(doc) = self.documents.get(&id)
        {
            let view = doc.view.clone();
            let (text, start, end) = {
                let e = view.read(cx).editor();
                let sel = e.primary_selection();
                (
                    e.buffer().text_for_range(sel.range()),
                    sel.start(),
                    sel.end(),
                )
            };
            let whole = compiled.ranges(text.as_bytes());
            if !text.is_empty() && whole.len() == 1 && whole[0] == (0..text.len()) {
                let new = compiled
                    .replacements(text.as_bytes(), replacement)
                    .pop()
                    .map(|(_, t)| t)
                    .unwrap_or_default();
                self.flush_change(&id, cx);
                let (a, b) = {
                    let e = view.read(cx).editor();
                    (
                        lsp_position(e.buffer(), start),
                        lsp_position(e.buffer(), end),
                    )
                };
                let edit = json!({ "changes": { path_to_uri(Path::new(&id)): [
                    {"range": {"start": a, "end": b}, "newText": new}
                ]}});
                self.run(
                    workspace::WORKSPACE_APPLY_EDIT,
                    json!({ "edit": edit, "label": "Replace Next" }),
                    window,
                    cx,
                );
            }
        }
        self.run(
            cmds::RESULTS,
            json!({ "results_window": n, "navigate": "next" }),
            window,
            cx,
        );
    }

    // ----- History and settings -----

    fn history_key(&self) -> String {
        self.workspace_root()
            .map(|r| r.to_string_lossy().into_owned())
            .unwrap_or_default()
    }

    /// The dialog's queries for this workspace, the newest first.
    pub fn search_history(&self) -> Vec<String> {
        self.search
            .history
            .get(&self.history_key())
            .cloned()
            .unwrap_or_default()
    }

    /// Remember `query` (at most [`HISTORY`] per workspace), written off the UI thread.
    fn search_remember(&mut self, query: String, cx: &mut Context<Self>) {
        let key = self.history_key();
        let list = self.search.history.entry(key).or_default();
        list.retain(|q| *q != query);
        list.insert(0, query);
        list.truncate(HISTORY);
        let history = self.search_history();
        if let Some(d) = &self.search.dialog {
            d.update(cx, |d, cx| d.set_history(history, cx));
        }
        if let Some(dir) = self.search.service.state_dir() {
            let map = self.search.history.clone();
            cx.background_spawn(async move {
                let _ = std::fs::create_dir_all(&dir);
                if let Ok(text) = serde_json::to_string_pretty(&map) {
                    let _ = std::fs::write(dir.join("history.json"), text);
                }
            })
            .detach();
        }
    }

    /// Read the queries remembered (off the UI thread).
    pub(super) fn search_load_history(&mut self, cx: &mut Context<Self>) {
        let Some(dir) = self.search.service.state_dir() else {
            return;
        };
        let read = cx.background_spawn(async move {
            std::fs::read_to_string(dir.join("history.json"))
                .ok()
                .and_then(|t| serde_json::from_str::<BTreeMap<String, Vec<String>>>(&t).ok())
                .unwrap_or_default()
        });
        cx.spawn(async move |this, cx| {
            let map = read.await;
            let _ = this.update(cx, |shell, _| {
                for (k, v) in map {
                    shell.search.history.entry(k).or_insert(v);
                }
            });
        })
        .detach();
    }

    /// `search.*` from the settings store.
    pub(super) fn search_apply_settings(&mut self) {
        let settings = {
            let s = self.settings.lock();
            SearchSettings {
                excludes: s
                    .effective("search.excludes")
                    .0
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(Value::as_str)
                            .map(str::to_owned)
                            .collect()
                    })
                    .unwrap_or_default(),
                use_gitignore: s.bool("search.useGitignore"),
                max_file_size_mb: s.effective("search.maxFileSize").0.as_u64().unwrap_or(4),
                follow_symlinks: s.bool("search.followSymlinks"),
            }
        };
        self.search.service.set_settings(settings);
    }

    #[cfg(test)]
    pub fn search_ui(&self) -> &SearchUi {
        &self.search
    }
}

/// The LSP position (line, UTF-16 column) of byte `offset` in `buffer`.
fn lsp_position(buffer: &eludite_editor::Buffer, offset: usize) -> Value {
    let p = buffer.offset_to_point(offset);
    let line = buffer.line(p.row);
    let col = line
        .get(..p.column as usize)
        .map_or(0, eludite_search::utf16_len);
    json!({ "line": p.row, "character": col })
}

/// Select a result's match in its editor.
fn select_match(view: &Entity<EditorView>, t: &Target, cx: &mut gpui::App) {
    view.update(cx, |v, cx| {
        v.update_editor(cx, |e| {
            let start = offset_of(e.buffer(), t.line as u32, t.column as u32);
            let end = offset_of(e.buffer(), t.line as u32, (t.column + t.len) as u32);
            e.set_selections(
                vec![SelectionRange {
                    tail: start,
                    head: end,
                }],
                0,
            );
        });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_read_as_visual_studios_count_line() {
        let args = FindArgs {
            query: Some("Order".into()),
            case_sensitive: true,
            include: vec!["*.cs".into()],
            ..FindArgs::default()
        };
        assert_eq!(
            block_title(&args, None, Some(1), "Entire Solution"),
            "Find all \"Order\", Match case, Subfolders, Find Results 1, Entire Solution, \"*.cs\""
        );
        assert_eq!(
            block_title(
                &FindArgs {
                    query: Some("a".into()),
                    ..FindArgs::default()
                },
                Some(("b", true)),
                Some(2),
                "All Open Documents"
            ),
            "Replace all \"a\", \"b\", Subfolders, Keep modified files open, Find Results 2, All Open Documents, \"*.*\""
        );
        let v = args_json(&args);
        assert_eq!(v["scope"], "solution");
        assert_eq!(v["include"], json!(["*.cs"]));
        assert!(cmds::parse(cmds::FIND, v).is_ok());
    }
}
