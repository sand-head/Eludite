//! Quick Actions and Refactorings (brief 0015): the light bulb in the editor's margin and its menu (Ctrl+.).
//!
//! - **The light bulb.** [`LIGHTBULB_DEBOUNCE`] after the caret rests on a new position in a document the language
//!   server handles, `textDocument/codeAction` (trigger kind 2) asks for the actions at the caret, with the
//!   diagnostics shown on its line. When there are any, the bulb appears in the margin of the caret's line: yellow
//!   when there are fixes, blue-gray when there are only refactorings. A newer request cancels the older one; moving
//!   the caret to another line hides the bulb.
//! - **The menu** (Ctrl+., or a click on the bulb) lists the actions as Visual Studio groups them: fixes first, then
//!   refactorings, then the rest. It opens at once from the bulb's answer when that is for the caret's position and
//!   the current text, else after an invoked request (trigger kind 1). Up, Down, Enter, Escape, Right and Left
//!   (nested actions), and clicks.
//! - **Applying** an action resolves it lazily with `codeAction/resolve` when it has no edit, then hands the edit to
//!   the workspace-edit applier with the generation and document versions it was computed for. Roslyn's nested
//!   actions (`roslyn.client.nestedCodeAction`) are shown as a submenu; its Fix All actions
//!   (`roslyn.client.fixAllCodeAction`) are not offered (out of scope); an action that is only some other command is
//!   reported as unsupported, because the pinned Roslyn runs none through `workspace/executeCommand`.
//! - Answers are dropped when they are not the newest request's, or when the generation or the document's version
//!   moved on (CLAUDE.md invariant 12).

use std::collections::HashMap;
use std::time::{Duration, Instant};

use eludite_commands::CommandError;
use eludite_commands::workspace::{
    self, ApplyCodeActionOutput, ApplyCodeActionState, CodeActionRow, CodeActionsOutput,
    CodeActionsState, WorkspaceOutput,
};
use eludite_editor::LightbulbKind;
use eludite_lsp::lsp;
use eludite_ui::{Theme, menu_row, section_heading};
use gpui::{
    App, AppContext as _, Context, EventEmitter, FocusHandle, Focusable, InteractiveElement,
    IntoElement, KeyDownEvent, ParentElement, Pixels, Point, Render, StatefulInteractiveElement,
    Styled, Task, Window, anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use super::documents::trace;
use super::intellisense::{Provider, line_column, lsp_position};
use super::navigation::{NavEntry, OUTDATED};
use super::session::{RequestError, RequestHandle};
use super::workspace_edit::{ApplyOptions, ApplySummary};
use super::{Caret, Shell};

/// Quiet time after the caret moves before the light bulb asks for code actions.
pub const LIGHTBULB_DEBOUNCE: Duration = Duration::from_millis(50);

/// Roslyn's client-side command for an action with nested actions (shown as a submenu).
pub const NESTED_COMMAND: &str = "roslyn.client.nestedCodeAction";
/// Roslyn's client-side Fix All command (not offered: fix-all is out of scope for brief 0015).
pub const FIX_ALL_COMMAND: &str = "roslyn.client.fixAllCodeAction";

/// Visual Studio's message when Ctrl+. finds nothing.
pub const NO_ACTIONS: &str = "No quick actions available here.";

/// Actions the menu keeps at most.
pub const MAX_ACTIONS: usize = 200;

/// How Visual Studio groups an action.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Group {
    Fix,
    Refactoring,
    Other,
}

impl Group {
    pub fn of(kind: Option<&str>) -> Self {
        match kind {
            Some(k) if k == "quickfix" || k.starts_with("quickfix.") => Group::Fix,
            Some(k) if k == "refactor" || k.starts_with("refactor.") => Group::Refactoring,
            _ => Group::Other,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Group::Fix => "fix",
            Group::Refactoring => "refactoring",
            Group::Other => "other",
        }
    }
}

/// One action of the menu.
#[derive(Debug, Clone)]
pub struct MenuAction {
    pub index: usize,
    pub title: String,
    pub group: Group,
    pub kind: Option<String>,
    pub preferred: bool,
    /// The action this one is nested under.
    pub parent: Option<usize>,
    pub children: Vec<usize>,
    pub disabled: Option<String>,
    pub action: lsp::CodeAction,
}

/// The actions at one position, and what they were computed for.
#[derive(Debug, Clone)]
pub struct ActionList {
    pub doc: String,
    /// The caret offset and buffer version the request was made at.
    pub offset: usize,
    pub buffer_version: clock::Global,
    pub generation: u64,
    /// Every open document's LSP version when the request was made.
    pub versions: HashMap<String, i32>,
    pub actions: Vec<MenuAction>,
}

impl ActionList {
    /// Fixes, then refactorings, then the rest; nested actions under their parent.
    pub fn display_order(&self) -> Vec<usize> {
        let mut top: Vec<&MenuAction> =
            self.actions.iter().filter(|a| a.parent.is_none()).collect();
        top.sort_by_key(|a| (a.group, a.index));
        let mut out = Vec::new();
        fn push(list: &ActionList, a: &MenuAction, out: &mut Vec<usize>) {
            out.push(a.index);
            for &c in &a.children {
                push(list, &list.actions[c], out);
            }
        }
        for a in top {
            push(self, a, &mut out);
        }
        out
    }

