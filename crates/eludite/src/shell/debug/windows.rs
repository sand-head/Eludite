//! The debugger's tool windows (brief 0018), as Visual Studio lays them out: Locals and Watch 1 (Name, Value, Type,
//! expanding lazily; a selected local's value is edited below the Locals list, brief 0026), Call Stack, Threads,
//! Breakpoints (enable, condition, hit count, When Hit's message, delete; tracepoints with Visual Studio's diamond,
//! function breakpoints by name with a name box to add one, `run_until`'s and `trace`'s points marked temporary),
//! Exception Settings (Common Language Runtime Exceptions, with exception types under it: Add, Remove, Clear). The program's output goes to the Output window's Debug source (brief 0020
//! retired the Debug Console window); expressions are evaluated in the Watch window.
//!
//! With several debugging sessions (brief 0028) the Call Stack and Threads windows gain a session selector
//! ([`SessionChoice`], drawn with `eludite_ui::selector_bar`): each session with its mode, the active one pressed; a
//! click runs `eludite.debug.select_frame` with the `session`, which makes it the active one, and Locals and Watch
//! follow. One row per breakpoint in the Breakpoints window, its binding per session in the row's tooltip.
//!
//! Each window shows a snapshot the shell gives it after the debugger's state changes, and turns clicks into the
//! `eludite.debug.*` commands by dispatching [`RunCommand`], so they reach the same command bus agents use. Expanding
//! a variable is view state (like expanding a Workspace folder): it is an event the shell answers by fetching the
//! members.

use eludite_commands::debug::{self as cmds, BreakpointKind, BreakpointRow, ExceptionSettingsRow};
use eludite_ui::{RunCommand, Theme, text_box, toggle_button};
use gpui::{
    App, AppContext as _, ClickEvent, Context, Entity, EventEmitter, FocusHandle, FontWeight,
    InteractiveElement, IntoElement, KeyDownEvent, ParentElement, Render, SharedString,
    StatefulInteractiveElement, Styled, Window, div, px, uniform_list,
};
use serde_json::json;

use super::state::FlatRow;

pub const ROW_HEIGHT: f32 = 20.;
const INDENT: f32 = 14.;

/// A one-line text box's text and focus; the window draws it with [`text_box`].
pub struct LineInput {
    pub text: String,
    pub focus: FocusHandle,
}

pub enum InputKey {
    Changed,
    Submit(String),
    Ignored,
}

impl LineInput {
    pub fn new(cx: &mut App) -> Self {
        Self {
            text: String::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Apply a key: typing, Backspace, Escape (clear) and Enter (submit, clearing the box).
    pub fn key(&mut self, event: &KeyDownEvent) -> InputKey {
        let k = &event.keystroke;
        if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
            return InputKey::Ignored;
        }
        match k.key.as_str() {
            "backspace" => {
                self.text.pop();
            }
            "escape" => self.text.clear(),
            "enter" => return InputKey::Submit(std::mem::take(&mut self.text)),
            "space" => self.text.push(' '),
            _ => {
                // Platforms report the typed text in `key_char`; a bare one-character key (tests) is the text too.
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
                        self.text.push_str(&c)
                    }
                    _ => return InputKey::Ignored,
                }
            }
        }
        InputKey::Changed
    }
}

fn run(window: &mut Window, cx: &mut App, command: &str, args: serde_json::Value) {
    window.dispatch_action(Box::new(RunCommand::new(command.to_owned(), args)), cx);
}

fn cell(text: impl Into<SharedString>, width: Option<f32>) -> gpui::Div {
    let c = div()
        .px_1()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_ellipsis()
        .child(text.into());
    match width {
        Some(w) => c.flex_none().w(px(w)),
        None => c.flex_1().min_w_0(),
    }
}

fn header(t: &Theme, cells: Vec<gpui::Div>) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .flex_row()
        .flex_none()
        .h(px(ROW_HEIGHT))
        .items_center()
        .border_b_1()
        .border_color(t.border)
        .text_size(t.typography.small)
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(t.text_muted)
        .children(cells)
}

fn row_base(t: &Theme, selector: String) -> gpui::Stateful<gpui::Div> {
    div()
        .id(SharedString::from(selector.clone()))
        .debug_selector(move || selector)
        .w_full()
        .overflow_hidden()
        .flex()
        .flex_row()
        // Rows keep their height in a scrolling column instead of shrinking to fit it.
        .flex_none()
        .h(px(ROW_HEIGHT))
        .items_center()
        .text_size(t.typography.ui)
        .cursor_pointer()
}

fn empty_note(t: &Theme, text: &str) -> gpui::Div {
    div()
        .p_2()
        .text_size(t.typography.ui)
        .text_color(t.text_muted)
        .child(text.to_owned())
}

/// Locals or Watch 1; brief 0077 adds Watch 2 to 4, Autos, QuickWatch's result and a DataTip's tree.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum VarsKind {
    Locals,
    Watch,
    Watch2,
    Watch3,
    Watch4,
    Autos,
    QuickWatch,
    DataTip,
}

impl VarsKind {
    pub fn prefix(self) -> &'static str {
        match self {
            VarsKind::Locals => "debug-locals",
            VarsKind::Watch => "debug-watch",
            VarsKind::Watch2 => "debug-watch-2",
            VarsKind::Watch3 => "debug-watch-3",
            VarsKind::Watch4 => "debug-watch-4",
            VarsKind::Autos => "debug-autos",
            VarsKind::QuickWatch => "debug-quickwatch",
            VarsKind::DataTip => "debug-datatip",
        }
    }

    /// The Watch window's number (1 to 4).
    pub fn watch_window(self) -> Option<usize> {
        match self {
            VarsKind::Watch => Some(1),
            VarsKind::Watch2 => Some(2),
            VarsKind::Watch3 => Some(3),
            VarsKind::Watch4 => Some(4),
            _ => None,
        }
    }

    /// Watch window `n` (1 to 4).
    pub fn watch(n: usize) -> Option<VarsKind> {
        Some(match n {
            1 => VarsKind::Watch,
            2 => VarsKind::Watch2,
            3 => VarsKind::Watch3,
            4 => VarsKind::Watch4,
            _ => return None,
        })
    }

    fn watch_args(self, mut args: serde_json::Value) -> serde_json::Value {
        if let Some(n) = self.watch_window().filter(|n| *n > 1) {
            args["window"] = json!(n);
        }
        args
    }
}

/// What a variables window asks of the shell beyond expanding a row (brief 0077).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VarsEvent {
    /// Expand the row at this path and its loaded descendants.
    ExpandAll(Vec<usize>),
    /// Collapse every expanded row.
    CollapseAll,
}

/// The variables windows' context menu (Visual Studio's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarsMenu {
    Copy,
    CopyValue,
    EditValue,
    AddWatch,
    Hexadecimal,
    ExpandAll,
    CollapseAll,
}

impl VarsMenu {
    pub const ALL: [VarsMenu; 7] = [
        VarsMenu::Copy,
        VarsMenu::CopyValue,
        VarsMenu::EditValue,
        VarsMenu::AddWatch,
        VarsMenu::Hexadecimal,
        VarsMenu::ExpandAll,
        VarsMenu::CollapseAll,
    ];

    pub fn label(self) -> &'static str {
        match self {
            VarsMenu::Copy => "Copy",
            VarsMenu::CopyValue => "Copy Value",
            VarsMenu::EditValue => "Edit Value",
            VarsMenu::AddWatch => "Add Watch",
            VarsMenu::Hexadecimal => "Hexadecimal Display",
            VarsMenu::ExpandAll => "Expand All Children",
            VarsMenu::CollapseAll => "Collapse All Parents",
        }
    }

    pub fn slug(self) -> &'static str {
        match self {
            VarsMenu::Copy => "copy",
            VarsMenu::CopyValue => "copy-value",
            VarsMenu::EditValue => "edit-value",
            VarsMenu::AddWatch => "add-watch",
            VarsMenu::Hexadecimal => "hex",
            VarsMenu::ExpandAll => "expand-all",
            VarsMenu::CollapseAll => "collapse-all",
        }
    }
}

/// An in-place edit of a row: its expression (Watch) or its value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditField {
    Expression,
    Value,
}

/// The setting Hexadecimal Display toggles (brief 0077).
pub const HEX_SETTING: &str = "debugger.hexadecimalDisplay";

/// Expand or collapse the variable at this path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleVariable(pub Vec<usize>);

/// The Locals and Watch windows: a virtualized tree of Name, Value and Type.
pub struct VarsWindow {
    theme: Theme,
    kind: VarsKind,
    rows: Vec<FlatRow>,
    note: Option<String>,
    selected: Option<usize>,
    input: Option<LineInput>,
    /// The Locals window's value box for the selected row (brief 0026).
    value: Option<LineInput>,
    /// Each row's parent value's variables reference (0 at the top).
    parents: Vec<i64>,
    // Brief 0077: changed values (red), each row's expression (Add Watch), the in-place edit, the context menu,
    // keyboard focus and Hexadecimal Display's state.
    changed: Vec<bool>,
    expressions: Vec<String>,
    editing: Option<(usize, EditField, LineInput)>,
    menu: Option<usize>,
    focus: FocusHandle,
    hex: bool,
}

impl EventEmitter<ToggleVariable> for VarsWindow {}
// Brief 0077.
impl EventEmitter<VarsEvent> for VarsWindow {}

impl VarsWindow {
    pub fn new(theme: Theme, kind: VarsKind, cx: &mut App) -> Self {
        Self {
            theme,
            kind,
            rows: Vec::new(),
            note: None,
            selected: None,
            input: kind.watch_window().map(|_| LineInput::new(cx)),
            value: (kind == VarsKind::Locals).then(|| LineInput::new(cx)),
            parents: Vec::new(),
            changed: Vec::new(),
            expressions: Vec::new(),
            editing: None,
            menu: None,
            focus: cx.focus_handle(),
            hex: false,
        }
    }

    // ----- Brief 0077: changed values, in-place edits, the context menu and the keys -----

