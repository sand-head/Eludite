//! The workspace-edit applier (brief 0015): one place that applies an LSP `WorkspaceEdit` for rename, code actions,
//! completion's additional edits, the language server's `workspace/applyEdit` and `eludite.workspace.apply_edit`.
//!
//! - **Open documents** change in their buffers, one undo step per document ([`eludite_editor::Editor::apply_edits`]),
//!   and the language server gets `didChange` at once. Ranges are read in the text the server last saw (the
//!   document's sent snapshot) and anchored there, so typing that has not been sent yet does not shift them.
//! - **Closed files** are read, edited and written off the UI thread, atomically: every new content goes to a
//!   temporary file beside its target first, and only when all of them are written are they renamed into place. The
//!   language server then gets `workspace/didChangeWatchedFiles`. Byte order marks and line endings are kept
//!   ([`Buffer::to_file_bytes`]).
//! - **Resource operations** (create, rename, delete) run in order with the text edits, on a model of the affected
//!   files, so an edit to a file created or renamed earlier in the same `WorkspaceEdit` lands in the right place. An
//!   open document a resource operation touches must have no unsaved changes; its tab follows a rename and closes on
//!   a delete.
//! - **Refusal is all or nothing.** A document version that is not current (a `TextDocumentEdit` version, or the
//!   version a rename or code action was computed for), an edit computed under an older solution generation, a range
//!   past the end of its document, overlapping edits, a read-only document or a failed resource operation refuse the
//!   whole edit before anything is changed.
//!
//! Documents are matched by normalized path or URI, never by a raw path string.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::io::Write as _;
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use eludite_editor::text::{self, Anchor, PointUtf16, TransactionId, Unclipped};
use eludite_editor::{Buffer, LineEnding};
use eludite_lsp::lsp;
use gpui::{AppContext as _, Context, Window};
use serde_json::json;

use super::Shell;
use super::documents::{normalize_path, path_to_uri, trace, uri_to_path};

/// One change of a `WorkspaceEdit`, in order, with its URIs resolved to normalized paths.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    Text {
        path: PathBuf,
        version: Option<i32>,
        edits: Vec<lsp::TextEdit>,
    },
    Create {
        path: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
        overwrite: bool,
        ignore_if_exists: bool,
    },
    Delete {
        path: PathBuf,
        recursive: bool,
        ignore_if_not_exists: bool,
    },
}

impl Step {
    fn paths(&self) -> Vec<&Path> {
        match self {
            Step::Text { path, .. } | Step::Create { path, .. } | Step::Delete { path, .. } => {
                vec![path]
            }
            Step::Rename { from, to, .. } => vec![from, to],
        }
    }
}

fn path_of(uri: &str) -> Result<PathBuf, String> {
    uri_to_path(uri)
        .map(|p| normalize_path(&p))
        .ok_or_else(|| format!("{uri} is not a file URI"))
}

/// The steps of `edit`, in order: `documentChanges` when present (the LSP rule), else `changes`.
pub fn plan(edit: &lsp::WorkspaceEdit) -> Result<Vec<Step>, String> {
    let mut steps = Vec::new();
    if let Some(changes) = &edit.document_changes {
        for change in changes {
            steps.push(match change {
                lsp::DocumentChange::Edit(e) => Step::Text {
                    path: path_of(&e.text_document.uri)?,
                    version: e.text_document.version,
                    edits: e.edits.clone(),
                },
                lsp::DocumentChange::Operation(lsp::ResourceOperation::Create {
                    uri,
                    options,
                    ..
                }) => {
                    let o = options.clone().unwrap_or_default();
                    Step::Create {
                        path: path_of(uri)?,
                        overwrite: o.overwrite.unwrap_or(false),
                        ignore_if_exists: o.ignore_if_exists.unwrap_or(false),
                    }
                }
                lsp::DocumentChange::Operation(lsp::ResourceOperation::Rename {
                    old_uri,
                    new_uri,
                    options,
                    ..
                }) => {
                    let o = options.clone().unwrap_or_default();
                    Step::Rename {
                        from: path_of(old_uri)?,
                        to: path_of(new_uri)?,
                        overwrite: o.overwrite.unwrap_or(false),
                        ignore_if_exists: o.ignore_if_exists.unwrap_or(false),
                    }
                }
                lsp::DocumentChange::Operation(lsp::ResourceOperation::Delete {
                    uri,
                    options,
                    ..
                }) => {
                    let o = options.clone().unwrap_or_default();
                    Step::Delete {
                        path: path_of(uri)?,
                        recursive: o.recursive.unwrap_or(false),
                        ignore_if_not_exists: o.ignore_if_not_exists.unwrap_or(false),
                    }
                }
            });
        }
    } else if let Some(changes) = &edit.changes {
        for (uri, edits) in changes {
            steps.push(Step::Text {
                path: path_of(uri)?,
                version: None,
                edits: edits.clone(),
            });
        }
    }
    Ok(steps)
}

