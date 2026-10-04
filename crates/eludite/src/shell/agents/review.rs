//! Pending changes (brief 0016): an agent's edits held for review, one per file, until the user accepts or rejects
//! them.
//!
//! **What becomes a pending change** (the interception boundary):
//! - An Eludite edit command an agent calls through MCP: `eludite.workspace.apply_edit`, `eludite.editor.rename` with
//!   `apply`, `eludite.editor.apply_code_action`. They all end in the workspace-edit applier (brief 0015); while the
//!   agent's call runs, the applier's next edit is captured instead of applied ([`Shell::capture_edit`]), split per
//!   file, and the agent's MCP call waits (on its own thread) until every file is decided, then answers with the
//!   outcome.
//! - An edit the agent makes with its own file tool when the agent asks permission first (ACP
//!   `session/request_permission` for a call of kind `edit` carrying a `diff`, as Claude's `Write` and `Edit` do): the
//!   request is held until review. Accept allows the agent's write (and brings an open buffer up to date through the
//!   applier); Reject denies it, so the agent is told.
//!
//! Accepting goes through the applier as one undo step per open document, with the versions and solution generation
//! the edit was proposed at (a document changed since refuses it, and the change is `failed`). The editor marks the
//! lines a pending change touches in its gutter ([`GutterMarkers`]). Each decision is joined to the agent call's
//! audit entry.

use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Instant;

use eludite_commands::workspace::{ApplySummaryOutput, MAX_APPLY_PATHS};
use eludite_commands::{Caller, EditRecord, EditState};
use eludite_editor::EditorView;
use eludite_lsp::lsp;
use eludite_ui::Theme;
use eludite_ui::diff::{DiffLine, counts, diff_lines, diff_row, hunks};
use gpui::{
    AppContext as _, Context, Entity, EventEmitter, FontWeight, InteractiveElement, IntoElement,
    ParentElement, Render, ScrollStrategy, SharedString, StatefulInteractiveElement, Styled,
    UniformListScrollHandle, Window, div, px, rgb, uniform_list,
};
use serde_json::{Value, json};

use super::super::Shell;
use super::super::documents::{normalize_path, path_to_uri, uri_to_path};
use super::super::workspace_edit::{ApplyOptions, ApplySummary};

/// Document tab ids of review views start with this.
pub const REVIEW_PREFIX: &str = "agent-change:";

pub fn review_tab(id: u64) -> String {
    format!("{REVIEW_PREFIX}{id}")
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChangeState {
    Pending,
    Applying,
    Accepted,
    Rejected,
    Failed(String),
}

impl ChangeState {
    pub fn label(&self) -> &'static str {
        match self {
            ChangeState::Pending => "pending",
            ChangeState::Applying => "applying",
            ChangeState::Accepted => "accepted",
            ChangeState::Rejected => "rejected",
            ChangeState::Failed(_) => "failed",
        }
    }

    pub fn decided(&self) -> bool {
        matches!(
            self,
            ChangeState::Accepted | ChangeState::Rejected | ChangeState::Failed(_)
        )
    }

    fn audit(&self) -> EditState {
        match self {
            ChangeState::Pending | ChangeState::Applying => EditState::Pending,
            ChangeState::Accepted => EditState::Accepted,
            ChangeState::Rejected => EditState::Rejected,
            ChangeState::Failed(_) => EditState::Failed,
        }
    }
}

/// Where a pending change came from, and so how it is accepted.
#[derive(Debug, Clone)]
pub enum ChangeSource {
    /// This file's part of a workspace edit an Eludite command produced for the agent.
    Edit {
        edit: lsp::WorkspaceEdit,
        versions: HashMap<String, i32>,
        generation: u64,
        label: String,
    },
    /// The agent's own file tool, waiting at its permission request `key` (of agent session `generation`).
    AgentTool { key: u64, generation: u64 },
}

#[derive(Debug, Clone)]
pub struct PendingChange {
    pub id: u64,
    pub path: PathBuf,
    /// The agent tool call (audit join).
    pub call: u64,
    /// The agent's id for the tool call (the transcript row).
    pub tool_call: Option<String>,
    pub source: ChangeSource,
    pub edits: u64,
    pub old_text: Option<String>,
    pub new_text: Option<String>,
    pub diff: Option<Arc<Vec<DiffLine>>>,
    pub state: ChangeState,
    pub summary: Option<ApplySummaryOutput>,
}

impl PendingChange {
    pub fn file_name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    /// "+2 -1", or the state.
    pub fn short_summary(&self) -> String {
        match &self.diff {
            Some(d) => {
                let (added, removed) = counts(d);
                format!("+{added} -{removed}")
            }
            None => "\u{2026}".into(),
        }
    }
}

/// What the agent's MCP call is answered with, per change.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub id: u64,
    pub path: String,
    pub state: ChangeState,
    pub summary: Option<ApplySummaryOutput>,
}

/// Shared with the MCP endpoint's call threads: they wait here until the user decides.
#[derive(Debug, Default)]
pub struct ReviewBoard {
    changes: Mutex<BTreeMap<u64, Decided>>,
    /// Calls whose changes are all known (the capture finished).
    calls: Mutex<HashMap<u64, Vec<u64>>>,
    changed: Condvar,
}