    /// Which rows changed since the previous stop (drawn red), and each row's expression (Add Watch).
    pub fn set_marks(
        &mut self,
        changed: Vec<bool>,
        expressions: Vec<String>,
        cx: &mut Context<Self>,
    ) {
        if self.changed != changed || self.expressions != expressions {
            self.changed = changed;
            self.expressions = expressions;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn changed(&self) -> &[bool] {
        &self.changed
    }

    pub fn set_hex(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.hex != on {
            self.hex = on;
            cx.notify();
        }
    }

    pub fn select(&mut self, ix: Option<usize>, cx: &mut Context<Self>) {
        self.selected = ix.filter(|i| *i < self.rows.len());
        if let (Some(v), Some(r)) = (
            self.value.as_mut(),
            self.selected.and_then(|i| self.rows.get(i)),
        ) {
            v.text = r.value.clone();
        }
        cx.notify();
    }

    /// Start editing row `ix`'s expression (a Watch window's top-level row) or value.
    pub fn begin_edit(
        &mut self,
        ix: usize,
        field: EditField,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let Some(r) = self.rows.get(ix) else {
            return;
        };
        if field == EditField::Expression && (self.kind.watch_window().is_none() || r.depth > 0) {
            return;
        }
        let mut input = LineInput::new(cx);
        input.text = match field {
            EditField::Expression => r.name.clone(),
            EditField::Value => r.value.clone(),
        };
        input.focus.focus(window, cx);
        self.selected = Some(ix);
        self.editing = Some((ix, field, input));
        cx.notify();
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn editing(&self) -> Option<(usize, EditField, &str)> {
        self.editing
            .as_ref()
            .map(|(i, f, input)| (*i, *f, input.text.as_str()))
    }

    /// Type into the edit box (tests and the key handler).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn edit_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if let Some((_, _, input)) = self.editing.as_mut() {
            input.text = text.to_owned();
            cx.notify();
        }
    }

    /// Commit the in-place edit: `eludite.debug.watch` `edit` for an expression, `eludite.debug.set_variable` for a
    /// value (a member through its parent's reference).
    pub fn commit_edit(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some((ix, field, input)) = self.editing.take() else {
            return;
        };
        let text = input.text.trim().to_owned();
        cx.notify();
        let Some(r) = self.rows.get(ix) else {
            return;
        };
        if text.is_empty() || (field == EditField::Expression && text == r.name) {
            return;
        }
        match field {
            EditField::Expression => {
                let args = self
                    .kind
                    .watch_args(json!({"edit": {"index": r.path[0], "expression": text}}));
                run(window, cx, cmds::WATCH, args);
            }
            EditField::Value => {
                let parent = self.parents.get(ix).copied().unwrap_or(0);
                let args = if parent > 0 {
                    json!({"name": r.name, "value": text, "reference": parent})
                } else {
                    json!({"name": r.name, "value": text})
                };
                run(window, cx, cmds::SET_VARIABLE, args);
            }
        }
    }

    fn edit_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some((_, _, input)) = self.editing.as_mut() else {
            return;
        };
        match e.keystroke.key.as_str() {
            "escape" => {
                self.editing = None;
            }
            "enter" => self.commit_edit(window, cx),
            _ => {
                if matches!(input.key(e), InputKey::Ignored) {
                    return;
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    /// Open the context menu on row `ix`.
    pub fn open_menu(&mut self, ix: usize, cx: &mut Context<Self>) {
        if ix < self.rows.len() {
            self.selected = Some(ix);
            self.menu = Some(ix);
            cx.notify();
        }
    }

    /// Run a context menu item on row `ix`.
    pub fn menu_action(
        &mut self,
        ix: usize,
        action: VarsMenu,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        cx.notify();
        let Some(r) = self.rows.get(ix).cloned() else {
            return;
        };
        match action {
            VarsMenu::Copy => cx.write_to_clipboard(gpui::ClipboardItem::new_string(format!(
                "{} = {}",
                r.name, r.value
            ))),
            VarsMenu::CopyValue => cx.write_to_clipboard(gpui::ClipboardItem::new_string(r.value)),
            VarsMenu::EditValue => self.begin_edit(ix, EditField::Value, window, cx),
            VarsMenu::AddWatch => {
                let expression = self.expressions.get(ix).cloned().unwrap_or(r.name);
                run(window, cx, cmds::WATCH, json!({ "add": expression }));
            }
            VarsMenu::Hexadecimal => run(
                window,
                cx,
                eludite_commands::settings::SET,
                json!({"key": HEX_SETTING, "value": !self.hex}),
            ),
            VarsMenu::ExpandAll => cx.emit(VarsEvent::ExpandAll(r.path)),
            VarsMenu::CollapseAll => cx.emit(VarsEvent::CollapseAll),
        }
    }

    /// The list's keys: Up and Down select, Right and Left expand and collapse (Left on a member goes to its parent),
    /// F2 edits (a Watch row's expression, else the value), Delete removes a watch.
    fn list_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        if self.editing.is_some() {
            return self.edit_key(e, window, cx);
        }
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let at = self.selected.unwrap_or(0).min(n - 1);
        match e.keystroke.key.as_str() {
            "down" => self.select(Some((at + 1).min(n - 1)), cx),
            "up" => self.select(Some(at.saturating_sub(1)), cx),
            "home" => self.select(Some(0), cx),
            "end" => self.select(Some(n - 1), cx),
            "right" => {
                if self.rows[at].expanded == Some(false) {
                    cx.emit(ToggleVariable(self.rows[at].path.clone()));
                }
            }
            "left" => {
                let r = &self.rows[at];
                if r.expanded == Some(true) {
                    cx.emit(ToggleVariable(r.path.clone()));
                } else if r.depth > 0 {
                    let parent = &r.path[..r.path.len() - 1];
                    let p = self.rows.iter().position(|x| x.path == parent);
                    self.select(p, cx);
                }
            }
            "f2" => {
                let field = if self.kind.watch_window().is_some() && self.rows[at].depth == 0 {
                    EditField::Expression
                } else {
                    EditField::Value
                };
                self.begin_edit(at, field, window, cx);
            }
            "delete" if self.kind.watch_window().is_some() && self.rows[at].depth == 0 => {
                let args = self
                    .kind
                    .watch_args(json!({ "remove": self.rows[at].path[0] }));
                run(window, cx, cmds::WATCH, args);
            }
            "escape" if self.menu.is_some() => {
                self.menu = None;
                cx.notify();
            }
            _ => return,
        }
        cx.stop_propagation();
    }

    /// Hexadecimal Display: a decimal integer as `0x...`; anything else unchanged.
    pub fn display_value(value: &str, hex: bool) -> String {
        if !hex {
            return value.to_owned();
        }
        match value.parse::<i64>() {
            Ok(n) if n < 0 => format!("0x{:x}", n),
            Ok(n) => format!("0x{n:08x}"),
            Err(_) => value.to_owned(),
        }
    }

    fn menu_panel(&self, ix: usize, cx: &mut Context<Self>) -> gpui::AnyElement {
        let t = self.theme;
        let prefix = self.kind.prefix();
        let watch_top =
            self.kind.watch_window().is_some() && self.rows.get(ix).is_some_and(|r| r.depth == 0);
        let items = VarsMenu::ALL
            .into_iter()
            .filter(|a| !(*a == VarsMenu::AddWatch && self.kind == VarsKind::Watch && watch_top))
            .map(|a| {
                let sel = format!("{prefix}-menu-{}", a.slug());
                let label = if a == VarsMenu::Hexadecimal && self.hex {
                    format!("\u{2713} {}", a.label())
                } else {
                    a.label().to_owned()
                };
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px_2()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .text_size(t.typography.ui)
                    .text_color(t.text)
                    .hover(|s| s.bg(t.menu_hover))
                    .cursor_pointer()
                    .child(label)
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.menu_action(ix, a, window, cx);
                    }))
            });
        gpui::deferred(
            gpui::anchored().snap_to_window_with_margin(px(4.)).child(
                eludite_ui::popup::popup_panel(&t)
                    .id(SharedString::from(format!("{prefix}-menu")))
                    .occlude()
                    .min_w(px(180.))
                    .py_1()
                    .flex()
                    .flex_col()
                    .children(items)
                    .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                        this.menu = None;
                        cx.notify();
                    })),
            ),
        )
        .with_priority(5)
        .into_any_element()
    }

    /// The rows' parents' variables references, as [`super::state::flatten_parents`] lists them.
    pub fn set_parents(&mut self, parents: Vec<i64>) {
        self.parents = parents;
    }

    /// Enter in the value box: `eludite.debug.set_variable` for the selected row (a member through its parent's
    /// reference).
    fn value_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.value.as_mut() else {
            return;
        };
        match input.key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                if let Some(ix) = self.selected
                    && let Some(r) = self.rows.get(ix)
                    && !text.trim().is_empty()
                {
                    let parent = self.parents.get(ix).copied().unwrap_or(0);
                    let args = if parent > 0 {
                        json!({"name": r.name, "value": text.trim(), "reference": parent})
                    } else {
                        json!({"name": r.name, "value": text.trim()})
                    };
                    run(window, cx, cmds::SET_VARIABLE, args);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    pub fn set_rows(&mut self, rows: Vec<FlatRow>, note: Option<String>, cx: &mut Context<Self>) {
        if self.rows != rows || self.note != note {
            self.rows = rows;
            self.note = note;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[FlatRow] {
        &self.rows
    }

    fn input_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let Some(input) = self.input.as_mut() else {
            return;
        };
        match input.key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                if !text.trim().is_empty() {
                    let args = self.kind.watch_args(json!({ "add": text.trim() }));
                    run(window, cx, cmds::WATCH, args);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for VarsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let prefix = self.kind.prefix();
        let toolbar = self.input.as_ref().map(|input| {
            let focused = input.focus.is_focused(window);
            let sel = format!("{prefix}-input");
            div()
                .flex()
                .flex_row()
                .flex_none()
                .h(px(ROW_HEIGHT + 6.))
                .items_center()
                .px_2()
                .child(
                    text_box(sel.clone(), &input.text, "Add item to watch", focused, &t)
                        .track_focus(&input.focus)
                        .key_context("DebugWatchInput")
                        .on_key_down(cx.listener(Self::input_key))
                        .on_click(cx.listener(|this, _, window, cx| {
                            if let Some(i) = &this.input {
                                i.focus.focus(window, cx);
                            }
                            cx.notify();
                        })),
                )
        });
        let watch = self.kind.watch_window().is_some();
        let kind = self.kind;
        let count = self.rows.len();
        let list = uniform_list(
            SharedString::from(format!("{prefix}-rows")),
            count,
            cx.processor(move |this, range: std::ops::Range<usize>, _, cx| {
                let t = this.theme;
                range
                    .filter_map(|ix| {
                        let r = this.rows.get(ix)?.clone();
                        let triangle = match r.expanded {
                            Some(true) => "\u{25E2}",
                            Some(false) => "\u{25B7}",
                            None => "",
                        };
                        let toggle_sel = format!("{prefix}-toggle-{ix}");
                        let path = r.path.clone();
                        let toggle = div()
                            .id(SharedString::from(toggle_sel.clone()))
                            .debug_selector(move || toggle_sel)
                            .flex_none()
                            .w(px(12.))
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(triangle)
                            .on_click(cx.listener(move |_, _, _, cx| {
                                cx.stop_propagation();
                                cx.emit(ToggleVariable(path.clone()));
                            }));
                        let remove = (watch && r.depth == 0).then(|| {
                            let sel = format!("{prefix}-remove-{ix}");
                            let index = r.path[0];
                            div()
                                .id(SharedString::from(sel.clone()))
                                .debug_selector(move || sel)
                                .flex_none()
                                .w(px(16.))
                                .text_color(t.text_muted)
                                .child("\u{2715}")
                                .on_click(cx.listener(move |_, _, window, cx| {
                                    cx.stop_propagation();
                                    let args = kind.watch_args(json!({ "remove": index }));
                                    run(window, cx, cmds::WATCH, args);
                                }))
                        });
                        let path = r.path.clone();
                        // Brief 0077: the in-place edit box replaces the name or the value being edited.
                        let editing = this
                            .editing
                            .as_ref()
                            .filter(|(i, _, _)| *i == ix)
                            .map(|(_, f, input)| (*f, input.text.clone(), input.focus.clone()));
                        let edit_box = |field: EditField| {
                            editing.as_ref().filter(|(f, _, _)| *f == field).map(
                                |(_, text, focus)| {
                                    text_box(format!("{prefix}-edit-{ix}"), text, "", true, &t)
                                        .track_focus(focus)
                                        .key_context("DebugVarsEdit")
                                        .on_key_down(cx.listener(Self::edit_key))
                                        .flex_1()
                                },
                            )
                        };
                        let name_cell = match edit_box(EditField::Expression) {
                            Some(b) => b.into_any_element(),
                            None => {
                                let name_sel = format!("{prefix}-name-{ix}");
                                cell(r.name.clone(), None)
                                    .id(SharedString::from(name_sel.clone()))
                                    .debug_selector(move || name_sel)
                                    .on_click(cx.listener(
                                        move |this, e: &ClickEvent, window, cx| {
                                            if e.click_count() >= 2
                                                && watch
                                                && this.rows.get(ix).is_some_and(|r| r.depth == 0)
                                            {
                                                cx.stop_propagation();
                                                this.begin_edit(
                                                    ix,
                                                    EditField::Expression,
                                                    window,
                                                    cx,
                                                );
                                            }
                                        },
                                    ))
                                    .into_any_element()
                            }
                        };
                        let name = div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_none()
                            .w(px(220.))
                            .overflow_hidden()
                            .pl(px(4. + INDENT * r.depth as f32))
                            .child(toggle)
                            .child(name_cell);
                        let changed = this.changed.get(ix).copied().unwrap_or(false);
                        let shown = Self::display_value(&r.value, this.hex);
                        let value = match edit_box(EditField::Value) {
                            Some(b) => div().flex_1().min_w_0().child(b),
                            None => {
                                let value_sel = format!("{prefix}-value-{ix}");
                                let v = cell(shown, None);
                                let v = if r.error || changed {
                                    v.text_color(gpui::rgb(0xF14C4C))
                                } else {
                                    v
                                };
                                div().flex_1().min_w_0().child(
                                    v.w_full()
                                        .id(SharedString::from(value_sel.clone()))
                                        .debug_selector(move || value_sel)
                                        .on_click(cx.listener(
                                            move |this, e: &ClickEvent, window, cx| {
                                                if e.click_count() >= 2
                                                    && !this.rows.get(ix).is_some_and(|r| r.error)
                                                {
                                                    cx.stop_propagation();
                                                    this.begin_edit(
                                                        ix,
                                                        EditField::Value,
                                                        window,
                                                        cx,
                                                    );
                                                }
                                            },
                                        )),
                                )
                            }
                        };
                        let row = row_base(&t, format!("{prefix}-row-{ix}"))
                            .child(name)
                            .child(value)
                            .child(cell(r.type_name.clone(), Some(160.)))
                            .children(remove)
                            .on_mouse_down(
                                gpui::MouseButton::Right,
                                cx.listener(move |this, _, _, cx| {
                                    cx.stop_propagation();
                                    this.open_menu(ix, cx);
                                }),
                            )
                            .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                                this.focus.focus(window, cx);
                                this.selected = Some(ix);
                                if let Some(v) = this.value.as_mut() {
                                    v.text = r.value.clone();
                                }
                                if e.click_count() >= 2 && r.expanded.is_some() {
                                    cx.emit(ToggleVariable(path.clone()));
                                }
                                cx.notify();
                            }));
                        Some(if this.selected == Some(ix) {
                            row.bg(t.accent).text_color(t.text_on_accent)
                        } else {
                            row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
                        })
                    })
                    .collect()
            }),
        )
        .flex_1();
        let menu = self.menu.map(|ix| self.menu_panel(ix, cx));
        div()
            .id(SharedString::from(prefix))
            .size_full()
            .flex()
            .flex_col()
            .track_focus(&self.focus)
            .key_context("DebugVars")
            .on_key_down(cx.listener(Self::list_key))
            .children(menu)
            .children(toolbar)
            .child(header(
                &t,
                vec![
                    cell("Name", Some(220.)),
                    cell("Value", None),
                    cell("Type", Some(160.)),
                ],
            ))
            .children(self.note.as_deref().map(|n| empty_note(&t, n)))
            .child(list)
            .children(self.value_editor(window, cx))
    }
}

