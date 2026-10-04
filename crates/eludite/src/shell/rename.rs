//! Rename (brief 0015): Ctrl+R, Ctrl+R or F2 on a symbol opens Visual Studio's Rename dialog: the new name, a preview
//! of every file and changed line, Apply and Cancel.
//!
//! - **Prepare first.** `textDocument/prepareRename` at the position checks that it is on a renameable symbol and gives
//!   its range; `null` is Visual Studio's "You must rename an identifier." and no dialog opens.
//! - **Preview.** [`RENAME_PREVIEW_DEBOUNCE`] after the name changes, `textDocument/rename` asks for the edit; a newer
//!   request cancels the older one. The changed lines are computed off the UI thread (closed files are read from
//!   disk) and shown as a tree of files and lines.
//! - **Apply** hands the edit to the workspace-edit applier ([`super::workspace_edit`]) with the solution generation
//!   and document versions it was computed for, so a stale edit is refused: open documents change in their buffers
//!   (one undo step each), closed files on disk. Escape cancels.
//! - Answers are dropped when they are not the newest request's, or when the generation or the document's version
//!   moved on (CLAUDE.md invariant 12).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_editor::Buffer;
use eludite_lsp::lsp;
use eludite_ui::{Theme, dialog_panel, push_button, section_heading, text_box};
use gpui::{
    App, AppContext as _, Context, Entity, EventEmitter, FocusHandle, Focusable,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, anchored, deferred, div, point, px,
};

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    self, RenameFileRow, RenameLineRow, RenameOutput, WorkspaceOutput,
};
use serde_json::json;

use super::documents::trace;
use super::intellisense::{Provider, line_column, lsp_position};
use super::navigation::{NavEntry, OUTDATED};
use super::session::{RequestError, RequestHandle};
use super::workspace_edit::{self, ApplyOptions, ApplySummary, Step};
use super::{Caret, Shell};

/// Quiet time after the name changes before the preview is asked for.
pub const RENAME_PREVIEW_DEBOUNCE: Duration = Duration::from_millis(150);

/// Visual Studio's message when the caret is not on a renameable symbol.
pub const NOT_RENAMEABLE: &str = "You must rename an identifier.";

/// The language server answered no edit for the new name (the pinned Roslyn gives no reason).
pub const NAME_REFUSED: &str = "The rename cannot be performed: the new name is not valid here or conflicts with another symbol.";

/// Files and lines per file the preview keeps at most.
pub const MAX_PREVIEW_FILES: usize = 100;
pub const MAX_PREVIEW_LINES: usize = 100;

/// Where a rename is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RenameState {
    Loading,
    Dialog,
    Preview,
    Applied,
    Rejected,
    Failed,
}

/// One changed line of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineChange {
    /// 1-based, in the file before the rename.
    pub line: u32,
    pub before: String,
    pub after: String,
}

/// One file of the preview.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewFile {
    pub path: PathBuf,
    /// Open in an editor (it changes in the buffer, unsaved).
    pub open: bool,
    pub changes: Vec<LineChange>,
    pub edits: usize,
}

/// A computed rename: what it changes and the edit to apply.
#[derive(Debug, Clone)]
pub struct Preview {
    pub new_name: String,
    pub files: Vec<PreviewFile>,
    pub total_edits: usize,
    pub edit: lsp::WorkspaceEdit,
    /// The open documents' versions and the generation the edit was computed for.
    pub versions: HashMap<String, i32>,
    pub generation: u64,
}

/// What the last rename did (`eludite.editor.rename`'s output).
#[derive(Debug, Clone, Default)]
pub struct RenameStatus {
    pub origin: Option<NavEntry>,
    pub state: Option<RenameState>,
    pub symbol: Option<String>,
    pub new_name: Option<String>,
    pub preview: Option<Arc<Preview>>,
    pub summary: Option<ApplySummary>,
    pub message: Option<String>,
    /// The symbol's position in the text the server saw.
    position: Option<lsp::Position>,
    /// The document's LSP version and the generation at prepareRename.
    version: i32,
    generation: u64,
    /// Apply as soon as the preview for `new_name` arrives.
    apply_when_ready: bool,
}

/// When the steps of a rename happened (the `--bench-rename` harness and the report).
#[derive(Debug, Clone, Default)]
pub struct RenameTiming {
    /// `textDocument/rename` written and its reply read.
    pub sent: Option<Instant>,
    pub received: Option<Instant>,
    /// The preview was handed to the dialog.
    pub shown: Option<Instant>,
    pub edits: usize,
    pub files: usize,
    /// Apply: the applier's start and end.
    pub apply_started: Option<Instant>,
    pub applied: Option<Instant>,
}

