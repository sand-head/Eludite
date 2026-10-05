//! `TextInput` (brief 0056): a text box over the editor core ([`Editor`]) for prompts and other short text, with its
//! own element and none of the editor view's gutter, margins, line numbers, highlighting thread, find bar, popups or
//! decorations.
//!
//! - Text wraps at the box's width with GPUI's wrapped shaping (`TextSystem::shape_text` with a wrap width, as GPUI's
//!   text elements do) and never scrolls horizontally. The box is `min_rows` to `max_rows` visual rows tall, then
//!   scrolls vertically, keeping the caret visible.
//! - The caret, a click, a drag and Up and Down map through the wrapped rows of the last layout ([`InputLayout`]).
//!   Vertical movement is by visual row with a goal x, so the editor core's `move_vertical` is not used. Up on the
//!   first visual row and Down on the last, with no selection, emit [`TextInputEvent::Up`] / [`TextInputEvent::Down`]
//!   instead of moving (a prompt box uses them for its history).
//! - Keys are actions in the key context [`INPUT_KEY_CONTEXT`] (bound by [`crate::key_bindings`]); typed text,
//!   including IME composition, arrives through [`EntityInputHandler`]. Enter emits [`TextInputEvent::Submit`];
//!   Shift+Enter and Ctrl+Enter insert a line break in a multiline input; Escape emits [`TextInputEvent::Escape`]. Tab
//!   is bound to [`Tab`], which the input does not handle: its owner can (a completion menu), else it falls through to
//!   the next binding.
//! - Every edit is the editor core's (`insert`, `backspace`, `delete`, the word deletions, `cut`, `paste`, `undo`,
//!   `redo`); cut and paste are undo steps of their own, typing groups as in the editor.

use std::ops::Range;
use std::sync::Arc;

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, EventEmitter, FocusHandle, Focusable,
    GlobalElementId, Hsla, InspectorElementId, IntoElement, KeyBinding, KeyContext, LayoutId,
    LineLayout, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement, Pixels,
    Point, Render, ScrollWheelEvent, SharedString, Size, Style, Styled, TextAlign, TextRun,
    UTF16Selection, UnderlineStyle, Window, WrappedLine, actions, div, fill, point, prelude::*, px,
    relative, size,
};
use text::OffsetUtf16;

use crate::buffer::Buffer;
use crate::editor::{ClickKind, Editor, SelectionRange};
use crate::view::EditorStyle;

actions!(
    text_input,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        MoveWordLeft,
        MoveWordRight,
        SelectWordLeft,
        SelectWordRight,
        MoveHome,
        MoveEnd,
        SelectHome,
        SelectEnd,
        MoveToStart,
        MoveToEnd,
        SelectToStart,
        SelectToEnd,
        SelectAll,
        Copy,
        Cut,
        Paste,
        Undo,
        Redo,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Submit,
        Newline,
        Escape,
        Tab,
    ]
);

/// The key context of a [`TextInput`].
pub const INPUT_KEY_CONTEXT: &str = "TextInput";

/// The bindings of [`INPUT_KEY_CONTEXT`] (Visual Studio's text box keys). `secondary` is Ctrl on Windows and Linux and
/// Cmd on macOS (clipboard, undo, select all); on macOS Alt+Left and Alt+Right also move by word.
pub(crate) fn bindings() -> Vec<KeyBinding> {
    let c = Some(INPUT_KEY_CONTEXT);
    let mut b = vec![
        KeyBinding::new("left", MoveLeft, c),
        KeyBinding::new("right", MoveRight, c),
        KeyBinding::new("up", MoveUp, c),
        KeyBinding::new("down", MoveDown, c),
        KeyBinding::new("shift-left", SelectLeft, c),
        KeyBinding::new("shift-right", SelectRight, c),
        KeyBinding::new("shift-up", SelectUp, c),
        KeyBinding::new("shift-down", SelectDown, c),
        KeyBinding::new("ctrl-left", MoveWordLeft, c),
        KeyBinding::new("ctrl-right", MoveWordRight, c),
        KeyBinding::new("ctrl-shift-left", SelectWordLeft, c),
        KeyBinding::new("ctrl-shift-right", SelectWordRight, c),
        KeyBinding::new("home", MoveHome, c),
        KeyBinding::new("end", MoveEnd, c),
        KeyBinding::new("shift-home", SelectHome, c),
        KeyBinding::new("shift-end", SelectEnd, c),
        KeyBinding::new("ctrl-home", MoveToStart, c),
        KeyBinding::new("ctrl-end", MoveToEnd, c),
        KeyBinding::new("ctrl-shift-home", SelectToStart, c),
        KeyBinding::new("ctrl-shift-end", SelectToEnd, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-y", Redo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("enter", Submit, c),
        KeyBinding::new("shift-enter", Newline, c),
        KeyBinding::new("ctrl-enter", Newline, c),
        KeyBinding::new("escape", Escape, c),
        KeyBinding::new("tab", Tab, c),
    ];
    if cfg!(target_os = "macos") {
        b.extend([
            KeyBinding::new("alt-left", MoveWordLeft, c),
            KeyBinding::new("alt-right", MoveWordRight, c),
            KeyBinding::new("alt-shift-left", SelectWordLeft, c),
            KeyBinding::new("alt-shift-right", SelectWordRight, c),
        ]);
    }
    b
}

/// What a [`TextInput`] tells its owner.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextInputEvent {
    /// Enter.
    Submit,
    /// Escape.
    Escape,
    /// The text changed (typing, an edit, undo, [`TextInput::set_text`]).
    Changed,
    /// Up on the first visual row with no selection.
    Up,
    /// Down on the last visual row with no selection.
    Down,
}

/// One visual row of a layout: bytes `start..end` of logical line `line`, which starts at `line_start` in the text.
#[derive(Debug, Clone, PartialEq, Eq)]
struct VisualRow {
    line: usize,
    line_start: usize,
    start: usize,
    end: usize,
    /// The last row of its logical line (a hard break or the end of the text follows).
    last: bool,
    /// The largest offset (absolute) a click or a vertical move lands on in this row: its end on the last row of a
    /// line, else the start of its last character (the end is the next row's start).
    max: usize,
}

