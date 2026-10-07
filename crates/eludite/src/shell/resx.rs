//! The `.resx` editor (proposal 0005, PLAN.md 4.11): a `.resx` file opens as its whole resource set, a document tab
//! titled by the neutral file with the grid of [`editor::ResxEditor`], and `eludite.resx.*` reads and writes the
//! same sets.
//!
//! - **Opening.** [`Shell::open_file`] routes a `.resx` here unless `resx.openAsText` is on or the command asks for
//!   the text editor: the tab is `resx:<neutral file>`, one per set whichever of its files was opened. The files are
//!   read and parsed off the UI thread ([`model::SetModel`]); the host, when one runs, lists the set's project, its
//!   neutral language, its designer file and its access modifier (`eludite/resx/sets`), cached per generation.
//! - **Writing.** The grid's edits change the model and mark the tab dirty; Save (Ctrl+S, `eludite.editor.save` on
//!   the tab) writes every dirty file as one whole-file edit through the workspace-edit applier (brief 0015), so an
//!   open text document of the file changes in its buffer and a closed one is written atomically; the host then
//!   regenerates the designer when keys changed. Closing a dirty tab asks, as a text document does.
//! - **Agents.** The commands run on the UI thread through [`ResxBus`]: reads come from the open editor's model or a
//!   fresh load; a write loads the set afresh (an open editor with unsaved changes refuses it), applies the operation
//!   and writes through the applier, held as pending changes for an agent under `edit_buffer: review`
//!   (`capture_next`), then the open editor reloads. `access_modifier` is the host's `eludite/resx/designer`.

pub mod editor;
pub mod model;

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::sync::mpsc;
use std::thread::ThreadId;
use std::time::{Duration, Instant};

use eludite_commands::resx::{
    self as cmd, AccessModifier, AccessModifierOutput, AddOutput, AddStatus, CellOut, CellResult,
    CultureSummary, EntriesOutput, EntryOut, Field, ModifierStatus, RemoveOutput, RemoveStatus,
    RemovedKey, RenameOutput, RenameStatus, ResxCommands, ResxOutput, ResxRequest, SetOutput,
    SetSource, SetSummary, SetsOutput, ValidateOutput, WarningOut, WriteStatus, WrittenFile,
};
use eludite_commands::view::ViewTarget as _;
use eludite_commands::workspace::{
    CloseSave, FileCloseOutput, FileOpenOutput, SaveOutput, WorkspaceOutput,
};
use eludite_commands::{Caller, CommandError, CommandRegistry, current_caller};
use eludite_lsp::{host, lsp};
use eludite_protocol::RequestType;
use eludite_resx::{Row, Rules};
use futures::StreamExt as _;
use futures::channel::mpsc::{UnboundedReceiver, UnboundedSender, unbounded};
use gpui::{AnyView, AppContext as _, Context, Entity, Focusable as _, PromptLevel, Window};
use serde_json::{Map, Value, json};

use self::editor::{EditorEvent, ResxEditor};
use self::model::{FileWrite, SetModel};
use super::Shell;
use super::documents::{normalize_path, path_to_uri};
use super::session::RequestError;
use super::workspace_edit::{ApplyOptions, ApplySummary};

/// The document tab of a set: `resx:<absolute neutral file>`.
pub const TAB_PREFIX: &str = "resx:";

pub fn tab_id(neutral: &Path) -> String {
    format!("{TAB_PREFIX}{}", neutral.display())
}

/// The neutral file of a resx tab id.
pub fn neutral_of_tab(id: &str) -> Option<&str> {
    id.strip_prefix(TAB_PREFIX)
}

pub type Documents = Rc<RefCell<HashMap<String, AnyView>>>;

type Outcome = Result<ResxOutput, CommandError>;

/// How long an agent's call waits for the UI, the files and the host.
const AGENT_TIMEOUT: Duration = Duration::from_secs(120);

thread_local! {
    static STAGED: RefCell<Option<Outcome>> = const { RefCell::new(None) };
}

/// The answer the shell computed for the bus invocation it is about to make on this (the UI) thread.
pub fn stage(outcome: Outcome) {
    STAGED.with(|s| *s.borrow_mut() = Some(outcome));
}

/// A request from another thread, for the UI thread.
pub struct ResxJob {
    pub request: ResxRequest,
    pub caller: Caller,
    pub reply: mpsc::SyncSender<Outcome>,
}

/// The shell's [`ResxCommands`].
pub struct ResxBus {
    pub ui_thread: ThreadId,
    pub jobs: UnboundedSender<ResxJob>,
}

impl ResxCommands for ResxBus {
    fn apply(&self, request: ResxRequest) -> Outcome {
        if std::thread::current().id() == self.ui_thread {
            return STAGED.with(|s| s.borrow_mut().take()).unwrap_or_else(|| {
                Err(CommandError::Failed(format!(
                    "{} runs on the UI thread through the shell",
                    request.command()
                )))
            });
        }
        let (reply, rx) = mpsc::sync_channel(1);
        self.jobs
            .unbounded_send(ResxJob {
                request,
                caller: current_caller(),
                reply,
            })
            .map_err(|_| CommandError::Failed("the window is closed".into()))?;
        rx.recv_timeout(AGENT_TIMEOUT)
            .map_err(|_| CommandError::Failed("the editor did not answer in time".into()))?
    }
}

/// Register the resx commands; returns what the shell drains on the UI thread.
pub fn register(commands: &CommandRegistry) -> UnboundedReceiver<ResxJob> {
    let (tx, rx) = unbounded();
    let bus: std::sync::Arc<dyn ResxCommands> = std::sync::Arc::new(ResxBus {
        ui_thread: std::thread::current().id(),
        jobs: tx,
    });
    cmd::register(commands, bus);
    rx
}

/// When the steps of opening a set happened (the report's numbers).
#[derive(Debug, Clone, Default)]
#[cfg_attr(not(test), allow(dead_code))]
pub struct ResxTimings {
    pub opened: Option<Instant>,
    pub shown: Option<Instant>,
}

/// The shell's resx state.
#[derive(Default)]
pub struct ResxUi {
    pub documents: Documents,
    pub editors: HashMap<String, Entity<ResxEditor>>,
    pub rules: Rules,
    pub sort_on_save: bool,
    pub open_as_text: bool,
    pub timings: ResxTimings,
    /// `eludite/resx/sets`'s answer of a generation.
    pub sets: Option<(u64, Vec<host::ResxSet>)>,
}