impl VarsWindow {
    /// The Locals window's "Value:" box under the list while a row is selected.
    fn value_editor(&self, window: &Window, cx: &mut Context<Self>) -> Option<gpui::Div> {
        let t = self.theme;
        let input = self.value.as_ref()?;
        let r = self.selected.and_then(|i| self.rows.get(i))?;
        let focused = input.focus.is_focused(window);
        Some(
            div()
                .flex()
                .flex_row()
                .flex_none()
                .gap_2()
                .items_center()
                .p_2()
                .border_t_1()
                .border_color(t.border)
                .text_size(t.typography.ui)
                .child(format!("Value of {}:", r.name))
                .child(
                    text_box(
                        "debug-locals-value",
                        &input.text,
                        "C# expression",
                        focused,
                        &t,
                    )
                    .track_focus(&input.focus)
                    .key_context("DebugLocalsValue")
                    .on_key_down(cx.listener(Self::value_key))
                    .on_click(cx.listener(|this, _, window, cx| {
                        if let Some(v) = &this.value {
                            v.focus.focus(window, cx);
                        }
                        cx.notify();
                    })),
                ),
        )
    }
}

/// A breakpoint row's tooltip with several sessions (brief 0028): its binding in each, `Session 1: bound, 1 hit`.
pub fn breakpoint_tooltip(r: &BreakpointRow) -> Option<String> {
    (r.sessions.len() > 1).then(|| {
        r.sessions
            .iter()
            .map(|s| {
                let bound = if s.verified {
                    "bound".to_owned()
                } else {
                    match &s.message {
                        Some(m) => format!("not bound ({m})"),
                        None => "not bound".to_owned(),
                    }
                };
                format!(
                    "Session {}: {bound}, {} hit{}",
                    s.session,
                    s.hits,
                    if s.hits == 1 { "" } else { "s" }
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    })
}

/// A plain text tooltip.
struct TextTip {
    text: String,
    theme: Theme,
}

impl Render for TextTip {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        div()
            .px_2()
            .py_1()
            .bg(t.panel)
            .border_1()
            .border_color(t.border)
            .text_size(t.typography.ui)
            .text_color(t.text)
            .children(self.text.lines().map(|l| div().child(l.to_owned())))
    }
}

/// One session of the Call Stack and Threads windows' selector (brief 0028).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionChoice {
    pub id: u32,
    /// `1: App (break)`.
    pub label: String,
    pub active: bool,
}

/// The selector of session `id` in window `window` (`callstack` or `threads`).
pub fn session_option(window: &str, id: u32) -> String {
    format!("debug-{window}-session-{id}")
}

/// The session selector bar: nothing with one session.
fn session_bar(
    t: &Theme,
    window: &'static str,
    sessions: &[SessionChoice],
) -> Option<gpui::Stateful<gpui::Div>> {
    if sessions.len() < 2 {
        return None;
    }
    let options = sessions.iter().map(|c| {
        let id = c.id;
        eludite_ui::selector_option(session_option(window, id), c.label.clone(), c.active, t)
            .on_click(move |_, window, cx| {
                run(window, cx, cmds::SELECT_FRAME, json!({ "session": id }))
            })
    });
    Some(
        eludite_ui::selector_bar(format!("debug-{window}-sessions"), "Process:", t)
            .children(options),
    )
}

/// One Call Stack row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StackRow {
    pub name: String,
    pub location: String,
    pub selected: bool,
}