impl ReviewBoard {
    fn add(&self, call: u64, d: Decided) {
        self.calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(call)
            .or_default()
            .push(d.id);
        self.changes
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(d.id, d);
        self.changed.notify_all();
    }

    fn set(&self, id: u64, state: ChangeState, summary: Option<ApplySummaryOutput>) {
        let mut changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(d) = changes.get_mut(&id) {
            d.state = state;
            d.summary = summary;
        }
        self.changed.notify_all();
    }

    /// Block until every change of `call` is decided; empty when the call proposed none.
    pub fn wait_decided(&self, call: u64) -> Vec<Decided> {
        let ids = self
            .calls
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(&call)
            .cloned()
            .unwrap_or_default();
        if ids.is_empty() {
            return Vec::new();
        }
        let mut changes = self.changes.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            let all: Vec<Decided> = ids.iter().filter_map(|i| changes.get(i).cloned()).collect();
            if all.iter().all(|d| d.state.decided()) {
                return all;
            }
            changes = self
                .changed
                .wait(changes)
                .unwrap_or_else(|e| e.into_inner());
        }
    }
}

/// The outcome text the agent reads.
pub fn outcome_message(decided: &[Decided]) -> String {
    let names = |pred: &dyn Fn(&ChangeState) -> bool| {
        decided
            .iter()
            .filter(|d| pred(&d.state))
            .map(|d| {
                Path::new(&d.path)
                    .file_name()
                    .map_or(d.path.clone(), |n| n.to_string_lossy().into_owned())
            })
            .collect::<Vec<_>>()
    };
    let mut parts = Vec::new();
    let accepted = names(&|s| *s == ChangeState::Accepted);
    if !accepted.is_empty() {
        parts.push(format!("Accepted by the user: {}.", accepted.join(", ")));
    }
    let rejected = names(&|s| *s == ChangeState::Rejected);
    if !rejected.is_empty() {
        parts.push(format!(
            "Rejected by the user (discarded, not applied): {}.",
            rejected.join(", ")
        ));
    }
    for d in decided {
        if let ChangeState::Failed(why) = &d.state {
            parts.push(format!("Not applied, {}: {why}", d.path));
        }
    }
    parts.join(" ")
}

/// The accepted changes' summaries, merged.
pub fn merged_summary(decided: &[Decided]) -> ApplySummaryOutput {
    let mut out = ApplySummaryOutput {
        applied: !decided.is_empty() && decided.iter().all(|d| d.state == ChangeState::Accepted),
        ..Default::default()
    };
    let mut paths = std::collections::BTreeSet::new();
    for s in decided.iter().filter_map(|d| d.summary.as_ref()) {
        out.files += s.files;
        out.edits += s.edits;
        out.open_documents += s.open_documents;
        out.files_on_disk += s.files_on_disk;
        out.created += s.created;
        out.renamed += s.renamed;
        out.deleted += s.deleted;
        paths.extend(s.paths.iter().cloned());
    }
    out.paths = paths.into_iter().take(MAX_APPLY_PATHS).collect();
    let message = outcome_message(decided);
    out.message = (!message.is_empty()).then_some(message);
    out
}

/// The command's output once its changes are decided: the same command's output shape (its `state`, `summary` and
/// `message`), so it still follows the command's output schema.
pub fn amend_output(command: &str, mut output: Value, decided: &[Decided]) -> Value {
    use eludite_commands::workspace::{
        EDITOR_APPLY_CODE_ACTION, EDITOR_RENAME, WORKSPACE_APPLY_EDIT,
    };
    let summary = merged_summary(decided);
    let all = summary.applied;
    let any = decided.iter().any(|d| d.state == ChangeState::Accepted);
    match command {
        WORKSPACE_APPLY_EDIT => {
            let state = if any { "applied" } else { "failed" };
            let mut v = serde_json::to_value(&summary).unwrap_or_default();
            if let Some(o) = v.as_object_mut() {
                o.insert("state".into(), json!(state));
            }
            v
        }
        EDITOR_RENAME | EDITOR_APPLY_CODE_ACTION => {
            if let Some(o) = output.as_object_mut() {
                let state = match (command, all) {
                    (_, true) => "applied",
                    (EDITOR_RENAME, false) => "rejected",
                    _ => "failed",
                };
                o.insert("state".into(), json!(state));
                o.insert(
                    "summary".into(),
                    serde_json::to_value(&summary).unwrap_or_default(),
                );
                if let Some(m) = &summary.message {
                    o.insert("message".into(), json!(m));
                }
            }
            output
        }
        _ => output,
    }
}

