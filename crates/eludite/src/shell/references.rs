//! Find All References (brief 0014): Shift+F12 sends `textDocument/references` (declaration included) and fills the
//! Find All References tool window, docked at the bottom beside the Error List.
//!
//! - **Rows** are grouped by project, then file, as Visual Studio groups them; each shows the line's text with the
//!   symbol highlighted and its line and column. Group rows collapse and expand; double-clicking a reference
//!   navigates there (the position before is pushed on the navigation history).
//! - **Line text** is read off the UI thread: open documents from a snapshot of their buffer, other files from disk.
//! - **One search at a time**: a new search replaces the results and cancels a search still running; an answer for
//!   an older solution generation or document version is dropped, never shown (CLAUDE.md invariant 12).
//! - The list is virtualized (`uniform_list`), so a thousand rows cost what the visible ones cost.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::ops::Range;
use std::path::PathBuf;
use std::time::Instant;

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    MAX_REFERENCE_ROWS, ReferenceRow, ReferencesOutput, ReferencesState, WorkspaceOutput,
};
use eludite_docking::ids;
use eludite_editor::text;
use eludite_lsp::lsp;
use eludite_ui::{TREE_ROW_HEIGHT, Theme, TreeRowStyle, highlighted_code, tree_row};
use gpui::{
    AppContext as _, ClickEvent, Context, EventEmitter, InteractiveElement, IntoElement,
    ParentElement, Render, SharedString, StatefulInteractiveElement, Styled, Task, Window, div, px,
    uniform_list,
};
use serde_json::json;

use super::documents::{trace, uri_to_path};
use super::intellisense::{Provider, line_column, lsp_position};
use super::navigation::{NavEntry, document_title};
use super::session::{RequestError, RequestHandle};
use super::{Caret, Shell};

/// The group of files outside every project (Visual Studio's name).
pub const MISCELLANEOUS_FILES: &str = "Miscellaneous Files";

/// One reference, ready to draw. Line and column are 1-based (the column in characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub project: Option<String>,
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    /// The line's text without leading and trailing white space.
    pub text: String,
    /// The symbol in `text` (bytes).
    pub highlight: Range<usize>,
}

/// A row of the window's flattened tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefRow {
    Project {
        name: String,
        count: usize,
        expanded: bool,
    },
    File {
        key: String,
        title: String,
        count: usize,
        expanded: bool,
    },
    /// Index into the references.
    Reference(usize),
}

/// The visible rows for `refs` (already sorted by project, file and position) given the collapsed group keys
/// (project names, and `project|path` for files).
pub fn visible_rows(refs: &[Reference], collapsed: &HashSet<String>) -> Vec<RefRow> {
    let mut rows = Vec::new();
    let mut i = 0;
    while i < refs.len() {
        let project = project_name(&refs[i]);
        let project_end = refs[i..]
            .iter()
            .position(|r| project_name(r) != project)
            .map_or(refs.len(), |n| i + n);
        let expanded = !collapsed.contains(project);
        rows.push(RefRow::Project {
            name: project.to_owned(),
            count: project_end - i,
            expanded,
        });
        let mut j = i;
        while j < project_end {
            let path = &refs[j].path;
            let file_end = refs[j..project_end]
                .iter()
                .position(|r| &r.path != path)
                .map_or(project_end, |n| j + n);
            if expanded {
                let key = file_key(project, path);
                let file_expanded = !collapsed.contains(&key);
                rows.push(RefRow::File {
                    title: document_title(path),
                    key,
                    count: file_end - j,
                    expanded: file_expanded,
                });
                if file_expanded {
                    rows.extend((j..file_end).map(RefRow::Reference));
                }
            }
            j = file_end;
        }
        i = project_end;
    }
    rows
}

fn project_name(r: &Reference) -> &str {
    r.project.as_deref().unwrap_or(MISCELLANEOUS_FILES)
}

fn file_key(project: &str, path: &std::path::Path) -> String {
    format!("{project}|{}", path.display())
}

/// Sort references as the window shows them: by project (files outside every project last), file, then position.
pub fn sort_references(refs: &mut [Reference]) {
    refs.sort_by(|a, b| {
        (
            a.project.is_none(),
            project_name(a),
            &a.path,
            a.line,
            a.column,
        )
            .cmp(&(
                b.project.is_none(),
                project_name(b),
                &b.path,
                b.line,
                b.column,
            ))
    });
}