pub struct CallStackWindow {
    theme: Theme,
    rows: Vec<StackRow>,
    sessions: Vec<SessionChoice>,
    // Brief 0077: each row's frame index (`None`: an `[External Code]` row), the selection, the context menu and focus.
    frames: Vec<Option<usize>>,
    /// Each row's file and line (Run To Cursor's target); `None` without source.
    targets: Vec<Option<(String, u32)>>,
    menu: Option<usize>,
    focus: Option<FocusHandle>,
}

/// The Call Stack's context menu (Visual Studio's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StackMenu {
    Copy,
    CopyAll,
    SwitchToFrame,
    RunToCursor,
    ShowExternalCode,
}

impl StackMenu {
    pub const ALL: [StackMenu; 5] = [
        StackMenu::Copy,
        StackMenu::CopyAll,
        StackMenu::SwitchToFrame,
        StackMenu::RunToCursor,
        StackMenu::ShowExternalCode,
    ];

    fn label(self) -> &'static str {
        match self {
            StackMenu::Copy => "Copy",
            StackMenu::CopyAll => "Copy All",
            StackMenu::SwitchToFrame => "Switch To Frame",
            StackMenu::RunToCursor => "Run To Cursor",
            StackMenu::ShowExternalCode => "Show External Code",
        }
    }

    fn slug(self) -> &'static str {
        match self {
            StackMenu::Copy => "copy",
            StackMenu::CopyAll => "copy-all",
            StackMenu::SwitchToFrame => "switch",
            StackMenu::RunToCursor => "run-to-cursor",
            StackMenu::ShowExternalCode => "external",
        }
    }
}

/// The setting Show External Code toggles (brief 0077).
pub const EXTERNAL_CODE_SETTING: &str = "debugger.showExternalCode";

impl CallStackWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            sessions: Vec::new(),
            frames: Vec::new(),
            targets: Vec::new(),
            menu: None,
            focus: None,
        }
    }

    // ----- Brief 0077 -----

    /// Each row's frame index; `None` for an `[External Code]` row. Empty: row `i` is frame `i`.
    pub fn set_frames(
        &mut self,
        frames: Vec<Option<usize>>,
        targets: Vec<Option<(String, u32)>>,
        cx: &mut Context<Self>,
    ) {
        if self.frames != frames || self.targets != targets {
            self.frames = frames;
            self.targets = targets;
            cx.notify();
        }
    }

    fn frame_of(&self, ix: usize) -> Option<usize> {
        if self.frames.is_empty() {
            Some(ix)
        } else {
            self.frames.get(ix).copied().flatten()
        }
    }

    fn row_text(r: &StackRow) -> String {
        if r.location.is_empty() {
            r.name.clone()
        } else {
            format!("{} ({})", r.name, r.location)
        }
    }

    /// Run a context menu item on row `ix`.
    pub fn menu_action(
        &mut self,
        ix: usize,
        action: StackMenu,
        external: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.menu = None;
        cx.notify();
        match action {
            StackMenu::Copy => {
                if let Some(r) = self.rows.get(ix) {
                    cx.write_to_clipboard(gpui::ClipboardItem::new_string(Self::row_text(r)));
                }
            }
            StackMenu::CopyAll => {
                let all: Vec<String> = self.rows.iter().map(Self::row_text).collect();
                cx.write_to_clipboard(gpui::ClipboardItem::new_string(all.join("\n")));
            }
            StackMenu::SwitchToFrame => {
                if let Some(f) = self.frame_of(ix) {
                    run(window, cx, cmds::SELECT_FRAME, json!({ "frame": f }));
                }
            }
            StackMenu::RunToCursor => {
                // Visual Studio runs to the frame's return point: the caller's line, as a one-shot breakpoint.
                if let Some(loc) = self.targets.get(ix).cloned().flatten() {
                    run(
                        window,
                        cx,
                        cmds::RUN_TO_CURSOR,
                        json!({"path": loc.0, "line": loc.1}),
                    );
                }
            }
            StackMenu::ShowExternalCode => run(
                window,
                cx,
                eludite_commands::settings::SET,
                json!({"key": EXTERNAL_CODE_SETTING, "value": !external}),
            ),
        }
    }

    fn key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let n = self.rows.len();
        if n == 0 {
            return;
        }
        let at = self.rows.iter().position(|r| r.selected).unwrap_or(0);
        let next = match e.keystroke.key.as_str() {
            "down" => (at + 1).min(n - 1),
            "up" => at.saturating_sub(1),
            "enter" => at,
            "escape" => {
                self.menu = None;
                cx.notify();
                return;
            }
            _ => return,
        };
        cx.stop_propagation();
        if let Some(f) = self.frame_of(next) {
            run(window, cx, cmds::SELECT_FRAME, json!({ "frame": f }));
        }
    }

    /// The session selector's choices (brief 0028; none or one: no selector).
    pub fn set_sessions(&mut self, sessions: Vec<SessionChoice>, cx: &mut Context<Self>) {
        if self.sessions != sessions {
            self.sessions = sessions;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sessions(&self) -> &[SessionChoice] {
        &self.sessions
    }

    pub fn set_rows(&mut self, rows: Vec<StackRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[StackRow] {
        &self.rows
    }
}

impl Render for CallStackWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focus = self.focus.get_or_insert_with(|| cx.focus_handle()).clone();
        let external = self.rows.iter().any(|r| r.name == "[External Code]");
        // Brief 0077: the context menu.
        let menu = self.menu.map(|ix| {
            let items = StackMenu::ALL.into_iter().map(|a| {
                let sel = format!("debug-callstack-menu-{}", a.slug());
                div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .px_2()
                    .h(px(ROW_HEIGHT))
                    .flex()
                    .items_center()
                    .text_size(t.typography.ui)
                    .text_color(t.text)
                    .hover(|s| s.bg(t.menu_hover))
                    .cursor_pointer()
                    .child(a.label())
                    .on_click(cx.listener(move |this, _, window, cx| {
                        cx.stop_propagation();
                        this.menu_action(ix, a, !external, window, cx);
                    }))
            });
            gpui::deferred(
                gpui::anchored().snap_to_window_with_margin(px(4.)).child(
                    eludite_ui::popup::popup_panel(&t)
                        .id("debug-callstack-menu")
                        .occlude()
                        .min_w(px(180.))
                        .py_1()
                        .flex()
                        .flex_col()
                        .children(items)
                        .on_mouse_down_out(cx.listener(|this, _, _, cx| {
                            this.menu = None;
                            cx.notify();
                        })),
                ),
            )
            .with_priority(5)
        });
        let frames = self.frames.clone();
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
            let frame = if frames.is_empty() {
                Some(ix)
            } else {
                frames.get(ix).copied().flatten()
            };
            // The yellow arrow is the top frame (where execution is); the green one a selected caller.
            let marker = match (ix, r.selected) {
                (0, _) => div().text_color(gpui::rgb(0xFFCC00)).child("\u{27A4}"),
                (_, true) => div().text_color(gpui::rgb(0x5FB760)).child("\u{27A4}"),
                _ => div(),
            };
            let row = row_base(&t, format!("debug-callstack-row-{ix}"))
                .child(marker.flex_none().w(px(18.)).px_1())
                .child(cell(r.name.clone(), None))
                .child(cell(r.location.clone(), Some(220.)))
                .on_mouse_down(
                    gpui::MouseButton::Right,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.menu = Some(ix);
                        cx.notify();
                    }),
                )
                .on_click(cx.listener(move |_, _, window, cx| {
                    if let Some(f) = frame {
                        run(window, cx, cmds::SELECT_FRAME, json!({ "frame": f }))
                    }
                }));
            let row = if frame.is_none() {
                row.text_color(t.text_muted)
            } else {
                row
            };
            if r.selected {
                row.bg(t.menu_hover).text_color(t.text)
            } else {
                row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
            }
        });
        div()
            .id("debug-callstack")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_focus(&focus)
            .key_context("DebugCallStack")
            .on_key_down(cx.listener(Self::key))
            .children(menu)
            .children(session_bar(&t, "callstack", &self.sessions))
            .child(header(
                &t,
                vec![
                    cell("", Some(18.)),
                    cell("Name", None),
                    cell("Location", Some(220.)),
                ],
            ))
            .children(rows)
    }
}

/// One Threads row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ThreadLine {
    pub id: i64,
    pub name: String,
    pub location: String,
    pub current: bool,
}

pub struct ThreadsWindow {
    theme: Theme,
    rows: Vec<ThreadLine>,
    sessions: Vec<SessionChoice>,
}

impl ThreadsWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            sessions: Vec::new(),
        }
    }

    /// The session selector's choices (brief 0028).
    pub fn set_sessions(&mut self, sessions: Vec<SessionChoice>, cx: &mut Context<Self>) {
        if self.sessions != sessions {
            self.sessions = sessions;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn sessions(&self) -> &[SessionChoice] {
        &self.sessions
    }

    pub fn set_rows(&mut self, rows: Vec<ThreadLine>, cx: &mut Context<Self>) {
        if self.rows != rows {
            self.rows = rows;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[ThreadLine] {
        &self.rows
    }
}

impl Render for ThreadsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
            let id = r.id;
            let row = row_base(&t, format!("debug-threads-row-{ix}"))
                .child(
                    cell(if r.current { "\u{27A4}" } else { "" }, Some(18.))
                        .text_color(gpui::rgb(0xFFCC00)),
                )
                .child(cell(r.id.to_string(), Some(80.)))
                .child(cell(r.name.clone(), Some(180.)))
                .child(cell(r.location.clone(), None))
                .on_click(cx.listener(move |_, _, window, cx| {
                    run(window, cx, cmds::SELECT_FRAME, json!({ "thread": id }))
                }));
            row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
        });
        div()
            .id("debug-threads")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .children(session_bar(&t, "threads", &self.sessions))
            .child(header(
                &t,
                vec![
                    cell("", Some(18.)),
                    cell("ID", Some(80.)),
                    cell("Name", Some(180.)),
                    cell("Location", None),
                ],
            ))
            .children(rows)
    }
}