/// Apply LSP `edits` (UTF-16 positions) to `text`.
pub fn apply_text_edits(text: &str, edits: &[lsp::TextEdit]) -> Result<String, String> {
    let line_starts: Vec<usize> = std::iter::once(0)
        .chain(text.match_indices('\n').map(|(i, _)| i + 1))
        .collect();
    let offset = |p: lsp::Position| -> Result<usize, String> {
        let start = *line_starts
            .get(p.line as usize)
            .ok_or_else(|| format!("line {} is past the end", p.line + 1))?;
        let end = line_starts
            .get(p.line as usize + 1)
            .map_or(text.len(), |e| e - 1);
        let line = &text[start..end];
        let mut units = 0u32;
        for (i, c) in line.char_indices() {
            if units >= p.character {
                return Ok(start + i);
            }
            units += c.len_utf16() as u32;
        }
        Ok(end)
    };
    let mut ranges = edits
        .iter()
        .map(|e| {
            Ok((
                offset(e.range.start)?,
                offset(e.range.end)?,
                e.new_text.as_str(),
            ))
        })
        .collect::<Result<Vec<_>, String>>()?;
    ranges.sort_by_key(|r| r.0);
    for w in ranges.windows(2) {
        if w[0].1 > w[1].0 {
            return Err("overlapping edits".into());
        }
    }
    let mut out = text.to_owned();
    for (start, end, new) in ranges.into_iter().rev() {
        out.replace_range(start..end.max(start), new);
    }
    Ok(out)
}

/// One file's part of `edit`, in order: (path, its sub-edit, its text edits).
fn split_edit(edit: &lsp::WorkspaceEdit) -> Result<Vec<(PathBuf, lsp::WorkspaceEdit)>, String> {
    let path = |uri: &str| {
        uri_to_path(uri)
            .map(|p| normalize_path(&p))
            .ok_or_else(|| format!("{uri} is not a file URI"))
    };
    let changes: Vec<lsp::DocumentChange> = match (&edit.document_changes, &edit.changes) {
        (Some(dc), _) => dc.clone(),
        (None, Some(map)) => map
            .iter()
            .map(|(uri, edits)| {
                lsp::DocumentChange::Edit(lsp::TextDocumentEdit {
                    text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                        uri: uri.clone(),
                        version: None,
                    },
                    edits: edits.clone(),
                })
            })
            .collect(),
        (None, None) => Vec::new(),
    };
    let mut out: Vec<(PathBuf, lsp::WorkspaceEdit)> = Vec::new();
    for change in changes {
        let p = match &change {
            lsp::DocumentChange::Edit(e) => path(&e.text_document.uri)?,
            lsp::DocumentChange::Operation(op) => match op {
                lsp::ResourceOperation::Create { uri, .. }
                | lsp::ResourceOperation::Delete { uri, .. } => path(uri)?,
                lsp::ResourceOperation::Rename { old_uri, .. } => path(old_uri)?,
            },
        };
        match out.iter_mut().find(|(q, _)| *q == p) {
            Some((_, e)) => e.document_changes.get_or_insert_with(Vec::new).push(change),
            None => out.push((
                p,
                lsp::WorkspaceEdit {
                    document_changes: Some(vec![change]),
                    ..Default::default()
                },
            )),
        }
    }
    Ok(out)
}

/// The text edits and resource operations of a one-file edit, applied to `old` (the file's text, empty when it does
/// not exist).
fn preview(old: &str, edit: &lsp::WorkspaceEdit) -> Result<(String, u64), String> {
    let mut text = old.to_owned();
    let mut n = 0u64;
    for change in edit.document_changes.iter().flatten() {
        match change {
            lsp::DocumentChange::Edit(e) => {
                text = apply_text_edits(&text, &e.edits)?;
                n += e.edits.len() as u64;
            }
            lsp::DocumentChange::Operation(lsp::ResourceOperation::Delete { .. }) => {
                text.clear();
            }
            lsp::DocumentChange::Operation(_) => {}
        }
    }
    Ok((text, n))
}

/// Events from a review view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReviewEvent {
    Decide { change: u64, accept: bool },
}

pub fn review_button(change: u64, accept: bool) -> String {
    format!(
        "review-{change}-{}",
        if accept { "accept" } else { "reject" }
    )
}

/// The review view: one pending change as an inline diff, with Accept and Reject. Rows are virtualized, so a
/// 2000-line file costs what its visible rows cost.
pub struct DiffView {
    theme: Theme,
    pub change: u64,
    title: String,
    pub lines: Option<Arc<Vec<DiffLine>>>,
    pub state: ChangeState,
    mono: SharedString,
    /// When it was opened, and the frame that first showed the diff (the render budget).
    pub opened: Instant,
    pub first_diff_frame_ms: Option<f64>,
    painted: super::window::Painted,
    scroll: UniformListScrollHandle,
    /// Scrolled to the first change once.
    scrolled: bool,
}

impl EventEmitter<ReviewEvent> for DiffView {}

impl DiffView {
    pub fn new(theme: Theme, change: &PendingChange, painted: super::window::Painted) -> Self {
        Self {
            theme,
            change: change.id,
            title: format!("{} \u{2014} change #{}", change.path.display(), change.id),
            lines: change.diff.clone(),
            state: change.state.clone(),
            mono: eludite_editor::default_font_family(),
            opened: Instant::now(),
            first_diff_frame_ms: None,
            painted,
            scroll: UniformListScrollHandle::new(),
            scrolled: false,
        }
    }
}