    pub fn bulb(&self) -> Option<LightbulbKind> {
        let shown = self.actions.iter().filter(|a| a.parent.is_none());
        let mut any = false;
        for a in shown {
            any = true;
            if a.group == Group::Fix {
                return Some(LightbulbKind::Fix);
            }
        }
        any.then_some(LightbulbKind::Refactoring)
    }
}

fn nested_actions(command: &lsp::Command) -> Vec<lsp::CodeAction> {
    let Some(arg) = command.arguments.as_ref().and_then(|a| a.first()) else {
        return Vec::new();
    };
    let nested = arg
        .get("NestedCodeActions")
        .or_else(|| arg.get("nestedCodeActions"))
        .and_then(Value::as_array);
    nested
        .map(|items| {
            items
                .iter()
                .filter_map(|v| serde_json::from_value::<lsp::CodeAction>(v.clone()).ok())
                .collect()
        })
        .unwrap_or_default()
}

/// The menu's actions from a `textDocument/codeAction` answer: commands and Fix All dropped, nested actions
/// flattened under their parent.
pub fn menu_actions(answer: Vec<lsp::CodeActionOrCommand>) -> Vec<MenuAction> {
    fn add(out: &mut Vec<MenuAction>, action: lsp::CodeAction, parent: Option<usize>) {
        if out.len() >= MAX_ACTIONS {
            return;
        }
        let command = action.command.as_ref().map(|c| c.command.as_str());
        if command == Some(FIX_ALL_COMMAND) {
            return;
        }
        let index = out.len();
        let nested = match &action.command {
            Some(c) if c.command == NESTED_COMMAND => nested_actions(c),
            _ => Vec::new(),
        };
        out.push(MenuAction {
            index,
            title: action.title.clone(),
            group: Group::of(action.kind.as_deref()),
            kind: action.kind.clone(),
            preferred: action.is_preferred == Some(true),
            parent,
            children: Vec::new(),
            disabled: action.disabled.as_ref().map(|d| d.reason.clone()),
            action,
        });
        if let Some(p) = parent {
            out[p].children.push(index);
        }
        for child in nested {
            add(out, child, Some(index));
        }
    }
    let mut out = Vec::new();
    for entry in answer {
        match entry {
            lsp::CodeActionOrCommand::Action(a) => add(&mut out, *a, None),
            // A bare command: shown, and reported unsupported when chosen.
            lsp::CodeActionOrCommand::Command(c) => add(
                &mut out,
                lsp::CodeAction {
                    title: c.title.clone(),
                    command: Some(c),
                    ..Default::default()
                },
                None,
            ),
        }
    }
    out
}

/// Where the menu's last invocation is (`eludite.editor.code_actions`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MenuState {
    Loading,
    Open,
    None,
    Failed,
}

/// What applying an action did (`eludite.editor.apply_code_action`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplyState {
    Resolving,
    Applying,
    Applied,
    Expanded,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Default)]
pub struct MenuStatus {
    pub origin: Option<NavEntry>,
    pub state: Option<MenuState>,
    pub message: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct ApplyStatus {
    pub state: Option<ApplyState>,
    pub title: String,
    pub summary: Option<ApplySummary>,
    pub message: Option<String>,
}

/// When the steps of one light bulb happened (the `--bench-lightbulb` harness and the report).
#[derive(Debug, Clone, Default)]
pub struct LightbulbTiming {
    /// The caret came to rest (the debounce started).
    pub moved: Option<Instant>,
    pub sent: Option<Instant>,
    pub received: Option<Instant>,
    /// The bulb was handed to the editor.
    pub shown: Option<Instant>,
    pub actions: usize,
    pub dropped: bool,
}

struct Pending {
    handle: RequestHandle,
    ticket: u64,
    /// Ctrl+. (not the light bulb): the caret moving does not cancel it.
    invoked: bool,
    _task: Task<()>,
}

/// The window's code actions.
#[derive(Default)]
pub struct CodeActions {
    pending: Option<Pending>,
    resolve: Option<Pending>,
    debounce: Option<Task<()>>,
    /// The document, caret and buffer version last probed (the bulb asks again when they change).
    probed: Option<(String, usize, clock::Global)>,
    /// The newest answer for a caret position.
    pub list: Option<ActionList>,
    pub menu: Option<gpui::Entity<CodeActionMenu>>,
    pub status: MenuStatus,
    pub apply: ApplyStatus,
    next_ticket: u64,
    pub timings: Vec<LightbulbTiming>,
}

impl CodeActions {
    /// Cancel everything in flight (a new solution generation).
    pub fn cancel(&mut self) {
        self.debounce = None;
        self.probed = None;
        self.list = None;
        for p in [self.pending.take(), self.resolve.take()]
            .into_iter()
            .flatten()
        {
            p.handle.cancel();
        }
        if self.status.state == Some(MenuState::Loading) {
            self.status.state = Some(MenuState::Failed);
            self.status.message = Some(OUTDATED.into());
        }
        if matches!(self.apply.state, Some(ApplyState::Resolving)) {
            self.apply.state = Some(ApplyState::Failed);
            self.apply.message = Some(OUTDATED.into());
        }
    }