struct Pending {
    handle: RequestHandle,
    ticket: u64,
    _task: Task<()>,
}

/// The window's rename.
#[derive(Default)]
pub struct Rename {
    pending: Option<Pending>,
    pub status: RenameStatus,
    pub dialog: Option<Entity<RenameDialog>>,
    next_ticket: u64,
    debounce: Option<Task<()>>,
    pub timings: Vec<RenameTiming>,
}

impl Rename {
    /// Cancel what is in flight (a new solution generation).
    pub fn cancel(&mut self) {
        self.debounce = None;
        if let Some(p) = self.pending.take() {
            p.handle.cancel();
            if self.status.state == Some(RenameState::Loading) {
                self.status.state = Some(RenameState::Failed);
                self.status.message = Some(OUTDATED.into());
            }
        }
    }
}

/// The changed lines of `text` under `edits`: for each line an edit starts on, the line before and after.
pub fn line_changes(text: &str, edits: &[lsp::TextEdit]) -> Result<Vec<LineChange>, String> {
    let mut buffer = Buffer::new(text);
    let ranges = workspace_edit::resolve_edits(buffer.snapshot(), edits)?;
    let rows: Vec<u32> = {
        let mut rows: Vec<u32> = ranges
            .iter()
            .map(|(r, _)| buffer.offset_to_point(r.start).row)
            .collect();
        rows.dedup();
        rows
    };
    let anchors: Vec<_> = rows
        .iter()
        .map(|&row| {
            let start = buffer.point_to_offset(eludite_editor::text::Point::new(row, 0));
            (row, buffer.line(row), buffer.anchor_before(start))
        })
        .collect();
    buffer.edit(ranges);
    Ok(anchors
        .into_iter()
        .map(|(row, before, anchor)| {
            let new_row = buffer
                .offset_to_point(buffer.offset_for_anchor(&anchor))
                .row;
            LineChange {
                line: row + 1,
                before: before.trim().to_owned(),
                after: buffer.line(new_row).trim().to_owned(),
            }
        })
        .collect())
}

/// The preview of `steps`: `texts` holds the text the server saw for open documents; other files are read from
/// disk. Runs off the UI thread.
pub fn preview_files(
    steps: &[Step],
    texts: &HashMap<PathBuf, String>,
) -> Result<(Vec<PreviewFile>, usize), String> {
    let mut by_file: BTreeMap<PathBuf, Vec<lsp::TextEdit>> = BTreeMap::new();
    for step in steps {
        if let Step::Text { path, edits, .. } = step {
            by_file
                .entry(path.clone())
                .or_default()
                .extend(edits.iter().cloned());
        }
    }
    let mut total = 0;
    let mut files = Vec::new();
    for (path, edits) in by_file {
        if edits.is_empty() {
            continue;
        }
        total += edits.len();
        let (text, open) = match texts.get(&path) {
            Some(t) => (t.clone(), true),
            None => (
                Buffer::load(&path)
                    .map_err(|e| format!("{}: {e}", path.display()))?
                    .text(),
                false,
            ),
        };
        let mut changes =
            line_changes(&text, &edits).map_err(|e| format!("{}: {e}", path.display()))?;
        changes.truncate(MAX_PREVIEW_LINES);
        files.push(PreviewFile {
            path,
            open,
            edits: edits.len(),
            changes,
        });
    }
    Ok((files, total))
}

/// What the dialog reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RenameDialogEvent {
    NameChanged(String),
    Apply,
    Cancel,
}

/// What the dialog's preview area shows.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PreviewView {
    /// Nothing asked yet (the name is unchanged).
    Empty,
    Computing,
    Ready {
        files: Vec<PreviewFile>,
        total: usize,
    },
    Error(String),
}

/// Debug selectors of the dialog.
pub const RENAME_DIALOG: &str = "rename-dialog";
pub const RENAME_NAME_BOX: &str = "rename-dialog-name";
pub const RENAME_APPLY: &str = "rename-dialog-apply";
pub const RENAME_CANCEL: &str = "rename-dialog-cancel";

/// Visual Studio's Rename dialog: the new name, the preview of changes, Apply and Cancel.
pub struct RenameDialog {
    theme: Theme,
    symbol: String,
    name: String,
    /// The whole name is selected (as when the dialog opens): typing replaces it.
    selected: bool,
    preview: PreviewView,
    focus: FocusHandle,
}

