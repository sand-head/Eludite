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
    /// The Locals window's value box for the selected row (brief 0026).
    value: Option<LineInput>,
    /// Each row's parent value's variables reference (0 at the top).
    parents: Vec<i64>,
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
            value: (kind == VarsKind::Locals).then(|| LineInput::new(cx)),
            parents: Vec::new(),
        }
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
}

impl CallStackWindow {
    pub fn new(theme: Theme) -> Self {
        Self {
            theme,
            rows: Vec::new(),
            sessions: Vec::new(),
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
}

/// Which text box of the Breakpoints window a key is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BpField {
    Condition,
    Hit,
    Message,
    Function,
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
        }
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
        let rows = self.rows.iter().enumerate().map(|(ix, r)| {
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
}

impl ExceptionsWindow {
    pub fn new(theme: Theme, cx: &mut App) -> Self {
        Self {
            theme,
            settings: ExceptionSettingsRow::default(),
            add: LineInput::new(cx),
        }
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
                    .child(rust_panics),
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

/// What the dialog asks the shell to do: `eludite.debug.processes` (Refresh), `eludite.debug.attach` (Attach), close.
#[derive(Debug, Clone, PartialEq)]
pub enum AttachEvent {
    Refresh { filter: Option<String> },
    Attach { pid: u32 },
    Close,
}

/// Visual Studio's Attach to Process dialog: the processes (`eludite.debug.processes`) with their id, name, runtime,
/// whether Eludite started them and their command line; a filter box (it narrows the list as you type; Refresh lists
/// again with it); Refresh, Attach (the selected process, or a double click) and Cancel. Mono programs need their
/// debugger agent, which the dialog says.
pub struct AttachDialog {
    theme: Theme,
    rows: Vec<cmds::ProcessRow>,
    filter: LineInput,
    selected: Option<u32>,
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
            filter: LineInput::new(cx),
            selected: None,
            message: Some("Listing processes\u{2026}".into()),
            focus: cx.focus_handle(),
        }
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
        cx.notify();
    }

    fn attach(&mut self, cx: &mut Context<Self>) {
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
        let can_attach = selected.is_some_and(|pid| self.visible().iter().any(|r| r.pid == pid));
        let filter = text_box(
            ATTACH_FILTER,
            &self.filter.text,
            "Filter processes",
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
        let panel = eludite_ui::dialog_panel(&t, "Attach to Process")
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
                            .child("Available processes")
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
                            .child(header)
                            .child(
                                div()
                                    .id("attach-rows")
                                    .flex()
                                    .flex_col()
                                    .overflow_y_scroll()
                                    .children(rows),
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