impl Render for DiffView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        if self.lines.is_some() && self.first_diff_frame_ms.is_none() {
            let opened = self.opened;
            cx.defer_in_frame_end(opened);
        }
        let (added, removed) = self.lines.as_deref().map(|l| counts(l)).unwrap_or_default();
        let id = self.change;
        let pending = self.state == ChangeState::Pending;
        let painted = self.painted.clone();
        let button = |accept: bool, label: &'static str| {
            super::window::tracked(
                &painted,
                review_button(id, accept),
                eludite_ui::push_button(review_button(id, accept), label, accept, pending, &t),
            )
            .on_click(cx.listener(move |this, _, _, cx| {
                if this.state == ChangeState::Pending {
                    cx.emit(ReviewEvent::Decide {
                        change: this.change,
                        accept,
                    })
                }
            }))
        };
        let header = div()
            .flex()
            .flex_none()
            .items_center()
            .gap_2()
            .p_1()
            .border_b_1()
            .border_color(t.border)
            .bg(t.panel)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .font_weight(FontWeight::SEMIBOLD)
                    .child(SharedString::from(self.title.clone())),
            )
            .child(
                div()
                    .text_color(rgb(0x89D185))
                    .child(SharedString::from(format!("+{added}"))),
            )
            .child(
                div()
                    .text_color(rgb(0xF48771))
                    .child(SharedString::from(format!("-{removed}"))),
            )
            .child(
                div()
                    .text_color(t.text_muted)
                    .child(SharedString::from(self.state.label())),
            )
            .child(button(true, "Accept"))
            .child(button(false, "Reject"));
        // Open at the first change, with a little context above it.
        if !self.scrolled
            && let Some(lines) = &self.lines
        {
            self.scrolled = true;
            if let Some(first) = lines
                .iter()
                .position(|l| l.kind != eludite_ui::diff::DiffKind::Same)
            {
                self.scroll
                    .scroll_to_item(first.saturating_sub(5), ScrollStrategy::Top);
            }
        }
        let body = match self.lines.clone() {
            None => div()
                .p_2()
                .text_color(t.text_muted)
                .child("Computing the diff\u{2026}")
                .into_any_element(),
            Some(lines) => {
                let mono = self.mono.clone();
                uniform_list(
                    "agent-change-diff",
                    lines.len(),
                    cx.processor(move |this, range: std::ops::Range<usize>, _, _| {
                        range
                            .filter_map(|ix| {
                                lines
                                    .get(ix)
                                    .map(|l| diff_row(l, &this.theme, mono.clone()))
                            })
                            .collect()
                    }),
                )
                .track_scroll(&self.scroll)
                .flex_1()
                .into_any_element()
            }
        };
        div()
            .id(SharedString::from(review_tab(id)))
            .debug_selector(move || review_tab(id))
            .size_full()
            .flex()
            .flex_col()
            .bg(t.background)
            .text_color(t.text)
            .text_size(t.typography.ui)
            .child(header)
            .child(body)
    }
}

/// Defer helper: record when the first frame showing the diff was presented.
trait FrameEnd {
    fn defer_in_frame_end(&mut self, opened: Instant);
}

impl FrameEnd for Context<'_, DiffView> {
    fn defer_in_frame_end(&mut self, opened: Instant) {
        let this = self.entity().downgrade();
        self.defer(move |cx| {
            let _ = this.update(cx, |v, _| {
                v.first_diff_frame_ms
                    .get_or_insert(opened.elapsed().as_secs_f64() * 1e3);
            });
        });
    }
}

/// The pending-change marks in an editor's gutter: a bar beside each run of lines a pending change touches (an
/// insertion is a short bar at its line). It follows the editor's scrolling.
pub struct GutterMarkers {
    view: Entity<EditorView>,
    /// (first row, rows); 0 rows is an insertion before that row.
    pub rows: Vec<(u32, u32)>,
    _observe: gpui::Subscription,
}

/// The gutter marker's color: Visual Studio's pending-change amber.
pub const PENDING_MARK: u32 = 0xD7BA7D;

impl GutterMarkers {
    pub fn new(view: Entity<EditorView>, rows: Vec<(u32, u32)>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&view, |_, _, cx| cx.notify());
        Self {
            view,
            rows,
            _observe: observe,
        }
    }
}

impl Render for GutterMarkers {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let v = self.view.read(cx);
        let lh = v.line_height();
        let top = v.scroll_position().y;
        div()
            .absolute()
            .top_0()
            .left_0()
            .size_full()
            .children(self.rows.iter().enumerate().map(|(i, &(row, len))| {
                let sel = format!("pending-mark-{i}");
                div()
                    .debug_selector(move || sel)
                    .absolute()
                    .left_0()
                    // Placed by the editor's layout, which counts CodeLens rows (brief 0052).
                    .top(v.row_top(row) - top)
                    .w(px(4.))
                    .h(if len == 0 {
                        lh * 0.35
                    } else {
                        v.row_bottom(row + len - 1) - v.row_top(row)
                    })
                    .bg(rgb(PENDING_MARK))
            }))
    }
}