/// The neutral file of the set `path` belongs to (`None` for a file that is not `.resx`).
pub fn neutral_of(path: &Path) -> Option<PathBuf> {
    let name = path.file_name()?.to_str()?;
    let (base, _) = eludite_resx::base_name(name)?;
    Some(path.with_file_name(format!("{base}.resx")))
}

pub fn is_resx(path: &Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("resx"))
}

fn describe(e: RequestError) -> String {
    match e {
        RequestError::NoHost => "no .NET solution is open (eludite-host is not running)".into(),
        RequestError::Canceled => "canceled".into(),
        RequestError::Stale => "the solution changed while the host answered; ask again".into(),
        other => format!("{other:?}"),
    }
}

fn failed(message: impl Into<String>) -> CommandError {
    CommandError::Failed(message.into())
}

fn modifier_of(m: host::AccessModifier) -> AccessModifier {
    match m {
        host::AccessModifier::Internal => AccessModifier::Internal,
        host::AccessModifier::Public => AccessModifier::Public,
        host::AccessModifier::None => AccessModifier::None,
    }
}

fn host_modifier(m: AccessModifier) -> host::AccessModifier {
    match m {
        AccessModifier::Internal => host::AccessModifier::Internal,
        AccessModifier::Public => host::AccessModifier::Public,
        AccessModifier::None => host::AccessModifier::None,
    }
}

fn same_file(a: &Path, b: &Path) -> bool {
    normalize_path(a) == normalize_path(b)
}

/// The filter of `eludite.resx.sets`.
struct SetsFilter {
    project: Option<String>,
    path: Option<PathBuf>,
    include_non_string: bool,
}

/// The sets under `root`, with what the host lists of them.
fn compute_sets(
    root: &Path,
    excludes: &[String],
    gitignore: bool,
    rules: Rules,
    host_sets: &[host::ResxSet],
    filter: &SetsFilter,
) -> Vec<SetSummary> {
    let found = eludite_resx::discover(root, excludes, gitignore).unwrap_or_default();
    let mut out = Vec::new();
    for files in found {
        if let Some(p) = &filter.path {
            let under = files.neutral.starts_with(p) || same_file(&files.neutral, p);
            let same_set = neutral_of(p).is_some_and(|n| same_file(&n, &files.neutral));
            if !under && !same_set {
                continue;
            }
        }
        let info = host_sets
            .iter()
            .find(|s| same_file(Path::new(&s.path), &files.neutral));
        if let Some(project) = &filter.project {
            let Some(i) = info else { continue };
            if i.project_name != *project && !same_file(Path::new(&i.project), Path::new(project)) {
                continue;
            }
        }
        let set = match eludite_resx::ResourceSet::load(files) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("eludite: resx: {e}");
                continue;
            }
        };
        let strings = set.neutral.strings().count() as u32;
        if strings == 0 && !filter.include_non_string {
            continue;
        }
        let rows = set.rows(&rules);
        let mut cultures = Vec::new();
        for culture in set.culture_names() {
            let file = set.file(&culture).expect("listed");
            let missing = if culture.is_empty() {
                0
            } else {
                rows.iter()
                    .filter(|r| !r.invariant && r.cell(&culture).is_none_or(|c| c.missing()))
                    .count() as u32
            };
            let warnings = rows
                .iter()
                .map(|r| r.cell(&culture).map_or(0, |c| c.warnings.len()))
                .sum::<usize>() as u32;
            cultures.push(CultureSummary {
                name: culture.clone(),
                path: file.path().display().to_string(),
                strings: file.strings().count() as u32,
                missing,
                warnings,
            });
        }
        let folder = set
            .files
            .neutral
            .parent()
            .and_then(|d| d.strip_prefix(root).ok())
            .map(|d| d.display().to_string())
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| ".".into());
        out.push(SetSummary {
            neutral: set.files.neutral.display().to_string(),
            base_name: set.files.base_name.clone(),
            folder,
            project: info.map(|i| i.project_name.clone()),
            neutral_language: info.and_then(|i| i.neutral_language.clone()),
            cultures,
            strings,
            non_strings: set.neutral.entries().len() as u32 - strings,
            missing: rows.iter().map(Row::missing_count).sum::<usize>() as u32,
            warnings: rows.iter().map(Row::warning_count).sum::<usize>() as u32,
            designer: info.and_then(|i| i.designer.clone()),
            access_modifier: info.map(|i| modifier_of(i.access_modifier)),
            source: if info.is_some() {
                SetSource::Project
            } else {
                SetSource::Folder
            },
        });
    }
    out
}

fn warning_out(w: &eludite_resx::Warning) -> WarningOut {
    WarningOut {
        key: w.key.clone(),
        culture: w.culture.clone(),
        rule: w.rule.as_str().into(),
        message: w.message.clone(),
    }
}

/// `eludite.resx.entries` over a model.
#[allow(clippy::too_many_arguments)]
fn entries_output(
    model: &SetModel,
    info: Option<&host::ResxSet>,
    query: Option<&str>,
    missing: bool,
    warnings: bool,
    invariant: Option<bool>,
    cultures: Option<&[String]>,
    skip: usize,
    take: usize,
) -> EntriesOutput {
    let all = model.cultures();
    let cultures: Vec<String> = match cultures {
        Some(c) => all
            .iter()
            .filter(|a| c.iter().any(|x| x.eq_ignore_ascii_case(a)))
            .cloned()
            .collect(),
        None => all,
    };
    let q = query.map(str::to_lowercase);
    let rows: Vec<&Row> = model
        .rows()
        .iter()
        .filter(|r| {
            let cells: Vec<&eludite_resx::Cell> = r
                .cells
                .iter()
                .filter(|c| cultures.contains(&c.culture))
                .collect();
            (!missing
                || (!r.invariant && cells.iter().any(|c| !c.culture.is_empty() && c.missing())))
                && (!warnings || cells.iter().any(|c| !c.warnings.is_empty()))
                && invariant.is_none_or(|i| r.invariant == i)
                && q.as_ref().is_none_or(|q| {
                    r.key.to_lowercase().contains(q)
                        || cells.iter().any(|c| {
                            c.value
                                .as_deref()
                                .is_some_and(|v| v.to_lowercase().contains(q))
                                || c.comment
                                    .as_deref()
                                    .is_some_and(|v| v.to_lowercase().contains(q))
                        })
                })
        })
        .collect();
    let total = rows.len() as u32;
    let entries = rows
        .into_iter()
        .skip(skip)
        .take(take)
        .map(|r| EntryOut {
            key: r.key.clone(),
            invariant: r.invariant,
            cells: r
                .cells
                .iter()
                .filter(|c| cultures.contains(&c.culture))
                .map(|c| CellOut {
                    culture: c.culture.clone(),
                    value: c.value.clone(),
                    comment: c.comment.clone(),
                    missing: c.missing(),
                    warnings: c.warnings.iter().map(warning_out).collect(),
                    line: c.line,
                })
                .collect(),
            references: None,
            changed: None,
        })
        .collect();
    EntriesOutput {
        set: model.neutral_path().display().to_string(),
        neutral_language: info.and_then(|i| i.neutral_language.clone()),
        cultures,
        total,
        skip: skip as u32,
        entries,
    }
}