/// The Breakpoints window: every breakpoint with its enabled box, glyph (Visual Studio's diamond for a tracepoint),
/// name (the file and line, or a function breakpoint's function), condition, hit count and When Hit message; a
/// selected one's condition, hit count and message are edited in the fields below the list, and the toolbar's name box
/// adds a function breakpoint. Every edit is `eludite.debug.toggle_breakpoint`.
pub struct BreakpointsWindow {
    theme: Theme,
    rows: Vec<BreakpointRow>,
    selected: Option<usize>,
    condition: LineInput,
    hit: LineInput,
    message: LineInput,
    function: LineInput,
    /// Brief 0077: the search box.
    search: LineInput,
}

/// Brief 0077: the Breakpoints window's search: the row's name, condition, hit count or message contains it
/// (ignoring case).
pub fn breakpoint_matches(r: &BreakpointRow, search: &str) -> bool {
    let s = search.trim().to_lowercase();
    s.is_empty()
        || [
            Some(r.label()),
            r.condition.clone(),
            r.hit_condition.clone(),
            r.log_message.clone(),
        ]
        .into_iter()
        .flatten()
        .any(|t| t.to_lowercase().contains(&s))
}

/// Which text box of the Breakpoints window a key is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BpField {
    Condition,
    Hit,
    Message,
    Function,
    Search,
}

/// The arguments naming a row's breakpoint: its function, or its file and line.
fn target(r: &BreakpointRow) -> serde_json::Value {
    match &r.function {
        Some(f) => json!({ "function": f }),
        None => json!({"path": r.path, "line": r.line}),
    }
}

fn with(mut base: serde_json::Value, extra: serde_json::Value) -> serde_json::Value {
    if let (Some(b), Some(e)) = (base.as_object_mut(), extra.as_object()) {
        for (k, v) in e {
            b.insert(k.clone(), v.clone());
        }
    }
    base
}

impl BreakpointsWindow {
    pub fn new(theme: Theme, cx: &mut App) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            selected: None,
            condition: LineInput::new(cx),
            hit: LineInput::new(cx),
            message: LineInput::new(cx),
            function: LineInput::new(cx),
            search: LineInput::new(cx),
        }
    }

    /// Brief 0077: the rows the search shows, with their indices.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn visible_rows(&self) -> Vec<usize> {
        (0..self.rows.len())
            .filter(|i| breakpoint_matches(&self.rows[*i], &self.search.text))
            .collect()
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn set_search(&mut self, text: &str, cx: &mut Context<Self>) {
        self.search.text = text.to_owned();
        cx.notify();
    }

    pub fn set_rows(&mut self, rows: Vec<BreakpointRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            // Keep the selection on the same breakpoint.
            let key = |r: &BreakpointRow| (r.path.clone(), r.line, r.function.clone());
            let was = self.selected.and_then(|i| self.rows.get(i)).map(key);
            self.rows = rows;
            self.selected = was.and_then(|k| self.rows.iter().position(|r| key(r) == k));
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn rows(&self) -> &[BreakpointRow] {
        &self.rows
    }

    fn select(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected = Some(ix);
        if let Some(r) = self.rows.get(ix) {
            self.condition.text = r.condition.clone().unwrap_or_default();
            self.hit.text = r.hit_condition.clone().unwrap_or_default();
            self.message.text = r.log_message.clone().unwrap_or_default();
        }
        cx.notify();
    }

    /// Apply the fields to the selected breakpoint: its condition, hit count and (a line's) When Hit message.
    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.selected.and_then(|i| self.rows.get(i)) else {
            return;
        };
        let mut args = with(
            target(r),
            json!({"action": "set", "condition": self.condition.text, "hit_condition": self.hit.text}),
        );
        if r.function.is_none() {
            args["log_message"] = json!(self.message.text);
        }
        run(window, cx, cmds::TOGGLE_BREAKPOINT, args);
    }

    fn add_function(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = std::mem::take(&mut self.function.text);
        if !name.trim().is_empty() {
            run(
                window,
                cx,
                cmds::TOGGLE_BREAKPOINT,
                json!({"action": "set", "function": name.trim()}),
            );
        }
        cx.notify();
    }

    fn input(&mut self, field: BpField) -> &mut LineInput {
        match field {
            BpField::Condition => &mut self.condition,
            BpField::Hit => &mut self.hit,
            BpField::Message => &mut self.message,
            BpField::Function => &mut self.function,
            BpField::Search => &mut self.search,
        }
    }

    fn field_key(
        &mut self,
        field: BpField,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match self.input(field).key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                self.input(field).text = text;
                if field == BpField::Function {
                    self.add_function(window, cx);
                } else if field == BpField::Search {
                    // The search filters as it is typed.
                } else {
                    self.apply(window, cx);
                }
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn text_field(
        &self,
        field: BpField,
        selector: &'static str,
        placeholder: &'static str,
        window: &Window,
        cx: &mut Context<Self>,
    ) -> gpui::Stateful<gpui::Div> {
        let t = self.theme;
        let input = match field {
            BpField::Condition => &self.condition,
            BpField::Hit => &self.hit,
            BpField::Message => &self.message,
            BpField::Function => &self.function,
            BpField::Search => &self.search,
        };
        let focus = input.focus.clone();
        text_box(
            selector,
            &input.text,
            placeholder,
            input.focus.is_focused(window),
            &t,
        )
        .track_focus(&input.focus)
        .key_context("DebugBreakpointField")
        .on_key_down(cx.listener(move |this, e, window, cx| this.field_key(field, e, window, cx)))
        .on_click(cx.listener(move |_, _, window, cx| {
            focus.focus(window, cx);
            cx.notify();
        }))
    }
}

impl Render for BreakpointsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_none()
            .gap_1()
            .h(px(ROW_HEIGHT + 6.))
            .items_center()
            .px_2()
            .child(
                toggle_button("debug-bp-delete", "\u{2715} Delete", false, &t).on_click(
                    cx.listener(|this, _, window, cx| {
                        if let Some(r) = this.selected.and_then(|i| this.rows.get(i)) {
                            let args = with(target(r), json!({"action": "delete"}));
                            run(window, cx, cmds::TOGGLE_BREAKPOINT, args);
                        }
                    }),
                ),
            )
            .child(
                toggle_button("debug-bp-delete-all", "Delete All", false, &t).on_click(
                    cx.listener(|_, _, window, cx| {
                        run(
                            window,
                            cx,
                            cmds::TOGGLE_BREAKPOINT,
                            json!({"action": "delete_all"}),
                        )
                    }),
                ),
            )
            // Brief 0077: Enable All, Disable All and the search box.
            .child(
                toggle_button("debug-bp-enable-all", "Enable All", false, &t).on_click(
                    cx.listener(|_, _, window, cx| {
                        run(
                            window,
                            cx,
                            eludite_commands::debug_inspect::ENABLE_BREAKPOINTS,
                            json!({"all": true, "enabled": true}),
                        )
                    }),
                ),
            )
            .child(
                toggle_button("debug-bp-disable-all", "Disable All", false, &t).on_click(
                    cx.listener(|_, _, window, cx| {
                        run(
                            window,
                            cx,
                            eludite_commands::debug_inspect::ENABLE_BREAKPOINTS,
                            json!({"all": true, "enabled": false}),
                        )
                    }),
                ),
            )
            .child(self.text_field(BpField::Search, "debug-bp-search", "Search", window, cx))
            .child(div().flex_none().px_1().child("New Function Breakpoint:"))
            .child(self.text_field(
                BpField::Function,
                "debug-bp-function",
                "Namespace.Type.Method",
                window,
                cx,
            ))
            .child(
                toggle_button("debug-bp-function-add", "Add", false, &t)
                    .on_click(cx.listener(|this, _, window, cx| this.add_function(window, cx))),
            );
        let search = self.search.text.clone();
        let rows = self
            .rows
            .iter()
            .enumerate()
            .filter(|(_, r)| breakpoint_matches(r, &search))
            .map(|(ix, r)| {
                let enabled = r.enabled;
                let toggle_args = with(target(r), json!({"action": "set", "enabled": !enabled}));
                let check_sel = format!("debug-bp-enabled-{ix}");
                let check = div()
                    .id(SharedString::from(check_sel.clone()))
                    .debug_selector(move || check_sel)
                    .flex_none()
                    .w(px(22.))
                    .px_1()
                    .child(if enabled { "\u{2611}" } else { "\u{2610}" })
                    .on_click(cx.listener(move |_, _, window, cx| {
                        cx.stop_propagation();
                        run(window, cx, cmds::TOGGLE_BREAKPOINT, toggle_args.clone());
                    }));
                let glyph_color = if !r.enabled || !r.verified {
                    gpui::rgb(0x9C9C9C)
                } else {
                    gpui::rgb(0xE51400)
                };
                // Visual Studio's glyphs: a circle, a tracepoint's diamond; hollow when disabled.
                let glyph = match (r.kind, r.enabled) {
                    (BreakpointKind::Tracepoint, true) => "\u{25C6}",
                    (BreakpointKind::Tracepoint, false) => "\u{25C7}",
                    (_, true) => "\u{25CF}",
                    (_, false) => "\u{25CB}",
                };
                let glyph_sel = format!("debug-bp-glyph-{ix}");
                let mut label = r.label();
                if r.temporary {
                    label.push_str(" (temporary)");
                }
                let open = r.path.clone().zip(r.line);
                let tip = breakpoint_tooltip(r);
                let row = row_base(&t, format!("debug-bp-row-{ix}"));
                // Its binding per session (brief 0028), as a tooltip.
                let row = match tip {
                    Some(text) => row.tooltip(move |_, cx| {
                        let text = text.clone();
                        cx.new(|_| TextTip { text, theme: t }).into()
                    }),
                    None => row,
                };
                let row = row
                    .child(check)
                    .child(
                        cell(glyph, Some(16.))
                            .text_color(glyph_color)
                            .id(SharedString::from(glyph_sel.clone()))
                            .debug_selector(move || glyph_sel),
                    )
                    .child(cell(label, None))
                    .child(cell(r.condition.clone().unwrap_or_default(), Some(140.)))
                    .child(cell(
                        r.hit_condition
                            .as_deref()
                            .map(|h| format!("{h} (hit {})", r.hits))
                            .unwrap_or_else(|| format!("hit {}", r.hits)),
                        Some(110.),
                    ))
                    .child(cell(
                        r.log_message
                            .as_deref()
                            .map(|m| format!("Print: {m}"))
                            .unwrap_or_default(),
                        Some(180.),
                    ))
                    .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                        this.select(ix, cx);
                        if e.click_count() >= 2
                            && let Some((path, line)) = open.clone()
                        {
                            run(
                                window,
                                cx,
                                eludite_commands::workspace::FILE_OPEN,
                                json!({"path": path, "line": line}),
                            );
                        }
                    }));
                if self.selected == Some(ix) {
                    row.bg(t.accent).text_color(t.text_on_accent)
                } else {
                    row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
                }
            });
        let rows: Vec<_> = rows.collect();
        let editor = self
            .selected
            .and_then(|i| self.rows.get(i))
            .map(|r| (r.function.is_none(), r.remove_after))
            .map(|(line_bp, remove_after)| {
                let fields = div()
                    .flex()
                    .flex_row()
                    .flex_none()
                    .gap_2()
                    .items_center()
                    .child("Condition:")
                    .child(self.text_field(
                        BpField::Condition,
                        "debug-bp-condition",
                        "C# expression",
                        window,
                        cx,
                    ))
                    .child("Hit count:")
                    .child(self.text_field(
                        BpField::Hit,
                        "debug-bp-hit",
                        "N, >=N or %N",
                        window,
                        cx,
                    ));
                // When Hit... Print a message and continue (a line breakpoint becomes a tracepoint).
                let when_hit = line_bp.then(|| {
                    div()
                        .flex()
                        .flex_row()
                        .flex_none()
                        .gap_2()
                        .items_center()
                        .child("When hit, print a message and continue:")
                        .child(self.text_field(
                            BpField::Message,
                            "debug-bp-message",
                            "{expression}, $FUNCTION, $CALLER, $TID, $TNAME",
                            window,
                            cx,
                        ))
                });
                div()
                    .flex()
                    .flex_col()
                    .flex_none()
                    .gap_1()
                    .p_2()
                    .border_t_1()
                    .border_color(t.border)
                    .text_size(t.typography.ui)
                    .child(fields)
                    .children(when_hit)
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .flex_none()
                            .gap_2()
                            .child(
                                toggle_button("debug-bp-apply", "Apply", false, &t).on_click(
                                    cx.listener(|this, _, window, cx| this.apply(window, cx)),
                                ),
                            )
                            // Visual Studio's "Delete breakpoint when hit" (`remove_after`).
                            .child(
                                toggle_button(
                                    "debug-bp-remove-after",
                                    "Delete breakpoint when hit",
                                    remove_after,
                                    &t,
                                )
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    if let Some(r) = this.selected.and_then(|i| this.rows.get(i)) {
                                        let args = with(
                                            target(r),
                                            json!({"action": "set", "remove_after": !remove_after}),
                                        );
                                        run(window, cx, cmds::TOGGLE_BREAKPOINT, args);
                                    }
                                })),
                            ),
                    )
            });
        div()
            .id("debug-breakpoints")
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(header(
                &t,
                vec![
                    cell("", Some(38.)),
                    cell("Name", None),
                    cell("Condition", Some(140.)),
                    cell("Hit Count", Some(110.)),
                    cell("When Hit", Some(180.)),
                ],
            ))
            .child(
                div()
                    .id("debug-bp-list")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(rows),
            )
            .children(editor)
    }
}