/// The byte ranges and texts of `edits` in `snapshot`, sorted (inserts at one position keep their order). A
/// position past the last line, an end before its start, or overlapping edits are an error: the edits were not
/// computed for this text. A character past the end of its line means the line's end (LSP).
pub fn resolve_edits(
    snapshot: &text::BufferSnapshot,
    edits: &[lsp::TextEdit],
) -> Result<Vec<(Range<usize>, String)>, String> {
    let last_row = snapshot.max_point_utf16().row;
    let offset = |p: lsp::Position| -> Result<usize, String> {
        if p.line > last_row {
            return Err(format!(
                "line {} is past the end of the document ({} lines)",
                p.line + 1,
                last_row + 1
            ));
        }
        let point = snapshot.clip_point_utf16(
            Unclipped(PointUtf16::new(p.line, p.character)),
            text::Bias::Left,
        );
        Ok(snapshot.point_utf16_to_offset(point))
    };
    let mut out = Vec::with_capacity(edits.len());
    for e in edits {
        let start = offset(e.range.start)?;
        let end = offset(e.range.end)?;
        if end < start {
            return Err("an edit ends before it starts".into());
        }
        out.push((start..end, e.new_text.clone()));
    }
    out.sort_by_key(|(r, _)| r.start);
    if out.windows(2).any(|w| w[0].0.end > w[1].0.start) {
        return Err("the edits overlap".into());
    }
    Ok(out)
}

/// `ranges` anchored in `snapshot` so they survive later edits: text typed at either end stays outside the range.
fn anchor_ranges(
    snapshot: &text::BufferSnapshot,
    ranges: Vec<(Range<usize>, String)>,
) -> Vec<(Range<Anchor>, String)> {
    ranges
        .into_iter()
        .map(|(r, t)| {
            (
                snapshot.anchor_after(r.start)..snapshot.anchor_before(r.end),
                t,
            )
        })
        .collect()
}

/// What the closed-file phase did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiskOutcome {
    /// Files written: path, and whether it existed before.
    pub written: Vec<(PathBuf, bool)>,
    pub deleted: Vec<PathBuf>,
    pub edits: usize,
    pub created: usize,
    pub renamed: usize,
    pub deleted_count: usize,
    /// Renames, for open tabs to follow.
    pub moves: Vec<(PathBuf, PathBuf)>,
}

enum Entry {
    File { buffer: Box<Buffer>, dirty: bool },
    Deleted { dir: bool, recursive: bool },
}

/// The files `steps` touch, as they will be: a model over the disk.
struct Model {
    entries: BTreeMap<PathBuf, Entry>,
    /// Files this apply creates: their line ending follows the text written, not the platform.
    created: BTreeSet<PathBuf>,
}

impl Model {
    fn exists(&self, path: &Path) -> bool {
        match self.entries.get(path) {
            Some(Entry::File { .. }) => true,
            Some(Entry::Deleted { .. }) => false,
            None => path.exists(),
        }
    }

    fn load(&mut self, path: &Path) -> Result<&mut Buffer, String> {
        if !self.entries.contains_key(path) {
            if path.is_dir() {
                return Err(format!("{} is a folder", path.display()));
            }
            let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
            let buffer =
                Buffer::from_bytes(&bytes).map_err(|e| format!("{}: {e}", path.display()))?;
            self.entries.insert(
                path.to_path_buf(),
                Entry::File {
                    buffer: Box::new(buffer),
                    dirty: false,
                },
            );
        }
        match self.entries.get_mut(path) {
            Some(Entry::File { buffer, .. }) => Ok(buffer),
            _ => Err(format!("{} does not exist", path.display())),
        }
    }

    fn put(&mut self, path: &Path, buffer: Box<Buffer>) {
        self.entries.insert(
            path.to_path_buf(),
            Entry::File {
                buffer,
                dirty: true,
            },
        );
    }
}

/// The caller's expected version for a document, with the key normalized like document ids are,
/// so an agent may pass the path in any separator form.
fn version_option(versions: &HashMap<String, i32>, id: &str) -> Option<i32> {
    versions
        .iter()
        .find(|(k, _)| normalize_path(Path::new(k)).to_string_lossy() == id)
        .map(|(_, v)| *v)
}

