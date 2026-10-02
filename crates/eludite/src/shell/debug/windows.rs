//! The debugger's tool windows (brief 0018), as Visual Studio lays them out: Locals and Watch 1 (Name, Value, Type,
//! expanding lazily), Call Stack, Threads, Breakpoints (enable, condition, hit count, delete), Exception Settings
//! (Common Language Runtime Exceptions). The program's output goes to the Output window's Debug source (brief 0020
//! retired the Debug Console window); expressions are evaluated in the Watch window.
//!
//! Each window shows a snapshot the shell gives it after the debugger's state changes, and turns clicks into the
//! `eludite.debug.*` commands by dispatching [`RunCommand`], so they reach the same command bus agents use. Expanding
//! a variable is view state (like expanding a Workspace folder): it is an event the shell answers by fetching the
//! members.

use eludite_commands::debug::{self as cmds, BreakpointRow, ExceptionSettingsRow};
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
    fn new(cx: &mut App) -> Self {
        Self {
            text: String::new(),
            focus: cx.focus_handle(),
        }
    }

    /// Apply a key: typing, Backspace, Escape (clear) and Enter (submit, clearing the box).
    fn key(&mut self, event: &KeyDownEvent) -> InputKey {
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

/// Locals or Watch 1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VarsKind {
    Locals,
    Watch,
}

impl VarsKind {
    fn prefix(self) -> &'static str {
        match self {
            VarsKind::Locals => "debug-locals",
            VarsKind::Watch => "debug-watch",
        }
    }
}

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
}

impl EventEmitter<ToggleVariable> for VarsWindow {}

impl VarsWindow {
    pub fn new(theme: Theme, kind: VarsKind, cx: &mut App) -> Self {
        Self {
            theme,
            kind,
            rows: Vec::new(),
            note: None,
            selected: None,
            input: (kind == VarsKind::Watch).then(|| LineInput::new(cx)),
        }
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
                    run(window, cx, cmds::WATCH, json!({ "add": text.trim() }));
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
        let watch = self.kind == VarsKind::Watch;
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
                                    run(window, cx, cmds::WATCH, json!({ "remove": index }));
                                }))
                        });
                        let path = r.path.clone();
                        let name = div()
                            .flex()
                            .flex_row()
                            .items_center()
                            .flex_none()
                            .w(px(220.))
                            .overflow_hidden()
                            .pl(px(4. + INDENT * r.depth as f32))
                            .child(toggle)
                            .child(cell(r.name.clone(), None));
                        let value = cell(r.value.clone(), None);
                        let value = if r.error {
                            value.text_color(gpui::rgb(0xF14C4C))
                        } else {
                            value
                        };
                        let row = row_base(&t, format!("{prefix}-row-{ix}"))
                            .child(name)
                            .child(value)
                            .child(cell(r.type_name.clone(), Some(160.)))
                            .children(remove)
                            .on_click(cx.listener(move |this, e: &ClickEvent, _, cx| {
                                this.selected = Some(ix);
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
        div()
            .id(SharedString::from(prefix))
            .size_full()
            .flex()
            .flex_col()
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
    }
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
}

impl CallStackWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
        }
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
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
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
                .on_click(cx.listener(move |_, _, window, cx| {
                    run(window, cx, cmds::SELECT_FRAME, json!({ "frame": ix }))
                }));
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
}

impl ThreadsWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
        }
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

/// The Breakpoints window: every breakpoint with its enabled box, condition and hit count; a selected one's
/// condition and hit count are edited in the fields below the list.
pub struct BreakpointsWindow {
    theme: Theme,
    rows: Vec<BreakpointRow>,
    selected: Option<usize>,
    condition: LineInput,
    hit: LineInput,
}

impl BreakpointsWindow {
    pub fn new(theme: Theme, cx: &mut App) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            selected: None,
            condition: LineInput::new(cx),
            hit: LineInput::new(cx),
        }
    }

    pub fn set_rows(&mut self, rows: Vec<BreakpointRow>, cx: &mut Context<Self>) {
        if self.rows != rows {
            // Keep the selection on the same breakpoint.
            let was = self
                .selected
                .and_then(|i| self.rows.get(i))
                .map(|r| (r.path.clone(), r.line));
            self.rows = rows;
            self.selected =
                was.and_then(|(p, l)| self.rows.iter().position(|r| r.path == p && r.line == l));
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
        }
        cx.notify();
    }

    fn apply(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(r) = self.selected.and_then(|i| self.rows.get(i)) else {
            return;
        };
        run(
            window,
            cx,
            cmds::TOGGLE_BREAKPOINT,
            json!({"action": "set", "path": r.path, "line": r.line,
                   "condition": self.condition.text, "hit_condition": self.hit.text}),
        );
    }

    fn field_key(
        &mut self,
        hit: bool,
        e: &KeyDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let input = if hit {
            &mut self.hit
        } else {
            &mut self.condition
        };
        match input.key(e) {
            InputKey::Ignored => return,
            InputKey::Changed => {}
            InputKey::Submit(text) => {
                input.text = text;
                self.apply(window, cx);
            }
        }
        cx.stop_propagation();
        cx.notify();
    }
}