    fn push_timing(&mut self, t: LightbulbTiming) {
        if self.timings.len() >= 4096 {
            self.timings.remove(0);
        }
        self.timings.push(t);
    }
}

/// What the menu reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuEvent {
    /// Apply (or expand) the action with this index.
    Chosen(usize),
    Dismissed,
}

/// Debug selector of menu row `index` (the action's index).
pub fn action_row_selector(index: usize) -> String {
    format!("code-action-{index}")
}

pub const CODE_ACTION_MENU: &str = "code-action-menu";

/// The light bulb menu.
pub struct CodeActionMenu {
    theme: Theme,
    list: ActionList,
    /// Expanded parents.
    expanded: Vec<usize>,
    /// The selected action's index.
    selected: usize,
    anchor: Point<Pixels>,
    focus: FocusHandle,
}

impl CodeActionMenu {
    pub fn new(
        theme: Theme,
        list: ActionList,
        anchor: Point<Pixels>,
        cx: &mut Context<Self>,
    ) -> Self {
        let mut this = Self {
            theme,
            list,
            expanded: Vec::new(),
            selected: 0,
            anchor,
            focus: cx.focus_handle(),
        };
        // The preferred action, else the first one shown.
        let rows = this.visible();
        this.selected = rows
            .iter()
            .copied()
            .find(|&i| this.list.actions[i].preferred)
            .or_else(|| rows.first().copied())
            .unwrap_or(0);
        this
    }

    /// The rows shown, in order: top-level actions, and the children of expanded ones.
    pub fn visible(&self) -> Vec<usize> {
        self.list
            .display_order()
            .into_iter()
            .filter(|&i| {
                let mut p = self.list.actions[i].parent;
                while let Some(parent) = p {
                    if !self.expanded.contains(&parent) {
                        return false;
                    }
                    p = self.list.actions[parent].parent;
                }
                true
            })
            .collect()
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn list(&self) -> &ActionList {
        &self.list
    }

    /// Show the children of `index` (a nested action), selecting the first.
    pub fn expand(&mut self, index: usize, cx: &mut Context<Self>) {
        if !self.expanded.contains(&index) {
            self.expanded.push(index);
        }
        if let Some(&first) = self.list.actions[index].children.first() {
            self.selected = first;
        }
        cx.notify();
    }

    fn key_down(&mut self, event: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return;
        }
        let rows = self.visible();
        let at = rows.iter().position(|&i| i == self.selected).unwrap_or(0);
        match k.key.as_str() {
            "up" => self.selected = rows[at.saturating_sub(1)],
            "down" => self.selected = rows[(at + 1).min(rows.len().saturating_sub(1))],
            "right" if !self.list.actions[self.selected].children.is_empty() => {
                self.expand(self.selected, cx)
            }
            "left" => {
                let a = &self.list.actions[self.selected];
                if let Some(p) = a.parent {
                    self.expanded.retain(|&e| e != p);
                    self.selected = p;
                } else {
                    self.expanded.retain(|&e| e != self.selected);
                }
            }
            "enter" => cx.emit(MenuEvent::Chosen(self.selected)),
            "escape" => cx.emit(MenuEvent::Dismissed),
            _ => return,
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl EventEmitter<MenuEvent> for CodeActionMenu {}

impl Focusable for CodeActionMenu {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for CodeActionMenu {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let mut children = Vec::new();
        let mut last_group = None;
        for i in self.visible() {
            let a = &self.list.actions[i];
            let depth = {
                let mut d = 0;
                let mut p = a.parent;
                while let Some(parent) = p {
                    d += 1;
                    p = self.list.actions[parent].parent;
                }
                d
            };
            if depth == 0 && last_group != Some(a.group) {
                if last_group.is_some() {
                    children.push(
                        div()
                            .mx_2()
                            .my_1()
                            .h(px(1.))
                            .bg(t.border)
                            .into_any_element(),
                    );
                }
                let heading = match a.group {
                    Group::Fix => "Fixes",
                    Group::Refactoring => "Refactorings",
                    Group::Other => "Other actions",
                };
                children.push(section_heading(heading, &t).into_any_element());
                last_group = Some(a.group);
            }
            children.push(
                menu_row(
                    action_row_selector(i),
                    a.title.clone(),
                    depth,
                    i == self.selected,
                    !a.children.is_empty(),
                    a.disabled.is_some(),
                    &t,
                )
                .on_click(cx.listener(move |_, _, _, cx| cx.emit(MenuEvent::Chosen(i))))
                .into_any_element(),
            );
        }
        let panel = eludite_ui::popup::popup_panel(&t)
            .id(CODE_ACTION_MENU)
            .debug_selector(|| CODE_ACTION_MENU.into())
            .track_focus(&self.focus)
            .key_context("CodeActionMenu")
            .on_key_down(cx.listener(Self::key_down))
            .on_mouse_down_out(cx.listener(|_, _, _, cx| cx.emit(MenuEvent::Dismissed)))
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(300.))
            .max_w(px(700.))
            .py_1()
            .children(children);
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
    /// The editor of document `id` changed or its caret moved: if the caret came to a new position, ask for the light
    /// bulb after [`LIGHTBULB_DEBOUNCE`].
    pub(super) fn probe_lightbulb(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        if self.controller.active_document().as_deref() != Some(id)
            || doc.read_only
            || self.provider(doc) != Provider::Server
        {
            return;
        }
        let (caret, version, row) = {
            let e = doc.view.read(cx).editor();
            let caret = e.primary_selection().head;
            (
                caret,
                e.buffer().version(),
                e.buffer().offset_to_point(caret).row,
            )
        };
        let probe = (id.to_owned(), caret, version);
        if self.code_actions.probed.as_ref() == Some(&probe) {
            return;
        }
        self.code_actions.probed = Some(probe);
        // The bulb belongs to its line: moving elsewhere hides it at once.
        let view = doc.view.clone();
        if view.read(cx).lightbulb().is_some_and(|(r, _)| r != row) {
            view.update(cx, |v, cx| v.set_lightbulb(None, cx));
        }
        if self
            .code_actions
            .pending
            .as_ref()
            .is_some_and(|p| !p.invoked)
            && let Some(p) = self.code_actions.pending.take()
        {
            p.handle.cancel();
        }
        let moved = Instant::now();
        let doc_id = id.to_owned();
        self.code_actions.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(LIGHTBULB_DEBOUNCE).await;
            let _ = this.update(cx, |shell, cx| {
                shell.code_actions.debounce = None;
                shell.request_code_actions(&doc_id, None, false, Some(moved), None, cx);
            });
        }));
    }