/// Apply `steps` (closed files and resource operations) to a model of the files, then write the result
/// atomically. Nothing on disk changes unless every step succeeds and every new content was written to its temporary
/// file. Runs off the UI thread.
pub fn apply_on_disk(steps: &[Step]) -> Result<DiskOutcome, String> {
    let mut model = Model {
        entries: BTreeMap::new(),
        created: BTreeSet::new(),
    };
    let mut out = DiskOutcome::default();
    for step in steps {
        match step {
            Step::Text { path, edits, .. } => {
                let is_created = model.created.contains(path);
                let buffer = model.load(path)?;
                let ranges = resolve_edits(buffer.snapshot(), edits)
                    .map_err(|e| format!("{}: {e}", path.display()))?;
                out.edits += ranges.len();
                if is_created {
                    // A new file takes the ending of the text the server wrote, not the
                    // platform's: an LF edit must not become a CRLF file on Windows.
                    let crlf = edits.iter().any(|e| e.new_text.contains("\r\n"));
                    buffer.set_line_ending(if crlf {
                        LineEnding::CrLf
                    } else {
                        LineEnding::Lf
                    });
                }
                buffer.edit(ranges);
                if let Some(Entry::File { dirty, .. }) = model.entries.get_mut(path) {
                    *dirty = true;
                }
            }
            Step::Create {
                path,
                overwrite,
                ignore_if_exists,
            } => {
                if model.exists(path) && !overwrite {
                    if *ignore_if_exists {
                        continue;
                    }
                    return Err(format!("{} already exists", path.display()));
                }
                model.put(path, Box::new(Buffer::new("")));
                model.created.insert(path.clone());
                out.created += 1;
            }
            Step::Rename {
                from,
                to,
                overwrite,
                ignore_if_exists,
            } => {
                if !model.exists(from) {
                    return Err(format!("{} does not exist", from.display()));
                }
                if model.exists(to) && !overwrite {
                    if *ignore_if_exists {
                        continue;
                    }
                    return Err(format!("{} already exists", to.display()));
                }
                if !model.entries.contains_key(from) && from.is_dir() {
                    return Err(format!(
                        "renaming the folder {} is not supported",
                        from.display()
                    ));
                }
                model.load(from)?;
                let Some(Entry::File { buffer, .. }) = model.entries.insert(
                    from.clone(),
                    Entry::Deleted {
                        dir: false,
                        recursive: false,
                    },
                ) else {
                    unreachable!("loaded above")
                };
                model.put(to, buffer);
                out.renamed += 1;
                out.moves.push((from.clone(), to.clone()));
            }
            Step::Delete {
                path,
                recursive,
                ignore_if_not_exists,
            } => {
                if !model.exists(path) {
                    if *ignore_if_not_exists {
                        continue;
                    }
                    return Err(format!("{} does not exist", path.display()));
                }
                let dir = !model.entries.contains_key(path) && path.is_dir();
                if dir
                    && !recursive
                    && std::fs::read_dir(path).is_ok_and(|mut d| d.next().is_some())
                {
                    return Err(format!("{} is a folder that is not empty", path.display()));
                }
                model.entries.insert(
                    path.clone(),
                    Entry::Deleted {
                        dir,
                        recursive: *recursive,
                    },
                );
                out.deleted_count += 1;
            }
        }
    }
    // Write every new content to a temporary file; only then move them into place.
    let mut temps: Vec<(PathBuf, PathBuf, bool)> = Vec::new();
    let cleanup = |temps: &[(PathBuf, PathBuf, bool)]| {
        for (tmp, _, _) in temps {
            let _ = std::fs::remove_file(tmp);
        }
    };
    for (path, entry) in &model.entries {
        let Entry::File {
            buffer,
            dirty: true,
        } = entry
        else {
            continue;
        };
        let existed = path.is_file();
        match write_temp(path, &buffer.to_file_bytes(), existed) {
            Ok(tmp) => temps.push((tmp, path.clone(), existed)),
            Err(e) => {
                cleanup(&temps);
                return Err(format!("{}: {e}", path.display()));
            }
        }
    }
    for (i, (tmp, path, existed)) in temps.iter().enumerate() {
        if let Err(e) = std::fs::rename(tmp, path) {
            cleanup(&temps[i..]);
            return Err(format!(
                "{}: {e} ({} of {} files were written)",
                path.display(),
                i,
                temps.len()
            ));
        }
        out.written.push((path.clone(), *existed));
    }
    for (path, entry) in &model.entries {
        let Entry::Deleted { dir, recursive } = entry else {
            continue;
        };
        let result = match (dir, recursive) {
            (true, true) => std::fs::remove_dir_all(path),
            (true, false) => std::fs::remove_dir(path),
            (false, _) if path.exists() => std::fs::remove_file(path),
            (false, _) => Ok(()),
        };
        result.map_err(|e| format!("{}: {e}", path.display()))?;
        out.deleted.push(path.clone());
    }
    Ok(out)
}