impl Shell {
    /// Hold `edit` (computed by an Eludite command an agent called) as pending changes, one per file. Returns their
    /// ids. Nothing is applied.
    pub(in crate::shell) fn capture_edit(
        &mut self,
        edit: &lsp::WorkspaceEdit,
        options: &ApplyOptions,
        caller: &Caller,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<Vec<u64>, String> {
        // An agent's call is joined to its audit entry; the person's Replace All (brief 0042) gets a call of its own.
        let (call, tool_call) = match caller {
            Caller::Agent {
                call, tool_call, ..
            } => (*call, tool_call.clone()),
            _ => (eludite_commands::next_call_id(), None),
        };
        let (call, tool_call) = (&call, &tool_call);
        let parts = split_edit(edit)?;
        let label = options.label.clone().unwrap_or_else(|| "Agent edit".into());
        let mut ids = Vec::new();
        for (path, sub) in parts {
            let id = self.agents.next_change();
            let open = self.open_document_at(&path);
            let mut versions = options.versions.clone();
            let mut old_text = None;
            if let Some(doc_id) = &open
                && let Some(doc) = self.documents.get(doc_id)
            {
                // Proposed against what the language server saw; accepting refuses if the document moved on.
                versions.entry(doc_id.clone()).or_insert(doc.lsp_version);
                old_text = Some(doc.sent.text());
            }
            let change = PendingChange {
                id,
                path: path.clone(),
                call: *call,
                tool_call: tool_call.clone(),
                source: ChangeSource::Edit {
                    edit: sub.clone(),
                    versions,
                    generation: options.generation.unwrap_or(self.generation),
                    label: label.clone(),
                },
                edits: 0,
                old_text: None,
                new_text: None,
                diff: None,
                state: ChangeState::Pending,
                summary: None,
            };
            self.add_change(change, cx);
            // The texts and the diff off the UI thread (a closed file is read from disk).
            let task = cx.background_spawn(async move {
                let old = match old_text {
                    Some(t) => t,
                    None => std::fs::read_to_string(&path).unwrap_or_default(),
                };
                let (new, edits) = preview(&old, &sub)?;
                let lines = diff_lines(&old, &new);
                Ok::<_, String>((old, new, edits, lines))
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.set_diff(id, result, window, cx)
                });
            })
            .detach();
            ids.push(id);
        }
        Ok(ids)
    }

    /// The agent's own file tool asked to write: hold its permission request `key` as pending changes (one per
    /// file the call's diffs name).
    pub(in crate::shell) fn capture_tool_edit(
        &mut self,
        key: u64,
        request: &eludite_acp::protocol::RequestPermissionRequest,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let diffs = request.tool_call.diffs();
        if diffs.is_empty() {
            return false;
        }
        let call = eludite_commands::next_call_id();
        let tool = request
            .tool_call
            .agent_tool_name()
            .or(request.tool_call.title.as_deref())
            .unwrap_or("edit")
            .to_owned();
        // An agent tool call is not a bus command; it is audited all the same.
        let seq = self.commands.audit_log().record_call(
            &tool,
            Some(eludite_commands::PermissionClass::EditBuffer),
            eludite_commands::Outcome::Ok,
            Caller::Agent {
                agent: self.agents.current_name(),
                call,
                tool_call: Some(request.tool_call.tool_call_id.clone()),
            },
            request.tool_call.raw_input.clone(),
        );
        let tc = request.tool_call.tool_call_id.clone();
        self.agents
            .window
            .update(cx, |w, _| w.transcript.set_audit(&tc, seq));
        let generation = self.agents.generation;
        for d in diffs {
            let id = self.agents.next_change();
            let path = normalize_path(Path::new(&d.path));
            let open_text = self
                .open_document_at(&path)
                .and_then(|doc| self.documents.get(&doc))
                .map(|doc| doc.view.read(cx).editor().text());
            self.add_change(
                PendingChange {
                    id,
                    path: path.clone(),
                    call,
                    tool_call: Some(request.tool_call.tool_call_id.clone()),
                    source: ChangeSource::AgentTool { key, generation },
                    edits: 1,
                    old_text: None,
                    new_text: None,
                    diff: None,
                    state: ChangeState::Pending,
                    summary: None,
                },
                cx,
            );
            let task = cx.background_spawn(async move {
                let current = match open_text {
                    Some(t) => t,
                    None => std::fs::read_to_string(&path).unwrap_or_default(),
                };
                // A write replaces the file; an edit replaces its old text (the first occurrence).
                let new = match &d.old_text {
                    None => d.new_text.clone(),
                    Some(old) if !old.is_empty() && current.contains(old.as_str()) => {
                        current.replacen(old.as_str(), &d.new_text, 1)
                    }
                    Some(old) if old.is_empty() => d.new_text.clone(),
                    Some(_) => return Err("the agent's edit no longer matches the file".to_owned()),
                };
                let lines = diff_lines(&current, &new);
                Ok((current, new, 1, lines))
            });
            cx.spawn_in(window, async move |this, cx| {
                let result = task.await;
                let _ = this.update_in(cx, |shell, window, cx| {
                    shell.set_diff(id, result, window, cx)
                });
            })
            .detach();
        }
        true
    }

    fn add_change(&mut self, change: PendingChange, cx: &mut Context<Self>) {
        let (id, call, path) = (
            change.id,
            change.call,
            change.path.to_string_lossy().into_owned(),
        );
        super::super::documents::trace(format_args!("agents pending change #{id} {path}"));
        self.agents.board.add(
            call,
            Decided {
                id,
                path: path.clone(),
                state: ChangeState::Pending,
                summary: None,
            },
        );
        self.commands.audit_log().attach_edit(
            call,
            EditRecord {
                change: id,
                path: path.clone(),
                edits: change.edits,
                state: EditState::Pending,
                summary: None,
            },
        );
        if let Some(tc) = &change.tool_call {
            let tc = tc.clone();
            self.agents.window.update(cx, |w, cx| {
                w.transcript.change(&tc, id, &path, "pending");
                w.sync(cx);
            });
        }
        self.agents.changes.insert(id, change);
        self.sync_changes(cx);
    }

    fn set_diff(
        &mut self,
        id: u64,
        result: Result<(String, String, u64, Vec<DiffLine>), String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(change) = self.agents.changes.get_mut(&id) else {
            return;
        };
        match result {
            Ok((old, new, edits, lines)) => {
                change.old_text = Some(old);
                change.new_text = Some(new);
                change.edits = edits;
                change.diff = Some(Arc::new(lines));
            }
            Err(e) => {
                change.diff = Some(Arc::new(Vec::new()));
                change.summary = Some(ApplySummaryOutput {
                    message: Some(e),
                    ..Default::default()
                });
            }
        }
        let diff = change.diff.clone();
        if let Some(view) = self.agents.reviews.borrow().get(&review_tab(id)) {
            view.update(cx, |v, cx| {
                v.lines = diff;
                cx.notify();
            });
        }
        self.sync_changes(cx);
        // The first change of a call opens for review.
        let first = self
            .agents
            .changes
            .values()
            .find(|c| c.state == ChangeState::Pending)
            .map(|c| c.id);
        if first == Some(id) && !self.agents.opened_once.contains(&id) {
            self.agents.opened_once.push(id);
            self.open_review(id, window, cx);
        }
    }

    /// Open the review view of change `id` as a document tab.
    pub fn open_review(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(change) = self.agents.changes.get(&id) else {
            return;
        };
        let tab = review_tab(id);
        let theme = self.theme;
        let view = self
            .agents
            .reviews
            .borrow()
            .get(&tab)
            .cloned()
            .unwrap_or_else(|| {
                let painted = self.agents.window.read(cx).painted.clone();
                let view = cx.new(|_| DiffView::new(theme, change, painted));
                cx.subscribe_in(
                    &view,
                    window,
                    // The review view's buttons run `eludite.agents.review`, like the window's.
                    |shell, _, e: &ReviewEvent, window, cx| match *e {
                        ReviewEvent::Decide { change, accept } => shell.run(
                            eludite_commands::agents::REVIEW,
                            json!({
                                "change": change,
                                "decision": if accept { "accept" } else { "reject" }
                            }),
                            window,
                            cx,
                        ),
                    },
                )
                .detach();
                view
            });
        view.update(cx, |v, _| v.opened = Instant::now());
        self.agents.reviews.borrow_mut().insert(tab.clone(), view);
        self.controller
            .open_document(&tab, &format!("Review: {}", change.file_name()));
        cx.notify();
    }

    /// The window and the editors show the changes as they are now.
    pub(in crate::shell) fn sync_changes(&mut self, cx: &mut Context<Self>) {
        let items: Vec<super::window::ChangeItem> = self
            .agents
            .changes
            .values()
            .map(|c| super::window::ChangeItem {
                id: c.id,
                path: c.path.to_string_lossy().into_owned(),
                summary: c.short_summary(),
                pending: c.state == ChangeState::Pending,
            })
            .collect();
        self.agents.window.update(cx, |w, cx| {
            w.changes = items;
            cx.notify();
        });
        // Gutter marks: the old rows each pending change touches, per open document.
        let mut marks: HashMap<String, Vec<(u32, u32)>> = HashMap::new();
        for c in self.agents.changes.values() {
            if c.state != ChangeState::Pending {
                continue;
            }
            let (Some(doc), Some(lines)) = (self.open_document_at(&c.path), &c.diff) else {
                continue;
            };
            marks
                .entry(doc)
                .or_default()
                .extend(hunks(lines).iter().map(|h| (h.old_start, h.old_len)));
        }
        let mut gutters = self.agents.gutters.borrow_mut();
        gutters.retain(|doc, _| marks.contains_key(doc));
        for (doc, rows) in marks {
            match gutters.get(&doc) {
                Some(g) => g.update(cx, |g, cx| {
                    if g.rows != rows {
                        g.rows = rows;
                        cx.notify();
                    }
                }),
                None => {
                    if let Some(view) = self.documents.get(&doc).map(|d| d.view.clone()) {
                        let g = cx.new(|cx| GutterMarkers::new(view, rows, cx));
                        gutters.insert(doc, g);
                    }
                }
            }
        }
        drop(gutters);
        cx.notify();
    }

    /// The pending changes `change` names (all of them for `None`).
    fn pending_ids(&self, change: Option<u64>) -> Vec<u64> {
        self.agents
            .changes
            .values()
            .filter(|c| c.state == ChangeState::Pending && change.is_none_or(|id| id == c.id))
            .map(|c| c.id)
            .collect()
    }

    /// Accept or reject pending changes `ids`. Accepted edits go through the applier together (one undo step per
    /// open document).
    pub fn decide(
        &mut self,
        ids: &[u64],
        accept: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        if ids.is_empty() {
            return Err("no pending change".into());
        }
        // Replace All's changes are checked against the searched text first (brief 0042).
        let checked;
        let ids = if accept {
            checked = self.search_before_accept(ids, window, cx);
            &checked[..]
        } else {
            ids
        };
        if !accept {
            for &id in ids {
                if let Some(ChangeSource::AgentTool { key, generation }) =
                    self.agents.changes.get(&id).map(|c| c.source.clone())
                {
                    self.answer_tool_change(key, generation, false, id);
                }
                self.finish_change(id, ChangeState::Rejected, None, cx);
            }
            return Ok(());
        }
        // The agent's own tools: allow the write, and bring an open buffer up to date.
        let mut merged = lsp::WorkspaceEdit {
            document_changes: Some(Vec::new()),
            ..Default::default()
        };
        let mut versions = HashMap::new();
        let mut generation = None;
        let mut label = None;
        let mut through_applier = Vec::new();
        for &id in ids {
            let Some(change) = self.agents.changes.get(&id).cloned() else {
                continue;
            };
            match &change.source {
                ChangeSource::Edit {
                    edit,
                    versions: v,
                    generation: g,
                    label: l,
                } => {
                    label.get_or_insert_with(|| format!("Accept: {l}"));
                    merged
                        .document_changes
                        .as_mut()
                        .expect("set")
                        .extend(edit.document_changes.clone().unwrap_or_default());
                    versions.extend(v.clone());
                    generation = Some(*g);
                    through_applier.push(id);
                }
                ChangeSource::AgentTool { key, generation: g } => {
                    self.answer_tool_change(*key, *g, true, id);
                    if let (Some(doc), Some(new)) =
                        (self.open_document_at(&change.path), change.new_text.clone())
                    {
                        let current = self.documents[&doc].view.read(cx).editor().text();
                        if current != new {
                            let end = end_position(&current);
                            let uri = path_to_uri(&change.path);
                            merged.document_changes.as_mut().expect("set").push(
                                lsp::DocumentChange::Edit(lsp::TextDocumentEdit {
                                    text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                                        uri,
                                        version: None,
                                    },
                                    edits: vec![lsp::TextEdit::new(
                                        lsp::Range {
                                            start: lsp::Position {
                                                line: 0,
                                                character: 0,
                                            },
                                            end,
                                        },
                                        new,
                                    )],
                                }),
                            );
                            through_applier.push(id);
                            continue;
                        }
                    }
                    self.finish_change(id, ChangeState::Accepted, None, cx);
                }
            }
        }
        if through_applier.is_empty() {
            return Ok(());
        }
        for &id in &through_applier {
            if let Some(c) = self.agents.changes.get_mut(&id) {
                c.state = ChangeState::Applying;
            }
        }
        let options = ApplyOptions {
            label: Some(label.unwrap_or_else(|| "Accept agent change".into())),
            generation,
            // An agent's edit was checked against the solution generation.
            server: Default::default(),
            versions,
        };
        let ids = through_applier;
        self.apply_workspace_edit(
            &merged,
            options,
            window,
            cx,
            Box::new(move |shell, summary: ApplySummary, _, cx| {
                let out = summary.output();
                for id in ids {
                    let state = if summary.applied {
                        ChangeState::Accepted
                    } else {
                        ChangeState::Failed(
                            summary
                                .message
                                .clone()
                                .unwrap_or_else(|| "the edit was refused".into()),
                        )
                    };
                    shell.finish_change(id, state, Some(out.clone()), cx);
                }
            }),
        );
        Ok(())
    }

    fn answer_tool_change(&mut self, key: u64, generation: u64, allow: bool, id: u64) {
        // Every change of the request must agree; the request is answered once (the first decision).
        let others_pending = self.agents.changes.values().any(|c| {
            c.id != id
                && c.state == ChangeState::Pending
                && matches!(c.source, ChangeSource::AgentTool { key: k, .. } if k == key)
        });
        if allow && others_pending {
            return;
        }
        if generation != self.agents.generation {
            return;
        }
        if let Some(session) = self.agents.session() {
            let kind = if allow {
                eludite_acp::protocol::PermissionOptionKind::AllowOnce
            } else {
                eludite_acp::protocol::PermissionOptionKind::RejectOnce
            };
            let _ = session.answer(key, kind);
        }
    }

    pub(in crate::shell) fn finish_change(
        &mut self,
        id: u64,
        state: ChangeState,
        summary: Option<ApplySummaryOutput>,
        cx: &mut Context<Self>,
    ) {
        let Some(change) = self.agents.changes.get_mut(&id) else {
            return;
        };
        change.state = state.clone();
        if summary.is_some() {
            change.summary = summary.clone();
        }
        let path = change.path.to_string_lossy().into_owned();
        let (call, tool_call, edits) = (change.call, change.tool_call.clone(), change.edits);
        super::super::documents::trace(format_args!(
            "agents change #{id} {} {path}",
            state.label()
        ));
        self.agents.board.set(id, state.clone(), summary.clone());
        let text = match (&state, &summary) {
            (ChangeState::Failed(why), _) => Some(why.clone()),
            (_, Some(s)) => Some(format!("{} edits in {} files", s.edits, s.files)),
            _ => None,
        };
        self.commands.audit_log().attach_edit(
            call,
            EditRecord {
                change: id,
                path: path.clone(),
                edits,
                state: state.audit(),
                summary: text,
            },
        );
        if let Some(tc) = tool_call {
            let label = state.label();
            self.agents.window.update(cx, |w, cx| {
                w.transcript.change(&tc, id, &path, label);
                w.sync(cx);
            });
        }
        if let Some(view) = self.agents.reviews.borrow().get(&review_tab(id)) {
            view.update(cx, |v, cx| {
                v.state = state;
                cx.notify();
            });
        }
        self.sync_changes(cx);
    }

    /// The transcript's link to change `id`: an accepted change opens its file at the first changed line (what was
    /// applied, in the editor); any other opens its review view.
    pub fn open_change(&mut self, id: u64, window: &mut Window, cx: &mut Context<Self>) {
        let Some(change) = self.agents.changes.get(&id) else {
            return;
        };
        if change.state == ChangeState::Accepted && change.path.is_file() {
            let line = change
                .diff
                .as_deref()
                .and_then(|d| hunks(d).first().map(|h| h.new_start + 1))
                .unwrap_or(1);
            let path = change.path.to_string_lossy().into_owned();
            self.run(
                eludite_commands::workspace::FILE_OPEN,
                json!({ "path": path, "line": line, "column": 1 }),
                window,
                cx,
            );
            return;
        }
        self.open_review(id, window, cx);
    }

    /// The pending changes `eludite.agents.review` names, by id or path.
    pub fn changes_for(&self, change: Option<u64>, path: Option<&str>) -> Vec<u64> {
        match (change, path) {
            (Some(id), _) => self.pending_ids(Some(id)),
            (None, Some(p)) => {
                let p = normalize_path(Path::new(p));
                self.agents
                    .changes
                    .values()
                    .filter(|c| c.state == ChangeState::Pending && c.path == p)
                    .map(|c| c.id)
                    .collect()
            }
            (None, None) => self.pending_ids(None),
        }
    }

    /// Reject every pending change (the turn was cancelled).
    pub(in crate::shell) fn reject_pending(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let ids = self.pending_ids(None);
        if !ids.is_empty() {
            let _ = self.decide(&ids, false, window, cx);
        }
    }
}