fn validate_output(model: &SetModel, cultures: Option<&[String]>) -> ValidateOutput {
    let warnings = model
        .rows()
        .iter()
        .flat_map(|r| r.cells.iter())
        .filter(|c| cultures.is_none_or(|cs| cs.iter().any(|x| x.eq_ignore_ascii_case(&c.culture))))
        .flat_map(|c| c.warnings.iter().map(warning_out))
        .collect();
    ValidateOutput {
        set: model.neutral_path().display().to_string(),
        warnings,
        rules: model
            .rules()
            .enabled()
            .iter()
            .map(|r| r.as_str().to_owned())
            .collect(),
    }
}

/// The outputs' statuses when the write is pending (held for review, or sent off the UI thread).
fn mark_pending(out: &mut ResxOutput, message: Option<String>) {
    match out {
        ResxOutput::Set(o) => {
            for r in &mut o.results {
                if !matches!(r.status, WriteStatus::Error | WriteStatus::Unchanged) {
                    r.status = WriteStatus::Pending;
                    r.message = message.clone();
                }
            }
        }
        ResxOutput::Add(o) => {
            if o.status == AddStatus::Added {
                o.status = AddStatus::Pending;
                o.message = message;
            }
        }
        ResxOutput::Remove(o) => {
            if o.status == RemoveStatus::Removed {
                o.status = RemoveStatus::Pending;
                o.message = message;
            }
        }
        ResxOutput::Rename(o) => {
            if o.status == RenameStatus::Renamed {
                o.status = RenameStatus::Pending;
                o.message = message;
            }
        }
        ResxOutput::AccessModifier(o) => {
            if o.status == ModifierStatus::Written {
                o.status = ModifierStatus::Pending;
                o.message = message;
            }
        }
        ResxOutput::Sets(_) | ResxOutput::Entries(_) | ResxOutput::Validate(_) => {}
    }
}

/// The write of `request` on a model: the operation and its output.
fn mutate(model: &mut SetModel, request: &ResxRequest, create_culture: bool) -> Outcome {
    let set = model.neutral_path().display().to_string();
    Ok(match request {
        ResxRequest::Set { cells, .. } => {
            let mut results = Vec::with_capacity(cells.len());
            let mut written: HashMap<String, u32> = HashMap::new();
            for c in cells {
                let mut status = WriteStatus::Unchanged;
                let mut message = None;
                fn step(
                    status: &mut WriteStatus,
                    message: &mut Option<String>,
                    r: Result<WriteStatus, String>,
                ) {
                    match r {
                        Ok(WriteStatus::Unchanged) => {}
                        Ok(s) => {
                            if *status == WriteStatus::Unchanged {
                                *status = s;
                            }
                        }
                        Err(e) => {
                            *status = WriteStatus::Error;
                            *message = Some(e);
                        }
                    }
                }
                if let Field::Text(v) = &c.value {
                    step(
                        &mut status,
                        &mut message,
                        model.set_value(&c.key, &c.culture, Some(v), create_culture),
                    );
                } else if c.value == Field::Remove {
                    step(
                        &mut status,
                        &mut message,
                        model.set_value(&c.key, &c.culture, None, false),
                    );
                }
                if status != WriteStatus::Error {
                    match &c.comment {
                        Field::Keep => {}
                        Field::Remove => step(
                            &mut status,
                            &mut message,
                            model.set_comment(&c.key, &c.culture, None),
                        ),
                        Field::Text(t) => step(
                            &mut status,
                            &mut message,
                            model.set_comment(&c.key, &c.culture, Some(t)),
                        ),
                    }
                }
                if status != WriteStatus::Error
                    && let Some(i) = c.invariant
                {
                    step(&mut status, &mut message, model.set_invariant(&c.key, i));
                }
                if !matches!(status, WriteStatus::Error | WriteStatus::Unchanged) {
                    let path = model
                        .set
                        .files
                        .culture_path(&c.culture)
                        .display()
                        .to_string();
                    *written.entry(path).or_default() += 1;
                }
                results.push(CellResult {
                    set: set.clone(),
                    key: c.key.clone(),
                    culture: c.culture.clone(),
                    status,
                    message,
                });
            }
            let mut written: Vec<WrittenFile> = written
                .into_iter()
                .map(|(path, entries)| WrittenFile { path, entries })
                .collect();
            written.sort_by(|a, b| a.path.cmp(&b.path));
            ResxOutput::Set(SetOutput { results, written })
        }
        ResxRequest::Add {
            key,
            value,
            comment,
            invariant,
            ..
        } => match model.add_key(key, value, comment.as_deref(), *invariant) {
            Ok((status, line)) => ResxOutput::Add(AddOutput {
                set,
                key: key.clone(),
                status,
                line,
                message: None,
            }),
            Err(e) => ResxOutput::Add(AddOutput {
                set,
                key: key.clone(),
                status: AddStatus::Error,
                line: None,
                message: Some(e),
            }),
        },
        ResxRequest::Remove { keys, .. } => match model.remove_keys(keys) {
            Ok((removed, missing)) => ResxOutput::Remove(RemoveOutput {
                set,
                removed: removed
                    .into_iter()
                    .map(|(key, files)| RemovedKey { key, files })
                    .collect(),
                missing,
                status: RemoveStatus::Removed,
                message: None,
            }),
            Err(e) => ResxOutput::Remove(RemoveOutput {
                set,
                removed: vec![],
                missing: vec![],
                status: RemoveStatus::Error,
                message: Some(e),
            }),
        },
        ResxRequest::Rename { key, new_key, .. } => match model.rename_key(key, new_key) {
            Ok((status, files)) => ResxOutput::Rename(RenameOutput {
                set,
                key: key.clone(),
                new_key: new_key.clone(),
                files,
                references: None,
                status,
                message: None,
            }),
            Err(e) => ResxOutput::Rename(RenameOutput {
                set,
                key: key.clone(),
                new_key: new_key.clone(),
                files: 0,
                references: None,
                status: RenameStatus::Error,
                message: Some(e),
            }),
        },
        other => return Err(failed(format!("{} is not a write", other.command()))),
    })
}