/// The wrapped rows of the last painted frame, for mapping offsets to positions and back.
pub struct InputLayout {
    /// Where the text was painted (window coordinates); row 0's top is `bounds.top() - scroll`.
    pub bounds: Bounds<Pixels>,
    pub line_height: Pixels,
    pub scroll: Pixels,
    lines: Vec<Arc<LineLayout>>,
    rows: Vec<VisualRow>,
}

impl InputLayout {
    /// Build the rows of `text` from its wrapped lines (one per `\n`-separated line).
    fn new(
        text: &str,
        wrapped: &[WrappedLine],
        bounds: Bounds<Pixels>,
        line_height: Pixels,
    ) -> Self {
        let mut lines = Vec::with_capacity(wrapped.len());
        let mut rows = Vec::new();
        let mut line_start = 0;
        for (ix, (w, line_text)) in wrapped.iter().zip(text.split('\n')).enumerate() {
            let layout = w.unwrapped_layout.clone();
            let mut starts = vec![0];
            for b in w.wrap_boundaries() {
                let i = layout.runs[b.run_ix].glyphs[b.glyph_ix].index;
                if i > *starts.last().unwrap_or(&0) && i < line_text.len() {
                    starts.push(i);
                }
            }
            for (n, &start) in starts.iter().enumerate() {
                let last = n + 1 == starts.len();
                let end = if last { line_text.len() } else { starts[n + 1] };
                let max = if last {
                    end
                } else {
                    line_text[start..end]
                        .char_indices()
                        .last()
                        .map_or(start, |(i, _)| start + i)
                };
                rows.push(VisualRow {
                    line: ix,
                    line_start,
                    start,
                    end,
                    last,
                    max: line_start + max,
                });
            }
            lines.push(layout);
            line_start += line_text.len() + 1;
        }
        if rows.is_empty() {
            rows.push(VisualRow {
                line: 0,
                line_start: 0,
                start: 0,
                end: 0,
                last: true,
                max: 0,
            });
            lines.push(Arc::new(LineLayout::default()));
        }
        Self {
            bounds,
            line_height,
            scroll: px(0.),
            lines,
            rows,
        }
    }

    /// The number of visual rows.
    pub fn row_count(&self) -> usize {
        self.rows.len()
    }

    /// The visual row showing `offset` (at a soft wrap, the row it starts).
    pub fn row_of(&self, offset: usize) -> usize {
        self.rows
            .iter()
            .position(|r| {
                let (s, e) = (r.line_start + r.start, r.line_start + r.end);
                offset >= s && (offset < e || (r.last && offset == e))
            })
            .unwrap_or(self.rows.len() - 1)
    }

    /// `offset`'s x in its row, from the row's left edge.
    pub fn x_of(&self, offset: usize) -> Pixels {
        let r = &self.rows[self.row_of(offset)];
        let l = &self.lines[r.line];
        let i = offset.saturating_sub(r.line_start).min(r.end);
        l.x_for_index(i) - l.x_for_index(r.start)
    }

    /// The offset in visual row `row` closest to `x` (from the row's left edge).
    pub fn offset_at(&self, row: usize, x: Pixels) -> usize {
        let r = &self.rows[row.min(self.rows.len() - 1)];
        let l = &self.lines[r.line];
        let i = l.closest_index_for_x(x.max(px(0.)) + l.x_for_index(r.start));
        (r.line_start + i.clamp(r.start, r.end)).clamp(r.line_start + r.start, r.max)
    }

    /// The offset under a window position.
    pub fn offset_for_point(&self, p: Point<Pixels>) -> usize {
        let y = p.y - self.bounds.top() + self.scroll;
        let row = if y < px(0.) {
            0
        } else {
            ((y / self.line_height).floor() as usize).min(self.rows.len() - 1)
        };
        self.offset_at(row, p.x - self.bounds.left())
    }

    /// Window position of the top-left of `offset`'s caret.
    pub fn point_for_offset(&self, offset: usize) -> Point<Pixels> {
        let row = self.row_of(offset);
        point(
            self.bounds.left() + self.x_of(offset),
            self.bounds.top() + self.line_height * row as f32 - self.scroll,
        )
    }

    /// The first visual row of logical line `line`.
    fn first_row_of_line(&self, line: usize) -> usize {
        self.rows.iter().position(|r| r.line == line).unwrap_or(0)
    }
}

/// The number of visual rows `text` wraps to at `wrap_width`.
fn count_rows(lines: &[WrappedLine]) -> usize {
    lines
        .iter()
        .map(|l| l.wrap_boundaries().len() + 1)
        .sum::<usize>()
        .max(1)
}

/// A text box over [`Editor`]. See the module docs.
pub struct TextInput {
    editor: Editor,
    focus: FocusHandle,
    multiline: bool,
    min_rows: usize,
    max_rows: usize,
    placeholder: SharedString,
    style: EditorStyle,
    scroll: Pixels,
    autoscroll: bool,
    /// The x kept across Up and Down through shorter rows.
    goal_x: Option<Pixels>,
    dragging: bool,
    /// The IME composition in progress (bytes).
    marked: Option<Range<usize>>,
    layout: Option<InputLayout>,
    on_submit: Option<Callback>,
    on_escape: Option<Callback>,
}

/// An owner's handler for Enter or Escape.
type Callback = Box<dyn Fn(&mut Window, &mut App)>;

impl EventEmitter<TextInputEvent> for TextInput {}

impl Focusable for TextInput {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl TextInput {
    /// An empty input. A multiline input takes line breaks (Shift+Enter, Ctrl+Enter, paste) and is 2 to 8 rows tall
    /// until [`TextInput::set_rows`]; a single-line one turns pasted line breaks into spaces and is 1 row tall.
    pub fn new(multiline: bool, cx: &mut Context<Self>) -> Self {
        let rows = if multiline { (2, 8) } else { (1, 1) };
        Self {
            editor: Editor::new(Buffer::new("")),
            focus: cx.focus_handle(),
            multiline,
            min_rows: rows.0,
            max_rows: rows.1,
            placeholder: SharedString::default(),
            style: EditorStyle::default(),
            scroll: px(0.),
            autoscroll: false,
            goal_x: None,
            dragging: false,
            marked: None,
            layout: None,
            on_submit: None,
            on_escape: None,
        }
    }