/// Where a file's text comes from when the rows are built: an open document's buffer, or the disk.
pub enum LineSource {
    Buffer(text::BufferSnapshot),
    Disk,
}

/// The line `row` (0-based) of `text`, without its line break.
fn line_of(text: &str, row: u32) -> Option<&str> {
    text.split('\n')
        .nth(row as usize)
        .map(|l| l.strip_suffix('\r').unwrap_or(l))
}

/// Byte offset of UTF-16 column `utf16` in `line` (clipped).
fn byte_of_utf16(line: &str, utf16: u32) -> usize {
    let mut units = 0;
    for (i, c) in line.char_indices() {
        if units >= utf16 {
            return i;
        }
        units += c.len_utf16() as u32;
    }
    line.len()
}

/// Build the rows for `locations` (off the UI thread): each line's text, trimmed, with the symbol's range in it.
pub fn build_references(
    locations: Vec<(lsp::Location, PathBuf, Option<String>)>,
    mut sources: HashMap<PathBuf, LineSource>,
) -> Vec<Reference> {
    let mut texts: HashMap<PathBuf, Option<String>> = HashMap::new();
    let mut refs: Vec<Reference> = locations
        .into_iter()
        .map(|(loc, path, project)| {
            let text = texts
                .entry(path.clone())
                .or_insert_with(|| match sources.remove(&path) {
                    Some(LineSource::Buffer(snapshot)) => Some(snapshot.text()),
                    _ => std::fs::read_to_string(&path).ok(),
                });
            let start = loc.range.start;
            let line = text
                .as_deref()
                .and_then(|t| line_of(t, start.line))
                .unwrap_or_default();
            let from = byte_of_utf16(line, start.character);
            let to = if loc.range.end.line == start.line {
                byte_of_utf16(line, loc.range.end.character).max(from)
            } else {
                line.len()
            };
            let trimmed_start = line.len() - line.trim_start().len();
            let trimmed = line.trim();
            let shift = |b: usize| b.saturating_sub(trimmed_start).min(trimmed.len());
            Reference {
                project,
                line: start.line + 1,
                column: line[..from].chars().count() as u32 + 1,
                text: trimmed.to_owned(),
                highlight: shift(from)..shift(to),
                path,
            }
        })
        .collect();
    sort_references(&mut refs);
    refs.dedup_by(|a, b| a.path == b.path && a.line == b.line && a.column == b.column);
    refs
}

/// What the window reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReferencesEvent {
    /// Double-click on reference `index`.
    Navigate(usize),
}

/// The window's search state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SearchState {
    Idle,
    Loading,
    Done,
    Failed(String),
}

/// The Find All References tool window.
pub struct ReferencesWindow {
    theme: Theme,
    symbol: String,
    state: SearchState,
    refs: Vec<Reference>,
    collapsed: HashSet<String>,
    rows: Vec<RefRow>,
    selected: Option<usize>,
}

/// Debug selector of visible row `ix`.
pub fn row_selector(ix: usize) -> String {
    format!("reference-row-{ix}")
}

impl ReferencesWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            symbol: String::new(),
            state: SearchState::Idle,
            refs: Vec::new(),
            collapsed: HashSet::new(),
            rows: Vec::new(),
            selected: None,
        }
    }

    pub fn references(&self) -> &[Reference] {
        &self.refs
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[RefRow] {
        &self.rows
    }

    pub fn state(&self) -> &SearchState {
        &self.state
    }

    pub fn symbol(&self) -> &str {
        &self.symbol
    }

    /// The window's header line.
    pub fn header(&self) -> String {
        let n = self.refs.len();
        match &self.state {
            SearchState::Idle => String::new(),
            SearchState::Loading => format!("'{}' references: searching\u{2026}", self.symbol),
            SearchState::Failed(m) => format!("'{}' references: {m}", self.symbol),
            SearchState::Done => format!(
                "'{}' references: {n} result{}",
                self.symbol,
                if n == 1 { "" } else { "s" }
            ),
        }
    }

    /// A new search started: the old results go, as in Visual Studio.
    pub fn start(&mut self, symbol: String, cx: &mut Context<Self>) {
        self.symbol = symbol;
        self.state = SearchState::Loading;
        self.refs.clear();
        self.collapsed.clear();
        self.selected = None;
        self.rows.clear();
        cx.notify();
    }

    pub fn finish(&mut self, refs: Vec<Reference>, cx: &mut Context<Self>) {
        self.refs = refs;
        self.state = SearchState::Done;
        self.rows = visible_rows(&self.refs, &self.collapsed);
        cx.notify();
    }

    pub fn fail(&mut self, message: String, cx: &mut Context<Self>) {
        self.state = SearchState::Failed(message);
        cx.notify();
    }

    fn toggle(&mut self, key: String, cx: &mut Context<Self>) {
        if !self.collapsed.remove(&key) {
            self.collapsed.insert(key);
        }
        self.rows = visible_rows(&self.refs, &self.collapsed);
        self.selected = None;
        cx.notify();
    }

    fn click(&mut self, ix: usize, e: &ClickEvent, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        cx.notify();
        if e.click_count() < 2 {
            return;
        }
        match self.rows.get(ix).cloned() {
            Some(RefRow::Reference(r)) => cx.emit(ReferencesEvent::Navigate(r)),
            Some(RefRow::Project { name, .. }) => self.toggle(name, cx),
            Some(RefRow::File { key, .. }) => self.toggle(key, cx),
            None => {}
        }
    }
}