/// The Exception Settings window: Common Language Runtime Exceptions with its Thrown and User-Unhandled boxes, and the
/// exception types under it with theirs and Remove; a name box and Add add a type, Clear removes them all; then Rust
/// panics (brief 0029, one box in the Thrown column: a native session breaks at `rust_panic`). Every edit is
/// `eludite.debug.exception_settings`.
pub struct ExceptionsWindow {
    theme: Theme,
    settings: ExceptionSettingsRow,
    add: LineInput,
    /// A browser session is live: the JavaScript Exceptions group shows (brief 0038).
    javascript: bool,
}

impl ExceptionsWindow {
    pub fn new(theme: Theme, cx: &mut App) -> Self {
        Self {
            theme,
            settings: ExceptionSettingsRow::default(),
            add: LineInput::new(cx),
            javascript: false,
        }
    }

    /// Show the JavaScript Exceptions group (brief 0038): its boxes are the same Thrown and User-Unhandled settings,
    /// which a browser session's vscode-js-debug gets as its `all` and `uncaught` filters.
    pub fn set_javascript(&mut self, on: bool, cx: &mut Context<Self>) {
        if self.javascript != on {
            self.javascript = on;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn javascript(&self) -> bool {
        self.javascript
    }

    pub fn set(&mut self, settings: ExceptionSettingsRow, cx: &mut Context<Self>) {
        if self.settings != settings {
            self.settings = settings;
            cx.notify();
        }
    }

    #[cfg_attr(not(test), allow(dead_code))]
    pub fn settings(&self) -> &ExceptionSettingsRow {
        &self.settings
    }

    fn add_type(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let name = std::mem::take(&mut self.add.text);
        if !name.trim().is_empty() {
            run(
                window,
                cx,
                cmds::EXCEPTION_SETTINGS,
                json!({"types": [{"type": name.trim(), "break_when_thrown": true}]}),
            );
        }
        cx.notify();
    }

    fn add_key(&mut self, e: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        match self.add.key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                self.add.text = text;
                self.add_type(window, cx);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

impl Render for ExceptionsWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let s = self.settings.clone();
        let check = |sel: String, on: bool, args: serde_json::Value| {
            div()
                .id(SharedString::from(sel.clone()))
                .debug_selector(move || sel)
                .flex_none()
                .w(px(90.))
                .px_1()
                .child(if on { "\u{2611}" } else { "\u{2610}" })
                .on_click(cx.listener(move |_, _, window, cx| {
                    cx.stop_propagation();
                    run(window, cx, cmds::EXCEPTION_SETTINGS, args.clone())
                }))
        };
        let focused = self.add.focus.is_focused(window);
        let toolbar = div()
            .flex()
            .flex_row()
            .flex_none()
            .gap_1()
            .h(px(ROW_HEIGHT + 6.))
            .items_center()
            .px_2()
            .child(
                text_box(
                    "debug-exc-add-input",
                    &self.add.text,
                    "Exception type, e.g. System.InvalidOperationException",
                    focused,
                    &t,
                )
                .track_focus(&self.add.focus)
                .key_context("DebugExceptionType")
                .on_key_down(cx.listener(Self::add_key))
                .on_click(cx.listener(|this, _, window, cx| {
                    this.add.focus.focus(window, cx);
                    cx.notify();
                })),
            )
            .child(
                toggle_button("debug-exc-add", "Add", false, &t)
                    .on_click(cx.listener(|this, _, window, cx| this.add_type(window, cx))),
            )
            .child(
                toggle_button("debug-exc-clear", "Clear Types", false, &t).on_click(cx.listener(
                    |_, _, window, cx| {
                        run(window, cx, cmds::EXCEPTION_SETTINGS, json!({"clear": true}))
                    },
                )),
            );
        let category = row_base(&t, "debug-exc-category".to_owned())
            .text_color(t.text)
            .child(check(
                "debug-exc-thrown".into(),
                s.break_when_thrown,
                json!({"break_when_thrown": !s.break_when_thrown}),
            ))
            .child(check(
                "debug-exc-unhandled".into(),
                s.break_when_user_unhandled,
                json!({"break_when_user_unhandled": !s.break_when_user_unhandled}),
            ))
            .child(cell("\u{25E2} Common Language Runtime Exceptions", None));
        let types = s.types.iter().enumerate().map(|(ix, ty)| {
            let entry = |thrown: bool, unhandled: bool| {
                json!({"types": [{"type": ty.type_name, "break_when_thrown": thrown,
                                  "break_when_user_unhandled": unhandled}]})
            };
            let remove_sel = format!("debug-exc-type-remove-{ix}");
            let name = ty.type_name.clone();
            row_base(&t, format!("debug-exc-type-{ix}"))
                .text_color(t.text)
                .hover(|st| st.bg(t.menu_hover))
                .child(check(
                    format!("debug-exc-type-thrown-{ix}"),
                    ty.break_when_thrown,
                    entry(!ty.break_when_thrown, ty.break_when_user_unhandled),
                ))
                .child(check(
                    format!("debug-exc-type-unhandled-{ix}"),
                    ty.break_when_user_unhandled,
                    entry(ty.break_when_thrown, !ty.break_when_user_unhandled),
                ))
                .child(cell(ty.type_name.clone(), None).pl(px(INDENT + 4.)))
                .child(
                    div()
                        .id(SharedString::from(remove_sel.clone()))
                        .debug_selector(move || remove_sel)
                        .flex_none()
                        .w(px(70.))
                        .text_color(t.text_muted)
                        .child("\u{2715} Remove")
                        .on_click(cx.listener(move |_, _, window, cx| {
                            cx.stop_propagation();
                            run(
                                window,
                                cx,
                                cmds::EXCEPTION_SETTINGS,
                                json!({ "remove": name }),
                            )
                        })),
                )
        });
        let types: Vec<_> = types.collect();
        let rust_panics = row_base(&t, "debug-exc-rust-panic-row".to_owned())
            .text_color(t.text)
            .child(check(
                "debug-exc-rust-panic".into(),
                s.break_on_rust_panic,
                json!({"break_on_rust_panic": !s.break_on_rust_panic}),
            ))
            .child(div().flex_none().w(px(90.)))
            .child(cell("Rust panics", None));
        // Brief 0038: while a browser session is live, the same two boxes as vscode-js-debug's filters.
        let javascript = self.javascript.then(|| {
            row_base(&t, "debug-exc-javascript".to_owned())
                .text_color(t.text)
                .child(check(
                    "debug-exc-javascript-thrown".into(),
                    s.break_when_thrown,
                    json!({"break_when_thrown": !s.break_when_thrown}),
                ))
                .child(check(
                    "debug-exc-javascript-unhandled".into(),
                    s.break_when_user_unhandled,
                    json!({"break_when_user_unhandled": !s.break_when_user_unhandled}),
                ))
                .child(cell(
                    "\u{25E2} JavaScript Exceptions (caught: all, uncaught: User-Unhandled)",
                    None,
                ))
        });
        div()
            .id("debug-exceptions")
            .size_full()
            .flex()
            .flex_col()
            .child(toolbar)
            .child(header(
                &t,
                vec![
                    cell("Thrown", Some(90.)),
                    cell("User-Unhandled", Some(90.)),
                    cell("Break When", None),
                ],
            ))
            .child(category)
            .child(
                div()
                    .id("debug-exc-types")
                    .flex_1()
                    .overflow_y_scroll()
                    .children(types)
                    // Brief 0029: a native (Cargo) session breaks at `rust_panic`.
                    .child(rust_panics)
                    .children(javascript),
            )
    }
}

/// The debugger windows.
#[derive(Clone)]
pub struct DebugWindows {
    pub locals: Entity<VarsWindow>,
    pub watch: Entity<VarsWindow>,
    pub call_stack: Entity<CallStackWindow>,
    pub threads: Entity<ThreadsWindow>,
    pub breakpoints: Entity<BreakpointsWindow>,
    pub exceptions: Entity<ExceptionsWindow>,
    // Brief 0077: Watch 2 to 4, Autos, Immediate and Modules.
    pub watch_more: [Entity<VarsWindow>; 3],
    pub autos: Entity<VarsWindow>,
    pub immediate: Entity<super::immediate::ImmediateWindow>,
    pub modules: Entity<super::modules::ModulesWindow>,
}

impl DebugWindows {
    pub fn new<T>(theme: Theme, cx: &mut Context<T>) -> Self {
        Self {
            locals: cx.new(|cx| VarsWindow::new(theme, VarsKind::Locals, cx)),
            watch: cx.new(|cx| VarsWindow::new(theme, VarsKind::Watch, cx)),
            call_stack: cx.new(|_| CallStackWindow::new(theme)),
            threads: cx.new(|_| ThreadsWindow::new(theme)),
            breakpoints: cx.new(|cx| BreakpointsWindow::new(theme, cx)),
            exceptions: cx.new(|cx| ExceptionsWindow::new(theme, cx)),
            watch_more: [VarsKind::Watch2, VarsKind::Watch3, VarsKind::Watch4]
                .map(|k| cx.new(|cx| VarsWindow::new(theme, k, cx))),
            autos: cx.new(|cx| VarsWindow::new(theme, VarsKind::Autos, cx)),
            immediate: cx.new(|cx| super::immediate::ImmediateWindow::new(theme, cx)),
            modules: cx.new(|_| super::modules::ModulesWindow::new(theme)),
        }
    }

    /// The variables window of `kind`, if it is a window (QuickWatch's and a DataTip's trees are not).
    pub fn vars(&self, kind: VarsKind) -> Option<&Entity<VarsWindow>> {
        Some(match kind {
            VarsKind::Locals => &self.locals,
            VarsKind::Watch => &self.watch,
            VarsKind::Watch2 => &self.watch_more[0],
            VarsKind::Watch3 => &self.watch_more[1],
            VarsKind::Watch4 => &self.watch_more[2],
            VarsKind::Autos => &self.autos,
            VarsKind::QuickWatch | VarsKind::DataTip => return None,
        })
    }

    /// The body of tool window `id`, if it is one of these.
    pub fn body(&self, id: &str) -> Option<gpui::AnyElement> {
        use eludite_docking::ids;
        use gpui::{StyleRefinement, Styled as _};
        let style = || StyleRefinement::default().size_full();
        Some(match id {
            ids::LOCALS => self.locals.clone().cached(style()).into_any_element(),
            ids::WATCH => self.watch.clone().cached(style()).into_any_element(),
            ids::CALL_STACK => self.call_stack.clone().cached(style()).into_any_element(),
            ids::THREADS => self.threads.clone().cached(style()).into_any_element(),
            ids::BREAKPOINTS => self.breakpoints.clone().cached(style()).into_any_element(),
            ids::EXCEPTION_SETTINGS => self.exceptions.clone().cached(style()).into_any_element(),
            // Brief 0077.
            ids::WATCH_2 => self.watch_more[0]
                .clone()
                .cached(style())
                .into_any_element(),
            ids::WATCH_3 => self.watch_more[1]
                .clone()
                .cached(style())
                .into_any_element(),
            ids::WATCH_4 => self.watch_more[2]
                .clone()
                .cached(style())
                .into_any_element(),
            ids::AUTOS => self.autos.clone().cached(style()).into_any_element(),
            ids::IMMEDIATE => self.immediate.clone().cached(style()).into_any_element(),
            ids::MODULES => self.modules.clone().cached(style()).into_any_element(),
            _ => return None,
        })
    }
}

#[cfg(test)]
mod tests {
    use gpui::{TestAppContext, size};

    use super::*;

    /// A deep stack in a short Call Stack window keeps every row at full height and scrolls, rather than squeezing
    /// the rows on top of each other (seen in the brief 0018 manual run).
    #[gpui::test]
    fn call_stack_rows_keep_their_height_when_the_stack_overflows(cx: &mut TestAppContext) {
        let (view, vcx) = cx.add_window_view(|_, _| CallStackWindow::new(Theme::dark()));
        vcx.simulate_resize(size(px(600.), px(160.)));
        let rows = (0..40)
            .map(|i| StackRow {
                name: format!("Frame{i}()"),
                location: format!("File.cs, line {i}"),
                selected: i == 0,
            })
            .collect();
        view.update(vcx, |w, cx| w.set_rows(rows, cx));
        vcx.run_until_parked();
        for sel in [
            "debug-callstack-row-0",
            "debug-callstack-row-1",
            "debug-callstack-row-2",
        ] {
            let b = vcx.debug_bounds(sel).expect("row drawn");
            assert_eq!(b.size.height, px(ROW_HEIGHT), "{sel}");
        }
        let (a, b) = (
            vcx.debug_bounds("debug-callstack-row-0").unwrap(),
            vcx.debug_bounds("debug-callstack-row-1").unwrap(),
        );
        assert_eq!(b.origin.y - a.origin.y, px(ROW_HEIGHT));
    }
}

// ----- Debug > Attach to Process... (brief 0027) -----

/// Debug selectors of the Attach to Process dialog.
pub const ATTACH_DIALOG: &str = "attach-dialog";
pub const ATTACH_FILTER: &str = "attach-filter";
pub const ATTACH_REFRESH: &str = "attach-refresh";
pub const ATTACH_ATTACH: &str = "attach-attach";
pub const ATTACH_CANCEL: &str = "attach-cancel";

/// The row of process `pid`.
pub fn attach_row(pid: u32) -> String {
    format!("attach-row-{pid}")
}

/// The row of browser tab `tab` (brief 0038).
pub fn attach_tab_row(tab: &str) -> String {
    format!("attach-tab-{tab}")
}

/// What the dialog asks the shell to do: `eludite.debug.processes` (Refresh), `eludite.debug.attach` (Attach), close.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachEvent {
    Refresh {
        filter: Option<String>,
    },
    Attach {
        pid: u32,
    },
    /// A browser tab, with vscode-js-debug (brief 0038).
    AttachTab {
        tab: String,
    },
    Close,
}

/// What the dialog has selected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AttachPick {
    Process(u32),
    Tab(String),
}