    pub fn text(&self) -> String {
        self.editor.text()
    }

    /// Replace the whole text (a new undo history), with the caret at its end.
    pub fn set_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = if self.multiline {
            text.to_owned()
        } else {
            text.replace('\n', " ")
        };
        self.editor = Editor::new(Buffer::new(&text));
        self.editor.set_caret(self.editor.buffer().len());
        self.marked = None;
        self.goal_x = None;
        self.autoscroll = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    pub fn clear(&mut self, cx: &mut Context<Self>) {
        self.set_text("", cx);
    }

    pub fn is_empty(&self) -> bool {
        self.editor.buffer().is_empty()
    }

    /// The caret (the selection's head), as a byte offset.
    pub fn caret(&self) -> usize {
        self.editor.primary_selection().head
    }

    /// The selection as byte offsets (`tail`, `head`).
    pub fn selection(&self) -> SelectionRange {
        self.editor.primary_selection()
    }

    /// The selected text, or nothing when the selection is empty.
    pub fn selected_text(&self) -> String {
        let s = self.selection();
        if s.is_empty() {
            String::new()
        } else {
            self.editor.selected_text()
        }
    }

    pub fn set_caret(&mut self, offset: usize, cx: &mut Context<Self>) {
        self.movement(cx, |e| e.set_caret(offset));
    }

    pub fn select_all(&mut self, cx: &mut Context<Self>) {
        self.movement(cx, Editor::select_all);
    }

    pub fn placeholder(&self) -> &SharedString {
        &self.placeholder
    }

    /// The text shown, muted, while the input is empty.
    pub fn set_placeholder(&mut self, text: impl Into<SharedString>, cx: &mut Context<Self>) {
        self.placeholder = text.into();
        cx.notify();
    }

    /// At least `min` and at most `max` visual rows tall; it scrolls beyond `max`.
    pub fn set_rows(&mut self, min: usize, max: usize, cx: &mut Context<Self>) {
        self.min_rows = min.max(1);
        self.max_rows = max.max(self.min_rows);
        cx.notify();
    }

    /// The selection color and the placeholder's (the theme's muted text).
    pub fn set_style(&mut self, style: EditorStyle, cx: &mut Context<Self>) {
        self.style = style;
        cx.notify();
    }

    /// Called on Enter, after [`TextInputEvent::Submit`] is emitted.
    pub fn on_submit(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_submit = Some(Box::new(f));
    }

    /// Called on Escape, after [`TextInputEvent::Escape`] is emitted.
    pub fn on_escape(&mut self, f: impl Fn(&mut Window, &mut App) + 'static) {
        self.on_escape = Some(Box::new(f));
    }

    /// The editor core, read-only.
    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// The last painted layout (`None` before the first frame).
    pub fn layout(&self) -> Option<&InputLayout> {
        self.layout.as_ref()
    }

    /// The vertical scroll offset in pixels.
    pub fn scroll(&self) -> Pixels {
        self.scroll
    }

    // ----- changes -----

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.notify();
    }