/// The set a request names.
fn request_set(request: &ResxRequest) -> Option<&str> {
    match request {
        ResxRequest::Entries { set, .. }
        | ResxRequest::Add { set, .. }
        | ResxRequest::Remove { set, .. }
        | ResxRequest::Rename { set, .. }
        | ResxRequest::Validate { set, .. }
        | ResxRequest::AccessModifier { set, .. } => Some(set),
        ResxRequest::Set { cells, .. } => cells.first().map(|c| c.set.as_str()),
        ResxRequest::Sets { .. } => None,
    }
}

type Reply = Option<mpsc::SyncSender<Outcome>>;

/// Called once the files are written (or the write refused or held): the summary and the writes.
type WritesDone =
    Box<dyn FnOnce(&mut Shell, ApplySummary, Vec<FileWrite>, &mut Window, &mut Context<Shell>)>;

fn answer(reply: &Reply, outcome: Outcome) -> Outcome {
    if let Some(r) = reply {
        let _ = r.send(outcome.clone());
    }
    outcome
}

impl Shell {
    /// Wire the jobs from other threads (call once from `new`).
    pub(super) fn resx_install(
        &mut self,
        mut jobs: UnboundedReceiver<ResxJob>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let task = cx.spawn_in(window, async move |this, cx| {
            while let Some(job) = jobs.next().await {
                let ResxJob {
                    request,
                    caller,
                    reply,
                } = job;
                let sent = this.update_in(cx, |shell, window, cx| {
                    let _ = shell.apply_resx(request, caller, Some(reply.clone()), window, cx);
                });
                if sent.is_err() {
                    let _ = reply.send(Err(failed("the window is closed")));
                }
            }
        });
        self._tasks.push(task);
    }

    /// `resx.*` from the settings store.
    pub(super) fn resx_apply_settings(&mut self, cx: &mut Context<Self>) {
        let (rules, sort, text) = {
            let s = self.settings.lock();
            (
                Rules {
                    placeholders: s.bool("resx.rules.placeholders"),
                    punctuation: s.bool("resx.rules.punctuation"),
                    whitespace: s.bool("resx.rules.whitespace"),
                    untranslated: s.bool("resx.rules.untranslated"),
                },
                s.bool("resx.sortOnSave"),
                s.bool("resx.openAsText"),
            )
        };
        self.resx.sort_on_save = sort;
        self.resx.open_as_text = text;
        if self.resx.rules != rules {
            self.resx.rules = rules;
            for view in self.resx.editors.values() {
                view.update(cx, |v, cx| {
                    if let Some(m) = v.model.as_mut() {
                        m.set_rules(rules);
                    }
                    cx.notify();
                });
            }
        }
    }