/// The LSP position of the end of `text`.
fn end_position(text: &str) -> lsp::Position {
    let line = text.matches('\n').count() as u32;
    let last = text.rsplit('\n').next().unwrap_or_default();
    lsp::Position {
        line,
        character: last.encode_utf16().count() as u32,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn text_edits_apply_in_utf16() {
        let text = "a\u{1F600}b\nsecond\n";
        assert_eq!(
            apply_text_edits(text, &[edit(0, 3, 0, 4, "X"), edit(1, 0, 1, 0, "// ")]).unwrap(),
            "a\u{1F600}X\n// second\n"
        );
        assert_eq!(
            apply_text_edits(text, &[edit(9, 0, 9, 0, "x")]).unwrap_err(),
            "line 10 is past the end"
        );
        assert!(apply_text_edits(text, &[edit(0, 0, 0, 2, "x"), edit(0, 1, 0, 3, "y")]).is_err());
        assert_eq!(
            end_position("ab\ncd"),
            lsp::Position {
                line: 1,
                character: 2
            }
        );
    }

    #[test]
    fn workspace_edits_split_per_file_in_order() {
        let a = if cfg!(windows) {
            "file:///C:/s/A.cs"
        } else {
            "file:///s/A.cs"
        };
        let b = if cfg!(windows) {
            "file:///C:/s/B.cs"
        } else {
            "file:///s/B.cs"
        };
        let e: lsp::WorkspaceEdit = serde_json::from_value(json!({"changes": {
            a: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "x"}],
            b: [{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "y"}]
        }}))
        .unwrap();
        let parts = split_edit(&e).unwrap();
        assert_eq!(parts.len(), 2);
        assert!(parts[0].0.ends_with("A.cs"));
        let (new, n) = preview("old\n", &parts[1].1).unwrap();
        assert_eq!((new.as_str(), n), ("yold\n", 1));
    }

    #[test]
    fn outputs_follow_the_command_after_review() {
        let decided = vec![
            Decided {
                id: 1,
                path: "/s/A.cs".into(),
                state: ChangeState::Accepted,
                summary: Some(ApplySummaryOutput {
                    applied: true,
                    files: 1,
                    edits: 1,
                    open_documents: 1,
                    paths: vec!["/s/A.cs".into()],
                    ..Default::default()
                }),
            },
            Decided {
                id: 2,
                path: "/s/B.cs".into(),
                state: ChangeState::Rejected,
                summary: None,
            },
        ];
        let out = amend_output("eludite.workspace.apply_edit", json!({}), &decided);
        assert_eq!(out["state"], "applied");
        assert_eq!(out["applied"], false);
        assert_eq!(out["edits"], 1);
        let m = out["message"].as_str().unwrap();
        assert!(m.contains("Accepted by the user: A.cs."), "{m}");
        assert!(
            m.contains("Rejected by the user (discarded, not applied): B.cs."),
            "{m}"
        );
        let schema: Value = serde_json::from_str(include_str!(
            "../../../../../protocol/schemas/workspace-apply-edit.output.json"
        ))
        .unwrap();
        for k in out.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let rename = amend_output(
            "eludite.editor.rename",
            json!({"state": "failed", "files": []}),
            &decided[1..],
        );
        assert_eq!(rename["state"], "rejected");
        assert_eq!(rename["summary"]["applied"], false);
    }
}