/// Write `bytes` to a new temporary file beside `path` (same folder, so the rename is atomic), with `path`'s
/// permissions when it exists.
fn write_temp(path: &Path, bytes: &[u8], existed: bool) -> std::io::Result<PathBuf> {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir)?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let tmp = dir.join(format!(
        ".{name}.eludite-{}-{}.tmp",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    let write = || -> std::io::Result<()> {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
        if existed {
            std::fs::set_permissions(&tmp, std::fs::metadata(path)?.permissions())?;
        }
        Ok(())
    };
    match write() {
        Ok(()) => Ok(tmp),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            Err(e)
        }
    }
}

/// How an edit may be applied.
#[derive(Debug, Clone, Default)]
pub struct ApplyOptions {
    /// Shown in the status bar.
    pub label: Option<String>,
    /// The solution generation the edit was computed under; another one refuses it.
    pub generation: Option<u64>,
    /// Document id to the LSP version the edit was computed for (a rename or code action request); another version
    /// refuses it, like a `TextDocumentEdit` version.
    pub versions: HashMap<String, i32>,
}

/// What the applier did (`eludite.workspace.apply_edit`'s output without its state).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ApplySummary {
    pub applied: bool,
    pub files: u64,
    pub edits: u64,
    pub open_documents: u64,
    pub files_on_disk: u64,
    pub created: u64,
    pub renamed: u64,
    pub deleted: u64,
    /// Sorted.
    pub paths: Vec<String>,
    pub message: Option<String>,
}