    fn search_filters(&self) -> (Vec<String>, bool) {
        let s = self.settings.lock();
        let excludes = s
            .effective("search.excludes")
            .0
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(Value::as_str)
                    .map(str::to_owned)
                    .collect()
            })
            .unwrap_or_default();
        (excludes, s.bool("search.useGitignore"))
    }

    fn resx_root(&self) -> Option<PathBuf> {
        self.workspace_root().or_else(|| {
            self.solution
                .as_ref()
                .and_then(|s| s.parent().map(Path::to_path_buf))
        })
    }

    /// Send `R` to the host; `then` gets the answer on the UI thread (the UI never waits).
    fn resx_host_call<R>(
        &mut self,
        params: R::Params,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, Result<R::Result, RequestError>, &mut Window, &mut Context<Shell>)
        + 'static,
    ) where
        R: RequestType + 'static,
        R::Params: Send + 'static,
        R::Result: Send + 'static,
    {
        let (_, rx) = self.session.request::<R>(params);
        cx.spawn_in(window, async move |this, cx| {
            let result = match rx.await {
                Ok(reply) => reply.result,
                Err(_) => Err(RequestError::NoHost),
            };
            let _ = this.update_in(cx, |shell, window, cx| then(shell, result, window, cx));
        })
        .detach();
    }

    /// The host's sets of the current generation, fetched when the cache is older; `None` without a solution.
    fn with_host_sets(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, Option<Vec<host::ResxSet>>, &mut Window, &mut Context<Shell>)
        + 'static,
    ) {
        self.with_host_sets_tries(5, window, cx, then);
    }

    /// [`Shell::with_host_sets`], asking again (after a moment, under the generation the shell knows by then) when
    /// the host answered that the solution moved on, up to `tries` times: the editor opens while the solution is
    /// still loading.
    fn with_host_sets_tries(
        &mut self,
        tries: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
        then: impl FnOnce(&mut Shell, Option<Vec<host::ResxSet>>, &mut Window, &mut Context<Shell>)
        + 'static,
    ) {
        if self.solution.is_none() {
            return then(self, None, window, cx);
        }
        let generation = self.generation;
        if let Some((g, sets)) = &self.resx.sets
            && *g == generation
        {
            let sets = sets.clone();
            return then(self, Some(sets), window, cx);
        }
        self.resx_host_call::<host::ResxSets>(
            host::ResxSetsParams {
                generation,
                projects: None,
            },
            window,
            cx,
            move |shell, result, window, cx| {
                let sets = match result {
                    Ok(r) => {
                        shell.resx.sets = Some((r.generation, r.sets.clone()));
                        Some(r.sets)
                    }
                    Err(RequestError::Stale) if tries > 0 => {
                        let timer = cx.background_executor().timer(Duration::from_millis(300));
                        cx.spawn_in(window, async move |this, cx| {
                            timer.await;
                            let _ = this.update_in(cx, |shell, window, cx| {
                                shell.with_host_sets_tries(tries - 1, window, cx, then)
                            });
                        })
                        .detach();
                        return;
                    }
                    Err(e) => {
                        eprintln!("eludite: eludite/resx/sets: {}", describe(e));
                        None
                    }
                };
                then(shell, sets, window, cx);
            },
        );
    }

    /// What the host lists for `neutral`, from the cache.
    fn resx_info(&self, neutral: &Path) -> Option<host::ResxSet> {
        self.resx.sets.as_ref().and_then(|(_, sets)| {
            sets.iter()
                .find(|s| same_file(Path::new(&s.path), neutral))
                .cloned()
        })
    }

    /// The open editor of the set `path` belongs to.
    pub fn resx_editor(&self, path: &Path) -> Option<Entity<ResxEditor>> {
        let neutral = neutral_of(path)?;
        self.resx
            .editors
            .get(&tab_id(&normalize_path(&neutral)))
            .cloned()
    }

    #[allow(dead_code)]
    pub fn resx_timings(&self) -> &ResxTimings {
        &self.resx.timings
    }

    /// Whether `resx.openAsText` is on (tests).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn resx_open_as_text(&self) -> bool {
        self.resx.open_as_text
    }

    /// Open (or activate) the editor of the set `path` belongs to.
    pub(super) fn open_resx(
        &mut self,
        path: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let Some(neutral) = neutral_of(path) else {
            return Err(CommandError::InvalidInput(format!(
                "{} is not a .resx file",
                path.display()
            )));
        };
        let neutral = normalize_path(&neutral);
        if !neutral.is_file() && !path.is_file() {
            return Err(CommandError::InvalidInput(format!(
                "{} is not a file",
                path.display()
            )));
        }
        self.resx.timings = ResxTimings {
            opened: Some(Instant::now()),
            shown: None,
        };
        let tab = tab_id(&neutral);
        let title = neutral
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Resources.resx".into());
        let already_open = self.resx.editors.contains_key(&tab);
        if !already_open {
            let theme = self.theme;
            let probe = self.ui_bounds.clone();
            let (t, ti) = (tab.clone(), title.clone());
            let view = cx.new(|cx| {
                let mut v = ResxEditor::new(theme, t, ti, cx);
                v.probe = probe;
                v
            });
            cx.subscribe_in(&view, window, Self::on_resx_event).detach();
            cx.observe(&view, |_, _, cx| cx.notify()).detach();
            self.resx
                .documents
                .borrow_mut()
                .insert(tab.clone(), view.clone().into());
            self.resx.editors.insert(tab.clone(), view);
            self.controller.open_document(&tab, &title);
            self.load_resx(&tab, &neutral, window, cx);
        }
        // Opening a culture file selects its column once the set is loaded (the tab stays the set's).
        let _ = self
            .controller
            .apply(eludite_commands::view::ViewRequest::Show { id: tab.clone() });
        if let Some(view) = self.resx.editors.get(&tab) {
            view.read(cx).focus_handle(cx).focus(window, cx);
        }
        cx.notify();
        Ok(WorkspaceOutput::FileOpen(FileOpenOutput {
            path: tab,
            already_open,
        }))
    }

    /// Read and parse the set off the UI thread, then ask the host what it knows of it.
    fn load_resx(
        &mut self,
        tab: &str,
        neutral: &Path,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let rules = self.resx.rules;
        let path = neutral.to_path_buf();
        let tab = tab.to_owned();
        let task = cx.background_spawn(async move { SetModel::load(&path, rules) });
        let info_path = neutral.to_path_buf();
        cx.spawn_in(window, async move |this, cx| {
            let loaded = task.await;
            let _ = this.update_in(cx, |shell, window, cx| {
                let Some(view) = shell.resx.editors.get(&tab).cloned() else {
                    return;
                };
                view.update(cx, |v, cx| v.set_model(loaded, cx));
                shell.resx.timings.shown = Some(Instant::now());
                let tab = tab.clone();
                shell.with_host_sets(window, cx, move |shell, _, _, cx| {
                    let Some(info) = shell.resx_info(&info_path) else {
                        return;
                    };
                    if let Some(view) = shell.resx.editors.get(&tab) {
                        view.update(cx, |v, cx| v.set_info(Some(info), cx));
                    }
                });
            });
        })
        .detach();
    }

    /// Reload an open editor's set from disk (after an agent's write), unless it has unsaved changes.
    fn reload_resx(&mut self, neutral: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let tab = tab_id(&normalize_path(neutral));
        let Some(view) = self.resx.editors.get(&tab).cloned() else {
            return;
        };
        if view.read(cx).is_dirty() {
            return;
        }
        self.load_resx(&tab, neutral, window, cx);
    }

    fn on_resx_event(
        &mut self,
        view: &Entity<ResxEditor>,
        event: &EditorEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let tab = view.read(cx).tab.clone();
        match event {
            EditorEvent::Dirty(d) => self.controller.set_document_dirty(&tab, *d),
            EditorEvent::Save => self.run(
                eludite_commands::workspace::EDITOR_SAVE,
                json!({ "path": tab }),
                window,
                cx,
            ),
            EditorEvent::Modifier(m) => {
                if let Some(neutral) = neutral_of_tab(&tab) {
                    self.run(
                        cmd::ACCESS_MODIFIER,
                        json!({ "set": neutral, "modifier": m.as_str() }),
                        window,
                        cx,
                    );
                }
            }
        }
    }

    /// The resx tab `path` names (a tab id, or any file of an open set), or the active document when it is one.
    pub(super) fn resx_tab(&self, path: Option<&str>) -> Option<String> {
        let id = match path {
            Some(p) => p.to_owned(),
            None => self.controller.active_document()?,
        };
        if self.resx.editors.contains_key(&id) {
            return Some(id);
        }
        let p = Path::new(&id);
        if is_resx(p) {
            let tab = tab_id(&normalize_path(&neutral_of(&self.resolve_file(&id))?));
            if self.resx.editors.contains_key(&tab) {
                return Some(tab);
            }
        }
        None
    }

    /// Write `writes` through the applier as whole-file edits; `done` gets the summary.
    fn resx_apply_writes(
        &mut self,
        writes: Vec<FileWrite>,
        label: String,
        capture: Option<Caller>,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: WritesDone,
    ) {
        let mut changes = Vec::new();
        for w in &writes {
            let uri = path_to_uri(&w.path);
            if w.created {
                changes.push(lsp::DocumentChange::Operation(
                    lsp::ResourceOperation::Create {
                        uri: uri.clone(),
                        options: Some(lsp::CreateFileOptions {
                            overwrite: None,
                            ignore_if_exists: Some(true),
                        }),
                        annotation_id: None,
                    },
                ));
            }
            let last_line = w
                .previous
                .as_ref()
                .map_or(0, |t| t.bytes().filter(|b| *b == b'\n').count())
                as u32;
            changes.push(lsp::DocumentChange::Edit(lsp::TextDocumentEdit {
                text_document: lsp::OptionalVersionedTextDocumentIdentifier { uri, version: None },
                edits: vec![lsp::TextEdit {
                    range: lsp::Range {
                        start: lsp::Position {
                            line: 0,
                            character: 0,
                        },
                        end: lsp::Position {
                            line: last_line,
                            character: u32::MAX,
                        },
                    },
                    new_text: w.text.clone(),
                    extra: Map::new(),
                }],
            }));
        }
        let edit = lsp::WorkspaceEdit {
            document_changes: Some(changes),
            ..Default::default()
        };
        let options = ApplyOptions {
            label: Some(label),
            ..Default::default()
        };
        self.capture_next = capture;
        self.apply_workspace_edit(
            &edit,
            options,
            window,
            cx,
            Box::new(move |shell, summary, window, cx| done(shell, summary, writes, window, cx)),
        );
    }

    /// Regenerate the designer of `neutral` through the host when it lists one.
    fn regenerate_designer(&mut self, neutral: &Path, window: &mut Window, cx: &mut Context<Self>) {
        let path = neutral.to_path_buf();
        self.with_host_sets(window, cx, move |shell, _, window, cx| {
            let Some(info) = shell.resx_info(&path) else {
                return;
            };
            if info.designer.is_none() && info.generator.is_none() {
                return;
            }
            shell.regenerate_listed_designer(info, path, window, cx);
        });
    }

    fn regenerate_listed_designer(
        &mut self,
        info: host::ResxSet,
        path: PathBuf,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.resx_host_call::<host::ResxDesigner>(
            host::ResxDesignerParams {
                generation: self.generation,
                path: info.path.clone(),
                action: host::ResxDesignerAction::Generate,
                modifier: None,
            },
            window,
            cx,
            move |shell, result, _, cx| {
                let message = match result {
                    Ok(r) => match r.status {
                        host::ResxDesignerStatus::Written => "Designer regenerated.".to_owned(),
                        host::ResxDesignerStatus::Unchanged => "Designer unchanged.".to_owned(),
                        _ => return,
                    },
                    Err(e) => format!("Designer not regenerated: {}", describe(e)),
                };
                if let Some(view) = shell.resx_editor(&path) {
                    view.update(cx, |v, cx| v.set_message(message, cx));
                }
            },
        );
    }

    /// `eludite.editor.save` on a resx tab: every dirty file through the applier.
    pub(super) fn save_resx(
        &mut self,
        tab: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let view = self
            .resx
            .editors
            .get(tab)
            .cloned()
            .ok_or_else(|| failed(format!("{tab} is not open")))?;
        let neutral = PathBuf::from(neutral_of_tab(tab).unwrap_or_default());
        let (writes, keys_changed) = {
            let v = view.read(cx);
            match v.model.as_ref() {
                Some(m) => (m.writes(self.resx.sort_on_save), m.keys_changed),
                None => (Vec::new(), false),
            }
        };
        let bytes = std::fs::metadata(&neutral).map(|m| m.len()).unwrap_or(0);
        let path = neutral.display().to_string();
        if writes.is_empty() {
            return Ok(WorkspaceOutput::Save(SaveOutput {
                path,
                bytes,
                pending: false,
            }));
        }
        let title = view.read(cx).title.clone();
        let label = format!("Save {title}");
        let regenerate = neutral.clone();
        self.resx_apply_writes(
            writes,
            label,
            None,
            window,
            cx,
            Box::new(move |shell, summary, writes, window, cx| {
                if summary.applied {
                    let n = writes.len();
                    let message = format!("Saved {n} file{}.", if n == 1 { "" } else { "s" });
                    view.update(cx, |v, cx| v.saved(&writes, message, cx));
                    if keys_changed {
                        shell.regenerate_designer(&regenerate, window, cx);
                    }
                } else {
                    let why = summary.message.unwrap_or_else(|| "refused".into());
                    view.update(cx, |v, cx| v.set_message(format!("Not saved: {why}"), cx));
                }
            }),
        );
        Ok(WorkspaceOutput::Save(SaveOutput {
            path,
            bytes,
            pending: true,
        }))
    }

    /// `eludite.file.close` on a resx tab.
    pub(super) fn close_resx(
        &mut self,
        tab: &str,
        save: Option<CloseSave>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let dirty = self
            .resx
            .editors
            .get(tab)
            .is_some_and(|v| v.read(cx).is_dirty());
        let mut saved = false;
        if dirty {
            match save {
                None => {
                    return Err(failed(format!(
                        "{tab} has unsaved changes; pass \"save\": \"save\" or \"discard\""
                    )));
                }
                Some(CloseSave::Save) => {
                    self.save_resx(tab, window, cx)?;
                    saved = true;
                }
                Some(CloseSave::Discard) => {}
            }
        }
        self.resx.editors.remove(tab);
        self.resx.documents.borrow_mut().remove(tab);
        self.controller.close_document(tab);
        cx.notify();
        Ok(WorkspaceOutput::FileClose(FileCloseOutput {
            path: tab.to_owned(),
            closed: true,
            saved,
        }))
    }

    /// Visual Studio's question before closing a dirty resx tab. Returns true when it handled `command`.
    pub(super) fn run_resx(
        &mut self,
        command: &str,
        args: &mut Value,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        if command == eludite_commands::workspace::FILE_CLOSE
            && args.get("save").is_none()
            && let Some(path) = args.get("path").and_then(Value::as_str)
            && let Some(tab) = self.resx_tab(Some(path))
            && let Some(view) = self.resx.editors.get(&tab).cloned()
            && view.read(cx).is_dirty()
        {
            let answer = window.prompt(
                PromptLevel::Warning,
                &format!("Save changes to {}?", view.read(cx).title),
                Some("The resource set has unsaved changes."),
                &["Save", "Don't Save", "Cancel"],
                cx,
            );
            cx.spawn_in(window, async move |this, cx| {
                let save = match answer.await {
                    Ok(0) => "save",
                    Ok(1) => "discard",
                    _ => return,
                };
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.run(
                        eludite_commands::workspace::FILE_CLOSE,
                        json!({ "path": tab, "save": save }),
                        window,
                        cx,
                    )
                });
            })
            .detach();
            return true;
        }
        false
    }

    /// Apply a resx command. The answer is immediate for a UI-thread call (staged; a write is `pending` while the
    /// applier runs); with `waiter` (another thread's call) the answer goes there once the files are written.
    pub(super) fn apply_resx(
        &mut self,
        request: ResxRequest,
        caller: Caller,
        waiter: Reply,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Outcome {
        match &request {
            ResxRequest::Sets {
                project,
                path,
                include_non_string,
            } => {
                let Some(root) = self.resx_root() else {
                    return answer(&waiter, Err(failed("no workspace is open")));
                };
                let (excludes, gitignore) = self.search_filters();
                let rules = self.resx.rules;
                let filter = SetsFilter {
                    project: project.clone(),
                    path: path.as_ref().map(|p| self.resolve_file(p)),
                    include_non_string: *include_non_string,
                };
                if waiter.is_none() {
                    // The UI has no caller of this; a test's direct call computes it here.
                    let host_sets = self
                        .resx
                        .sets
                        .as_ref()
                        .map(|(_, s)| s.clone())
                        .unwrap_or_default();
                    let sets =
                        compute_sets(&root, &excludes, gitignore, rules, &host_sets, &filter);
                    return Ok(ResxOutput::Sets(SetsOutput {
                        sets,
                        generation: self.resx.sets.as_ref().map(|(g, _)| *g),
                    }));
                }
                self.with_host_sets(window, cx, move |shell, host_sets, window, cx| {
                    let generation = host_sets.as_ref().map(|_| shell.generation);
                    let host_sets = host_sets.unwrap_or_default();
                    let task = cx.background_spawn(async move {
                        compute_sets(&root, &excludes, gitignore, rules, &host_sets, &filter)
                    });
                    cx.spawn_in(window, async move |_, _| {
                        let sets = task.await;
                        let _ = answer(
                            &waiter,
                            Ok(ResxOutput::Sets(SetsOutput { sets, generation })),
                        );
                    })
                    .detach();
                });
                Err(failed("eludite.resx.sets answers off the UI thread"))
            }
            ResxRequest::Entries { .. } | ResxRequest::Validate { .. } => {
                let set = self.resolve_file(request_set(&request).unwrap_or_default());
                let Some(neutral) = neutral_of(&set) else {
                    return answer(
                        &waiter,
                        Err(CommandError::InvalidInput(format!(
                            "{} is not a .resx file",
                            set.display()
                        ))),
                    );
                };
                let rules = self.resx.rules;
                let info = self.resx_info(&neutral);
                let read = move |model: &SetModel| -> ResxOutput {
                    match &request {
                        ResxRequest::Entries {
                            query,
                            missing,
                            warnings,
                            invariant,
                            cultures,
                            skip,
                            take,
                            ..
                        } => ResxOutput::Entries(Box::new(entries_output(
                            model,
                            info.as_ref(),
                            query.as_deref(),
                            *missing,
                            *warnings,
                            *invariant,
                            cultures.as_deref(),
                            *skip,
                            *take,
                        ))),
                        ResxRequest::Validate { cultures, .. } => {
                            ResxOutput::Validate(validate_output(model, cultures.as_deref()))
                        }
                        _ => unreachable!(),
                    }
                };
                if let Some(view) = self.resx_editor(&neutral)
                    && let Some(m) = view.read(cx).model.as_ref()
                {
                    return answer(&waiter, Ok(read(m)));
                }
                if waiter.is_none() {
                    return match SetModel::load(&neutral, rules) {
                        Ok(m) => Ok(read(&m)),
                        Err(e) => Err(failed(e)),
                    };
                }
                let task = cx.background_spawn(async move { SetModel::load(&neutral, rules) });
                cx.spawn_in(window, async move |_, _| {
                    let outcome = match task.await {
                        Ok(m) => Ok(read(&m)),
                        Err(e) => Err(failed(e)),
                    };
                    let _ = answer(&waiter, outcome);
                })
                .detach();
                Err(failed("answers off the UI thread"))
            }
            ResxRequest::AccessModifier { modifier, .. } => {
                let set = self.resolve_file(request_set(&request).unwrap_or_default());
                let Some(neutral) = neutral_of(&set) else {
                    return answer(
                        &waiter,
                        Err(CommandError::InvalidInput(format!(
                            "{} is not a .resx file",
                            set.display()
                        ))),
                    );
                };
                let modifier = *modifier;
                let set = neutral.display().to_string();
                if self.solution.is_none() {
                    return answer(
                        &waiter,
                        Ok(ResxOutput::AccessModifier(AccessModifierOutput {
                            set,
                            modifier,
                            designer: None,
                            status: ModifierStatus::Error,
                            message: Some(
                                "the Access Modifier needs a .NET workspace with the host".into(),
                            ),
                        })),
                    );
                }
                let neutral_for = neutral.clone();
                self.resx_host_call::<host::ResxDesigner>(
                    host::ResxDesignerParams {
                        generation: self.generation,
                        path: set.clone(),
                        action: host::ResxDesignerAction::SetModifier,
                        modifier: Some(host_modifier(modifier)),
                    },
                    window,
                    cx,
                    move |shell, result, window, cx| {
                        let out = match result {
                            Ok(r) => {
                                // The project changed: the host's list is stale.
                                shell.resx.sets = None;
                                let status = match r.status {
                                    host::ResxDesignerStatus::Written
                                    | host::ResxDesignerStatus::Deleted => ModifierStatus::Written,
                                    host::ResxDesignerStatus::Unchanged => {
                                        ModifierStatus::Unchanged
                                    }
                                    host::ResxDesignerStatus::None => ModifierStatus::Error,
                                };
                                let message = (r.status == host::ResxDesignerStatus::None)
                                    .then(|| "no project lists this file".to_owned());
                                let designer = r.designer.clone();
                                let tab = tab_id(&normalize_path(&neutral_for));
                                if let Some(view) = shell.resx.editors.get(&tab).cloned() {
                                    view.update(cx, |v, cx| {
                                        if let Some(i) = v.info.as_mut() {
                                            i.access_modifier = r.modifier;
                                            i.designer = r.designer.clone();
                                        }
                                        v.set_message(
                                            format!("Access modifier: {}.", modifier.as_str()),
                                            cx,
                                        );
                                    });
                                    // The host reloaded: its list is asked for again under the new generation.
                                    shell.generation = shell.generation.max(r.generation);
                                    shell.with_host_sets(window, cx, move |shell, _, _, cx| {
                                        let Some(info) = shell.resx_info(&neutral_for) else {
                                            return;
                                        };
                                        if let Some(view) = shell.resx.editors.get(&tab) {
                                            view.update(cx, |v, cx| v.set_info(Some(info), cx));
                                        }
                                    });
                                }
                                Ok(ResxOutput::AccessModifier(AccessModifierOutput {
                                    set: set.clone(),
                                    modifier,
                                    designer,
                                    status,
                                    message,
                                }))
                            }
                            Err(e) => Ok(ResxOutput::AccessModifier(AccessModifierOutput {
                                set: set.clone(),
                                modifier,
                                designer: None,
                                status: ModifierStatus::Error,
                                message: Some(describe(e)),
                            })),
                        };
                        let _ = answer(&waiter, out);
                    },
                );
                let mut out = ResxOutput::AccessModifier(AccessModifierOutput {
                    set: neutral.display().to_string(),
                    modifier,
                    designer: None,
                    status: ModifierStatus::Written,
                    message: None,
                });
                mark_pending(&mut out, None);
                Ok(out)
            }
            ResxRequest::Set { .. }
            | ResxRequest::Add { .. }
            | ResxRequest::Remove { .. }
            | ResxRequest::Rename { .. } => {
                let set = self.resolve_file(request_set(&request).unwrap_or_default());
                let Some(neutral) = neutral_of(&set) else {
                    return answer(
                        &waiter,
                        Err(CommandError::InvalidInput(format!(
                            "{} is not a .resx file",
                            set.display()
                        ))),
                    );
                };
                if let ResxRequest::Set { cells, .. } = &request
                    && cells
                        .iter()
                        .any(|c| !same_file(&self.resolve_file(&c.set), &set))
                {
                    return answer(
                        &waiter,
                        Err(CommandError::InvalidInput(
                            "every cell must name the same set".into(),
                        )),
                    );
                }
                let create_culture = matches!(
                    &request,
                    ResxRequest::Set {
                        create_culture: true,
                        ..
                    }
                );
                let neutral = normalize_path(&neutral);
                let open = self.resx.editors.get(&tab_id(&neutral)).cloned();
                let command = request.command();
                // The person's write lands in the open editor; the grid shows it and Save writes it with the rest.
                if let Some(view) = open.clone()
                    && !caller.is_agent()
                    && view.read(cx).model.is_some()
                {
                    let outcome = view.update(cx, |v, cx| {
                        let out =
                            mutate(v.model.as_mut().expect("checked"), &request, create_culture);
                        v.set_model_changed(cx);
                        out
                    });
                    let outcome = match outcome {
                        Ok(mut out) => {
                            let tab = tab_id(&neutral);
                            let _ = self.save_resx(&tab, window, cx);
                            mark_pending(&mut out, None);
                            Ok(out)
                        }
                        Err(e) => Err(e),
                    };
                    return answer(&waiter, outcome);
                }
                if let Some(view) = open
                    && view.read(cx).is_dirty()
                {
                    return answer(
                        &waiter,
                        Err(failed(format!(
                            "{} is open in the editor with unsaved changes; save or discard them first",
                            neutral.display()
                        ))),
                    );
                }
                let rules = self.resx.rules;
                let sort = self.resx.sort_on_save;
                let capture = (caller.is_agent() && self.agent_edits_reviewed()).then_some(caller);
                let label = format!(
                    "{command} {}",
                    neutral
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default()
                );
                let load_path = neutral.clone();
                let task = cx.background_spawn(async move { SetModel::load(&load_path, rules) });
                let immediate = waiter.is_none();
                let staged: Outcome = if immediate {
                    // The UI thread never waits: the write runs after this answer.
                    match SetModel::load(&neutral, rules) {
                        Ok(mut m) => mutate(&mut m, &request, create_culture).map(|mut o| {
                            mark_pending(&mut o, None);
                            o
                        }),
                        Err(e) => Err(failed(e)),
                    }
                } else {
                    Err(failed("answers off the UI thread"))
                };
                cx.spawn_in(window, async move |this, cx| {
                    let loaded = task.await;
                    let _ = this.update_in(cx, |shell, window, cx| {
                        let mut model = match loaded {
                            Ok(m) => m,
                            Err(e) => {
                                let _ = answer(&waiter, Err(failed(e)));
                                return;
                            }
                        };
                        let outcome = match mutate(&mut model, &request, create_culture) {
                            Ok(o) => o,
                            Err(e) => {
                                let _ = answer(&waiter, Err(e));
                                return;
                            }
                        };
                        let writes = model.writes(sort);
                        if writes.is_empty() {
                            let _ = answer(&waiter, Ok(outcome));
                            return;
                        }
                        let keys_changed = model.keys_changed;
                        let neutral = neutral.clone();
                        shell.resx_apply_writes(
                            writes,
                            label,
                            capture,
                            window,
                            cx,
                            Box::new(move |shell, summary, _, window, cx| {
                                let out = if summary.applied {
                                    if keys_changed {
                                        shell.regenerate_designer(&neutral, window, cx);
                                    }
                                    shell.reload_resx(&neutral, window, cx);
                                    Ok(outcome)
                                } else if summary
                                    .message
                                    .as_deref()
                                    .is_some_and(|m| m.starts_with("held for review"))
                                {
                                    let mut out = outcome;
                                    mark_pending(&mut out, summary.message.clone());
                                    Ok(out)
                                } else {
                                    Err(failed(
                                        summary
                                            .message
                                            .unwrap_or_else(|| "the edit was refused".into()),
                                    ))
                                };
                                let _ = answer(&waiter, out);
                            }),
                        );
                    });
                })
                .detach();
                staged
            }
        }
    }
}
