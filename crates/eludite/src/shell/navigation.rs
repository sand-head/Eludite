//! Navigation (brief 0014): Go To Definition (F12, Ctrl+click), metadata as source, the multiple-definitions picker
//! and the window's navigation history (Ctrl+- and Ctrl+Shift+-).
//!
//! - **Go To Definition** sends `textDocument/definition` through the session worker after the document's pending
//!   `didChange`, like the IntelliSense requests. One request is in flight per window: a new one cancels the old one,
//!   and an answer is applied only when it is the newest request's and its solution generation and document version
//!   are still current (CLAUDE.md invariant 12).
//! - **One target** opens its file at the definition through `eludite.file.open`, after the position the request
//!   was made at is pushed on the history. **Several targets** (partial types) open a picker at the caret.
//! - **Metadata as source**: for a symbol in a referenced assembly, the pinned Roslyn decompiles the type into a real
//!   file under `<temp>/MetadataAsSource/` and answers with a plain `file://` URI (host-rpc.md, "Metadata as
//!   source"). The shell opens such a file read-only in a tab titled `Type [from metadata]`, as Visual Studio does,
//!   and sends no document notifications for it.
//! - **History**: per window, at most [`MAX_HISTORY`] positions back; navigating anywhere new clears the forward
//!   list, as in Visual Studio and browsers.

use std::path::{Path, PathBuf};

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    self, DefinitionOutput, DefinitionState, DefinitionTarget, NavigationOutput, WorkspaceOutput,
};
use eludite_lsp::lsp;
use eludite_ui::Theme;
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Render, SharedString,
    StatefulInteractiveElement, Styled, Task, Window, anchored, deferred, div, point, px,
};
use serde_json::json;

use super::documents::{trace, uri_to_path};
use super::intellisense::{Provider, line_column, lsp_position};
use super::session::{RequestError, RequestHandle};
use super::{Caret, Shell};

/// Positions the history keeps behind the current one (per window).
pub const MAX_HISTORY: usize = 50;

/// Targets the picker and the command output list at most.
pub const MAX_TARGETS: usize = 100;

/// Why an answer was dropped although it was the newest (CLAUDE.md invariant 12).
pub const OUTDATED: &str =
    "The document or the solution changed while the language server answered; try again.";

/// Visual Studio's message when there is no definition.
pub const NO_DEFINITION: &str = "Cannot navigate to the symbol under the caret.";

/// A position in a document: its id (the path) and a 1-based line and column.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NavEntry {
    pub path: String,
    pub line: u32,
    pub column: u32,
}

/// The window's navigation history.
#[derive(Debug, Default)]
pub struct NavigationHistory {
    back: Vec<NavEntry>,
    forward: Vec<NavEntry>,
}

impl NavigationHistory {
    /// Remember `here` before navigating somewhere new; the forward list is cleared.
    pub fn push(&mut self, here: NavEntry) {
        if self.back.last() != Some(&here) {
            self.back.push(here);
            if self.back.len() > MAX_HISTORY {
                self.back.remove(0);
            }
        }
        self.forward.clear();
    }

    /// The position to go back to, skipping positions `valid` rejects (a deleted file); `here` (where the caret is
    /// now) becomes the next forward position.
    pub fn back(
        &mut self,
        here: Option<NavEntry>,
        valid: impl Fn(&NavEntry) -> bool,
    ) -> Option<NavEntry> {
        let to = pop_valid(&mut self.back, valid)?;
        if let Some(h) = here {
            self.forward.push(h);
        }
        Some(to)
    }

    /// The position to go forward to, skipping positions `valid` rejects; `here` goes back on the back list.
    pub fn forward(
        &mut self,
        here: Option<NavEntry>,
        valid: impl Fn(&NavEntry) -> bool,
    ) -> Option<NavEntry> {
        let to = pop_valid(&mut self.forward, valid)?;
        if let Some(h) = here {
            self.back.push(h);
            if self.back.len() > MAX_HISTORY {
                self.back.remove(0);
            }
        }
        Some(to)
    }

    pub fn back_len(&self) -> usize {
        self.back.len()
    }

    pub fn forward_len(&self) -> usize {
        self.forward.len()
    }
}