impl ApplySummary {
    /// Held for review as pending changes `ids` (an agent's edit, brief 0016): nothing applied yet.
    pub fn held(ids: &[u64]) -> Self {
        Self::refused(format!(
            "held for review as pending change{} {}",
            if ids.len() == 1 { "" } else { "s" },
            ids.iter()
                .map(|i| format!("#{i}"))
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }

    pub fn refused(message: impl Into<String>) -> Self {
        Self {
            message: Some(message.into()),
            ..Self::default()
        }
    }

    /// The command outputs' `summary`.
    pub fn output(&self) -> eludite_commands::workspace::ApplySummaryOutput {
        eludite_commands::workspace::ApplySummaryOutput {
            applied: self.applied,
            files: self.files,
            edits: self.edits,
            open_documents: self.open_documents,
            files_on_disk: self.files_on_disk,
            created: self.created,
            renamed: self.renamed,
            deleted: self.deleted,
            paths: self
                .paths
                .iter()
                .take(eludite_commands::workspace::MAX_APPLY_PATHS)
                .cloned()
                .collect(),
            message: self.message.clone(),
        }
    }

    /// The status bar text.
    pub fn status_text(&self, label: Option<&str>) -> String {
        if !self.applied {
            return format!(
                "{} was not applied: {}",
                label.unwrap_or("The edit"),
                self.message.as_deref().unwrap_or("unknown error")
            );
        }
        format!(
            "{}{} edit{} in {} file{}",
            label.map(|l| format!("{l}: ")).unwrap_or_default(),
            self.edits,
            if self.edits == 1 { "" } else { "s" },
            self.files,
            if self.files == 1 { "" } else { "s" }
        )
    }
}

/// A document's edits, anchored in the text the server saw.
struct BufferEdit {
    id: String,
    ranges: Vec<(Range<Anchor>, String)>,
}

/// Called once the edit is applied or refused.
pub type ApplyDone = Box<dyn FnOnce(&mut Shell, ApplySummary, &mut Window, &mut Context<Shell>)>;

impl Shell {
    /// The id of the open document at `path`, matched by normalized path, then by URI.
    pub(super) fn open_document_at(&self, path: &Path) -> Option<String> {
        let id = normalize_path(path).to_string_lossy().into_owned();
        if self.documents.contains_key(&id) {
            return Some(id);
        }
        let uri = path_to_uri(path);
        self.documents
            .iter()
            .find(|(_, d)| d.uri == uri)
            .map(|(id, _)| id.clone())
    }

    /// Apply `edits` (computed on `base`, the text the server saw) to open document `id` as one undo step, and send
    /// `didChange` at once. Returns the number of edits and the undo step.
    pub(super) fn apply_document_edits(
        &mut self,
        id: &str,
        edits: &[lsp::TextEdit],
        base: &text::BufferSnapshot,
        cx: &mut Context<Self>,
    ) -> Result<(usize, Option<TransactionId>), String> {
        let ranges = resolve_edits(base, edits)?;
        let n = ranges.len();
        let anchored = anchor_ranges(base, ranges);
        let tx = self.apply_anchored(id, anchored, cx);
        Ok((n, tx))
    }

    fn apply_anchored(
        &mut self,
        id: &str,
        ranges: Vec<(Range<Anchor>, String)>,
        cx: &mut Context<Self>,
    ) -> Option<TransactionId> {
        let doc = self.documents.get(id)?;
        let view = doc.view.clone();
        let tx = view.update(cx, |v, cx| {
            v.update_editor(cx, |e| {
                let b = e.buffer();
                let mut resolved: Vec<(Range<usize>, String)> = ranges
                    .into_iter()
                    .map(|(r, t)| {
                        let start = b.offset_for_anchor(&r.start);
                        let end = b.offset_for_anchor(&r.end).max(start);
                        (start..end, t)
                    })
                    .collect();
                resolved.sort_by_key(|(r, _)| r.start);
                // Typing inside an edited range since it was computed can make two ranges meet; keep the first.
                let mut kept: Vec<(Range<usize>, String)> = Vec::with_capacity(resolved.len());
                for (r, t) in resolved {
                    if kept.last().is_some_and(|(l, _)| l.end > r.start) {
                        continue;
                    }
                    kept.push((r, t));
                }
                e.apply_edits(kept)
            })
        });
        self.flush_change(id, cx);
        tx
    }

    /// Apply `edit`. Validation (generations, versions, ranges, open documents) happens now and refuses the whole
    /// edit; closed files and resource operations are applied off the UI thread, after which open buffers change.
    /// `done` gets the summary, at once when no file on disk is involved.
    pub(super) fn apply_workspace_edit(
        &mut self,
        edit: &lsp::WorkspaceEdit,
        options: ApplyOptions,
        window: &mut Window,
        cx: &mut Context<Self>,
        done: ApplyDone,
    ) {
        let started = std::time::Instant::now();
        // An agent's edit command: hold the edit as pending changes instead (brief 0016).
        if let Some(caller) = self.capture_next.take() {
            let summary = match self.capture_edit(edit, &options, &caller, window, cx) {
                Ok(ids) => ApplySummary::held(&ids),
                Err(e) => ApplySummary::refused(e),
            };
            self.report_apply(&options, &summary, started);
            done(self, summary, window, cx);
            return;
        }
        match self.prepare_workspace_edit(edit, &options) {
            Err(message) => {
                trace(format_args!("workspace edit refused: {message}"));
                let summary = ApplySummary::refused(message);
                self.report_apply(&options, &summary, started);
                done(self, summary, window, cx);
            }
            Ok((buffers, disk, _)) if disk.is_empty() => {
                let summary = self.complete_apply(buffers, DiskOutcome::default(), cx);
                self.report_apply(&options, &summary, started);
                done(self, summary, window, cx);
            }
            Ok((buffers, disk, managed)) => {
                let task = cx.background_spawn(async move { apply_on_disk(&disk) });
                cx.spawn_in(window, async move |this, cx| {
                    let outcome = task.await;
                    let _ = this.update_in(cx, |shell, window, cx| {
                        let summary = match outcome {
                            Err(message) => ApplySummary::refused(message),
                            Ok(outcome) => {
                                shell.after_disk(&outcome, &managed, window, cx);
                                shell.complete_apply(buffers, outcome, cx)
                            }
                        };
                        shell.report_apply(&options, &summary, started);
                        done(shell, summary, window, cx);
                    });
                })
                .detach();
            }
        }
    }

    /// Validate `edit` and split it into buffer edits and disk steps. Also returns the open documents the disk steps
    /// manage (touched by a resource operation).
    #[allow(clippy::type_complexity)]
    fn prepare_workspace_edit(
        &self,
        edit: &lsp::WorkspaceEdit,
        options: &ApplyOptions,
    ) -> Result<(Vec<BufferEdit>, Vec<Step>, Vec<String>), String> {
        if let Some(g) = options.generation
            && g != self.generation
        {
            return Err(format!(
                "the edit was computed for solution generation {g}; the solution changed since (generation {})",
                self.generation
            ));
        }
        let steps = plan(edit)?;
        // Open documents a resource operation touches are handled on disk, and must be saved.
        let mut managed: BTreeSet<String> = BTreeSet::new();
        for step in &steps {
            if matches!(step, Step::Text { .. }) {
                continue;
            }
            for path in step.paths() {
                for (id, doc) in &self.documents {
                    let inside = Path::new(id).starts_with(path) || doc.path.starts_with(path);
                    if inside || self.open_document_at(path).as_deref() == Some(id.as_str()) {
                        if doc.dirty {
                            return Err(format!(
                                "{id} has unsaved changes; save it before a change that creates, renames or deletes it"
                            ));
                        }
                        managed.insert(id.clone());
                    }
                }
            }
        }
        let mut buffers: Vec<BufferEdit> = Vec::new();
        let mut disk = Vec::new();
        for step in steps {
            let Step::Text {
                path,
                version,
                edits,
            } = &step
            else {
                disk.push(step);
                continue;
            };
            let id = match self.open_document_at(path) {
                Some(id) if !managed.contains(&id) => id,
                _ => {
                    disk.push(step);
                    continue;
                }
            };
            let doc = &self.documents[&id];
            if doc.read_only {
                return Err(format!("{id} is read-only"));
            }
            for expected in [*version, version_option(&options.versions, &id)]
                .into_iter()
                .flatten()
            {
                if expected != doc.lsp_version {
                    return Err(format!(
                        "{} changed: the edit is for version {expected}, the document is at version {}",
                        doc.path.display(),
                        doc.lsp_version
                    ));
                }
            }
            if buffers.iter().any(|b| b.id == id) {
                return Err(format!(
                    "{id}: several edits of one open document in one change are not supported"
                ));
            }
            let base = doc.sent.clone();
            let ranges =
                resolve_edits(&base, edits).map_err(|e| format!("{}: {e}", doc.path.display()))?;
            buffers.push(BufferEdit {
                id,
                ranges: anchor_ranges(&base, ranges),
            });
        }
        Ok((buffers, disk, managed.into_iter().collect()))
    }

    /// Apply the buffer edits (after the disk phase, when there is one) and build the summary.
    fn complete_apply(
        &mut self,
        buffers: Vec<BufferEdit>,
        disk: DiskOutcome,
        cx: &mut Context<Self>,
    ) -> ApplySummary {
        let mut paths: BTreeSet<String> = BTreeSet::new();
        let mut edits = disk.edits as u64;
        let open_documents = buffers.len() as u64;
        for b in buffers {
            if let Some(doc) = self.documents.get(&b.id) {
                paths.insert(doc.path.to_string_lossy().into_owned());
            }
            edits += b.ranges.len() as u64;
            self.apply_anchored(&b.id, b.ranges, cx);
        }
        for (p, _) in &disk.written {
            paths.insert(p.to_string_lossy().into_owned());
        }
        for p in &disk.deleted {
            paths.insert(p.to_string_lossy().into_owned());
        }
        ApplySummary {
            applied: true,
            files: paths.len() as u64,
            edits,
            open_documents,
            files_on_disk: disk.written.len() as u64,
            created: disk.created as u64,
            renamed: disk.renamed as u64,
            deleted: disk.deleted_count as u64,
            paths: paths.into_iter().collect(),
            message: None,
        }
    }

    /// After the disk phase: tell the language server, and let open tabs follow renames and deletes.
    fn after_disk(
        &mut self,
        outcome: &DiskOutcome,
        managed: &[String],
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut changes: Vec<serde_json::Value> = outcome
            .written
            .iter()
            .map(
                |(p, existed)| json!({"uri": path_to_uri(p), "type": if *existed { 2 } else { 1 }}),
            )
            .collect();
        changes.extend(
            outcome
                .deleted
                .iter()
                .map(|p| json!({"uri": path_to_uri(p), "type": 3})),
        );
        if !changes.is_empty() {
            self.session.notify_untyped(
                "workspace/didChangeWatchedFiles",
                json!({ "changes": changes }),
            );
        }
        for id in managed {
            let Some(doc) = self.documents.get(id) else {
                continue;
            };
            let path = doc.path.clone();
            let caret = doc.view.read(cx).editor().primary_selection().head;
            let (line, column) = super::intellisense::line_column(&doc.view, caret, cx);
            let reopen = outcome
                .moves
                .iter()
                .find(|(from, _)| *from == path)
                .map(|(_, to)| to.clone())
                .or_else(|| path.is_file().then(|| path.clone()));
            let _ = self.close_file(
                id,
                Some(eludite_commands::workspace::CloseSave::Discard),
                cx,
            );
            if let Some(to) = reopen {
                let _ = self.open_file(&to.to_string_lossy(), Some((line, column)), window, cx);
            }
        }
    }

    fn report_apply(
        &mut self,
        options: &ApplyOptions,
        summary: &ApplySummary,
        started: std::time::Instant,
    ) {
        trace(format_args!(
            "workspace edit {:?} in {:.1} ms: applied {}, {} edits in {} files ({} open, {} on disk) {:?}",
            options.label,
            started.elapsed().as_secs_f64() * 1e3,
            summary.applied,
            summary.edits,
            summary.files,
            summary.open_documents,
            summary.files_on_disk,
            summary.message
        ));
        self.status.set(
            eludite_ui::slots::STATE,
            summary.status_text(options.label.as_deref()),
        );
    }

    /// `eludite.workspace.apply_edit`.
    pub(super) fn apply_edit_command(
        &mut self,
        edit: serde_json::Value,
        label: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<eludite_commands::workspace::WorkspaceOutput, eludite_commands::CommandError> {
        use eludite_commands::workspace::ApplyEditState;
        let edit: lsp::WorkspaceEdit = serde_json::from_value(edit).map_err(|e| {
            eludite_commands::CommandError::InvalidInput(format!(
                "`edit` is not a WorkspaceEdit: {e}"
            ))
        })?;
        self.apply_edit = Some((ApplyEditState::Applying, ApplySummary::default()));
        self.apply_workspace_edit(
            &edit,
            ApplyOptions {
                label,
                ..Default::default()
            },
            window,
            cx,
            Box::new(|shell, summary, _, _| {
                let state = if summary.applied {
                    ApplyEditState::Applied
                } else {
                    ApplyEditState::Failed
                };
                shell.apply_edit = Some((state, summary));
                shell.wake_intellisense_waiters();
            }),
        );
        Ok(eludite_commands::workspace::WorkspaceOutput::ApplyEdit(
            self.apply_edit_output(),
        ))
    }

    pub fn apply_edit_output(&self) -> eludite_commands::workspace::ApplyEditOutput {
        use eludite_commands::workspace::{ApplyEditOutput, ApplyEditState};
        match &self.apply_edit {
            Some((state, summary)) => ApplyEditOutput::new(*state, summary.output()),
            None => ApplyEditOutput::new(
                ApplyEditState::Failed,
                ApplySummary::refused("no edit was applied").output(),
            ),
        }
    }

    /// `workspace/applyEdit` from the host: apply with the applier, answer `applied` (brief 0015).
    pub(super) fn on_host_apply_edit(
        &mut self,
        id: eludite_lsp::Id,
        generation: u64,
        params: lsp::ApplyWorkspaceEditParams,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let options = ApplyOptions {
            label: params.label.clone(),
            generation: Some(generation),
            versions: HashMap::new(),
        };
        self.apply_workspace_edit(
            &params.edit,
            options,
            window,
            cx,
            Box::new(move |shell, summary, _, _| {
                shell.session.respond_apply_edit(
                    id,
                    lsp::ApplyWorkspaceEditResult {
                        applied: summary.applied,
                        failure_reason: summary.message.clone().filter(|_| !summary.applied),
                        failed_change: None,
                    },
                );
            }),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn edit(line: u32, start: u32, end_line: u32, end: u32, text: &str) -> lsp::TextEdit {
        lsp::TextEdit::new(
            lsp::Range {
                start: lsp::Position {
                    line,
                    character: start,
                },
                end: lsp::Position {
                    line: end_line,
                    character: end,
                },
            },
            text,
        )
    }

    fn apply_text(text: &str, edits: &[lsp::TextEdit]) -> Result<String, String> {
        let mut b = Buffer::new(text);
        let ranges = resolve_edits(b.snapshot(), edits)?;
        b.edit(ranges);
        Ok(b.text())
    }

    #[test]
    fn edits_resolve_in_utf16_sorted_and_checked() {
        let text = "class A\n{\n    string s = \"𝄞\"; int Ping;\n}\n";
        // Out of order, a UTF-16 column after an astral character, and an insert.
        let out = apply_text(
            text,
            &[
                edit(2, 25, 2, 29, "Pong"),
                edit(0, 0, 0, 0, "using X;\n"),
                edit(2, 4, 2, 10, "var"),
            ],
        )
        .unwrap();
        assert_eq!(
            out,
            "using X;\nclass A\n{\n    var s = \"𝄞\"; int Pong;\n}\n"
        );
        // Two inserts at one position keep their order.
        assert_eq!(
            apply_text("ab", &[edit(0, 1, 0, 1, "1"), edit(0, 1, 0, 1, "2")]).unwrap(),
            "a12b"
        );
        // A character past the line's end means the end of the line.
        assert_eq!(
            apply_text("ab\ncd", &[edit(0, 99, 0, 99, "!")]).unwrap(),
            "ab!\ncd"
        );
        assert!(
            apply_text("ab", &[edit(5, 0, 5, 0, "x")]).is_err(),
            "past the end"
        );
        assert!(
            apply_text("abcdef", &[edit(0, 0, 0, 3, "x"), edit(0, 2, 0, 4, "y")]).is_err(),
            "overlap"
        );
        assert!(
            apply_text("ab", &[edit(0, 2, 0, 1, "x")]).is_err(),
            "reversed"
        );
    }

    #[test]
    fn plans_follow_document_changes_or_changes() {
        let dir = std::env::temp_dir();
        let a = path_to_uri(&dir.join("a.cs"));
        let e: lsp::WorkspaceEdit = serde_json::from_value(json!({
            "changes": {a.clone(): [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}]},
            "documentChanges": [{"kind": "create", "uri": a.clone()}]
        }))
        .unwrap();
        assert!(
            matches!(&plan(&e).unwrap()[..], [Step::Create { .. }]),
            "documentChanges wins"
        );
        let e: lsp::WorkspaceEdit =
            serde_json::from_value(json!({"changes": {a.clone(): []}})).unwrap();
        assert!(matches!(
            &plan(&e).unwrap()[..],
            [Step::Text { version: None, .. }]
        ));
        let bad: lsp::WorkspaceEdit = serde_json::from_value(
            json!({"documentChanges": [{"kind": "delete", "uri": "untitled:1"}]}),
        )
        .unwrap();
        assert!(plan(&bad).unwrap_err().contains("not a file URI"));
    }

    #[test]
    fn disk_steps_create_rename_edit_and_delete_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let p = |n: &str| normalize_path(&dir.path().join(n));
        std::fs::write(p("Old.cs"), "\u{feff}class Old\r\n{\r\n}\r\n").unwrap();
        std::fs::write(p("Gone.cs"), "x").unwrap();
        std::fs::create_dir(p("folder")).unwrap();
        std::fs::write(p("folder/In.cs"), "y").unwrap();
        let steps = vec![
            Step::Create {
                path: p("New/Made.cs"),
                overwrite: false,
                ignore_if_exists: false,
            },
            Step::Text {
                path: p("New/Made.cs"),
                version: None,
                edits: vec![edit(0, 0, 0, 0, "class Made { }\n")],
            },
            Step::Rename {
                from: p("Old.cs"),
                to: p("Renamed.cs"),
                overwrite: false,
                ignore_if_exists: false,
            },
            Step::Text {
                path: p("Renamed.cs"),
                version: None,
                edits: vec![edit(0, 6, 0, 9, "Renamed")],
            },
            Step::Delete {
                path: p("Gone.cs"),
                recursive: false,
                ignore_if_not_exists: false,
            },
            Step::Delete {
                path: p("folder"),
                recursive: true,
                ignore_if_not_exists: false,
            },
            Step::Delete {
                path: p("Never.cs"),
                recursive: false,
                ignore_if_not_exists: true,
            },
        ];
        let out = apply_on_disk(&steps).unwrap();
        assert_eq!(
            std::fs::read_to_string(p("New/Made.cs")).unwrap(),
            "class Made { }\n"
        );
        // The BOM and CRLF line endings survive.
        assert_eq!(
            std::fs::read(p("Renamed.cs")).unwrap(),
            "\u{feff}class Renamed\r\n{\r\n}\r\n".as_bytes()
        );
        assert!(!p("Old.cs").exists() && !p("Gone.cs").exists() && !p("folder").exists());
        assert_eq!(
            (out.created, out.renamed, out.deleted_count, out.edits),
            (1, 1, 2, 2)
        );
        assert_eq!(out.moves, [(p("Old.cs"), p("Renamed.cs"))]);
        assert_eq!(out.written.len(), 2);
        // No temporary files are left.
        let left: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        assert!(left.iter().all(|n| !n.ends_with(".tmp")), "{left:?}");

        // A failing step changes nothing on disk.
        let before = std::fs::read(p("Renamed.cs")).unwrap();
        let failing = vec![
            Step::Text {
                path: p("Renamed.cs"),
                version: None,
                edits: vec![edit(0, 0, 0, 5, "struct")],
            },
            Step::Create {
                path: p("New/Made.cs"),
                overwrite: false,
                ignore_if_exists: false,
            },
        ];
        assert!(
            apply_on_disk(&failing)
                .unwrap_err()
                .contains("already exists")
        );
        assert_eq!(std::fs::read(p("Renamed.cs")).unwrap(), before);
        let past_end = vec![Step::Text {
            path: p("Renamed.cs"),
            version: None,
            edits: vec![edit(40, 0, 40, 0, "x")],
        }];
        assert!(apply_on_disk(&past_end).is_err());
        assert_eq!(std::fs::read(p("Renamed.cs")).unwrap(), before);
        assert!(
            apply_on_disk(&[Step::Rename {
                from: p("Missing.cs"),
                to: p("X.cs"),
                overwrite: false,
                ignore_if_exists: false
            }])
            .is_err()
        );
    }
}