    /// Ask for the code actions at `at` (or the caret) in document `id`. `invoked`: Ctrl+. (the menu opens on the
    /// answer, in `window`).
    pub(super) fn request_code_actions(
        &mut self,
        id: &str,
        at: Option<Caret>,
        invoked: bool,
        moved: Option<Instant>,
        window: Option<&mut Window>,
        cx: &mut Context<Self>,
    ) {
        // The light bulb never replaces a Ctrl+. request in flight.
        if !invoked
            && self
                .code_actions
                .pending
                .as_ref()
                .is_some_and(|p| p.invoked)
        {
            return;
        }
        if let Some(p) = self.code_actions.pending.take() {
            p.handle.cancel();
        }
        let Some(doc) = self.documents.get(id) else {
            return;
        };
        let view = doc.view.clone();
        let offset = match at {
            Some((line, column)) => {
                super::documents::offset_of(view.read(cx).editor().buffer(), line, column)
            }
            None => view.read(cx).editor().primary_selection().head,
        };
        let (line, column) = line_column(&view, offset, cx);
        if invoked {
            self.code_actions.status = MenuStatus {
                origin: Some(NavEntry {
                    path: id.to_owned(),
                    line,
                    column,
                }),
                state: Some(MenuState::Loading),
                message: None,
            };
        }
        if doc.read_only || self.provider(doc) == Provider::Syntax {
            if invoked {
                self.finish_menu(MenuState::None, Some(NO_ACTIONS.into()), cx);
            }
            return;
        }
        self.flush_change(id, cx);
        let generation = self.doc_generation(id);
        let doc = &self.documents[id];
        let buffer_version = view.read(cx).editor().buffer().version();
        let position = lsp_position(&doc.sent, offset);
        let diagnostics: Vec<lsp::Diagnostic> = self
            .diagnostics
            .get(&doc.uri)
            .map(|ds| {
                ds.iter()
                    .filter(|d| {
                        d.range.start.line <= position.line && position.line <= d.range.end.line
                    })
                    .cloned()
                    .collect()
            })
            .unwrap_or_default();
        let params = lsp::CodeActionParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: doc.uri.clone(),
            },
            range: lsp::Range {
                start: position,
                end: position,
            },
            context: lsp::CodeActionContext {
                diagnostics,
                only: None,
                trigger_kind: Some(if invoked { 1 } else { 2 }),
            },
        };
        let versions: HashMap<String, i32> = self
            .documents
            .iter()
            .map(|(id, d)| (id.clone(), d.lsp_version))
            .collect();
        let version = doc.lsp_version;
        self.code_actions.next_ticket += 1;
        let ticket = self.code_actions.next_ticket;
        trace(format_args!(
            "codeAction {ticket} ({}) at {}:{} version {version}",
            if invoked { "invoked" } else { "light bulb" },
            position.line,
            position.character
        ));
        let (handle, rx) = doc.session.request::<lsp::CodeActionRequest>(params);
        let doc_id = id.to_owned();
        let on_reply = move |shell: &mut Shell,
                             reply: super::session::Reply<
            Option<Vec<lsp::CodeActionOrCommand>>,
        >,
                             window: Option<&mut Window>,
                             cx: &mut Context<Shell>| {
            let mut timing = LightbulbTiming {
                moved,
                sent: reply.sent,
                received: Some(reply.received),
                ..Default::default()
            };
            let newest = shell
                .code_actions
                .pending
                .as_ref()
                .is_some_and(|p| p.ticket == ticket);
            let doc_version = shell.documents.get(&doc_id).map(|d| d.lsp_version);
            let current = shell.doc_generation(&doc_id);
            let outdated = generation != current || doc_version != Some(version);
            if !newest || outdated {
                trace(format_args!(
                    "codeAction reply {ticket} dropped (stale: newest {newest}, generation {generation}/{current}, version {version}/{doc_version:?})"
                ));
                timing.dropped = true;
                shell.code_actions.push_timing(timing);
                if newest {
                    shell.code_actions.pending = None;
                    if invoked {
                        shell.finish_menu(MenuState::Failed, Some(OUTDATED.into()), cx);
                    }
                }
                return;
            }
            shell.code_actions.pending = None;
            let actions = match reply.result {
                Err(RequestError::Canceled) => return,
                Ok(answer) => menu_actions(answer.unwrap_or_default()),
                Err(e) => {
                    if invoked {
                        let message = match e {
                            RequestError::Failed(m) => m,
                            RequestError::NoHost => "No language server is running.".into(),
                            other => format!("{other:?}"),
                        };
                        shell.finish_menu(MenuState::Failed, Some(message), cx);
                    }
                    return;
                }
            };
            trace(format_args!(
                "codeAction reply {ticket}: {} actions (host {:.1} ms)",
                actions.len(),
                reply
                    .sent
                    .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
            ));
            timing.actions = actions.len();
            let list = ActionList {
                doc: doc_id.clone(),
                offset,
                buffer_version: buffer_version.clone(),
                generation,
                versions: versions.clone(),
                actions,
            };
            let bulb = list.bulb();
            if let Some(doc) = shell.documents.get(&doc_id) {
                doc.view
                    .update(cx, |v, cx| v.set_lightbulb(bulb.map(|k| (offset, k)), cx));
            }
            timing.shown = Some(Instant::now());
            shell.code_actions.push_timing(timing);
            shell.code_actions.list = Some(list);
            if invoked && let Some(window) = window {
                shell.open_code_action_menu(window, cx);
            }
            shell.wake_intellisense_waiters();
        };
        let task = match window {
            Some(window) => cx.spawn_in(window, async move |this, cx| {
                let Ok(reply) = rx.await else {
                    return;
                };
                let _ = this.update_in(cx, |shell, window, cx| {
                    on_reply(shell, reply, Some(window), cx)
                });
            }),
            None => cx.spawn(async move |this, cx| {
                let Ok(reply) = rx.await else {
                    return;
                };
                let _ = this.update(cx, |shell, cx| on_reply(shell, reply, None, cx));
            }),
        };
        self.code_actions.pending = Some(Pending {
            handle,
            ticket,
            invoked,
            _task: task,
        });
    }

    fn finish_menu(&mut self, state: MenuState, message: Option<String>, cx: &mut Context<Self>) {
        if let Some(m) = &message {
            self.status.set(eludite_ui::slots::STATE, m.clone());
        }
        self.code_actions.status.state = Some(state);
        self.code_actions.status.message = message;
        self.wake_intellisense_waiters();
        cx.notify();
    }

    /// Whether the last answer is for document `id` at `offset` with the text as it is now.
    fn current_list(&self, id: &str, offset: usize, cx: &App) -> bool {
        let Some(list) = &self.code_actions.list else {
            return false;
        };
        let Some(doc) = self.documents.get(id) else {
            return false;
        };
        list.doc == id
            && list.offset == offset
            && list.generation == self.doc_generation(id)
            && list.buffer_version == doc.view.read(cx).editor().buffer().version()
    }

    /// Ctrl+. (and a click on the bulb): open the menu for the caret (or `at`), from the bulb's answer when it is
    /// current, else after asking.
    pub(super) fn show_code_actions(
        &mut self,
        id: &str,
        at: Option<Caret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.close_code_action_menu(window, cx);
        let view = self.documents[id].view.clone();
        if let Some((line, column)) = at {
            super::documents::move_caret(&view, line, column, cx);
        }
        let offset = view.read(cx).editor().primary_selection().head;
        if self.current_list(id, offset, cx) {
            let (line, column) = line_column(&view, offset, cx);
            self.code_actions.status = MenuStatus {
                origin: Some(NavEntry {
                    path: id.to_owned(),
                    line,
                    column,
                }),
                state: Some(MenuState::Loading),
                message: None,
            };
            self.open_code_action_menu(window, cx);
            return;
        }
        self.request_code_actions(id, None, true, None, Some(window), cx);
    }

    fn open_code_action_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(list) = self.code_actions.list.clone() else {
            return;
        };
        if list.actions.is_empty() {
            self.finish_menu(MenuState::None, Some(NO_ACTIONS.into()), cx);
            return;
        }
        let anchor = self
            .documents
            .get(&list.doc)
            .and_then(|doc| {
                let v = doc.view.read(cx);
                let b = v.editor().buffer();
                let row = b.offset_to_point(list.offset.min(b.len())).row;
                let start = b.point_to_offset(eludite_editor::text::Point::new(row, 0));
                v.pixel_position_for_offset(start)
                    .map(|p| point(p.x, p.y + v.line_height()))
            })
            .unwrap_or_else(|| point(px(200.), px(120.)));
        let theme = self.theme;
        let menu = cx.new(|cx| CodeActionMenu::new(theme, list, anchor, cx));
        cx.subscribe_in(&menu, window, |shell, _, event, window, cx| match event {
            // Through the bus, as an agent applies an action.
            MenuEvent::Chosen(index) => shell.run(
                workspace::EDITOR_APPLY_CODE_ACTION,
                json!({ "index": index }),
                window,
                cx,
            ),
            MenuEvent::Dismissed => shell.close_code_action_menu(window, cx),
        })
        .detach();
        trace(format_args!(
            "code action menu: selected {:?}; {:?}",
            menu.read(cx).list().actions[menu.read(cx).selected()].title,
            menu.read(cx)
                .visible()
                .iter()
                .map(|&i| menu.read(cx).list().actions[i].title.clone())
                .collect::<Vec<_>>()
        ));
        menu.focus_handle(cx).focus(window, cx);
        self.code_actions.menu = Some(menu);
        self.finish_menu(MenuState::Open, None, cx);
    }

    pub(super) fn close_code_action_menu(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.code_actions.menu.take().is_some() {
            if let Some(id) = self.controller.active_document()
                && let Some(doc) = self.documents.get(&id)
            {
                doc.view.focus_handle(cx).focus(window, cx);
            }
            cx.notify();
        }
    }

    fn finish_apply_action(
        &mut self,
        state: ApplyState,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        if let Some(m) = &message {
            self.status.set(eludite_ui::slots::STATE, m.clone());
        }
        self.code_actions.apply.state = Some(state);
        self.code_actions.apply.message = message;
        self.wake_intellisense_waiters();
        cx.notify();
    }

    /// Apply action `index` of the last list: expand a nested one, resolve it when it has no edit, then apply.
    pub(super) fn apply_code_action(
        &mut self,
        index: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(list) = self.code_actions.list.clone() else {
            self.code_actions.apply = ApplyStatus {
                state: Some(ApplyState::Failed),
                message: Some("No code actions were listed.".into()),
                ..Default::default()
            };
            return;
        };
        let Some(entry) = list.actions.get(index).cloned() else {
            self.code_actions.apply = ApplyStatus {
                state: Some(ApplyState::Failed),
                message: Some(format!("There is no action {index}.")),
                ..Default::default()
            };
            return;
        };
        self.code_actions.apply = ApplyStatus {
            title: entry.title.clone(),
            ..Default::default()
        };
        if !entry.children.is_empty() {
            if let Some(menu) = &self.code_actions.menu {
                menu.update(cx, |m, cx| m.expand(index, cx));
            } else {
                // An agent applied it without the menu: open the menu with the nested actions shown.
                self.open_code_action_menu(window, cx);
                if let Some(menu) = &self.code_actions.menu {
                    menu.update(cx, |m, cx| m.expand(index, cx));
                }
            }
            self.finish_apply_action(ApplyState::Expanded, None, cx);
            return;
        }
        if let Some(reason) = &entry.disabled {
            self.finish_apply_action(ApplyState::Failed, Some(reason.clone()), cx);
            return;
        }
        self.close_code_action_menu(window, cx);
        if let Some(edit) = entry.action.edit.clone() {
            self.apply_action_edit(
                entry.title.clone(),
                edit,
                list.generation,
                list.versions.clone(),
                entry.action.command.clone(),
                window,
                cx,
            );
            return;
        }
        let can_resolve =
            entry.action.data.is_some() && self.doc_features(&list.doc).code_action_resolve;
        if !can_resolve {
            let message = match &entry.action.command {
                Some(c) => format!(
                    "'{}' runs the command {}, which Eludite cannot run.",
                    entry.title, c.command
                ),
                None => format!("'{}' has no edit.", entry.title),
            };
            self.finish_apply_action(ApplyState::Unsupported, Some(message), cx);
            return;
        }
        // Resolve lazily, with the text the server sees now.
        self.flush_change(&list.doc, cx);
        let generation = self.doc_generation(&list.doc);
        let doc_id = list.doc.clone();
        let versions: HashMap<String, i32> = self
            .documents
            .iter()
            .map(|(id, d)| (id.clone(), d.lsp_version))
            .collect();
        self.code_actions.apply.state = Some(ApplyState::Resolving);
        if let Some(p) = self.code_actions.resolve.take() {
            p.handle.cancel();
        }
        self.code_actions.next_ticket += 1;
        let ticket = self.code_actions.next_ticket;
        let title = entry.title.clone();
        trace(format_args!("codeAction/resolve {ticket}: {title:?}"));
        let (handle, rx) = self
            .session_for(&doc_id)
            .request::<lsp::ResolveCodeAction>(entry.action.clone());
        let task = cx.spawn_in(window, async move |this, cx| {
            let Ok(reply) = rx.await else {
                return;
            };
            let _ = this.update_in(cx, |shell, window, cx| {
                if !shell
                    .code_actions
                    .resolve
                    .as_ref()
                    .is_some_and(|p| p.ticket == ticket)
                {
                    return;
                }
                shell.code_actions.resolve = None;
                if generation != shell.doc_generation(&doc_id) {
                    shell.finish_apply_action(ApplyState::Failed, Some(OUTDATED.into()), cx);
                    return;
                }
                match reply.result {
                    Err(RequestError::Canceled) => {}
                    Err(e) => {
                        let message = match e {
                            RequestError::Failed(m) => m,
                            other => format!("{other:?}"),
                        };
                        shell.finish_apply_action(ApplyState::Failed, Some(message), cx);
                    }
                    Ok(resolved) => {
                        trace(format_args!(
                            "codeAction/resolve {ticket}: edit {} (host {:.1} ms)",
                            resolved.edit.is_some(),
                            reply
                                .sent
                                .map_or(0., |s| (reply.received - s).as_secs_f64() * 1e3)
                        ));
                        match resolved.edit {
                            Some(edit) => shell.apply_action_edit(
                                title,
                                edit,
                                generation,
                                versions,
                                resolved.command,
                                window,
                                cx,
                            ),
                            None => {
                                let message = match resolved.command {
                                    Some(c) => format!(
                                        "'{title}' runs the command {}, which Eludite cannot run.",
                                        c.command
                                    ),
                                    None => format!("'{title}' has no edit."),
                                };
                                shell.finish_apply_action(
                                    ApplyState::Unsupported,
                                    Some(message),
                                    cx,
                                );
                            }
                        }
                    }
                }
            });
        });
        self.code_actions.resolve = Some(Pending {
            handle,
            ticket,
            invoked: true,
            _task: task,
        });
        self.wake_intellisense_waiters();
    }

    #[allow(clippy::too_many_arguments)]
    fn apply_action_edit(
        &mut self,
        title: String,
        edit: lsp::WorkspaceEdit,
        generation: u64,
        versions: HashMap<String, i32>,
        command: Option<lsp::Command>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.code_actions.apply.state = Some(ApplyState::Applying);
        // Only the documents the edit touches are checked against their versions.
        // The server the actions came from (the light bulb's document's).
        let server = self
            .code_actions
            .list
            .as_ref()
            .and_then(|l| self.documents.get(&l.doc))
            .map(|d| d.server.clone())
            .unwrap_or_default();
        let options = ApplyOptions {
            label: Some(title.clone()),
            generation: Some(generation),
            server,
            versions,
        };
        self.apply_workspace_edit(
            &edit,
            options,
            window,
            cx,
            Box::new(move |shell, summary, _, cx| {
                let applied = summary.applied;
                let message = summary.message.clone();
                shell.code_actions.apply.summary = Some(summary);
                shell.code_actions.list = None;
                shell.code_actions.probed = None;
                if applied {
                    let note = command.map(|c| {
                        format!(
                            "The edit was applied; its command {} was not run.",
                            c.command
                        )
                    });
                    shell.finish_apply_action(ApplyState::Applied, note, cx);
                    if let Some(doc) = shell
                        .controller
                        .active_document()
                        .and_then(|id| shell.documents.get(&id))
                    {
                        doc.view.update(cx, |v, cx| v.set_lightbulb(None, cx));
                    }
                } else {
                    shell.finish_apply_action(
                        ApplyState::Failed,
                        Some(message.unwrap_or_else(|| format!("'{title}' was not applied."))),
                        cx,
                    );
                }
            }),
        );
    }

    /// `eludite.editor.code_actions` (Ctrl+.).
    pub(super) fn code_actions_command(
        &mut self,
        path: Option<&str>,
        at: Option<Caret>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let id = self.document_id(path)?;
        self.show_code_actions(&id, at, window, cx);
        Ok(WorkspaceOutput::CodeActions(self.code_actions_output()))
    }

    pub(super) fn code_actions_output(&self) -> CodeActionsOutput {
        let s = &self.code_actions.status;
        let origin = s.origin.clone().unwrap_or(NavEntry {
            path: String::new(),
            line: 1,
            column: 1,
        });
        let state = s.state.unwrap_or(MenuState::Loading);
        let actions = match (&self.code_actions.list, state) {
            (Some(list), MenuState::Open) => list
                .display_order()
                .into_iter()
                .take(workspace::MAX_CODE_ACTIONS)
                .map(|i| {
                    let a = &list.actions[i];
                    CodeActionRow {
                        index: a.index as u64,
                        title: a.title.clone(),
                        group: a.group.name().into(),
                        kind: a.kind.clone(),
                        preferred: a.preferred.then_some(true),
                        parent: a.parent.map(|p| p as u64),
                        disabled: a.disabled.clone(),
                    }
                })
                .collect(),
            _ => Vec::new(),
        };
        CodeActionsOutput {
            path: origin.path,
            line: origin.line,
            column: origin.column,
            state: match state {
                MenuState::Loading => CodeActionsState::Loading,
                MenuState::Open => CodeActionsState::Open,
                MenuState::None => CodeActionsState::None,
                MenuState::Failed => CodeActionsState::Failed,
            },
            actions,
            message: s.message.clone(),
        }
    }

    /// `eludite.editor.apply_code_action`.
    pub(super) fn apply_code_action_command(
        &mut self,
        index: Option<usize>,
        title: Option<&str>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Result<WorkspaceOutput, CommandError> {
        let list = self.code_actions.list.as_ref().ok_or_else(|| {
            CommandError::Failed(
                "no code actions were listed; run eludite.editor.code_actions first".into(),
            )
        })?;
        let index = match (index, title) {
            (Some(i), _) if i < list.actions.len() => i,
            (Some(i), _) => {
                return Err(CommandError::InvalidInput(format!(
                    "`index` {i} is out of range ({} actions)",
                    list.actions.len()
                )));
            }
            (None, Some(t)) => list
                .display_order()
                .into_iter()
                .find(|&i| list.actions[i].title == t)
                .ok_or_else(|| CommandError::InvalidInput(format!("no action is titled {t:?}")))?,
            (None, None) => {
                return Err(CommandError::InvalidInput(
                    "name the action by `index` or by `title`".into(),
                ));
            }
        };
        self.apply_code_action(index, window, cx);
        Ok(WorkspaceOutput::ApplyCodeAction(
            self.apply_code_action_output(),
        ))
    }

    pub(super) fn apply_code_action_output(&self) -> ApplyCodeActionOutput {
        let a = &self.code_actions.apply;
        ApplyCodeActionOutput {
            state: match a.state.unwrap_or(ApplyState::Failed) {
                ApplyState::Resolving => ApplyCodeActionState::Resolving,
                ApplyState::Applying => ApplyCodeActionState::Applying,
                ApplyState::Applied => ApplyCodeActionState::Applied,
                ApplyState::Expanded => ApplyCodeActionState::Expanded,
                ApplyState::Unsupported => ApplyCodeActionState::Unsupported,
                ApplyState::Failed => ApplyCodeActionState::Failed,
            },
            title: a.title.clone(),
            summary: a.summary.as_ref().map(ApplySummary::output),
            message: a.message.clone(),
        }
    }

    /// The light bulb timings so far (oldest first).
    pub fn lightbulb_timings(&self) -> &[LightbulbTiming] {
        &self.code_actions.timings
    }

    /// The light bulb menu, while it is open.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn code_action_menu(&self) -> Option<&gpui::Entity<CodeActionMenu>> {
        self.code_actions.menu.as_ref()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn roslyn_answers_become_grouped_menus_with_nested_actions() {
        let data = json!({"UniqueIdentifier": "x"});
        let answer: Vec<lsp::CodeActionOrCommand> = serde_json::from_value(json!([
            {"title": "Extract method", "kind": "refactor.extract", "data": data},
            {"title": "Use primary constructor", "kind": "quickfix", "data": data},
            {"title": "Fix All: Use primary constructor", "kind": "quickfix",
             "command": {"title": "Fix All", "command": FIX_ALL_COMMAND, "arguments": [data]}, "data": data},
            {"title": "Suppress or configure issues", "kind": "quickfix",
             "command": {"title": "Suppress", "command": NESTED_COMMAND,
                         "arguments": [{"NestedCodeActions": [
                             {"title": "Suppress IDE0290", "kind": "quickfix", "data": data},
                             {"title": "Configure IDE0290", "kind": "quickfix", "data": data,
                              "command": {"title": "c", "command": NESTED_COMMAND, "arguments": [{"NestedCodeActions": [
                                  {"title": "Configure severity: warning", "data": data}]}]}}]}]},
             "data": data},
            {"title": "Organize usings", "command": "x.organize"}
        ]))
        .unwrap();
        let actions = menu_actions(answer);
        let titles: Vec<&str> = actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(
            titles,
            [
                "Extract method",
                "Use primary constructor",
                "Suppress or configure issues",
                "Suppress IDE0290",
                "Configure IDE0290",
                "Configure severity: warning",
                "Organize usings"
            ],
            "Fix All is not offered"
        );
        assert_eq!(actions[2].children, [3, 4]);
        assert_eq!(actions[4].children, [5]);
        assert_eq!(actions[5].parent, Some(4));
        assert_eq!(actions[0].group, Group::Refactoring);
        assert_eq!(actions[6].group, Group::Other);
        let list = ActionList {
            doc: String::new(),
            offset: 0,
            buffer_version: Default::default(),
            generation: 1,
            versions: HashMap::new(),
            actions,
        };
        // Fixes first, then refactorings, then the rest; nested under their parent.
        assert_eq!(list.display_order(), [1, 2, 3, 4, 5, 0, 6]);
        assert_eq!(list.bulb(), Some(LightbulbKind::Fix));
        let refactorings = ActionList {
            actions: list.actions[..1].to_vec(),
            ..list.clone()
        };
        assert_eq!(refactorings.bulb(), Some(LightbulbKind::Refactoring));
        let none = ActionList {
            actions: vec![],
            ..list
        };
        assert_eq!(none.bulb(), None);
    }
}