fn pop_valid(list: &mut Vec<NavEntry>, valid: impl Fn(&NavEntry) -> bool) -> Option<NavEntry> {
    while let Some(e) = list.pop() {
        if valid(&e) {
            return Some(e);
        }
    }
    None
}

/// Where the language server's metadata-as-source files live: `<temp>/MetadataAsSource/` (host-rpc.md).
pub fn metadata_root() -> PathBuf {
    std::env::temp_dir().join("MetadataAsSource")
}

/// True for a metadata-as-source file (a decompiled type from a referenced assembly).
pub fn is_metadata_path(path: &Path) -> bool {
    path.starts_with(metadata_root())
}

/// The tab title of `path`: the file name, or `Type [from metadata]` for metadata as source (Visual Studio's
/// wording).
pub fn document_title(path: &Path) -> String {
    if is_metadata_path(path) {
        let stem = path
            .file_stem()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_default();
        return format!("{stem} [from metadata]");
    }
    path.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned())
}

/// A definition the language server returned, in shell terms (1-based line and column).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub path: PathBuf,
    pub line: u32,
    pub column: u32,
    pub metadata: bool,
}

impl Target {
    /// `None` for a URI that is not a file (no such answer from the pinned server; host-rpc.md).
    pub fn from_location(location: &lsp::Location) -> Option<Self> {
        let path = uri_to_path(&location.uri)?;
        Some(Self {
            metadata: is_metadata_path(&path),
            line: location.range.start.line + 1,
            column: location.range.start.character + 1,
            path,
        })
    }

    pub fn title(&self) -> String {
        document_title(&self.path)
    }

    fn output(&self) -> DefinitionTarget {
        DefinitionTarget {
            path: self.path.to_string_lossy().into_owned(),
            line: self.line,
            column: self.column,
            metadata: self.metadata,
            title: Some(self.title()),
        }
    }
}

/// What the last Go To Definition did (for `eludite.editor.go_to_definition` and the tests).
#[derive(Debug, Clone, Default)]
pub struct DefinitionStatus {
    /// The document and 1-based position the request was made at.
    pub origin: Option<NavEntry>,
    pub state: Option<DefinitionState>,
    pub targets: Vec<Target>,
    pub navigated: Option<Target>,
    pub message: Option<String>,
}

/// The Go To Definition request in flight.
pub struct DefinitionPending {
    handle: RequestHandle,
    ticket: u64,
    _task: Task<()>,
}

/// When the steps of one Go To Definition happened (the `--bench-navigate` harness and the report).
#[derive(Debug, Clone, Default)]
pub struct NavigationTiming {
    pub sent: Option<std::time::Instant>,
    pub received: Option<std::time::Instant>,
    /// The caret is at the target (the document may still be loading if it was not open).
    pub applied: Option<std::time::Instant>,
}

/// The window's navigation state.
#[derive(Default)]
pub struct Navigation {
    pub history: NavigationHistory,
    pub definition: Option<DefinitionPending>,
    pub status: DefinitionStatus,
    pub picker: Option<gpui::Entity<DefinitionPicker>>,
    next_ticket: u64,
    pub timings: Vec<NavigationTiming>,
}

impl Navigation {
    pub fn cancel(&mut self) {
        if let Some(p) = self.definition.take() {
            p.handle.cancel();
            if self.status.state == Some(DefinitionState::Loading) {
                self.status.state = Some(DefinitionState::Failed);
                self.status.message = Some(OUTDATED.into());
            }
        }
    }
}

/// What the picker reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickerEvent {
    Chosen(usize),
    Dismissed,
}

/// The list of definitions shown when there are several (partial types, for example): Up, Down, Enter and Escape,
/// or a click.
pub struct DefinitionPicker {
    theme: Theme,
    title: String,
    targets: Vec<Target>,
    selected: usize,
    anchor: Point<Pixels>,
    focus: FocusHandle,
}

/// Debug selector of picker row `ix`.
pub fn picker_row_selector(ix: usize) -> String {
    format!("definition-picker-row-{ix}")
}