impl EventEmitter<ReferencesEvent> for ReferencesWindow {}

impl Render for ReferencesWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let header = div()
            .id("references-header")
            .debug_selector(|| "references-header".into())
            .flex()
            .flex_none()
            .items_center()
            .h(px(TREE_ROW_HEIGHT + 4.))
            .px_2()
            .text_size(t.typography.ui)
            .border_b_1()
            .border_color(t.border)
            .child(self.header());
        div()
            .id("find-all-references")
            .debug_selector(|| "find-all-references".into())
            .size_full()
            .flex()
            .flex_col()
            .child(header)
            .child(
                uniform_list(
                    "reference-rows",
                    self.rows.len(),
                    cx.processor(move |this, range: Range<usize>, _, cx| {
                        let t = this.theme;
                        range
                            .filter_map(|ix| {
                                let selected = this.selected == Some(ix);
                                let id = row_selector(ix);
                                let row = match this.rows.get(ix)? {
                                    RefRow::Project {
                                        name,
                                        count,
                                        expanded,
                                    } => {
                                        let key = name.clone();
                                        tree_row(
                                            &t,
                                            id,
                                            None,
                                            format!("{name} ({count})"),
                                            TreeRowStyle {
                                                depth: 0,
                                                disclosure: Some(*expanded),
                                                selected,
                                                muted: false,
                                                bold: false,
                                            },
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle(key.clone(), cx)
                                            }),
                                        )
                                    }
                                    RefRow::File {
                                        key,
                                        title,
                                        count,
                                        expanded,
                                    } => {
                                        let key = key.clone();
                                        tree_row(
                                            &t,
                                            id,
                                            None,
                                            format!("{title} ({count})"),
                                            TreeRowStyle {
                                                depth: 1,
                                                disclosure: Some(*expanded),
                                                selected,
                                                muted: false,
                                                bold: false,
                                            },
                                            cx.listener(move |this, _, _, cx| {
                                                this.toggle(key.clone(), cx)
                                            }),
                                        )
                                    }
                                    RefRow::Reference(r) => {
                                        let r = this.refs.get(*r)?;
                                        tree_row(
                                            &t,
                                            id,
                                            None,
                                            "",
                                            TreeRowStyle {
                                                depth: 2,
                                                disclosure: None,
                                                selected,
                                                muted: false,
                                                bold: false,
                                            },
                                            |_, _, _| {},
                                        )
                                        .child(div().flex_1().min_w_0().overflow_hidden().child(
                                            highlighted_code(
                                                r.text.clone(),
                                                r.highlight.clone(),
                                                &t,
                                            ),
                                        ))
                                        .child(
                                            div()
                                                .flex_none()
                                                .pl_2()
                                                .text_color(if selected {
                                                    t.text_on_accent
                                                } else {
                                                    t.text_muted
                                                })
                                                .child(SharedString::from(format!(
                                                    "({}, {})",
                                                    r.line, r.column
                                                ))),
                                        )
                                    }
                                };
                                Some(row.w_full().cursor_pointer().on_click(
                                    cx.listener(move |this, e, _, cx| this.click(ix, e, cx)),
                                ))
                            })
                            .collect()
                    }),
                )
                .flex_1(),
            )
    }
}