impl RenameDialog {
    pub fn new(theme: Theme, symbol: String, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            name: symbol.clone(),
            symbol,
            selected: true,
            preview: PreviewView::Empty,
            focus: cx.focus_handle(),
        }
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn preview(&self) -> &PreviewView {
        &self.preview
    }

    pub fn set_preview(&mut self, preview: PreviewView, cx: &mut Context<Self>) {
        self.preview = preview;
        cx.notify();
    }

    fn set_name(&mut self, name: String, cx: &mut Context<Self>) {
        if name != self.name {
            self.name = name.clone();
            cx.emit(RenameDialogEvent::NameChanged(name));
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            if k.modifiers.control && k.key == "a" {
                self.selected = true;
                cx.stop_propagation();
                cx.notify();
            }
            return;
        }
        match k.key.as_str() {
            "escape" => cx.emit(RenameDialogEvent::Cancel),
            "enter" => cx.emit(RenameDialogEvent::Apply),
            "backspace" => {
                let name = if self.selected {
                    String::new()
                } else {
                    let mut n = self.name.clone();
                    n.pop();
                    n
                };
                self.selected = false;
                self.set_name(name, cx);
            }
            _ => {
                let typed = k.key_char.clone().or_else(|| {
                    (k.key.chars().count() == 1).then(|| {
                        if k.modifiers.shift {
                            k.key.to_uppercase()
                        } else {
                            k.key.clone()
                        }
                    })
                });
                match typed {
                    Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                        let name = if self.selected {
                            c
                        } else {
                            format!("{}{c}", self.name)
                        };
                        self.selected = false;
                        self.set_name(name, cx);
                    }
                    _ => return,
                }
            }
        }
        cx.stop_propagation();
    }

    fn summary_text(&self) -> String {
        match &self.preview {
            PreviewView::Empty => "Type the new name to preview the changes.".into(),
            PreviewView::Computing => "Computing the preview\u{2026}".into(),
            PreviewView::Ready { files, total } => format!(
                "{total} change{} in {} file{}",
                if *total == 1 { "" } else { "s" },
                files.len(),
                if files.len() == 1 { "" } else { "s" }
            ),
            PreviewView::Error(e) => e.clone(),
        }
    }
}

impl EventEmitter<RenameDialogEvent> for RenameDialog {}

impl Focusable for RenameDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RenameDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.focus.is_focused(window);
        let name_box = text_box(RENAME_NAME_BOX, &self.name, "", focused, &t)
            .w(px(380.))
            .when_selected(self.selected && focused, &t);
        let mut rows = Vec::new();
        if let PreviewView::Ready { files, .. } = &self.preview {
            for (fi, f) in files.iter().enumerate() {
                let name = f
                    .path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                let dir = f
                    .path
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_default();
                rows.push(
                    div()
                        .id(SharedString::from(format!("rename-preview-file-{fi}")))
                        .flex()
                        .gap_2()
                        .px_2()
                        .h(px(20.))
                        .items_center()
                        .whitespace_nowrap()
                        .child(format!("\u{25BE} {name}"))
                        .child(
                            div()
                                .text_color(t.text_muted)
                                .text_size(t.typography.small)
                                .child(format!(
                                    "{dir}  ({} change{}{})",
                                    f.edits,
                                    if f.edits == 1 { "" } else { "s" },
                                    if f.open { ", open" } else { "" }
                                )),
                        )
                        .into_any_element(),
                );
                for c in &f.changes {
                    rows.push(
                        div()
                            .flex()
                            .gap_2()
                            .pl(px(28.))
                            .h(px(20.))
                            .items_center()
                            .whitespace_nowrap()
                            .child(
                                div()
                                    .w(px(44.))
                                    .text_color(t.text_muted)
                                    .child(format!("{}", c.line)),
                            )
                            .child(
                                div()
                                    .font_family(eludite_editor::default_font_family())
                                    .child(c.after.clone()),
                            )
                            .into_any_element(),
                    );
                }
            }
        }
        let apply_enabled = !self.name.is_empty();
        let mut apply = push_button(RENAME_APPLY, "Apply", true, apply_enabled, &t);
        if apply_enabled {
            apply = apply.on_click(cx.listener(|_, _, _, cx| cx.emit(RenameDialogEvent::Apply)));
        }
        let cancel = push_button(RENAME_CANCEL, "Cancel", false, true, &t)
            .on_click(cx.listener(|_, _, _, cx| cx.emit(RenameDialogEvent::Cancel)));
        let panel = dialog_panel(&t, format!("Rename: {}", self.symbol))
            .id(RENAME_DIALOG)
            .debug_selector(|| RENAME_DIALOG.into())
            .track_focus(&self.focus)
            .key_context("RenameDialog")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(640.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .child("New name:")
                    .child(name_box),
            )
            .child(section_heading("Preview changes", &t))
            .child(
                div()
                    .id("rename-preview")
                    .debug_selector(|| "rename-preview".into())
                    .flex()
                    .flex_col()
                    .mx_2()
                    .h(px(260.))
                    .overflow_y_scroll()
                    .bg(t.background)
                    .border_1()
                    .border_color(t.border)
                    .children(rows),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .p_3()
                    .child(
                        div()
                            .flex_1()
                            .text_color(if matches!(self.preview, PreviewView::Error(_)) {
                                gpui::rgb(0xF14C4C)
                            } else {
                                t.text_muted
                            })
                            .child(self.summary_text()),
                    )
                    .child(apply)
                    .child(cancel),
            );
        let viewport = window.viewport_size();
        let at = point(
            ((viewport.width - px(640.)) / 2.).max(px(0.)),
            (viewport.height / 5.).max(px(0.)),
        );
        deferred(anchored().position(at).child(panel)).with_priority(5)
    }
}