impl DefinitionPicker {
    pub fn new(
        theme: Theme,
        title: String,
        targets: Vec<Target>,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self {
            theme,
            title,
            targets,
            selected: 0,
            anchor,
            focus: cx.focus_handle(),
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn targets(&self) -> &[Target] {
        &self.targets
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        match k.key.as_str() {
            "up" => self.selected = self.selected.saturating_sub(1),
            "down" => self.selected = (self.selected + 1).min(self.targets.len().saturating_sub(1)),
            "enter" => cx.emit(PickerEvent::Chosen(self.selected)),
            "escape" => cx.emit(PickerEvent::Dismissed),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EventEmitter<PickerEvent> for DefinitionPicker {}

impl Focusable for DefinitionPicker {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for DefinitionPicker {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let rows = self.targets.iter().enumerate().map(|(ix, target)| {
            let sel = picker_row_selector(ix);
            let selected = ix == self.selected;
            let row = div()
                .id(SharedString::from(sel.clone()))
                .debug_selector(move || sel)
                .flex()
                .flex_row()
                .gap_2()
                .px_2()
                .h(px(20.))
                .items_center()
                .whitespace_nowrap()
                .cursor_pointer()
                .child(format!(
                    "{} ({}, {})",
                    target.title(),
                    target.line,
                    target.column
                ))
                .child(
                    div()
                        .text_color(if selected {
                            t.text_on_accent
                        } else {
                            t.text_muted
                        })
                        .text_size(t.typography.small)
                        .child(target.path.to_string_lossy().into_owned()),
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(PickerEvent::Chosen(ix))));
            if selected {
                row.bg(t.accent).text_color(t.text_on_accent)
            } else {
                row.hover(|s| s.bg(t.menu_hover))
            }
        });
        let panel = eludite_ui::popup::popup_panel(&t)
            .id("definition-picker")
            .debug_selector(|| "definition-picker".into())
            .track_focus(&self.focus)
            .key_context("DefinitionPicker")
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(PickerEvent::Dismissed)))
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(320.))
            .max_w(px(800.))
            .py_1()
            .child(
                div()
                    .px_2()
                    .pb_1()
                    .text_color(t.text_muted)
                    .child(self.title.clone()),
            )
            .children(rows);
        deferred(
            anchored()
                .position(self.anchor)
                .snap_to_window_with_margin(px(4.))
                .child(panel),
        )
        .with_priority(4)
    }
}

impl Shell {
    /// The active document's caret, as a history entry.
    pub(super) fn caret_entry(&self, cx: &App) -> Option<NavEntry> {
        let id = self.controller.active_document()?;
        let doc = self.documents.get(&id)?;
        let caret = doc.view.read(cx).editor().primary_selection().head;
        let (line, column) = line_column(&doc.view, caret, cx);
        Some(NavEntry {
            path: id,
            line,
            column,
        })
    }