/// The search in flight.
pub struct ReferencesPending {
    handle: RequestHandle,
    ticket: u64,
    _task: Task<()>,
}

/// The window's Find All References state on the shell side.
#[derive(Default)]
pub struct References {
    pub pending: Option<ReferencesPending>,
    next_ticket: u64,
    /// Where the last search started.
    pub origin: Option<NavEntry>,
    pub timings: Vec<ReferencesTiming>,
}

/// When the steps of one search happened (the `--bench-navigate` harness and the report).
#[derive(Debug, Clone, Default)]
pub struct ReferencesTiming {
    pub sent: Option<Instant>,
    pub received: Option<Instant>,
    /// The window holds the rows.
    pub applied: Option<Instant>,
    pub count: usize,
}

impl References {
    /// Cancel the search in flight; true when there was one.
    pub fn cancel(&mut self) -> bool {
        match self.pending.take() {
            Some(p) => {
                p.handle.cancel();
                true
            }
            None => false,
        }
    }
}

impl Shell {
    /// Find All References for the symbol at `at` (or the caret) in document `id`.
    pub(super) fn find_references(
        &mut self,
        id: &str,
        at: Option<Caret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.references.cancel();
        let view = self.documents[id].view.clone();
        let offset = match at {
            Some((line, column)) => {
                super::documents::offset_of(view.read(cx).editor().buffer(), line, column)
            }
            None => view.read(cx).editor().primary_selection().head,
        };
        let (line, column) = line_column(&view, offset, cx);
        let origin = NavEntry {
            path: id.to_owned(),
            line,
            column,
        };
        let symbol = self.word_at(&origin, cx).unwrap_or_default();
        self.references.origin = Some(origin);
        self.references_window
            .update(cx, |w, cx| w.start(symbol, cx));
        // Visual Studio shows the window at once, searching.
        if let Err(e) = self.invoke(
            eludite_commands::view::SHOW,
            json!({ "id": ids::FIND_ALL_REFERENCES }),
            window,
            cx,
        ) {
            eprintln!("eludite: show Find All References: {e}");
        }
        if self.provider(&self.documents[id]) == Provider::Syntax {
            self.references_window.update(cx, |w, cx| {
                w.fail("no language server is available".into(), cx)
            });
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let version = doc.lsp_version;
        let params = lsp::ReferenceParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&doc.sent, offset),
            context: lsp::ReferenceContext {
                include_declaration: true,
            },
        };
        self.references.next_ticket += 1;
        let ticket = self.references.next_ticket;
        trace(format_args!(
            "references request {ticket} at {}:{} version {version}",
            params.position.line, params.position.character
        ));
        let (handle, rx) = doc.session.request::<lsp::References>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn(async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            // `Some(true)`: superseded; `Some(false)`: the newest, but computed for text or a solution that is gone.
            let stale = |shell: &Shell| {
                let newest = shell
                    .references
                    .pending
                    .as_ref()
                    .is_some_and(|p| p.ticket == ticket);
                let doc_version = shell.documents.get(&doc_id).map(|d| d.lsp_version);
                if !newest {
                    Some(true)
                } else if shell.doc_generation(&doc_id) != generation
                    || doc_version != Some(version)
                {
                    Some(false)
                } else {
                    None
                }
            };
            let outdated = |shell: &mut Shell, cx: &mut gpui::App| {
                shell.references.pending = None;
                shell.references_window.update(cx, |w, cx| {
                    w.fail(super::navigation::OUTDATED.to_owned(), cx)
                });
                shell.wake_intellisense_waiters();
            };
            // Decide on the UI thread, then read the lines off it.
            let job = this.update(cx, |shell, cx| {
                if let Some(superseded) = stale(shell) {
                    trace(format_args!("references reply {ticket} dropped (stale)"));
                    if !superseded {
                        outdated(shell, cx);
                    }
                    return None;
                }
                if matches!(reply.result, Err(RequestError::Stale)) {
                    outdated(shell, cx);
                    return None;
                }
                let locations = match reply.result {
                    Ok(l) => l.unwrap_or_default(),
                    Err(RequestError::Canceled | RequestError::Stale) => return None,
                    Err(e) => {
                        shell.references.pending = None;
                        let message = match e {
                            RequestError::NoHost => "no language server is running".to_owned(),
                            RequestError::Failed(m) => m,
                            _ => unreachable!(),
                        };
                        shell
                            .references_window
                            .update(cx, |w, cx| w.fail(message, cx));
                        shell.wake_intellisense_waiters();
                        return None;
                    }
                };
                trace(format_args!(
                    "references reply {ticket}: {} locations (host {:.1} ms)",
                    locations.len(),
                    reply
                        .sent
                        .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
                ));
                Some(shell.reference_inputs(locations, cx))
            });
            let Ok(Some((inputs, sources))) = job else {
                return;
            };
            let refs = cx
                .background_spawn(async move { build_references(inputs, sources) })
                .await;
            let _ = this.update(cx, |shell, cx| {
                if let Some(superseded) = stale(shell) {
                    trace(format_args!("references rows {ticket} dropped (stale)"));
                    if !superseded {
                        outdated(shell, cx);
                    }
                    return;
                }
                shell.references.pending = None;
                let count = refs.len();
                shell
                    .references_window
                    .update(cx, |w, cx| w.finish(refs, cx));
                let timing = ReferencesTiming {
                    sent: reply.sent,
                    received: Some(reply.received),
                    applied: Some(Instant::now()),
                    count,
                };
                if shell.references.timings.len() >= 4096 {
                    shell.references.timings.remove(0);
                }
                shell.references.timings.push(timing);
                shell.wake_intellisense_waiters();
            });
        });
        self.references.pending = Some(ReferencesPending {
            handle,
            ticket,
            _task: task,
        });
    }

    /// The timings of each search answered so far (oldest first).
    pub fn references_timings(&self) -> &[ReferencesTiming] {
        &self.references.timings
    }

    /// Each location with its path and project, and where each file's text comes from.
    #[allow(clippy::type_complexity)]
    pub(super) fn reference_inputs(
        &self,
        locations: Vec<lsp::Location>,
        cx: &gpui::App,
    ) -> (
        Vec<(lsp::Location, PathBuf, Option<String>)>,
        HashMap<PathBuf, LineSource>,
    ) {
        let model = self.explorer.read(cx).model();
        let mut sources = HashMap::new();
        let mut projects: BTreeMap<PathBuf, Option<String>> = BTreeMap::new();
        let inputs = locations
            .into_iter()
            .filter_map(|loc| {
                let path = uri_to_path(&loc.uri)?;
                let project = projects
                    .entry(path.clone())
                    .or_insert_with(|| model.and_then(|m| m.project_of(&path)).map(str::to_owned))
                    .clone();
                if !sources.contains_key(&path) {
                    // An open document's text is what the user sees (and what the server answered for).
                    let open = self.documents.values().find(|d| d.uri == loc.uri);
                    let source = match open {
                        Some(d) => {
                            LineSource::Buffer(d.view.read(cx).editor().buffer().snapshot().clone())
                        }
                        None => LineSource::Disk,
                    };
                    sources.insert(path.clone(), source);
                }
                Some((loc, path, project))
            })
            .collect();
        (inputs, sources)
    }

    /// Double-click on a reference: push where the caret is, then open the reference.
    pub(super) fn navigate_to_reference(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self
            .references_window
            .read(cx)
            .references()
            .get(index)
            .cloned()
        else {
            return;
        };
        if let Some(here) = self.caret_entry(cx) {
            self.navigation.history.push(here);
        }
        if let Err(e) = self.open_at(&r.path.to_string_lossy(), r.line, r.column, window, cx) {
            self.status.set(eludite_ui::slots::STATE, e.to_string());
        }
    }

    /// `eludite.editor.find_references`.
    pub(super) fn find_references_command(
        &mut self,
        path: Option<&str>,
        at: Option<Caret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        self.find_references(&id, at, window, cx);
        Ok(WorkspaceOutput::FindReferences(self.references_output(cx)))
    }

    pub(super) fn references_output(&self, cx: &gpui::App) -> ReferencesOutput {
        let w = self.references_window.read(cx);
        let origin = self.references.origin.clone().unwrap_or(NavEntry {
            path: String::new(),
            line: 1,
            column: 1,
        });
        let (state, message) = match w.state() {
            SearchState::Idle | SearchState::Loading => (ReferencesState::Loading, None),
            SearchState::Done => (ReferencesState::Done, None),
            SearchState::Failed(m) => (ReferencesState::Failed, Some(m.clone())),
        };
        ReferencesOutput {
            path: origin.path,
            line: origin.line,
            column: origin.column,
            state,
            symbol: w.symbol().to_owned(),
            total: w.references().len() as u64,
            references: w
                .references()
                .iter()
                .take(MAX_REFERENCE_ROWS)
                .map(|r| ReferenceRow {
                    project: r.project.clone(),
                    path: r.path.to_string_lossy().into_owned(),
                    line: r.line,
                    column: r.column,
                    text: r.text.clone(),
                })
                .collect(),
            message,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn loc(line: u32, start: u32, end: u32) -> lsp::Location {
        lsp::Location {
            uri: String::new(),
            range: lsp::Range {
                start: lsp::Position {
                    line,
                    character: start,
                },
                end: lsp::Position {
                    line,
                    character: end,
                },
            },
        }
    }

    #[test]
    fn rows_read_the_line_trim_it_and_highlight_the_symbol() {
        let dir = tempfile::tempdir().unwrap();
        let disk = dir.path().join("B.cs");
        std::fs::write(&disk, "class B\r\n{\r\n    Foo f = new Foo();\r\n}\r\n").unwrap();
        let open = dir.path().join("A.cs");
        // An astral character before the symbol: LSP columns are UTF-16.
        let buffer = eludite_editor::Buffer::new("  var \u{1F600} = Foo.Bar;\n");
        let sources =
            HashMap::from([(open.clone(), LineSource::Buffer(buffer.snapshot().clone()))]);
        let refs = build_references(
            vec![
                (loc(2, 16, 19), disk.clone(), Some("App".into())),
                (loc(2, 4, 7), disk.clone(), Some("App".into())),
                (loc(0, 11, 14), open.clone(), None),
            ],
            sources,
        );
        assert_eq!(refs.len(), 3);
        // Sorted: project rows first (App), files outside every project last.
        assert_eq!(refs[0].path, disk);
        assert_eq!((refs[0].line, refs[0].column), (3, 5));
        assert_eq!(refs[0].text, "Foo f = new Foo();");
        assert_eq!(&refs[0].text[refs[0].highlight.clone()], "Foo");
        assert_eq!(refs[1].column, 17);
        assert_eq!(&refs[1].text[refs[1].highlight.clone()], "Foo");
        assert_eq!(refs[2].path, open);
        assert_eq!(&refs[2].text[refs[2].highlight.clone()], "Foo");
        assert_eq!(refs[2].column, 11, "characters, not UTF-16 units");
        assert_eq!(refs[2].text, "var \u{1F600} = Foo.Bar;");
    }

    #[test]
    fn rows_group_by_project_then_file_and_collapse() {
        let r = |project: Option<&str>, path: &str, line| Reference {
            project: project.map(str::to_owned),
            path: PathBuf::from(path),
            line,
            column: 1,
            text: String::new(),
            highlight: 0..0,
        };
        let mut refs = vec![
            r(None, "/x/Loose.cs", 1),
            r(Some("Tests"), "/t/T.cs", 4),
            r(Some("Host"), "/h/B.cs", 9),
            r(Some("Host"), "/h/A.cs", 3),
            r(Some("Host"), "/h/A.cs", 1),
        ];
        sort_references(&mut refs);
        let rows = visible_rows(&refs, &HashSet::new());
        let label = |row: &RefRow| match row {
            RefRow::Project { name, count, .. } => format!("{name} {count}"),
            RefRow::File { title, count, .. } => format!("  {title} {count}"),
            RefRow::Reference(i) => format!("    {}", refs[*i].line),
        };
        assert_eq!(
            rows.iter().map(label).collect::<Vec<_>>(),
            [
                "Host 3",
                "  A.cs 2",
                "    1",
                "    3",
                "  B.cs 1",
                "    9",
                "Tests 1",
                "  T.cs 1",
                "    4",
                "Miscellaneous Files 1",
                "  Loose.cs 1",
                "    1"
            ]
        );
        let collapsed = HashSet::from(["Tests".to_owned(), file_key("Host", Path::new("/h/A.cs"))]);
        let rows = visible_rows(&refs, &collapsed);
        assert_eq!(
            rows.iter().map(label).collect::<Vec<_>>(),
            [
                "Host 3",
                "  A.cs 2",
                "  B.cs 1",
                "    9",
                "Tests 1",
                "Miscellaneous Files 1",
                "  Loose.cs 1",
                "    1"
            ]
        );
    }

    use std::path::Path;
}