/// Visual Studio's Attach to Process dialog: the processes (`eludite.debug.processes`) with their id, name, runtime,
/// whether Eludite started them and their command line; a filter box (it narrows the list as you type; Refresh lists
/// again with it); Refresh, Attach (the selected process, or a double click) and Cancel. Mono programs need their
/// debugger agent, which the dialog says.
pub struct AttachDialog {
    theme: Theme,
    rows: Vec<cmds::ProcessRow>,
    /// The browser's tabs, under the Web Browser heading (brief 0038).
    tabs: Vec<cmds::AttachTabRow>,
    /// Debug > Attach to Browser Tab...: the tabs only.
    tabs_only: bool,
    filter: LineInput,
    selected: Option<u32>,
    selected_tab: Option<String>,
    message: Option<String>,
    focus: FocusHandle,
}

impl EventEmitter<AttachEvent> for AttachDialog {}

impl gpui::Focusable for AttachDialog {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl AttachDialog {
    pub fn new(theme: Theme, cx: &mut Context<Self>) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            tabs: Vec::new(),
            tabs_only: false,
            filter: LineInput::new(cx),
            selected: None,
            selected_tab: None,
            message: Some("Listing processes\u{2026}".into()),
            focus: cx.focus_handle(),
        }
    }

    /// The browser's tabs (`eludite.debug.processes`' `tabs`; brief 0038).
    pub fn set_tabs(&mut self, tabs: Vec<cmds::AttachTabRow>, cx: &mut Context<Self>) {
        self.tabs = tabs;
        if self
            .selected_tab
            .as_ref()
            .is_some_and(|t| !self.tabs.iter().any(|r| r.id == *t))
        {
            self.selected_tab = None;
        }
        self.preselect_tab();
        cx.notify();
    }

    /// Debug > Attach to Browser Tab... lists the tabs only (brief 0038).
    pub fn set_tabs_only(&mut self, on: bool, cx: &mut Context<Self>) {
        self.tabs_only = on;
        self.preselect_tab();
        cx.notify();
    }

    /// Listing the tabs only, the first is selected until one is picked: Enter attaches to it.
    fn preselect_tab(&mut self) {
        if self.tabs_only && self.selected_tab.is_none() {
            self.selected_tab = self.visible_tabs().first().map(|r| r.id.clone());
            if self.selected_tab.is_some() {
                self.selected = None;
            }
        }
    }

    /// The tabs the filter leaves (title or url containing it).
    pub fn visible_tabs(&self) -> Vec<&cmds::AttachTabRow> {
        let needle = self.filter.text.trim().to_lowercase();
        self.tabs
            .iter()
            .filter(|r| {
                needle.is_empty()
                    || r.title.to_lowercase().contains(&needle)
                    || r.url.to_lowercase().contains(&needle)
            })
            .collect()
    }

    pub fn select_tab(&mut self, tab: &str, cx: &mut Context<Self>) {
        self.selected_tab = Some(tab.to_owned());
        self.selected = None;
        cx.notify();
    }

    /// What is selected.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn pick(&self) -> Option<AttachPick> {
        self.selected_tab
            .clone()
            .map(AttachPick::Tab)
            .or(self.selected.map(AttachPick::Process))
    }

    /// The listing (`eludite.debug.processes`' rows), or why there is none.
    pub fn set_rows(
        &mut self,
        rows: Vec<cmds::ProcessRow>,
        message: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.rows = rows;
        self.message = message;
        if self
            .selected
            .is_some_and(|pid| !self.rows.iter().any(|r| r.pid == pid))
        {
            self.selected = None;
        }
        cx.notify();
    }

    /// The filter box's text, when it has any.
    pub fn filter_text(&self) -> Option<String> {
        let f = self.filter.text.trim();
        (!f.is_empty()).then(|| f.to_owned())
    }

    /// The rows the filter leaves (name or command line containing it, case aside).
    pub fn visible(&self) -> Vec<&cmds::ProcessRow> {
        let needle = self.filter.text.trim().to_lowercase();
        self.rows
            .iter()
            .filter(|r| {
                needle.is_empty()
                    || r.name.to_lowercase().contains(&needle)
                    || r.command_line.to_lowercase().contains(&needle)
            })
            .collect()
    }

    // Read only by a Linux-only test.
    #[cfg_attr(any(not(test), not(target_os = "linux")), allow(dead_code))]
    pub fn selected(&self) -> Option<u32> {
        self.selected
    }

    pub fn select(&mut self, pid: u32, cx: &mut Context<Self>) {
        self.selected = Some(pid);
        self.selected_tab = None;
        cx.notify();
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
        if let Some(tab) = self
            .selected_tab
            .clone()
            .filter(|t| self.visible_tabs().iter().any(|r| r.id == *t))
        {
            cx.emit(AttachEvent::AttachTab { tab });
            return;
        }
        if let Some(pid) = self
            .selected
            .filter(|pid| self.visible().iter().any(|r| r.pid == *pid))
        {
            cx.emit(AttachEvent::Attach { pid });
        }
    }

    fn filter_key(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match self.filter.key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                self.filter.text = text;
                cx.emit(AttachEvent::Refresh {
                    filter: self.filter_text(),
                });
            }
        }
        cx.stop_propagation();
        cx.notify();
    }

    fn key_down(&mut self, e: &KeyDownEvent, _: &mut Window, cx: &mut Context<Self>) {
        match e.keystroke.key.as_str() {
            "escape" => cx.emit(AttachEvent::Close),
            "enter" => self.attach(cx),
            _ => return,
        }
        cx.stop_propagation();
    }
}