    /// Go To Definition for the symbol at `at` (or the caret) in document `id`.
    pub(super) fn go_to_definition(
        &mut self,
        id: &str,
        at: Option<Caret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.navigation.cancel();
        self.close_picker(window, cx);
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
        self.navigation.status = DefinitionStatus {
            origin: Some(origin.clone()),
            state: Some(DefinitionState::Loading),
            ..Default::default()
        };
        if self.provider(&self.documents[id]) == Provider::Syntax {
            self.finish_definition(
                DefinitionState::Failed,
                Some("No language server is available for this document.".into()),
                cx,
            );
            return;
        }
        self.flush_change(id, cx);
        let generation = self.generation;
        let doc = &self.documents[id];
        let version = doc.lsp_version;
        let params = lsp::TextDocumentPositionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            position: lsp_position(&doc.sent, offset),
        };
        self.navigation.next_ticket += 1;
        let ticket = self.navigation.next_ticket;
        trace(format_args!(
            "definition request {ticket} at {}:{} version {version}",
            params.position.line, params.position.character
        ));
        let (handle, rx) = self.session.request::<lsp::GotoDefinition>(params);
        let doc_id = id.to_owned();
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                let current = shell.generation;
                let newest = shell
                    .navigation
                    .definition
                    .as_ref()
                    .is_some_and(|p| p.ticket == ticket);
                let doc_version = shell.documents.get(&doc_id).map(|d| d.lsp_version);
                let outdated = generation != current || doc_version != Some(version);
                if !newest || outdated || matches!(reply.result, Err(RequestError::Stale)) {
                    trace(format_args!(
                        "definition reply {ticket} dropped (stale: newest {newest}, generation {generation}/{current}, version {version}/{doc_version:?})"
                    ));
                    if newest {
                        // Never shown: the answer was computed for text or a solution that is gone.
                        shell.navigation.definition = None;
                        shell.finish_definition(DefinitionState::Failed, Some(OUTDATED.into()), cx);
                        shell.wake_intellisense_waiters();
                    }
                    return;
                }
                shell.navigation.definition = None;
                let mut timing = NavigationTiming {
                    sent: reply.sent,
                    received: Some(reply.received),
                    applied: None,
                };
                match reply.result {
                    Err(RequestError::Canceled | RequestError::Stale) => {}
                    Err(e) => {
                        let message = match e {
                            RequestError::NoHost => "No language server is running.".to_owned(),
                            RequestError::Failed(m) => m,
                            _ => unreachable!(),
                        };
                        shell.finish_definition(DefinitionState::Failed, Some(message), cx);
                    }
                    Ok(result) => {
                        let locations = result
                            .map(lsp::DefinitionResponse::into_locations)
                            .unwrap_or_default();
                        trace(format_args!(
                            "definition reply {ticket}: {} locations (host {:.1} ms)",
                            locations.len(),
                            reply
                                .sent
                                .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
                        ));
                        shell.on_definitions(locations, window, cx);
                        timing.applied = Some(std::time::Instant::now());
                    }
                }
                shell.push_navigation_timing(timing);
                shell.wake_intellisense_waiters();
            });
        });
        self.navigation.definition = Some(DefinitionPending {
            handle,
            ticket,
            _task: task,
        });
    }

    fn push_navigation_timing(&mut self, timing: NavigationTiming) {
        if self.navigation.timings.len() >= 4096 {
            self.navigation.timings.remove(0);
        }
        self.navigation.timings.push(timing);
    }

    /// The timings of each Go To Definition answered so far (oldest first).
    pub fn navigation_timings(&self) -> &[NavigationTiming] {
        &self.navigation.timings
    }

    fn finish_definition(
        &mut self,
        state: DefinitionState,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = &message {
            self.status.set(eludite_ui::slots::STATE, m.clone());
        }
        self.navigation.status.state = Some(state);
        self.navigation.status.message = message;
        cx.notify();
    }

    fn on_definitions(
        &mut self,
        locations: Vec<lsp::Location>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut targets: Vec<Target> = Vec::new();
        let mut unsupported = 0;
        for l in &locations {
            match Target::from_location(l) {
                Some(t) if !targets.contains(&t) => targets.push(t),
                Some(_) => {}
                None => unsupported += 1,
            }
        }
        targets.truncate(MAX_TARGETS);
        self.navigation.status.targets = targets.clone();
        match targets.len() {
            0 if unsupported > 0 => self.finish_definition(
                DefinitionState::Failed,
                Some(format!(
                    "The definition is in a document Eludite cannot open ({}).",
                    locations[0].uri
                )),
                cx,
            ),
            0 => self.finish_definition(DefinitionState::None, Some(NO_DEFINITION.into()), cx),
            1 => self.navigate_to_target(0, window, cx),
            _ => self.open_picker(targets, window, cx),
        }
    }

    /// Navigate to target `index` of the last Go To Definition: push where the request was made, then open the file
    /// at the definition.
    pub(super) fn navigate_to_target(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.navigation.status.targets.get(index).cloned() else {
            return;
        };
        self.close_picker(window, cx);
        if let Some(origin) = self.navigation.status.origin.clone() {
            self.navigation.history.push(origin);
        }
        match self.open_at(
            &target.path.to_string_lossy(),
            target.line,
            target.column,
            window,
            cx,
        ) {
            Ok(()) => {
                self.navigation.status.navigated = Some(target);
                self.finish_definition(DefinitionState::Navigated, None, cx);
                self.status.set(eludite_ui::slots::STATE, "Ready");
            }
            Err(e) => self.finish_definition(DefinitionState::Failed, Some(e.to_string()), cx),
        }
    }

    /// Open `path` at a 1-based position through `eludite.file.open` (the bus records it).
    pub(super) fn open_at(
        &mut self,
        path: &str,
        line: u32,
        column: u32,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<(), CommandError> {
        self.invoke(
            workspace::FILE_OPEN,
            json!({"path": path, "line": line, "column": column}),
            window,
            cx,
        )
        .map(|_| ())
    }

    fn open_picker(&mut self, targets: Vec<Target>, window: &mut Window, cx: &mut Context<Self>) {
        self.finish_definition(DefinitionState::Choose, None, cx);
        // Below the symbol the request was made at, else near the top left of the document area.
        let anchor = self
            .navigation
            .status
            .origin
            .as_ref()
            .and_then(|o| {
                let doc = self.documents.get(&o.path)?;
                let v = doc.view.read(cx);
                let offset = super::documents::offset_of(v.editor().buffer(), o.line, o.column);
                v.pixel_position_for_offset(offset)
                    .map(|p| point(p.x, p.y + v.line_height()))
            })
            .unwrap_or_else(|| point(px(200.), px(120.)));
        let symbol = self
            .navigation
            .status
            .origin
            .as_ref()
            .and_then(|o| self.word_at(o, cx))
            .unwrap_or_default();
        let title = format!("{} definitions of {symbol}", targets.len());
        let theme = self.theme;
        let picker = cx.new(|cx| DefinitionPicker::new(theme, title, targets, anchor, cx));
        cx.subscribe_in(&picker, window, |shell, _, event, window, cx| match event {
            PickerEvent::Chosen(ix) => shell.run(
                workspace::EDITOR_GO_TO_DEFINITION,
                json!({ "target": ix }),
                window,
                cx,
            ),
            PickerEvent::Dismissed => shell.close_picker(window, cx),
        })
        .detach();
        picker.focus_handle(cx).focus(window, cx);
        self.navigation.picker = Some(picker);
        cx.notify();
    }

    pub(super) fn close_picker(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.navigation.picker.take().is_some() {
            if let Some(id) = self.controller.active_document()
                && let Some(doc) = self.documents.get(&id)
            {
                doc.view.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    /// The identifier at `entry` (for the picker and Find All References titles).
    pub(super) fn word_at(&self, entry: &NavEntry, cx: &App) -> Option<String> {
        let doc = self.documents.get(&entry.path)?;
        let buffer = doc.view.read(cx).editor().buffer();
        let row = entry.line.checked_sub(1)?;
        if row >= buffer.line_count() {
            return None;
        }
        let line = buffer.line(row);
        let chars: Vec<char> = line.chars().collect();
        let at = (entry.column as usize).saturating_sub(1).min(chars.len());
        let ident = |c: char| c.is_alphanumeric() || c == '_';
        let mut start = at;
        while start > 0 && ident(chars[start - 1]) {
            start -= 1;
        }
        let mut end = at;
        while end < chars.len() && ident(chars[end]) {
            end += 1;
        }
        (start < end).then(|| chars[start..end].iter().collect())
    }

    /// `eludite.editor.go_to_definition`.
    pub(super) fn go_to_definition_command(
        &mut self,
        path: Option<&str>,
        at: Option<Caret>,
        target: Option<usize>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        if let Some(ix) = target {
            if self.navigation.status.state != Some(DefinitionState::Choose) {
                return Err(CommandError::Failed(
                    "no Go To Definition picker is open".into(),
                ));
            }
            if ix >= self.navigation.status.targets.len() {
                return Err(CommandError::InvalidInput(format!(
                    "`target` {ix} is out of range ({} targets)",
                    self.navigation.status.targets.len()
                )));
            }
            self.navigate_to_target(ix, window, cx);
        } else {
            let id = self.document_id(path)?;
            self.go_to_definition(&id, at, window, cx);
        }
        Ok(WorkspaceOutput::GoToDefinition(self.definition_output()))
    }

    pub(super) fn definition_output(&self) -> DefinitionOutput {
        let s = &self.navigation.status;
        let origin = s.origin.clone().unwrap_or(NavEntry {
            path: String::new(),
            line: 1,
            column: 1,
        });
        DefinitionOutput {
            path: origin.path,
            line: origin.line,
            column: origin.column,
            state: s.state.unwrap_or(DefinitionState::Loading),
            targets: s.targets.iter().map(Target::output).collect(),
            navigated: s.navigated.as_ref().map(Target::output),
            message: s.message.clone(),
        }
    }

    /// `eludite.navigation.back` and `forward`.
    pub(super) fn navigate_history(
        &mut self,
        back: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let here = self.caret_entry(cx);
        // A position in a file that no longer exists is skipped.
        let open: std::collections::HashSet<String> = self.documents.keys().cloned().collect();
        let valid = |e: &NavEntry| open.contains(&e.path) || Path::new(&e.path).is_file();
        let to = if back {
            self.navigation.history.back(here, valid)
        } else {
            self.navigation.history.forward(here, valid)
        };
        let navigated = match to {
            Some(to) => {
                self.open_at(&to.path, to.line, to.column, window, cx)?;
                Some(to)
            }
            None => None,
        };
        Ok(WorkspaceOutput::Navigation(NavigationOutput {
            navigated: navigated.is_some(),
            path: navigated.as_ref().map(|n| n.path.clone()),
            line: navigated.as_ref().map(|n| n.line),
            column: navigated.as_ref().map(|n| n.column),
            back: self.navigation.history.back_len() as u32,
            forward: self.navigation.history.forward_len() as u32,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(line: u32) -> NavEntry {
        NavEntry {
            path: "/a.cs".into(),
            line,
            column: 1,
        }
    }

    #[test]
    fn history_walks_back_and_forward_and_caps_at_fifty() {
        let all = |_: &NavEntry| true;
        let mut h = NavigationHistory::default();
        assert_eq!(h.back(Some(at(1)), all), None);
        h.push(at(1));
        h.push(at(2));
        // Back from 3 goes to 2; 3 becomes forward.
        assert_eq!(h.back(Some(at(3)), all), Some(at(2)));
        assert_eq!(h.back(Some(at(2)), all), Some(at(1)));
        assert_eq!((h.back_len(), h.forward_len()), (0, 2));
        assert_eq!(h.forward(Some(at(1)), all), Some(at(2)));
        assert_eq!(h.forward(Some(at(2)), all), Some(at(3)));
        assert_eq!(h.forward(Some(at(3)), all), None);
        // A new navigation clears the forward list.
        h.back(Some(at(3)), all);
        h.push(at(10));
        assert_eq!(h.forward_len(), 0);
        // The same position twice is kept once.
        h.push(at(10));
        assert_eq!(h.back_len(), 2);
        // Positions `valid` rejects (a deleted file) are skipped.
        assert_eq!(h.back(None, |e| e.line != 10), Some(at(1)));
        h.push(at(5));
        for i in 0..100 {
            h.push(at(100 + i));
        }
        assert_eq!(h.back_len(), MAX_HISTORY);
        assert_eq!(h.back(None, all), Some(at(199)));
        let mut oldest = None;
        while let Some(e) = h.back(None, all) {
            oldest = Some(e);
        }
        assert_eq!(oldest, Some(at(150)), "the oldest positions were dropped");
    }

    #[test]
    fn metadata_files_get_visual_studio_titles() {
        let meta = metadata_root()
            .join("006fad54")
            .join("DecompilationMetadataAsSourceFileProvider")
            .join("c19c8186")
            .join("JsonRpc.cs");
        assert!(is_metadata_path(&meta));
        assert_eq!(document_title(&meta), "JsonRpc [from metadata]");
        let src = Path::new("/src/App/Program.cs");
        assert!(!is_metadata_path(src));
        assert_eq!(document_title(src), "Program.cs");
        let uri = super::super::documents::path_to_uri(&meta);
        let t = Target::from_location(&lsp::Location {
            uri,
            range: lsp::Range {
                start: lsp::Position {
                    line: 30,
                    character: 13,
                },
                end: lsp::Position {
                    line: 30,
                    character: 20,
                },
            },
        })
        .unwrap();
        assert!(t.metadata);
        assert_eq!((t.line, t.column), (31, 14));
        assert_eq!(t.path, meta);
        assert!(
            Target::from_location(&lsp::Location {
                uri: "roslyn-source-generated://x/y.cs".into(),
                range: lsp::Range {
                    start: lsp::Position {
                        line: 0,
                        character: 0
                    },
                    end: lsp::Position {
                        line: 0,
                        character: 0
                    },
                },
            })
            .is_none()
        );
    }
}