    fn movement(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        f(&mut self.editor);
        self.goal_x = None;
        self.moved(cx);
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.goal_x = None;
        self.autoscroll = true;
        cx.emit(TextInputEvent::Changed);
        cx.notify();
    }

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        let before = self.editor.buffer().version();
        f(&mut self.editor);
        self.marked = None;
        if self.editor.buffer().version() != before {
            self.changed(cx);
        } else {
            self.moved(cx);
        }
    }

    /// An edit that is an undo step of its own (cut, paste).
    fn edit_alone(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        self.edit(cx, |e| {
            e.buffer_mut().finalize_last_transaction();
            f(e);
            e.buffer_mut().finalize_last_transaction();
        });
    }

    /// Up (`-1`) or Down (`1`) by visual row, keeping the goal x.
    fn vertical(&mut self, delta: i64, extend: bool, cx: &mut Context<Self>) {
        let sel = self.editor.primary_selection();
        let (rows, row) = match &self.layout {
            Some(l) => (l.row_count(), l.row_of(sel.head)),
            None => (1, 0),
        };
        let target = row as i64 + delta;
        if target < 0 || target >= rows as i64 {
            if !extend && sel.is_empty() {
                cx.emit(if delta < 0 {
                    TextInputEvent::Up
                } else {
                    TextInputEvent::Down
                });
                return;
            }
            // Past the first or last row: to the start or the end, as text boxes do.
            let to = if delta < 0 {
                0
            } else {
                self.editor.buffer().len()
            };
            self.set_head(to, extend);
            self.goal_x = None;
            self.moved(cx);
            return;
        }
        let Some(l) = &self.layout else {
            return;
        };
        let goal = self.goal_x.unwrap_or_else(|| l.x_of(sel.head));
        let to = l.offset_at(target as usize, goal);
        self.set_head(to, extend);
        self.goal_x = Some(goal);
        self.moved(cx);
    }

    fn set_head(&mut self, head: usize, extend: bool) {
        let tail = if extend {
            self.editor.primary_selection().tail
        } else {
            head
        };
        self.editor
            .set_selections(vec![SelectionRange { tail, head }], 0);
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        let text = if self.multiline {
            text.to_owned()
        } else {
            text.replace('\n', " ")
        };
        self.edit(cx, |e| e.insert(&text));
    }

    // ----- actions -----

    fn submit(&mut self, _: &Submit, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Submit);
        if let Some(f) = &self.on_submit {
            f(window, cx);
        }
    }

    fn escape(&mut self, _: &Escape, window: &mut Window, cx: &mut Context<Self>) {
        cx.emit(TextInputEvent::Escape);
        if let Some(f) = &self.on_escape {
            f(window, cx);
        }
    }

    /// Shift+Enter and Ctrl+Enter: a line break in a multiline input; nothing in a single-line one.
    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.multiline {
            self.insert_text("\n", cx);
        }
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.selected_text();
        if !text.is_empty() {
            cx.write_to_clipboard(ClipboardItem::new_string(text));
        }
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        if self.selection().is_empty() {
            return;
        }
        let mut text = String::new();
        self.edit_alone(cx, |e| text = e.cut());
        cx.write_to_clipboard(ClipboardItem::new_string(text));
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) else {
            return;
        };
        let text = if self.multiline {
            text
        } else {
            text.replace(['\r', '\n'], " ")
        };
        self.edit_alone(cx, |e| e.paste(&text));
    }

    // ----- mouse -----

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(offset) = self
            .layout
            .as_ref()
            .map(|l| l.offset_for_point(event.position))
        else {
            return;
        };
        let kind = match event.click_count {
            0 | 1 => ClickKind::Single,
            2 => ClickKind::Double,
            _ => ClickKind::Triple,
        };
        self.editor
            .click(offset, kind, false, event.modifiers.shift);
        self.dragging = true;
        self.goal_x = None;
        self.marked = None;
        self.moved(cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(offset) = self
            .layout
            .as_ref()
            .map(|l| l.offset_for_point(event.position))
        {
            self.editor.drag_to(offset);
            self.moved(cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.dragging = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let Some(l) = &self.layout else {
            return;
        };
        let delta = event.delta.pixel_delta(l.line_height);
        let max = (l.line_height * l.row_count() as f32 - l.bounds.size.height).max(px(0.));
        self.scroll = (self.scroll - delta.y).clamp(px(0.), max);
        self.autoscroll = false;
        cx.notify();
    }

    /// Keep the caret's row in view; called while laying out.
    fn scroll_to_caret(
        &mut self,
        rows: usize,
        caret_row: usize,
        line_height: Pixels,
        height: Pixels,
    ) {
        let max = (line_height * rows as f32 - height).max(px(0.));
        if self.autoscroll {
            self.autoscroll = false;
            let top = line_height * caret_row as f32;
            if top < self.scroll {
                self.scroll = top;
            } else if top + line_height > self.scroll + height {
                self.scroll = top + line_height - height;
            }
        }
        self.scroll = self.scroll.clamp(px(0.), max);
    }
}

macro_rules! bind {
    ($el:expr, $cx:expr, $( $action:ident => |$e:ident| $body:expr, $kind:ident; )*) => {
        $el$(
            .on_action($cx.listener(|this: &mut TextInput, _: &$action, _: &mut Window, cx: &mut Context<TextInput>| {
                this.$kind(cx, |$e: &mut Editor| { $body; });
            }))
        )*
    };
}

impl Render for TextInput {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let mut context = KeyContext::default();
        context.add(INPUT_KEY_CONTEXT);
        let el = div()
            .key_context(context)
            .track_focus(&self.focus)
            .w_full()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::submit))
            .on_action(cx.listener(Self::escape))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_action(cx.listener(|this, _: &MoveUp, _, cx| this.vertical(-1, false, cx)))
            .on_action(cx.listener(|this, _: &MoveDown, _, cx| this.vertical(1, false, cx)))
            .on_action(cx.listener(|this, _: &SelectUp, _, cx| this.vertical(-1, true, cx)))
            .on_action(cx.listener(|this, _: &SelectDown, _, cx| this.vertical(1, true, cx)))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll));
        let el = bind!(el, cx,
            MoveLeft => |e| e.move_left(false), movement;
            MoveRight => |e| e.move_right(false), movement;
            SelectLeft => |e| e.move_left(true), movement;
            SelectRight => |e| e.move_right(true), movement;
            MoveWordLeft => |e| e.move_word_left(false), movement;
            MoveWordRight => |e| e.move_word_right(false), movement;
            SelectWordLeft => |e| e.move_word_left(true), movement;
            SelectWordRight => |e| e.move_word_right(true), movement;
            MoveHome => |e| e.move_home(false), movement;
            MoveEnd => |e| e.move_end(false), movement;
            SelectHome => |e| e.move_home(true), movement;
            SelectEnd => |e| e.move_end(true), movement;
            MoveToStart => |e| e.move_to_start(false), movement;
            MoveToEnd => |e| e.move_to_end(false), movement;
            SelectToStart => |e| e.move_to_start(true), movement;
            SelectToEnd => |e| e.move_to_end(true), movement;
            SelectAll => |e| e.select_all(), movement;
            Backspace => |e| e.backspace(), edit;
            Delete => |e| e.delete(), edit;
            DeleteWordLeft => |e| e.delete_word_left(), edit;
            DeleteWordRight => |e| e.delete_word_right(), edit;
            Undo => |e| e.undo(), edit;
            Redo => |e| e.redo(), edit;
        );
        el.child(TextInputElement { view: cx.entity() })
    }
}

impl EntityInputHandler for TextInput {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let range = self.range_from_utf16(&range_utf16);
        actual_range.replace(self.range_to_utf16(&range));
        Some(self.editor.buffer().text_for_range(range))
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let sel = self.editor.primary_selection();
        Some(UTF16Selection {
            range: self.range_to_utf16(&sel.range()),
            reversed: sel.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        self.marked.as_ref().map(|r| self.range_to_utf16(r))
    }

    fn unmark_text(&mut self, _: &mut Window, cx: &mut Context<Self>) {
        if self.marked.take().is_some() {
            cx.notify();
        }
    }

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or_else(|| self.marked.clone());
        if let Some(range) = range
            && range != self.editor.primary_selection().range()
        {
            self.editor.set_selections(
                vec![SelectionRange {
                    tail: range.start,
                    head: range.end,
                }],
                0,
            );
        }
        self.insert_text(text, cx);
    }