impl Render for AttachDialog {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let focused = self.filter.focus.is_focused(window);
        let header = div()
            .flex()
            .flex_row()
            .h(px(ROW_HEIGHT))
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(t.border)
            .child(cell("Process", Some(160.)))
            .child(cell("ID", Some(70.)))
            .child(cell("Runtime", Some(70.)))
            .child(cell("Launched by Eludite", Some(130.)))
            .child(cell("Command Line", None));
        let selected = self.selected;
        let rows: Vec<_> = self
            .visible()
            .into_iter()
            .map(|r| {
                let pid = r.pid;
                let sel = attach_row(pid);
                let runtime = match &r.debugger_agent {
                    Some(a) => format!("{} ({a})", r.runtime),
                    None => r.runtime.clone(),
                };
                let row = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .flex()
                    .flex_row()
                    .h(px(ROW_HEIGHT))
                    .items_center()
                    .cursor_pointer()
                    .child(cell(r.name.clone(), Some(160.)))
                    .child(cell(pid.to_string(), Some(70.)))
                    .child(cell(runtime, Some(70.)))
                    .child(cell(
                        if r.launched_by_eludite { "Yes" } else { "" },
                        Some(130.),
                    ))
                    .child(cell(r.command_line.clone(), None))
                    .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                        this.select(pid, cx);
                        if e.click_count() >= 2 {
                            this.attach(cx);
                        }
                    }));
                if selected == Some(pid) {
                    row.bg(t.accent).text_color(t.text_on_accent)
                } else {
                    row.hover(|s| s.bg(t.menu_hover))
                }
            })
            .collect();
        // Brief 0038: the browser's tabs under a Web Browser heading, attached with vscode-js-debug.
        let selected_tab = self.selected_tab.clone();
        let tab_rows: Vec<_> = self
            .visible_tabs()
            .into_iter()
            .map(|r| {
                let id = r.id.clone();
                let sel = attach_tab_row(&id);
                let note = r
                    .session
                    .map(|s| format!("debugged in session {s}"))
                    .unwrap_or_default();
                let row = div()
                    .id(SharedString::from(sel.clone()))
                    .debug_selector(move || sel)
                    .flex()
                    .flex_row()
                    .h(px(ROW_HEIGHT))
                    .items_center()
                    .cursor_pointer()
                    .child(cell(
                        if r.title.is_empty() {
                            r.url.clone()
                        } else {
                            r.title.clone()
                        },
                        Some(230.),
                    ))
                    .child(cell(r.id.clone(), Some(70.)))
                    .child(cell(note, Some(130.)))
                    .child(cell(r.url.clone(), None))
                    .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                        this.select_tab(&id, cx);
                        if e.click_count() >= 2 {
                            this.attach(cx);
                        }
                    }));
                if selected_tab.as_deref() == Some(r.id.as_str()) {
                    row.bg(t.accent).text_color(t.text_on_accent)
                } else {
                    row.hover(|s| s.bg(t.menu_hover))
                }
            })
            .collect();
        let web_heading = div()
            .id("attach-web-browser")
            .debug_selector(|| "attach-web-browser".into())
            .flex()
            .flex_row()
            .h(px(ROW_HEIGHT))
            .items_center()
            .font_weight(FontWeight::SEMIBOLD)
            .border_b_1()
            .border_color(t.border)
            .child(cell("Web Browser", Some(230.)))
            .child(cell("Tab", Some(70.)))
            .child(cell("", Some(130.)))
            .child(cell("Address", None));
        let no_tabs = tab_rows.is_empty().then(|| {
            div()
                .px_2()
                .text_color(t.text_muted)
                .child("No page is open in the Web Browser window.")
        });
        let tabs_only = self.tabs_only;
        let can_attach = selected.is_some_and(|pid| self.visible().iter().any(|r| r.pid == pid))
            || self
                .selected_tab
                .as_ref()
                .is_some_and(|tab| self.visible_tabs().iter().any(|r| r.id == *tab));
        let filter = text_box(
            ATTACH_FILTER,
            &self.filter.text,
            if tabs_only {
                "Filter tabs"
            } else {
                "Filter processes"
            },
            focused,
            &t,
        )
        .w(px(300.))
        .track_focus(&self.filter.focus)
        .key_context("AttachFilter")
        .on_key_down(cx.listener(Self::filter_key))
        .on_click(cx.listener(|this, _, window, cx| {
            this.filter.focus.focus(window, cx);
            cx.notify();
        }));
        let button = |id: &'static str, label: &'static str, default: bool, enabled: bool| {
            eludite_ui::push_button(id, label, default, enabled, &t)
        };
        let panel = eludite_ui::dialog_panel(
            &t,
            if tabs_only {
                "Attach to Browser Tab"
            } else {
                "Attach to Process"
            },
        )
            .id(ATTACH_DIALOG)
            .debug_selector(|| ATTACH_DIALOG.into())
            .track_focus(&self.focus)
            .key_context("AttachDialog")
            .on_key_down(cx.listener(Self::key_down))
            .occlude()
            .w(px(860.))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .p_2()
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .items_center()
                            .child(if tabs_only {
                                "Pages to debug with vscode-js-debug"
                            } else {
                                "Available processes and pages"
                            })
                            .child(div().flex_1())
                            .child(filter),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .h(px(320.))
                            .border_1()
                            .border_color(t.border)
                            .bg(t.background)
                            .child(
                                div()
                                    .id("attach-rows")
                                    .flex()
                                    .flex_col()
                                    .overflow_y_scroll()
                                    .child(web_heading)
                                    .children(tab_rows)
                                    .children(no_tabs)
                                    .children((!tabs_only).then_some(header))
                                    .children(if tabs_only { Vec::new() } else { rows }),
                            ),
                    )
                    .children(self.message.clone().map(|m| {
                        div().text_color(t.text_muted).child(m)
                    }))
                    .child(
                        div()
                            .text_size(t.typography.small)
                            .text_color(t.text_muted)
                            .child(
                                "A Mono program can be attached to only when it was started with a debugger agent that \
                                 listens: mono --debug --debugger-agent=transport=dt_socket,server=y,\
                                 address=127.0.0.1:PORT,suspend=n program.exe",
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_row()
                            .gap_2()
                            .justify_end()
                            .child(button(ATTACH_REFRESH, "Refresh", false, true).on_click(
                                cx.listener(|this, _, _, cx| {
                                    cx.emit(AttachEvent::Refresh {
                                        filter: this.filter_text(),
                                    })
                                }),
                            ))
                            .child(
                                button(ATTACH_ATTACH, "Attach", true, can_attach).on_click(
                                    cx.listener(|this, _, _, cx| this.attach(cx)),
                                ),
                            )
                            .child(button(ATTACH_CANCEL, "Cancel", false, true).on_click(
                                cx.listener(|_, _, _, cx| cx.emit(AttachEvent::Close)),
                            )),
                    ),
            );
        let viewport = window.viewport_size();
        let at = gpui::point(
            ((viewport.width - px(860.)) / 2.).max(px(0.)),
            (viewport.height / 8.).max(px(0.)),
        );
        gpui::deferred(gpui::anchored().position(at).child(panel)).with_priority(5)
    }
}