/// The name box with its whole text selected.
trait SelectedText {
    fn when_selected(self, selected: bool, theme: &Theme) -> Self;
}

impl SelectedText for gpui::Stateful<gpui::Div> {
    fn when_selected(self, selected: bool, theme: &Theme) -> Self {
        if selected {
            self.bg(theme.accent).text_color(theme.text_on_accent)
        } else {
            self
        }
    }
}

impl Shell {
    /// Start a rename of the symbol at `at` (or the caret) in document `id`: prepareRename, then the dialog, or with
    /// `new_name` the preview (and, with `apply`, the rename) without one.
    pub(super) fn start_rename(
        &mut self,
        id: &str,
        at: Option<Caret>,
        new_name: Option<String>,
        apply: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.rename.cancel();
        self.close_rename_dialog(window, cx);
        let view = self.documents[id].view.clone();
        let offset = match at {
            Some((line, column)) => {
                super::documents::offset_of(view.read(cx).editor().buffer(), line, column)
            }
            None => view.read(cx).editor().primary_selection().head,
        };
        let (line, column) = line_column(&view, offset, cx);
        self.rename.status = RenameStatus {
            origin: Some(NavEntry {
                path: id.to_owned(),
                line,
                column,
            }),
            state: Some(RenameState::Loading),
            apply_when_ready: apply && new_name.is_some(),
            new_name,
            ..Default::default()
        };
        if self.documents[id].read_only {
            self.finish_rename(RenameState::Rejected, Some(NOT_RENAMEABLE.into()), cx);
            return;
        }
        if self.provider(&self.documents[id]) == Provider::Syntax {
            self.finish_rename(
                RenameState::Failed,
                Some("No language server is available for this document.".into()),
                cx,
            );
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let version = doc.lsp_version;
        let sent = doc.sent.clone();
        let position = lsp_position(&sent, offset);
        self.rename.status.position = Some(position);
        self.rename.status.version = version;
        self.rename.status.generation = generation;
        let params = lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position,
        };
        self.rename.next_ticket += 1;
        let ticket = self.rename.next_ticket;
        trace(format_args!(
            "prepareRename {ticket} at {}:{} version {version}",
            position.line, position.character
        ));
        let (handle, rx) = doc.session.request::<lsp::PrepareRename>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                if !shell.rename_answer_is_current(ticket, &doc_id, generation, version) {
                    return;
                }
                shell.rename.pending = None;
                let range = match reply.result {
                    Err(RequestError::Canceled) => return,
                    Ok(Some(lsp::PrepareRenameResponse::Range(r)))
                    | Ok(Some(lsp::PrepareRenameResponse::RangeWithPlaceholder {
                        range: r, ..
                    })) => Some(r),
                    Ok(Some(lsp::PrepareRenameResponse::DefaultBehavior { .. })) => None,
                    Ok(None) | Err(_) => {
                        shell.finish_rename(RenameState::Rejected, Some(NOT_RENAMEABLE.into()), cx);
                        return;
                    }
                };
                let placeholder = match reply.result {
                    Ok(Some(lsp::PrepareRenameResponse::RangeWithPlaceholder {
                        placeholder,
                        ..
                    })) => Some(placeholder),
                    _ => None,
                };
                let symbol = placeholder
                    .or_else(|| {
                        range.map(|r| {
                            let s = super::intellisense::offset_in(&sent, r.start);
                            let e = super::intellisense::offset_in(&sent, r.end);
                            sent.text_for_range(s..e.max(s)).collect::<String>()
                        })
                    })
                    .filter(|s| !s.is_empty())
                    .or_else(|| {
                        shell
                            .rename
                            .status
                            .origin
                            .clone()
                            .and_then(|o| shell.word_at(&o, cx))
                    });
                let Some(symbol) = symbol else {
                    shell.finish_rename(RenameState::Rejected, Some(NOT_RENAMEABLE.into()), cx);
                    return;
                };
                if let Some(r) = range {
                    shell.rename.status.position = Some(r.start);
                }
                trace(format_args!("prepareRename {ticket}: {symbol:?}"));
                shell.rename.status.symbol = Some(symbol.clone());
                shell.rename.status.state = Some(RenameState::Dialog);
                match shell.rename.status.new_name.clone() {
                    Some(name) => shell.request_rename_preview(name, window, cx),
                    None => shell.open_rename_dialog(symbol, window, cx),
                }
                shell.wake_intellisense_waiters();
            });
        });
        self.rename.pending = Some(Pending {
            handle,
            ticket,
            _task: task,
        });
    }

    /// Whether an answer to request `ticket` may be applied; a newest answer that is outdated fails the rename.
    fn rename_answer_is_current(
        &mut self,
        ticket: u64,
        doc_id: &str,
        generation: u64,
        version: i32,
    ) -> bool {
        let newest = self
            .rename
            .pending
            .as_ref()
            .is_some_and(|p| p.ticket == ticket);
        let doc_version = self.documents.get(doc_id).map(|d| d.lsp_version);
        let current = self.doc_generation(doc_id);
        let outdated = generation != current || doc_version != Some(version);
        if !newest || outdated {
            trace(format_args!(
                "rename reply {ticket} dropped (stale: newest {newest}, generation {generation}/{current}, version {version}/{doc_version:?})"
            ));
            if newest {
                self.rename.pending = None;
                self.rename.status.state = Some(RenameState::Failed);
                self.rename.status.message = Some(OUTDATED.into());
                self.wake_intellisense_waiters();
            }
            return false;
        }
        true
    }

    fn finish_rename(
        &mut self,
        state: RenameState,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = &message {
            self.status.set(eludite_ui::slots::STATE, m.clone());
        }
        self.rename.status.state = Some(state);
        self.rename.status.message = message;
        self.rename.status.apply_when_ready = false;
        self.wake_intellisense_waiters();
        cx.notify();
    }

    fn open_rename_dialog(&mut self, symbol: String, window: &mut Window, cx: &mut Context<Self>) {
        let theme = self.theme;
        let dialog = cx.new(|cx| RenameDialog::new(theme, symbol, cx));
        cx.subscribe_in(&dialog, window, |shell, _, event, window, cx| match event {
            RenameDialogEvent::NameChanged(name) => {
                shell.rename_name_changed(name.clone(), window, cx)
            }
            // Apply goes through the bus, like an agent's rename with a new name.
            RenameDialogEvent::Apply => {
                let name = shell
                    .rename
                    .dialog
                    .as_ref()
                    .map(|d| d.read(cx).name().to_owned())
                    .unwrap_or_default();
                let path = shell.rename.status.origin.as_ref().map(|o| o.path.clone());
                if !name.is_empty() {
                    shell.run(
                        workspace::EDITOR_RENAME,
                        json!({ "path": path, "new_name": name }),
                        window,
                        cx,
                    );
                }
            }
            RenameDialogEvent::Cancel => shell.cancel_rename(window, cx),
        })
        .detach();
        dialog.focus_handle(cx).focus(window, cx);
        self.rename.dialog = Some(dialog);
        cx.notify();
    }

    pub(super) fn close_rename_dialog(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.rename.dialog.take().is_some() {
            if let Some(id) = self.controller.active_document()
                && let Some(doc) = self.documents.get(&id)
            {
                doc.view.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    /// Escape or Cancel.
    pub(super) fn cancel_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.rename.debounce = None;
        if let Some(p) = self.rename.pending.take() {
            p.handle.cancel();
        }
        self.rename.status.apply_when_ready = false;
        self.close_rename_dialog(window, cx);
        self.status.set(eludite_ui::slots::STATE, "Ready");
    }

    fn rename_name_changed(&mut self, name: String, window: &mut Window, cx: &mut Context<Self>) {
        self.rename.status.new_name = Some(name.clone());
        self.rename.status.preview = None;
        if let Some(p) = self.rename.pending.take() {
            p.handle.cancel();
        }
        let symbol = self.rename.status.symbol.clone().unwrap_or_default();
        let empty = name.is_empty() || name == symbol;
        if let Some(d) = &self.rename.dialog {
            d.update(cx, |d, cx| {
                d.set_preview(
                    if empty {
                        PreviewView::Empty
                    } else {
                        PreviewView::Computing
                    },
                    cx,
                )
            });
        }
        if empty {
            self.rename.debounce = None;
            return;
        }
        self.rename.debounce = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor()
                .timer(RENAME_PREVIEW_DEBOUNCE)
                .await;
            let _ = this.update_in(cx, |shell, window, cx| {
                shell.rename.debounce = None;
                if shell.rename.status.new_name.as_deref() == Some(name.as_str()) {
                    shell.request_rename_preview(name, window, cx);
                }
            });
        }));
    }

    /// Ask for the edit that renames the prepared symbol to `name`.
    pub(super) fn request_rename_preview(
        &mut self,
        name: String,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(p) = self.rename.pending.take() {
            p.handle.cancel();
        }
        let Some(origin) = self.rename.status.origin.clone() else {
            return;
        };
        let Some(position) = self.rename.status.position else {
            return;
        };
        let doc_id = origin.path.clone();
        let Some(doc) = self.documents.get(&doc_id) else {
            self.finish_rename(RenameState::Failed, Some(OUTDATED.into()), cx);
            return;
        };
        self.rename.status.new_name = Some(name.clone());
        self.rename.status.preview = None;
        self.rename.status.state = Some(RenameState::Loading);
        let generation = self.rename.status.generation;
        let version = self.rename.status.version;
        let params = lsp::RenameParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position,
            new_name: name.clone(),
        };
        self.rename.next_ticket += 1;
        let ticket = self.rename.next_ticket;
        trace(format_args!("rename {ticket} to {name:?}"));
        let (handle, rx) = doc.session.request::<lsp::Rename>(params);
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let prepared = this.update_in(cx, |shell, _, cx| {
                if !shell.rename_answer_is_current(ticket, &doc_id, generation, version) {
                    return None;
                }
                let edit = match reply.result {
                    Err(RequestError::Canceled) => return None,
                    Ok(Some(edit)) => edit,
                    Ok(None) => {
                        shell.rename.pending = None;
                        shell.rename_preview_failed(RenameState::Rejected, NAME_REFUSED.into(), cx);
                        return None;
                    }
                    Err(e) => {
                        shell.rename.pending = None;
                        let message = match e {
                            RequestError::Failed(m) => m,
                            other => format!("{other:?}"),
                        };
                        shell.rename_preview_failed(RenameState::Failed, message, cx);
                        return None;
                    }
                };
                let steps = match workspace_edit::plan(&edit) {
                    Ok(s) => s,
                    Err(e) => {
                        shell.rename.pending = None;
                        shell.rename_preview_failed(RenameState::Failed, e, cx);
                        return None;
                    }
                };
                // The text the server saw, for the open documents the edit touches, and their versions.
                let mut texts = HashMap::new();
                let mut versions = HashMap::new();
                let mut open: HashSet<PathBuf> = HashSet::new();
                for step in &steps {
                    if let Step::Text { path, .. } = step
                        && let Some(id) = shell.open_document_at(path)
                    {
                        let d = &shell.documents[&id];
                        texts.insert(path.clone(), d.sent.text());
                        versions.insert(id, d.lsp_version);
                        open.insert(path.clone());
                    }
                }
                Some((edit, steps, texts, versions, reply.sent, reply.received))
            });
            let Ok(Some((edit, steps, texts, versions, sent, received))) = prepared else {
                return;
            };
            let computed = cx
                .background_spawn(async move { preview_files(&steps, &texts) })
                .await;
            let _ = this.update_in(cx, |shell, window, cx| {
                if !shell
                    .rename
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.ticket == ticket)
                {
                    return;
                }
                shell.rename.pending = None;
                match computed {
                    Err(e) => shell.rename_preview_failed(RenameState::Failed, e, cx),
                    Ok((files, total)) => {
                        let mut files = files;
                        files.truncate(MAX_PREVIEW_FILES);
                        let timing = RenameTiming {
                            sent,
                            received: Some(received),
                            shown: Some(Instant::now()),
                            edits: total,
                            files: files.len(),
                            ..Default::default()
                        };
                        if shell.rename.timings.len() >= 4096 {
                            shell.rename.timings.remove(0);
                        }
                        shell.rename.timings.push(timing);
                        trace(format_args!(
                            "rename {ticket}: {total} edits in {} files (host {:.1} ms)",
                            files.len(),
                            sent.map_or(0., |s| (received - s).as_secs_f64() * 1e3)
                        ));
                        shell.rename.status.preview = Some(Arc::new(Preview {
                            new_name: name.clone(),
                            files: files.clone(),
                            total_edits: total,
                            edit,
                            versions,
                            generation,
                        }));
                        shell.rename.status.state = Some(RenameState::Preview);
                        shell.rename.status.message = None;
                        if let Some(d) = &shell.rename.dialog {
                            d.update(cx, |d, cx| {
                                d.set_preview(PreviewView::Ready { files, total }, cx)
                            });
                        }
                        shell.wake_intellisense_waiters();
                        if shell.rename.status.apply_when_ready {
                            shell.apply_rename(window, cx);
                        }
                        cx.notify();
                    }
                }
            });
        });
        self.rename.pending = Some(Pending {
            handle,
            ticket,
            _task: task,
        });
    }

    fn rename_preview_failed(
        &mut self,
        state: RenameState,
        message: String,
        cx: &mut Context<Self>,
    ) {
        if let Some(d) = &self.rename.dialog {
            d.update(cx, |d, cx| {
                d.set_preview(PreviewView::Error(message.clone()), cx)
            });
        }
        self.finish_rename(state, Some(message), cx);
    }

    /// Apply the preview for the current name (Apply, Enter, or `apply` on the command); when it is still being
    /// computed, apply it as soon as it arrives.
    pub(super) fn apply_rename(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = self.rename.status.new_name.clone().unwrap_or_default();
        let symbol = self.rename.status.symbol.clone().unwrap_or_default();
        if name.is_empty() {
            return;
        }
        if name == symbol {
            // Nothing to rename: Visual Studio closes the dialog.
            self.cancel_rename(window, cx);
            return;
        }
        let preview = self
            .rename
            .status
            .preview
            .clone()
            .filter(|p| p.new_name == name);
        let Some(preview) = preview else {
            self.rename.status.apply_when_ready = true;
            if self.rename.pending.is_none() && self.rename.debounce.is_none() {
                self.request_rename_preview(name, window, cx);
            } else if self.rename.debounce.is_some() {
                // Skip the rest of the debounce: the user wants it now.
                self.rename.debounce = None;
                self.request_rename_preview(name, window, cx);
            }
            return;
        };
        self.rename.status.apply_when_ready = false;
        // Loading until the applier is done (closed files are written off the UI thread).
        self.rename.status.state = Some(RenameState::Loading);
        let started = Instant::now();
        let server = self
            .rename
            .status
            .origin
            .as_ref()
            .map(|o| self.doc_key(&o.path))
            .unwrap_or_default();
        let options = ApplyOptions {
            label: Some(format!("Rename '{symbol}' to '{name}'")),
            generation: Some(preview.generation),
            server,
            versions: preview.versions.clone(),
        };
        self.apply_workspace_edit(
            &preview.edit,
            options,
            window,
            cx,
            Box::new(move |shell, summary, window, cx| {
                if let Some(t) = shell.rename.timings.last_mut() {
                    t.apply_started = Some(started);
                    t.applied = Some(Instant::now());
                }
                let applied = summary.applied;
                let message = summary.message.clone();
                shell.rename.status.summary = Some(summary);
                if applied {
                    shell.close_rename_dialog(window, cx);
                    shell.rename.status.state = Some(RenameState::Applied);
                    shell.rename.status.message = None;
                    shell.wake_intellisense_waiters();
                    cx.notify();
                } else {
                    let message = message.unwrap_or_else(|| "The rename was not applied.".into());
                    shell.rename_preview_failed(RenameState::Failed, message, cx);
                }
            }),
        );
    }

    /// `eludite.editor.rename`. With `new_name`, no position and the dialog open for the same document, it renames
    /// the dialog's symbol (the dialog's Apply); otherwise it starts over at the position.
    pub(super) fn rename_command(
        &mut self,
        path: Option<&str>,
        at: Option<Caret>,
        new_name: Option<String>,
        apply: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        let prepared = self.rename.dialog.is_some()
            && at.is_none()
            && self.rename.status.symbol.is_some()
            && self
                .rename
                .status
                .origin
                .as_ref()
                .is_some_and(|o| o.path == id);
        match new_name {
            Some(name) if prepared => {
                self.rename.status.new_name = Some(name.clone());
                if apply {
                    self.apply_rename(window, cx);
                } else {
                    self.request_rename_preview(name, window, cx);
                }
            }
            new_name => self.start_rename(&id, at, new_name, apply, window, cx),
        }
        Ok(WorkspaceOutput::Rename(self.rename_output()))
    }

    pub(super) fn rename_output(&self) -> RenameOutput {
        let s = &self.rename.status;
        let origin = s.origin.clone().unwrap_or(NavEntry {
            path: String::new(),
            line: 1,
            column: 1,
        });
        let preview = s
            .preview
            .as_ref()
            .filter(|p| s.new_name.as_deref() == Some(p.new_name.as_str()));
        RenameOutput {
            path: origin.path,
            line: origin.line,
            column: origin.column,
            state: match s.state.unwrap_or(RenameState::Loading) {
                RenameState::Loading => workspace::RenameState::Loading,
                RenameState::Dialog => workspace::RenameState::Dialog,
                RenameState::Preview => workspace::RenameState::Preview,
                RenameState::Applied => workspace::RenameState::Applied,
                RenameState::Rejected => workspace::RenameState::Rejected,
                RenameState::Failed => workspace::RenameState::Failed,
            },
            symbol: s.symbol.clone(),
            new_name: s.new_name.clone(),
            files: preview
                .map(|p| {
                    p.files
                        .iter()
                        .take(workspace::MAX_RENAME_FILES)
                        .map(|f| RenameFileRow {
                            path: f.path.to_string_lossy().into_owned(),
                            open: f.open,
                            changes: f
                                .changes
                                .iter()
                                .take(workspace::MAX_RENAME_LINES)
                                .map(|c| RenameLineRow {
                                    line: c.line,
                                    before: c.before.clone(),
                                    after: c.after.clone(),
                                })
                                .collect(),
                        })
                        .collect()
                })
                .unwrap_or_default(),
            total_edits: preview.map_or(0, |p| p.total_edits as u64),
            summary: s.summary.as_ref().map(ApplySummary::output),
            message: s.message.clone(),
        }
    }

    /// The rename timings so far (oldest first).
    pub fn rename_timings(&self) -> &[RenameTiming] {
        &self.rename.timings
    }

    /// The Rename dialog, while it is open.
    pub fn rename_dialog(&self) -> Option<&Entity<RenameDialog>> {
        self.rename.dialog.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(line: u32, start: u32, end: u32, text: &str) -> lsp::TextEdit {
        lsp::TextEdit::new(
            lsp::Range {
                start: lsp::Position {
                    line,
                    character: start,
                },
                end: lsp::Position {
                    line,
                    character: end,
                },
            },
            text,
        )
    }

    #[test]
    fn line_changes_show_each_changed_line_before_and_after() {
        let text = "class Host\n{\n    void Ping() { }\n    void Use() { Ping(); Ping(); }\n}\n";
        let changes = line_changes(
            text,
            &[
                edit(2, 9, 13, "Pong"),
                edit(3, 17, 21, "Pong"),
                edit(3, 25, 29, "Pong"),
            ],
        )
        .unwrap();
        assert_eq!(
            changes,
            [
                LineChange {
                    line: 3,
                    before: "void Ping() { }".into(),
                    after: "void Pong() { }".into()
                },
                LineChange {
                    line: 4,
                    before: "void Use() { Ping(); Ping(); }".into(),
                    after: "void Use() { Pong(); Pong(); }".into()
                },
            ]
        );
        // An inserted line break moves later lines.
        let changes = line_changes("a\nb\n", &[edit(0, 0, 0, "x\n"), edit(1, 0, 1, "B")]).unwrap();
        assert_eq!(changes[1].after, "B");
        assert!(line_changes("a", &[edit(9, 0, 0, "x")]).is_err());
    }

    #[test]
    fn preview_reads_closed_files_and_uses_the_open_text() {
        let dir = tempfile::tempdir().unwrap();
        let closed = dir.path().join("Closed.cs");
        let open = dir.path().join("Open.cs");
        std::fs::write(&closed, "Ping();\r\n").unwrap();
        std::fs::write(&open, "on disk\n").unwrap();
        let steps = vec![
            Step::Text {
                path: open.clone(),
                version: None,
                edits: vec![edit(0, 0, 4, "Pong")],
            },
            Step::Text {
                path: closed.clone(),
                version: None,
                edits: vec![edit(0, 0, 4, "Pong")],
            },
        ];
        let texts: HashMap<PathBuf, String> = [(open.clone(), "Ping() unsaved\n".into())].into();
        let (files, total) = preview_files(&steps, &texts).unwrap();
        assert_eq!(total, 2);
        assert_eq!(files[0].path, closed);
        assert!(!files[0].open);
        assert_eq!(files[0].changes[0].after, "Pong();");
        assert!(files[1].open);
        assert_eq!(files[1].changes[0].after, "Pong() unsaved");
    }
}