    /// IME composition: the composed text replaces the range (else the composition so far, else the selection) and is
    /// marked, underlined, until it is committed with [`EntityInputHandler::replace_text_in_range`] or unmarked.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range: Option<Range<usize>>,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let range = range_utf16
            .map(|r| self.range_from_utf16(&r))
            .or_else(|| self.marked.clone())
            .unwrap_or_else(|| self.editor.primary_selection().range());
        self.editor.set_selections(
            vec![SelectionRange {
                tail: range.start,
                head: range.end,
            }],
            0,
        );
        self.insert_text(new_text, cx);
        let start = range.start;
        self.marked = (!new_text.is_empty()).then(|| start..start + new_text.len());
        if let Some(sel) = new_selected_range {
            // Relative to the new text, in UTF-16.
            let to_bytes = |u: usize| {
                let mut units = 0;
                for (i, c) in new_text.char_indices() {
                    if units >= u {
                        return i;
                    }
                    units += c.len_utf16();
                }
                new_text.len()
            };
            self.editor.set_selections(
                vec![SelectionRange {
                    tail: start + to_bytes(sel.start),
                    head: start + to_bytes(sel.end),
                }],
                0,
            );
        }
        cx.notify();
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let start = self.range_from_utf16(&range_utf16).start;
        Some(Bounds::new(
            l.point_for_offset(start),
            size(px(1.), l.line_height),
        ))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.layout.as_ref()?.offset_for_point(point);
        Some(self.range_to_utf16(&(offset..offset)).start)
    }
}

impl TextInput {
    fn range_from_utf16(&self, r: &Range<usize>) -> Range<usize> {
        let s = self.editor.buffer().snapshot();
        let max = s.offset_to_offset_utf16(s.len()).0;
        s.offset_utf16_to_offset(OffsetUtf16(r.start.min(max)))
            ..s.offset_utf16_to_offset(OffsetUtf16(r.end.min(max)))
    }

    fn range_to_utf16(&self, r: &Range<usize>) -> Range<usize> {
        let s = self.editor.buffer().snapshot();
        s.offset_to_offset_utf16(r.start).0..s.offset_to_offset_utf16(r.end).0
    }
}

// ----- the element -----

struct TextInputElement {
    view: Entity<TextInput>,
}

impl IntoElement for TextInputElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

/// The text style the element shapes with: the inherited font, size, line height and color.
#[derive(Clone)]
struct Shaping {
    font: gpui::Font,
    font_size: Pixels,
    line_height: Pixels,
    color: Hsla,
}

impl Shaping {
    fn of(window: &Window) -> Self {
        let style = window.text_style();
        let rem = window.rem_size();
        Self {
            font: style.font(),
            font_size: style.font_size.to_pixels(rem),
            line_height: style.line_height_in_pixels(rem),
            color: style.color,
        }
    }

    /// The text's wrapped lines at `width` (one per `\n`-separated line), with `marked` underlined.
    fn shape(
        &self,
        text: &str,
        width: Option<Pixels>,
        marked: Option<&Range<usize>>,
        window: &Window,
    ) -> Vec<WrappedLine> {
        let run = |len: usize, underline: bool| TextRun {
            len,
            font: self.font.clone(),
            color: self.color,
            background_color: None,
            underline: underline.then(|| UnderlineStyle {
                thickness: px(1.),
                color: Some(self.color),
                wavy: false,
            }),
            strikethrough: None,
        };
        let runs: Vec<TextRun> = match marked {
            Some(m) if m.end <= text.len() && m.start < m.end => [
                run(m.start, false),
                run(m.end - m.start, true),
                run(text.len() - m.end, false),
            ]
            .into_iter()
            .filter(|r| r.len > 0)
            .collect(),
            _ => vec![run(text.len(), false)],
        };
        window
            .text_system()
            .shape_text(
                SharedString::from(text.to_owned()),
                self.font_size,
                &runs,
                width.map(wrap_width),
                None,
            )
            .map(|l| l.into_iter().collect())
            .unwrap_or_default()
    }
}

/// The wrap width for a box `width` wide: room for the caret at the end of a row.
fn wrap_width(width: Pixels) -> Pixels {
    (width - px(2.)).max(px(1.))
}

pub struct InputPrepaint {
    lines: Vec<WrappedLine>,
    /// Each line's origin.
    origins: Vec<Point<Pixels>>,
    selection: Vec<Bounds<Pixels>>,
    caret: Option<Bounds<Pixels>>,
    placeholder: Option<gpui::ShapedLine>,
    shaping: Shaping,
}