fn file_name(path: &str) -> String {
    std::path::Path::new(path)
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_owned())
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
                            run(
                                window,
                                cx,
                                cmds::TOGGLE_BREAKPOINT,
                                json!({"action": "delete", "path": r.path, "line": r.line}),
                            );
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
            );
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
            let (path, line, enabled) = (r.path.clone(), r.line, r.enabled);
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
                    run(
                        window,
                        cx,
                        cmds::TOGGLE_BREAKPOINT,
                        json!({"action": "set", "path": path, "line": line, "enabled": !enabled}),
                    );
                }));
            let glyph_color = if !r.enabled || !r.verified {
                gpui::rgb(0x9C9C9C)
            } else {
                gpui::rgb(0xE51400)
            };
            let open_path = r.path.clone();
            let row = row_base(&t, format!("debug-bp-row-{ix}"))
                .child(check)
                .child(cell("\u{25CF}", Some(16.)).text_color(glyph_color))
                .child(cell(
                    format!("{}, line {}", file_name(&r.path), r.line),
                    None,
                ))
                .child(cell(r.condition.clone().unwrap_or_default(), Some(160.)))
                .child(cell(
                    r.hit_condition
                        .as_deref()
                        .map(|h| format!("{h} (hit {})", r.hits))
                        .unwrap_or_else(|| format!("hit {}", r.hits)),
                    Some(120.),
                ))
                .on_click(cx.listener(move |this, e: &ClickEvent, window, cx| {
                    this.select(ix, cx);
                    if e.click_count() >= 2 {
                        run(
                            window,
                            cx,
                            eludite_commands::workspace::FILE_OPEN,
                            json!({"path": open_path, "line": line}),
                        );
                    }
                }));
            if self.selected == Some(ix) {
                row.bg(t.accent).text_color(t.text_on_accent)
            } else {
                row.text_color(t.text).hover(|s| s.bg(t.menu_hover))
            }
        });
        let editor = self.selected.filter(|i| *i < self.rows.len()).map(|_| {
            let cond_focused = self.condition.focus.is_focused(window);
            let hit_focused = self.hit.focus.is_focused(window);
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
                .child("Condition:")
                .child(
                    text_box(
                        "debug-bp-condition",
                        &self.condition.text,
                        "C# expression",
                        cond_focused,
                        &t,
                    )
                    .track_focus(&self.condition.focus)
                    .key_context("DebugBreakpointCondition")
                    .on_key_down(
                        cx.listener(|this, e, window, cx| this.field_key(false, e, window, cx)),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.condition.focus.focus(window, cx);
                        cx.notify();
                    })),
                )
                .child("Hit count:")
                .child(
                    text_box(
                        "debug-bp-hit",
                        &self.hit.text,
                        "N, >=N or %N",
                        hit_focused,
                        &t,
                    )
                    .track_focus(&self.hit.focus)
                    .key_context("DebugBreakpointHit")
                    .on_key_down(
                        cx.listener(|this, e, window, cx| this.field_key(true, e, window, cx)),
                    )
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.hit.focus.focus(window, cx);
                        cx.notify();
                    })),
                )
                .child(
                    toggle_button("debug-bp-apply", "Apply", false, &t)
                        .on_click(cx.listener(|this, _, window, cx| this.apply(window, cx))),
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
                    cell("Condition", Some(160.)),
                    cell("Hit Count", Some(120.)),
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

/// The Exception Settings window: Common Language Runtime Exceptions, and Rust panics (brief 0029).
pub struct ExceptionsWindow {
    theme: Theme,
    settings: ExceptionSettingsRow,
}

impl ExceptionsWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            settings: ExceptionSettingsRow::default(),
        }
    }

    pub fn set(&mut self, settings: ExceptionSettingsRow, cx: &mut Context<Self>) {
        if self.settings != settings {
            self.settings = settings;
            cx.notify();
        }
    }
}

impl Render for ExceptionsWindow {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let s = self.settings;
        let row = |sel: &'static str, label: &'static str, on: bool, member: &'static str| {
            row_base(&t, sel.to_owned())
                .text_color(t.text)
                .hover(|s| s.bg(t.menu_hover))
                .child(cell(if on { "\u{2611}" } else { "\u{2610}" }, Some(22.)))
                .child(cell(label, None))
                .on_click(cx.listener(move |_, _, window, cx| {
                    run(window, cx, cmds::EXCEPTION_SETTINGS, json!({ member: !on }))
                }))
        };
        div()
            .id("debug-exceptions")
            .size_full()
            .flex()
            .flex_col()
            .child(header(&t, vec![cell("Break When", None)]))
            .child(row(
                "debug-exc-thrown",
                "Common Language Runtime Exceptions: thrown",
                s.break_when_thrown,
                "break_when_thrown",
            ))
            .child(row(
                "debug-exc-unhandled",
                "Common Language Runtime Exceptions: user-unhandled",
                s.break_when_user_unhandled,
                "break_when_user_unhandled",
            ))
            // Brief 0029: a native (Cargo) session breaks at `rust_panic`.
            .child(row(
                "debug-exc-rust-panic",
                "Rust panics",
                s.break_on_rust_panic,
                "break_on_rust_panic",
            ))
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
}

impl DebugWindows {
    pub fn new<T>(theme: Theme, cx: &mut Context<T>) -> Self {
        Self {
            locals: cx.new(|cx| VarsWindow::new(theme, VarsKind::Locals, cx)),
            watch: cx.new(|cx| VarsWindow::new(theme, VarsKind::Watch, cx)),
            call_stack: cx.new(|_| CallStackWindow::new(theme)),
            threads: cx.new(|_| ThreadsWindow::new(theme)),
            breakpoints: cx.new(|cx| BreakpointsWindow::new(theme, cx)),
            exceptions: cx.new(|_| ExceptionsWindow::new(theme)),
        }
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
        let (view, vcx) = cx.add_window_view(|_, _| CallStackWindow::new(Theme::vs_dark()));
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