impl Element for TextInputElement {
    type RequestLayoutState = ();
    type PrepaintState = InputPrepaint;

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static core::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        let shaping = Shaping::of(window);
        let v = self.view.read(cx);
        let text = v.editor.text();
        let marked = v.marked.clone();
        let (min_rows, max_rows) = (v.min_rows, v.max_rows);
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        let id = window.request_measured_layout(style, move |known, available, window, _| {
            let width = known.width.or(match available.width {
                gpui::AvailableSpace::Definite(w) => Some(w),
                _ => None,
            });
            let lines = shaping.shape(&text, width, marked.as_ref(), window);
            let rows = count_rows(&lines).clamp(min_rows, max_rows);
            Size {
                width: width.unwrap_or_default(),
                height: shaping.line_height * rows as f32,
            }
        });
        (id, ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> InputPrepaint {
        let shaping = Shaping::of(window);
        let lh = shaping.line_height;
        let (text, marked) = {
            let v = self.view.read(cx);
            (v.editor.text(), v.marked.clone())
        };
        let lines = shaping.shape(&text, Some(bounds.size.width), marked.as_ref(), window);
        let mut layout = InputLayout::new(&text, &lines, bounds, lh);
        let focused = self.view.read(cx).focus.is_focused(window);
        let sel = self.view.read(cx).editor.primary_selection();
        let caret_row = layout.row_of(sel.head);
        self.view.update(cx, |v, _| {
            v.scroll_to_caret(layout.row_count(), caret_row, lh, bounds.size.height)
        });
        let v = self.view.read(cx);
        layout.scroll = v.scroll;
        let origins = (0..lines.len())
            .map(|i| {
                point(
                    bounds.left(),
                    bounds.top() + lh * layout.first_row_of_line(i) as f32 - layout.scroll,
                )
            })
            .collect();
        // The selection, row by row; a selected line break shows as a narrow block after its row.
        let mut selection = Vec::new();
        if !sel.is_empty() {
            let (s, e) = (sel.start(), sel.end());
            for (ix, r) in layout.rows.iter().enumerate() {
                let (rs, re) = (r.line_start + r.start, r.line_start + r.end);
                if e < rs || s > re || (s == re && !r.last) || (e == rs && rs != re) {
                    continue;
                }
                let a = s.max(rs);
                let b = e.min(re);
                let line = &layout.lines[r.line];
                let x0 = line.x_for_index(a - r.line_start) - line.x_for_index(r.start);
                let mut x1 = line.x_for_index(b - r.line_start) - line.x_for_index(r.start);
                if r.last && e > re {
                    x1 += shaping.font_size * 0.3;
                }
                if x1 > x0 {
                    selection.push(Bounds::from_corners(
                        point(
                            bounds.left() + x0,
                            bounds.top() + lh * ix as f32 - layout.scroll,
                        ),
                        point(
                            bounds.left() + x1,
                            bounds.top() + lh * (ix + 1) as f32 - layout.scroll,
                        ),
                    ));
                }
            }
        }
        let caret =
            focused.then(|| Bounds::new(layout.point_for_offset(sel.head), size(px(1.), lh)));
        let placeholder = (text.is_empty() && !v.placeholder.is_empty()).then(|| {
            let muted: Hsla = v.style.theme.text_muted.into();
            window.text_system().shape_line(
                v.placeholder.clone(),
                shaping.font_size,
                &[TextRun {
                    len: v.placeholder.len(),
                    font: shaping.font.clone(),
                    color: muted,
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            )
        });
        self.view.update(cx, |v, _| v.layout = Some(layout));
        InputPrepaint {
            lines,
            origins,
            selection,
            caret,
            placeholder,
            shaping,
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut InputPrepaint,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        let selection_color: Hsla = self.view.read(cx).style.selection.into();
        let lh = state.shaping.line_height;
        window.with_content_mask(Some(ContentMask { bounds }), |window| {
            for b in &state.selection {
                window.paint_quad(fill(*b, selection_color));
            }
            if let Some(p) = &state.placeholder {
                let _ = p.paint(bounds.origin, lh, TextAlign::Left, None, window, cx);
            }
            for (line, origin) in state.lines.iter().zip(&state.origins) {
                let _ = line.paint(*origin, lh, TextAlign::Left, None, window, cx);
            }
            if let Some(c) = state.caret {
                window.paint_quad(fill(c, state.shaping.color));
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::rc::Rc;

    use gpui::{Modifiers, TestAppContext, VisualTestContext};

    use super::*;

    /// Line height and character width in the test text system at a 10 px font (0.6 em per character).
    const LH: f32 = 20.;
    const CW: f32 = 6.;

    struct Host {
        input: Entity<TextInput>,
        width: Pixels,
    }

    impl Render for Host {
        fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
            div()
                .w(self.width)
                .text_size(px(10.))
                .line_height(px(LH))
                .child(self.input.clone())
        }
    }

    type Events = Rc<RefCell<Vec<TextInputEvent>>>;

    /// An input `width` wide, focused, and the events it emitted.
    fn open(
        cx: &mut TestAppContext,
        width: f32,
        multiline: bool,
    ) -> (Entity<TextInput>, Events, VisualTestContext) {
        cx.update(|cx| cx.bind_keys(crate::key_bindings()));
        let events: Events = Rc::default();
        let ev = events.clone();
        let window = cx.update(|cx| {
            cx.open_window(Default::default(), |window, cx| {
                let input = cx.new(|cx| TextInput::new(multiline, cx));
                window.focus(&input.focus_handle(cx), cx);
                cx.new(|cx| {
                    cx.subscribe(&input, move |_, _, e: &TextInputEvent, _| {
                        ev.borrow_mut().push(*e)
                    })
                    .detach();
                    Host {
                        input,
                        width: px(width),
                    }
                })
            })
            .unwrap()
        });
        let mut vcx = VisualTestContext::from_window(window.into(), cx);
        let host = window.root(&mut vcx).unwrap();
        vcx.run_until_parked();
        let input = host.read_with(&vcx, |h, _| h.input.clone());
        (input, events, vcx)
    }

    fn text(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> String {
        input.read_with(cx, |i, _| i.text())
    }

    fn caret(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> usize {
        input.read_with(cx, |i, _| i.caret())
    }

    fn type_text(cx: &mut VisualTestContext, s: &str) {
        let keys: Vec<String> = s
            .chars()
            .map(|c| match c {
                ' ' => "space".to_owned(),
                c => c.to_string(),
            })
            .collect();
        cx.simulate_keystrokes(&keys.join(" "));
    }

    /// (rows, element height, scroll) of the last layout.
    fn geometry(input: &Entity<TextInput>, cx: &mut VisualTestContext) -> (usize, f32, f32) {
        input.read_with(cx, |i, _| {
            let l = i.layout().unwrap();
            (
                l.row_count(),
                f32::from(l.bounds.size.height),
                f32::from(i.scroll()),
            )
        })
    }

    #[gpui::test]
    fn typing_editing_at_the_caret_and_selection_keys(cx: &mut TestAppContext) {
        let (input, events, mut cx) = open(cx, 600., true);
        type_text(&mut cx, "hello world");
        assert_eq!(text(&input, &mut cx), "hello world");
        assert!(events.borrow().contains(&TextInputEvent::Changed));
        // Backspace in the middle of the text deletes before the caret.
        cx.simulate_keystrokes("left left left left left backspace");
        assert_eq!(text(&input, &mut cx), "helloworld");
        assert_eq!(caret(&input, &mut cx), 5);
        // Shift+Left and Shift+Right extend; Left collapses to the start.
        cx.simulate_keystrokes("shift-left shift-left");
        let sel = input.read_with(&cx, |i, _| i.selection());
        assert_eq!((sel.tail, sel.head), (5, 3));
        cx.simulate_keystrokes("shift-right");
        assert_eq!(input.read_with(&cx, |i, _| i.selected_text()), "o");
        cx.simulate_keystrokes("left");
        assert_eq!(caret(&input, &mut cx), 4);
        // Typing replaces nothing at a caret, and goes where the caret is.
        type_text(&mut cx, "X");
        assert_eq!(text(&input, &mut cx), "hellXoworld");
        cx.simulate_keystrokes("backspace");
        // Home and End; Ctrl+Left and Ctrl+Right by word.
        cx.simulate_keystrokes("home");
        assert_eq!(caret(&input, &mut cx), 0);
        cx.simulate_keystrokes("end");
        assert_eq!(caret(&input, &mut cx), 10);
        type_text(&mut cx, " again");
        cx.simulate_keystrokes("ctrl-left");
        assert_eq!(caret(&input, &mut cx), 11);
        cx.simulate_keystrokes("ctrl-left");
        assert_eq!(caret(&input, &mut cx), 0);
        cx.simulate_keystrokes("ctrl-right");
        assert_eq!(caret(&input, &mut cx), 11);
        cx.simulate_keystrokes("ctrl-shift-right");
        assert_eq!(input.read_with(&cx, |i, _| i.selected_text()), "again");
        // Typing over a selection replaces it.
        type_text(&mut cx, "twice");
        assert_eq!(text(&input, &mut cx), "helloworld twice");
        // Ctrl+Backspace and Ctrl+Delete by word; Delete at the caret.
        cx.simulate_keystrokes("end ctrl-backspace");
        assert_eq!(text(&input, &mut cx), "helloworld ");
        cx.simulate_keystrokes("home delete");
        assert_eq!(text(&input, &mut cx), "elloworld ");
        cx.simulate_keystrokes("ctrl-delete");
        assert_eq!(text(&input, &mut cx), "");
        assert!(input.read_with(&cx, |i, _| i.is_empty()));
    }

    #[gpui::test]
    fn clipboard_undo_redo_and_select_all(cx: &mut TestAppContext) {
        let (input, _, mut cx) = open(cx, 600., true);
        type_text(&mut cx, "one two");
        cx.simulate_keystrokes("ctrl-a");
        assert_eq!(input.read_with(&cx, |i, _| i.selected_text()), "one two");
        cx.simulate_keystrokes("ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("one two")
        );
        // Cut: the clipboard holds it and the box is empty; paste twice.
        cx.simulate_keystrokes("ctrl-x");
        assert_eq!(text(&input, &mut cx), "");
        cx.simulate_keystrokes("ctrl-v ctrl-v");
        assert_eq!(text(&input, &mut cx), "one twoone two");
        // Undo the pastes one by one, then the cut; redo them.
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, &mut cx), "one two");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, &mut cx), "");
        cx.simulate_keystrokes("ctrl-z");
        assert_eq!(text(&input, &mut cx), "one two");
        cx.simulate_keystrokes("ctrl-y");
        assert_eq!(text(&input, &mut cx), "");
        cx.simulate_keystrokes("ctrl-shift-z");
        assert_eq!(text(&input, &mut cx), "one two");
        // Copy with no selection copies nothing.
        cx.write_to_clipboard(ClipboardItem::new_string("kept".into()));
        cx.simulate_keystrokes("end ctrl-c");
        assert_eq!(
            cx.read_from_clipboard().and_then(|c| c.text()).as_deref(),
            Some("kept")
        );
    }

    #[gpui::test]
    fn enter_submits_shift_enter_breaks_the_line_and_escape_is_told(cx: &mut TestAppContext) {
        let (input, events, mut cx) = open(cx, 600., true);
        type_text(&mut cx, "a");
        cx.simulate_keystrokes("shift-enter");
        type_text(&mut cx, "b");
        cx.simulate_keystrokes("ctrl-enter");
        type_text(&mut cx, "c");
        assert_eq!(text(&input, &mut cx), "a\nb\nc");
        events.borrow_mut().clear();
        cx.simulate_keystrokes("enter");
        assert_eq!(*events.borrow(), [TextInputEvent::Submit]);
        assert_eq!(text(&input, &mut cx), "a\nb\nc", "the owner decides");
        cx.simulate_keystrokes("escape");
        assert_eq!(events.borrow().last(), Some(&TextInputEvent::Escape));
        // The rows are the lines.
        assert_eq!(geometry(&input, &mut cx).0, 3);
    }

    #[gpui::test]
    fn a_single_line_input_takes_no_line_break(cx: &mut TestAppContext) {
        // It turns pasted line breaks into spaces and is one row tall.
        let (single, events, mut cx) = open(cx, 600., false);
        type_text(&mut cx, "x");
        cx.simulate_keystrokes("shift-enter");
        cx.write_to_clipboard(ClipboardItem::new_string("1\n2".into()));
        cx.simulate_keystrokes("ctrl-v");
        assert_eq!(text(&single, &mut cx), "x1 2");
        assert_eq!(geometry(&single, &mut cx).1, LH);
        assert!(!events.borrow().contains(&TextInputEvent::Submit));
    }

    #[gpui::test]
    fn the_box_grows_from_two_to_eight_rows_then_scrolls(cx: &mut TestAppContext) {
        // 20 characters a row (the wrap width leaves room for the caret).
        let (input, _, mut cx) = open(cx, 20. * CW + 2., true);
        assert_eq!(geometry(&input, &mut cx), (1, 2. * LH, 0.));
        let mut grown = Vec::new();
        for _ in 0..10 {
            // One row of words: four five-character words fill a row exactly.
            type_text(&mut cx, "abcd abcd abcd abcd ");
            let (rows, height, _) = geometry(&input, &mut cx);
            grown.push((rows, height / LH));
        }
        assert_eq!(
            grown,
            [
                (1, 2.),
                (2, 2.),
                (3, 3.),
                (4, 4.),
                (5, 5.),
                (6, 6.),
                (7, 7.),
                (8, 8.),
                (9, 8.),
                (10, 8.)
            ]
        );
        // Scrolled so the caret's row (the last) is at the bottom.
        let (rows, height, scroll) = geometry(&input, &mut cx);
        assert_eq!(scroll, rows as f32 * LH - height);
        // Ctrl+Home scrolls back to the top.
        cx.simulate_keystrokes("ctrl-home");
        assert_eq!(geometry(&input, &mut cx).2, 0.);
    }

    #[gpui::test]
    fn a_click_places_the_caret_in_a_wrapped_row_and_drag_selects(cx: &mut TestAppContext) {
        let (input, _, mut cx) = open(cx, 20. * CW + 2., true);
        // Three visual rows of one line: "abcd abcd abcd abcd " per row.
        type_text(&mut cx, &"abcd abcd abcd abcd ".repeat(3));
        let bounds = input.read_with(&cx, |i, _| i.layout().unwrap().bounds);
        assert_eq!(geometry(&input, &mut cx).0, 3);
        // The second row, between its third and fourth characters.
        let at = point(
            bounds.left() + px(3. * CW + 1.),
            bounds.top() + px(LH + LH / 2.),
        );
        cx.simulate_click(at, Modifiers::none());
        assert_eq!(caret(&input, &mut cx), 20 + 3);
        // Shift+click extends to the third row's start.
        cx.simulate_click(
            point(bounds.left() + px(1.), bounds.top() + px(2. * LH + 5.)),
            Modifiers::shift(),
        );
        let sel = input.read_with(&cx, |i, _| i.selection());
        assert_eq!((sel.tail, sel.head), (23, 40));
        // Double-click selects the word; a drag from it extends.
        let word = point(bounds.left() + px(6. * CW + 1.), bounds.top() + px(5.));
        cx.simulate_event(MouseDownEvent {
            position: word,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
            first_mouse: false,
        });
        assert_eq!(input.read_with(&cx, |i, _| i.selected_text()), "abcd");
        cx.simulate_event(MouseMoveEvent {
            position: point(bounds.left() + px(2. * CW + 1.), bounds.top() + px(LH + 5.)),
            modifiers: Modifiers::none(),
            pressed_button: Some(MouseButton::Left),
        });
        cx.simulate_event(MouseUpEvent {
            position: word,
            modifiers: Modifiers::none(),
            button: MouseButton::Left,
            click_count: 2,
        });
        let sel = input.read_with(&cx, |i, _| i.selection());
        assert_eq!((sel.tail, sel.head), (5, 22));
    }

    #[gpui::test]
    fn up_and_down_move_by_visual_row_and_tell_at_the_edges(cx: &mut TestAppContext) {
        let (input, events, mut cx) = open(cx, 20. * CW + 2., true);
        type_text(&mut cx, &"abcd abcd abcd abcd ".repeat(2));
        type_text(&mut cx, "abcdefg");
        assert_eq!(caret(&input, &mut cx), 47);
        events.borrow_mut().clear();
        // Down on the last row: told, not moved.
        cx.simulate_keystrokes("down");
        assert_eq!(*events.borrow(), [TextInputEvent::Down]);
        assert_eq!(caret(&input, &mut cx), 47);
        // Up keeps the column through the wrapped rows.
        cx.simulate_keystrokes("up");
        assert_eq!(caret(&input, &mut cx), 27);
        cx.simulate_keystrokes("up");
        assert_eq!(caret(&input, &mut cx), 7);
        assert_eq!(*events.borrow(), [TextInputEvent::Down]);
        // Up on the first row: told, not moved.
        cx.simulate_keystrokes("up");
        assert_eq!(caret(&input, &mut cx), 7);
        assert_eq!(*events.borrow(), [TextInputEvent::Down, TextInputEvent::Up]);
        cx.simulate_keystrokes("down down");
        assert_eq!(caret(&input, &mut cx), 47);
        // With a selection the edges move instead of telling: Shift+Up selects to the start of the first row.
        cx.simulate_keystrokes("shift-up shift-up shift-up");
        let sel = input.read_with(&cx, |i, _| i.selection());
        assert_eq!((sel.tail, sel.head), (47, 0));
        cx.simulate_keystrokes("up");
        assert_eq!(caret(&input, &mut cx), 0);
        assert_eq!(events.borrow().len(), 2);
        // The goal column: from the end of a long row through a shorter one and back.
        input.update(&mut cx, |i, cx| {
            i.set_text("abcdefghij\nab\nabcdefghij", cx)
        });
        cx.run_until_parked();
        input.update(&mut cx, |i, cx| i.set_caret(9, cx));
        cx.simulate_keystrokes("down");
        assert_eq!(caret(&input, &mut cx), 13);
        cx.simulate_keystrokes("down");
        assert_eq!(caret(&input, &mut cx), 23);
    }

    #[gpui::test]
    fn ime_composition_is_marked_then_committed(cx: &mut TestAppContext) {
        let (input, _, mut cx) = open(cx, 600., true);
        type_text(&mut cx, "a");
        input.update_in(&mut cx, |i, window, cx| {
            i.replace_and_mark_text_in_range(None, "ni", Some(2..2), window, cx)
        });
        assert_eq!(text(&input, &mut cx), "ani");
        let marked = input.update_in(&mut cx, |i, window, cx| i.marked_text_range(window, cx));
        assert_eq!(marked, Some(1..3));
        assert_eq!(caret(&input, &mut cx), 3);
        // The composition grows, then the IME commits a character in its place.
        input.update_in(&mut cx, |i, window, cx| {
            i.replace_and_mark_text_in_range(None, "nih", Some(3..3), window, cx)
        });
        assert_eq!(text(&input, &mut cx), "anih");
        input.update_in(&mut cx, |i, window, cx| {
            i.replace_text_in_range(None, "\u{4F60}", window, cx)
        });
        assert_eq!(text(&input, &mut cx), "a\u{4F60}");
        let marked = input.update_in(&mut cx, |i, window, cx| i.marked_text_range(window, cx));
        assert_eq!(marked, None);
        // UTF-16 ranges: the IME reads back what it wrote.
        let back = input.update_in(&mut cx, |i, window, cx| {
            i.text_for_range(1..2, &mut None, window, cx)
        });
        assert_eq!(back.as_deref(), Some("\u{4F60}"));
        cx.run_until_parked();
        let bounds = input.update_in(&mut cx, |i, window, cx| {
            i.bounds_for_range(2..2, Bounds::default(), window, cx)
        });
        assert!(bounds.is_some());
    }
}
