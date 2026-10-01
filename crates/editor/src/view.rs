//! `EditorView`: the GPUI view and element that draw and edit an [`Editor`].

use std::collections::BTreeMap;
use std::ops::Range;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use gpui::{
    App, Bounds, ClipboardItem, ContentMask, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, Focusable, GlobalElementId, Hsla,
    InspectorElementId, IntoElement, KeyBinding, LayoutId, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, ParentElement, Pixels, Point, Render, Rgba, ScrollWheelEvent,
    ShapedLine, SharedString, Size, Style, Styled, Task, TextAlign, TextRun, UTF16Selection,
    UnderlineStyle, Window, actions, div, fill, point, prelude::*, px, relative, rgb, size,
};
use text::{Anchor, BufferSnapshot, OffsetUtf16};

use crate::buffer::Buffer;
use crate::display::{
    byte_for_visual_column, expand_tabs, from_display, to_display, visual_column,
};
use crate::editor::{ClickKind, Editor, FindQuery, SelectionRange};
use crate::syntax::{
    HighlightStats, HighlightUpdate, Highlighter, Language, LineHighlights, Span, SyntaxTheme,
    SyntaxThread,
};

actions!(
    editor,
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
        MoveToLineStart,
        MoveToLineEnd,
        SelectToLineStart,
        SelectToLineEnd,
        MoveToStart,
        MoveToEnd,
        SelectToStart,
        SelectToEnd,
        PageUp,
        PageDown,
        SelectPageUp,
        SelectPageDown,
        Backspace,
        Delete,
        DeleteWordLeft,
        DeleteWordRight,
        Newline,
        Tab,
        Undo,
        Redo,
        SelectAll,
        Copy,
        Cut,
        Paste,
        AddCaretAbove,
        AddCaretBelow,
        SelectNextOccurrence,
        Find,
        FindNext,
        FindPrevious,
        ToggleFindCaseSensitive,
        Cancel,
    ]
);

/// The key context the bindings below are scoped to.
pub const KEY_CONTEXT: &str = "Editor";

/// Visual Studio's default editor bindings. `secondary` is Ctrl on Windows
/// and Linux and Cmd on macOS.
pub fn key_bindings() -> Vec<KeyBinding> {
    let c = Some(KEY_CONTEXT);
    vec![
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
        KeyBinding::new("home", MoveToLineStart, c),
        KeyBinding::new("end", MoveToLineEnd, c),
        KeyBinding::new("shift-home", SelectToLineStart, c),
        KeyBinding::new("shift-end", SelectToLineEnd, c),
        KeyBinding::new("ctrl-home", MoveToStart, c),
        KeyBinding::new("ctrl-end", MoveToEnd, c),
        KeyBinding::new("ctrl-shift-home", SelectToStart, c),
        KeyBinding::new("ctrl-shift-end", SelectToEnd, c),
        KeyBinding::new("pageup", PageUp, c),
        KeyBinding::new("pagedown", PageDown, c),
        KeyBinding::new("shift-pageup", SelectPageUp, c),
        KeyBinding::new("shift-pagedown", SelectPageDown, c),
        KeyBinding::new("backspace", Backspace, c),
        KeyBinding::new("shift-backspace", Backspace, c),
        KeyBinding::new("delete", Delete, c),
        KeyBinding::new("ctrl-backspace", DeleteWordLeft, c),
        KeyBinding::new("ctrl-delete", DeleteWordRight, c),
        KeyBinding::new("enter", Newline, c),
        KeyBinding::new("tab", Tab, c),
        KeyBinding::new("secondary-z", Undo, c),
        KeyBinding::new("secondary-y", Redo, c),
        KeyBinding::new("secondary-shift-z", Redo, c),
        KeyBinding::new("secondary-a", SelectAll, c),
        KeyBinding::new("secondary-c", Copy, c),
        KeyBinding::new("secondary-x", Cut, c),
        KeyBinding::new("secondary-v", Paste, c),
        KeyBinding::new("alt-shift-up", AddCaretAbove, c),
        KeyBinding::new("alt-shift-down", AddCaretBelow, c),
        KeyBinding::new("alt-shift-.", SelectNextOccurrence, c),
        KeyBinding::new("secondary-f", Find, c),
        KeyBinding::new("f3", FindNext, c),
        KeyBinding::new("shift-f3", FindPrevious, c),
        KeyBinding::new("alt-c", ToggleFindCaseSensitive, c),
        KeyBinding::new("escape", Cancel, c),
    ]
}

/// Fonts, metrics and colors for one editor.
#[derive(Clone, Debug)]
pub struct EditorStyle {
    pub font_family: SharedString,
    pub font_size: Pixels,
    pub line_height: Pixels,
    pub theme: eludite_ui::Theme,
    pub syntax: SyntaxTheme,
    pub line_number: Rgba,
    pub line_number_active: Rgba,
    pub selection: Rgba,
    pub caret: Rgba,
    pub current_line_border: Rgba,
    pub find_match: Rgba,
}

impl Default for EditorStyle {
    fn default() -> Self {
        let theme = eludite_ui::Theme::vs_dark();
        Self {
            font_family: default_font_family(),
            font_size: theme.typography.body,
            line_height: px(19.),
            syntax: SyntaxTheme::vs_dark(&theme),
            theme,
            line_number: rgb(0x2B91AF),
            line_number_active: rgb(0xC6C6C6),
            selection: rgb(0x264F78),
            caret: rgb(0xDCDCDC),
            current_line_border: rgb(0x464646),
            find_match: rgb(0x623315),
        }
    }
}

/// `ELUDITE_EDITOR_FONT` if set, else the platform's usual monospace font.
pub fn default_font_family() -> SharedString {
    if let Ok(f) = std::env::var("ELUDITE_EDITOR_FONT") {
        return f.into();
    }
    if cfg!(target_os = "windows") {
        "Cascadia Mono".into()
    } else if cfg!(target_os = "macos") {
        "Menlo".into()
    } else {
        "Noto Sans Mono".into()
    }
}

/// A styled range drawn on top of the text, for LSP results and other
/// overlays. Ranges are anchors, so they follow edits.
#[derive(Clone, Debug)]
pub struct Decoration {
    pub range: Range<Anchor>,
    pub style: DecorationStyle,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DecorationStyle {
    /// Fill behind the text (for example a highlighted reference).
    Background(Rgba),
    /// Underline (wavy for diagnostics squiggles).
    Underline { color: Rgba, wavy: bool },
    /// Text color, painted over syntax colors (semantic tokens).
    Foreground(Rgba),
}

struct SyntaxState {
    language: Option<Arc<Language>>,
    /// Idle highlighter; `None` while a step runs on the background thread.
    highlighter: Option<Highlighter>,
    cancel: Option<Arc<AtomicBool>>,
    task: Option<Task<()>>,
    highlights: LineHighlights,
    /// The snapshot `highlights` is aligned with.
    snapshot: Option<BufferSnapshot>,
    complete: bool,
    updates: usize,
    last_stats: HighlightStats,
}

/// What the last paint laid out; used to map mouse positions and IME
/// queries back to buffer positions.
struct LastLayout {
    bounds: Bounds<Pixels>,
    text_left: Pixels,
    line_height: Pixels,
    char_width: Pixels,
    scroll: Point<Pixels>,
    rows: Vec<(u32, String, ShapedLine)>,
}

/// A GPUI view showing one [`Editor`].
///
/// Embed it like any view (`cx.new(|cx| EditorView::new(buffer, language, cx))`)
/// and register [`key_bindings`] once per app. Input arrives through GPUI's
/// text input handler (typing), actions (keys) and mouse listeners.
/// Highlighting runs on the [`SyntaxThread`]; until a step finishes,
/// the last highlights are shown, moved through any edits made since.
pub struct EditorView {
    editor: Editor,
    focus: FocusHandle,
    style: EditorStyle,
    scroll: Point<Pixels>,
    autoscroll: bool,
    layout: Option<LastLayout>,
    syntax: SyntaxState,
    decorations: BTreeMap<&'static str, Vec<Decoration>>,
    find_bar_open: bool,
    find_match_count: usize,
    dragging: bool,
}

impl EditorView {
    /// A view over `buffer`, highlighted with `language` unless the buffer is
    /// over [`crate::LARGE_FILE_THRESHOLD`].
    pub fn new(buffer: Buffer, language: Option<Arc<Language>>, cx: &mut Context<Self>) -> Self {
        let language = language.filter(|_| !buffer.is_large());
        let highlighter = language.clone().map(Highlighter::new);
        let mut this = Self {
            editor: Editor::new(buffer),
            focus: cx.focus_handle(),
            style: EditorStyle::default(),
            scroll: Point::default(),
            autoscroll: false,
            layout: None,
            syntax: SyntaxState {
                language,
                cancel: highlighter.as_ref().map(|h| h.cancel_flag()),
                highlighter,
                task: None,
                highlights: LineHighlights::default(),
                snapshot: None,
                complete: false,
                updates: 0,
                last_stats: HighlightStats::default(),
            },
            decorations: BTreeMap::new(),
            find_bar_open: false,
            find_match_count: 0,
            dragging: false,
        };
        this.schedule_highlight(cx);
        this
    }

    pub fn editor(&self) -> &Editor {
        &self.editor
    }

    /// Change the editor (or its buffer) programmatically. Highlighting,
    /// scrolling and repaint follow as for user edits.
    pub fn update_editor<R>(
        &mut self,
        cx: &mut Context<Self>,
        f: impl FnOnce(&mut Editor) -> R,
    ) -> R {
        let r = f(&mut self.editor);
        self.changed(cx);
        r
    }

    pub fn style(&self) -> &EditorStyle {
        &self.style
    }

    pub fn set_style(&mut self, style: EditorStyle, cx: &mut Context<Self>) {
        self.style = style;
        cx.notify();
    }

    pub fn language(&self) -> Option<&Arc<Language>> {
        self.syntax.language.as_ref()
    }

    /// Replace one layer of decorations (for example `"diagnostics"`).
    pub fn set_decorations(
        &mut self,
        layer: &'static str,
        decorations: Vec<Decoration>,
        cx: &mut Context<Self>,
    ) {
        self.decorations.insert(layer, decorations);
        cx.notify();
    }

    pub fn clear_decorations(&mut self, layer: &'static str, cx: &mut Context<Self>) {
        self.decorations.remove(layer);
        cx.notify();
    }

    /// The decorations of one layer, as last set.
    pub fn decorations(&self, layer: &str) -> &[Decoration] {
        self.decorations.get(layer).map_or(&[], Vec::as_slice)
    }

    // ----- scrolling -----

    pub fn scroll_position(&self) -> Point<Pixels> {
        self.scroll
    }

    /// Vertical scroll offset in pixels (top of the viewport).
    pub fn set_scroll_y(&mut self, y: Pixels, cx: &mut Context<Self>) {
        self.scroll.y = y.max(px(0.)).min(self.max_scroll_y());
        self.autoscroll = false;
        cx.notify();
    }

    /// The largest vertical offset: the last row at the top of the viewport.
    pub fn max_scroll_y(&self) -> Pixels {
        self.style.line_height * (self.editor.buffer().line_count().saturating_sub(1)) as f32
    }

    pub fn line_height(&self) -> Pixels {
        self.style.line_height
    }

    /// Scroll so `row` is at the top.
    pub fn scroll_to_row(&mut self, row: u32, cx: &mut Context<Self>) {
        self.set_scroll_y(self.style.line_height * row as f32, cx);
    }

    /// Rows currently visible (from the last layout, or an estimate).
    pub fn visible_rows(&self) -> Range<u32> {
        let lh = self.style.line_height;
        let first = (self.scroll.y / lh).floor() as u32;
        let count = self
            .layout
            .as_ref()
            .map_or(60, |l| (l.bounds.size.height / lh).ceil() as u32 + 1);
        first..(first + count).min(self.editor.buffer().line_count())
    }

    // ----- highlighting -----

    /// True once highlights cover the whole current buffer version.
    pub fn highlights_complete(&self) -> bool {
        self.syntax.complete
            && self
                .syntax
                .snapshot
                .as_ref()
                .is_some_and(|s| s.version() == self.editor.buffer().snapshot().version())
    }

    /// The highlights shown for the current text (possibly stale, interpolated).
    pub fn highlights(&self) -> &LineHighlights {
        &self.syntax.highlights
    }

    /// Number of highlight results received, and the last step's stats.
    pub fn highlight_progress(&self) -> (usize, HighlightStats) {
        (self.syntax.updates, self.syntax.last_stats)
    }

    fn schedule_highlight(&mut self, cx: &mut Context<Self>) {
        let Some(mut highlighter) = self.syntax.highlighter.take() else {
            return; // A step is running; its completion re-checks the buffer version.
        };
        let snapshot = self.editor.buffer().snapshot().clone();
        let priority = self.visible_rows();
        let step = SyntaxThread::global().run(move || {
            let update = highlighter.step(&snapshot, priority);
            (highlighter, update)
        });
        self.syntax.task = Some(cx.spawn(async move |this, cx| {
            let Some((highlighter, update)) = step.await else {
                return;
            };
            this.update(cx, |this, cx| {
                this.finish_highlight(highlighter, update, cx)
            })
            .ok();
        }));
    }

    fn finish_highlight(
        &mut self,
        highlighter: Highlighter,
        update: Option<HighlightUpdate>,
        cx: &mut Context<Self>,
    ) {
        self.syntax.highlighter = Some(highlighter);
        self.syntax.task = None;
        let Some(update) = update else {
            return; // Cancelled.
        };
        let current = self.editor.buffer().snapshot().clone();
        let mut highlights = update.highlights;
        highlights.interpolate(&update.snapshot, &current);
        let old = std::mem::replace(&mut self.syntax.highlights, highlights);
        // Dropping 100k rows of spans is not free; keep it off the UI thread
        // (and in the syntax thread's malloc arena, where it was allocated).
        drop(SyntaxThread::global().run(move || drop(old)));
        let up_to_date = update.version == *current.version();
        self.syntax.snapshot = Some(current);
        self.syntax.complete = update.complete && up_to_date;
        self.syntax.updates += 1;
        self.syntax.last_stats = update.stats;
        if !self.syntax.complete {
            self.schedule_highlight(cx);
        }
        cx.notify();
    }

    /// Move the shown highlights through edits made since they were computed.
    fn sync_highlights(&mut self) {
        let current = self.editor.buffer().snapshot();
        if let Some(old) = &self.syntax.snapshot
            && old.version() != current.version()
        {
            self.syntax.highlights.interpolate(old, current);
            self.syntax.snapshot = Some(current.clone());
            self.syntax.complete = false;
        }
    }

    fn changed(&mut self, cx: &mut Context<Self>) {
        self.sync_highlights();
        if self.find_bar_open {
            self.recount_find_matches();
        }
        self.autoscroll = true;
        self.schedule_highlight(cx);
        cx.notify();
    }

    fn moved(&mut self, cx: &mut Context<Self>) {
        self.autoscroll = true;
        cx.notify();
    }

    // ----- input: actions -----

    fn edit(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        f(&mut self.editor);
        self.changed(cx);
    }

    fn movement(&mut self, cx: &mut Context<Self>, f: impl FnOnce(&mut Editor)) {
        f(&mut self.editor);
        self.moved(cx);
    }

    fn backspace(&mut self, _: &Backspace, _: &mut Window, cx: &mut Context<Self>) {
        if self.find_bar_open {
            let mut q = self.editor.find_query().clone();
            q.text.pop();
            self.set_find_text(q, cx);
            return;
        }
        self.edit(cx, Editor::backspace);
    }

    fn newline(&mut self, _: &Newline, _: &mut Window, cx: &mut Context<Self>) {
        if self.find_bar_open {
            self.movement(cx, |e| {
                e.find_next();
            });
            return;
        }
        self.edit(cx, Editor::newline);
    }

    fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        if self.find_bar_open {
            self.find_bar_open = false;
            cx.notify();
            return;
        }
        self.movement(cx, Editor::collapse_to_primary);
    }

    fn find(&mut self, _: &Find, _: &mut Window, cx: &mut Context<Self>) {
        self.find_bar_open = true;
        // Seed the query with a single-line selection, as Visual Studio does.
        let sel = self.editor.primary_selection();
        if !sel.is_empty() {
            let text = self.editor.buffer().text_for_range(sel.range());
            if !text.contains('\n') {
                let q = FindQuery {
                    text,
                    case_sensitive: self.editor.find_query().case_sensitive,
                };
                self.editor.set_find_query(q);
            }
        }
        self.recount_find_matches();
        cx.notify();
    }

    fn toggle_case(&mut self, _: &ToggleFindCaseSensitive, _: &mut Window, cx: &mut Context<Self>) {
        let mut q = self.editor.find_query().clone();
        q.case_sensitive = !q.case_sensitive;
        self.set_find_text(q, cx);
    }

    /// Find as you type: search again from the start of the current match.
    fn set_find_text(&mut self, query: FindQuery, cx: &mut Context<Self>) {
        let start = self.editor.primary_selection().start();
        self.editor.set_find_query(query);
        self.editor.set_caret(start);
        if !self.editor.find_query().text.is_empty() {
            self.editor.find_next();
        }
        self.recount_find_matches();
        self.moved(cx);
    }

    fn recount_find_matches(&mut self) {
        let len = self.editor.buffer().len();
        self.find_match_count = if self.editor.find_query().text.is_empty() {
            0
        } else {
            self.editor.find_matches(0..len).len()
        };
    }

    pub fn is_find_bar_open(&self) -> bool {
        self.find_bar_open
    }

    fn copy(&mut self, _: &Copy, _: &mut Window, cx: &mut Context<Self>) {
        cx.write_to_clipboard(ClipboardItem::new_string(self.editor.selected_text()));
    }

    fn cut(&mut self, _: &Cut, _: &mut Window, cx: &mut Context<Self>) {
        let text = self.editor.cut();
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.changed(cx);
    }

    fn paste(&mut self, _: &Paste, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(text) = cx.read_from_clipboard().and_then(|i| i.text()) {
            self.edit(cx, |e| e.paste(&text));
        }
    }

    fn insert_text(&mut self, text: &str, cx: &mut Context<Self>) {
        if self.find_bar_open {
            let mut q = self.editor.find_query().clone();
            q.text.push_str(text);
            self.set_find_text(q, cx);
            return;
        }
        self.edit(cx, |e| e.insert(text));
    }

    // ----- input: mouse -----

    fn offset_for_position(&self, position: Point<Pixels>) -> Option<usize> {
        let l = self.layout.as_ref()?;
        let buffer = self.editor.buffer();
        let y = position.y - l.bounds.top() + l.scroll.y;
        let row = ((y / l.line_height).floor().max(0.) as u32).min(buffer.line_count() - 1);
        let x = (position.x - l.text_left + l.scroll.x).max(px(0.));
        let col = match l.rows.iter().find(|(r, _, _)| *r == row) {
            Some((_, text, shaped)) => from_display(text, shaped.closest_index_for_x(x)),
            None => {
                let text = buffer.line(row);
                byte_for_visual_column(&text, (x / l.char_width).round() as u32)
            }
        };
        Some(buffer.point_to_offset(text::Point::new(row, col as u32)))
    }

    /// Window position of the top-left corner of the character at `offset`,
    /// if its row was in the last painted frame. For anchoring popups
    /// (completion, hover) to the text.
    pub fn pixel_position_for_offset(&self, offset: usize) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let p = self.editor.buffer().offset_to_point(offset);
        let (_, text, shaped) = l.rows.iter().find(|(r, _, _)| *r == p.row)?;
        let x = l.text_left - l.scroll.x + shaped.x_for_index(to_display(text, p.column as usize));
        let y = l.bounds.top() + l.line_height * p.row as f32 - l.scroll.y;
        Some(point(x, y))
    }

    fn on_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        window.focus(&self.focus, cx);
        let Some(offset) = self.offset_for_position(event.position) else {
            return;
        };
        let kind = match event.click_count {
            0 | 1 => ClickKind::Single,
            2 => ClickKind::Double,
            _ => ClickKind::Triple,
        };
        let m = event.modifiers;
        let add = m.control && m.alt;
        self.editor.click(offset, kind, add, m.shift);
        self.dragging = true;
        self.moved(cx);
    }

    fn on_mouse_move(&mut self, event: &MouseMoveEvent, _: &mut Window, cx: &mut Context<Self>) {
        if !self.dragging || event.pressed_button != Some(MouseButton::Left) {
            return;
        }
        if let Some(offset) = self.offset_for_position(event.position) {
            self.editor.drag_to(offset);
            self.moved(cx);
        }
    }

    fn on_mouse_up(&mut self, _: &MouseUpEvent, _: &mut Window, _: &mut Context<Self>) {
        self.dragging = false;
    }

    fn on_scroll(&mut self, event: &ScrollWheelEvent, _: &mut Window, cx: &mut Context<Self>) {
        let mut delta = event.delta.pixel_delta(self.style.line_height);
        if event.modifiers.shift && delta.x == px(0.) {
            delta = point(delta.y, px(0.));
        }
        self.scroll.y = (self.scroll.y - delta.y)
            .max(px(0.))
            .min(self.max_scroll_y());
        self.scroll.x = (self.scroll.x - delta.x).max(px(0.));
        self.autoscroll = false;
        cx.notify();
    }

    /// Called at the start of each layout: apply autoscroll and clamp.
    fn prepare_frame(&mut self, viewport: Size<Pixels>, text_width: Pixels, char_width: Pixels) {
        let lh = self.style.line_height;
        let rows = ((viewport.height / lh).floor() as u32).max(1);
        self.editor.page_rows = rows.saturating_sub(1).max(1);
        if self.autoscroll {
            self.autoscroll = false;
            let head = self.editor.primary_head();
            let top = lh * head.row as f32;
            if top < self.scroll.y {
                self.scroll.y = top;
            } else if top + lh > self.scroll.y + viewport.height {
                self.scroll.y = top + lh - viewport.height;
            }
            let line = self.editor.buffer().line(head.row);
            let x = char_width * visual_column(&line, head.column as usize) as f32;
            let margin = char_width * 4.;
            if x < self.scroll.x {
                self.scroll.x = (x - margin).max(px(0.));
            } else if x + margin > self.scroll.x + text_width {
                self.scroll.x = x + margin - text_width;
            }
        }
        self.scroll.y = self.scroll.y.max(px(0.)).min(self.max_scroll_y());
    }
}

macro_rules! bind {
    ($el:expr, $cx:expr, $( $action:ident => |$e:ident| $body:expr, $kind:ident; )*) => {
        $el$(
            .on_action($cx.listener(|this: &mut EditorView, _: &$action, _: &mut Window, cx: &mut Context<EditorView>| {
                this.$kind(cx, |$e: &mut Editor| { $body; });
            }))
        )*
    };
}

impl Render for EditorView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_highlights();
        let el = div()
            .id("editor")
            .key_context(KEY_CONTEXT)
            .track_focus(&self.focus)
            .size_full()
            .relative()
            .cursor(CursorStyle::IBeam)
            .on_action(cx.listener(Self::backspace))
            .on_action(cx.listener(Self::newline))
            .on_action(cx.listener(Self::cancel))
            .on_action(cx.listener(Self::find))
            .on_action(cx.listener(Self::toggle_case))
            .on_action(cx.listener(Self::copy))
            .on_action(cx.listener(Self::cut))
            .on_action(cx.listener(Self::paste))
            .on_mouse_down(MouseButton::Left, cx.listener(Self::on_mouse_down))
            .on_mouse_move(cx.listener(Self::on_mouse_move))
            .on_mouse_up(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_mouse_up_out(MouseButton::Left, cx.listener(Self::on_mouse_up))
            .on_scroll_wheel(cx.listener(Self::on_scroll));
        let el = bind!(el, cx,
            MoveLeft => |e| e.move_left(false), movement;
            MoveRight => |e| e.move_right(false), movement;
            MoveUp => |e| e.move_up(false), movement;
            MoveDown => |e| e.move_down(false), movement;
            SelectLeft => |e| e.move_left(true), movement;
            SelectRight => |e| e.move_right(true), movement;
            SelectUp => |e| e.move_up(true), movement;
            SelectDown => |e| e.move_down(true), movement;
            MoveWordLeft => |e| e.move_word_left(false), movement;
            MoveWordRight => |e| e.move_word_right(false), movement;
            SelectWordLeft => |e| e.move_word_left(true), movement;
            SelectWordRight => |e| e.move_word_right(true), movement;
            MoveToLineStart => |e| e.move_home(false), movement;
            MoveToLineEnd => |e| e.move_end(false), movement;
            SelectToLineStart => |e| e.move_home(true), movement;
            SelectToLineEnd => |e| e.move_end(true), movement;
            MoveToStart => |e| e.move_to_start(false), movement;
            MoveToEnd => |e| e.move_to_end(false), movement;
            SelectToStart => |e| e.move_to_start(true), movement;
            SelectToEnd => |e| e.move_to_end(true), movement;
            PageUp => |e| e.page_up(false), movement;
            PageDown => |e| e.page_down(false), movement;
            SelectPageUp => |e| e.page_up(true), movement;
            SelectPageDown => |e| e.page_down(true), movement;
            SelectAll => |e| e.select_all(), movement;
            AddCaretAbove => |e| e.add_caret_vertical(-1), movement;
            AddCaretBelow => |e| e.add_caret_vertical(1), movement;
            SelectNextOccurrence => |e| e.select_next_occurrence(), movement;
            FindNext => |e| e.find_next(), movement;
            FindPrevious => |e| e.find_previous(), movement;
            Delete => |e| e.delete(), edit;
            DeleteWordLeft => |e| e.delete_word_left(), edit;
            DeleteWordRight => |e| e.delete_word_right(), edit;
            Tab => |e| e.tab(), edit;
            Undo => |e| e.undo(), edit;
            Redo => |e| e.redo(), edit;
        );
        let find_bar = self.find_bar_open.then(|| {
            let q = self.editor.find_query();
            let theme = &self.style.theme;
            div()
                .absolute()
                .top_0()
                .right(px(16.))
                .px_2()
                .py_1()
                .bg(theme.panel)
                .border_1()
                .border_color(theme.accent)
                .text_color(theme.text)
                .text_size(theme.typography.ui)
                .child(SharedString::from(format!(
                    "Find: {}{}   {} match{}",
                    q.text,
                    if q.case_sensitive { "  [Aa]" } else { "" },
                    self.find_match_count,
                    if self.find_match_count == 1 { "" } else { "es" },
                )))
        });
        el.child(EditorElement { view: cx.entity() })
            .children(find_bar)
    }
}

impl Focusable for EditorView {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Drop for EditorView {
    fn drop(&mut self) {
        if let Some(cancel) = &self.syntax.cancel {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl EntityInputHandler for EditorView {
    fn text_for_range(
        &mut self,
        range_utf16: Range<usize>,
        actual_range: &mut Option<Range<usize>>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<String> {
        let s = self.editor.buffer().snapshot();
        let range = utf16_to_range(s, &range_utf16);
        actual_range.replace(range_to_utf16(s, &range));
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
            range: range_to_utf16(self.editor.buffer().snapshot(), &sel.range()),
            reversed: sel.reversed(),
        })
    }

    fn marked_text_range(&self, _: &mut Window, _: &mut Context<Self>) -> Option<Range<usize>> {
        None
    }

    fn unmark_text(&mut self, _: &mut Window, _: &mut Context<Self>) {}

    fn replace_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        text: &str,
        _: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(r) = range_utf16
            && !self.find_bar_open
        {
            let range = utf16_to_range(self.editor.buffer().snapshot(), &r);
            if range != self.editor.primary_selection().range() {
                self.editor.set_selections(
                    vec![SelectionRange {
                        tail: range.start,
                        head: range.end,
                    }],
                    0,
                );
            }
        }
        self.insert_text(text, cx);
    }

    /// IME composition is not supported yet (brief 0009, out of scope): the
    /// composed text is inserted as typed.
    fn replace_and_mark_text_in_range(
        &mut self,
        range_utf16: Option<Range<usize>>,
        new_text: &str,
        _new_selected_range: Option<Range<usize>>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.replace_text_in_range(range_utf16, new_text, window, cx);
    }

    fn bounds_for_range(
        &mut self,
        range_utf16: Range<usize>,
        _element_bounds: Bounds<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        let l = self.layout.as_ref()?;
        let s = self.editor.buffer().snapshot();
        let start = s.offset_to_point(utf16_to_range(s, &range_utf16).start);
        let (_, text, shaped) = l.rows.iter().find(|(r, _, _)| *r == start.row)?;
        let x =
            l.text_left - l.scroll.x + shaped.x_for_index(to_display(text, start.column as usize));
        let y = l.bounds.top() + l.line_height * start.row as f32 - l.scroll.y;
        Some(Bounds::new(point(x, y), size(px(1.), l.line_height)))
    }

    fn character_index_for_point(
        &mut self,
        point: Point<Pixels>,
        _: &mut Window,
        _: &mut Context<Self>,
    ) -> Option<usize> {
        let offset = self.offset_for_position(point)?;
        Some(
            self.editor
                .buffer()
                .snapshot()
                .offset_to_offset_utf16(offset)
                .0,
        )
    }
}

fn utf16_to_range(s: &BufferSnapshot, r: &Range<usize>) -> Range<usize> {
    let max = s.offset_to_offset_utf16(s.len()).0;
    s.offset_utf16_to_offset(OffsetUtf16(r.start.min(max)))
        ..s.offset_utf16_to_offset(OffsetUtf16(r.end.min(max)))
}

fn range_to_utf16(s: &BufferSnapshot, r: &Range<usize>) -> Range<usize> {
    s.offset_to_offset_utf16(r.start).0..s.offset_to_offset_utf16(r.end).0
}

// ----- the element -----

struct EditorElement {
    view: Entity<EditorView>,
}

impl IntoElement for EditorElement {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

struct RowLayout {
    row: u32,
    text: String,
    line: ShapedLine,
    number: ShapedLine,
}

pub struct PrepaintState {
    rows: Vec<RowLayout>,
    quads: Vec<gpui::PaintQuad>,
    carets: Vec<gpui::PaintQuad>,
    gutter_width: Pixels,
    char_width: Pixels,
    scroll: Point<Pixels>,
    line_height: Pixels,
    background: Rgba,
    focused: bool,
}

fn hsla(c: Rgba) -> Hsla {
    c.into()
}

impl Element for EditorElement {
    type RequestLayoutState = ();
    type PrepaintState = PrepaintState;

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
        let mut style = Style::default();
        style.size.width = relative(1.).into();
        style.size.height = relative(1.).into();
        (window.request_layout(style, [], cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) -> PrepaintState {
        let (font, font_size, line_count) = {
            let v = self.view.read(cx);
            (
                gpui::font(v.style.font_family.clone()),
                v.style.font_size,
                v.editor.buffer().line_count(),
            )
        };
        let text_system = window.text_system();
        let font_id = text_system.resolve_font(&font);
        let char_width = text_system
            .advance(font_id, font_size, 'm')
            .map(|s| s.width)
            .unwrap_or(font_size * 0.6);
        let digits = line_count.to_string().len().max(3);
        let gutter_width = char_width * (digits as f32 + 2.5);
        let text_width = bounds.size.width - gutter_width;
        self.view.update(cx, |v, _| {
            v.prepare_frame(bounds.size, text_width, char_width)
        });

        let v = self.view.read(cx);
        let style = &v.style;
        let lh = style.line_height;
        let buffer = v.editor.buffer();
        let snapshot = buffer.snapshot();
        let scroll = v.scroll;
        let first = (scroll.y / lh).floor() as u32;
        let count = (bounds.size.height / lh).ceil() as u32 + 1;
        let last = (first + count).min(line_count);
        let text_left = bounds.left() + gutter_width;
        let row_top = |row: u32| bounds.top() + lh * row as f32 - scroll.y;

        let selections = v.editor.selections();
        let primary = v.editor.primary_selection();
        let primary_row = snapshot.offset_to_point(primary.head).row;

        // Decorations resolved to offsets and filtered to the visible range.
        let vis_start = snapshot.point_to_offset(text::Point::new(first, 0));
        let vis_end = if last >= line_count {
            snapshot.len()
        } else {
            snapshot.point_to_offset(text::Point::new(last, 0))
        };
        let decorations: Vec<(Range<usize>, DecorationStyle)> = v
            .decorations
            .values()
            .flatten()
            .filter_map(|d| {
                let r = snapshot.offset_for_anchor(&d.range.start)
                    ..snapshot.offset_for_anchor(&d.range.end);
                (r.end >= vis_start && r.start <= vis_end).then_some((r, d.style))
            })
            .collect();
        let find_matches = if v.find_bar_open {
            v.editor.find_matches(vis_start..vis_end)
        } else {
            Vec::new()
        };

        let mut rows = Vec::with_capacity((last - first) as usize);
        let mut quads = Vec::new();
        let mut carets = Vec::new();
        for row in first..last {
            let text = buffer.line(row);
            let display = expand_tabs(&text);
            let row_start = snapshot.point_to_offset(text::Point::new(row, 0));
            let row_end = row_start + text.len();
            let runs = text_runs(
                &text,
                &display,
                v.syntax.highlights.spans(row),
                &decorations,
                row_start,
                &style.syntax,
                &font,
            );
            let line = text_system.shape_line(
                SharedString::from(display.into_owned()),
                font_size,
                &runs,
                None,
            );
            let x_of = |offset: usize| -> Pixels {
                text_left - scroll.x + line.x_for_index(to_display(&text, offset - row_start))
            };
            let top = row_top(row);
            let active = row == primary_row;
            if active && primary.is_empty() {
                quads.push(gpui::outline(
                    Bounds::new(point(text_left, top), size(text_width, lh)),
                    hsla(style.current_line_border),
                    gpui::BorderStyle::Solid,
                ));
            }
            // Find matches and background decorations.
            for m in &find_matches {
                if m.start <= row_end && m.end >= row_start && m.start < m.end {
                    let s = m.start.max(row_start);
                    let e = m.end.min(row_end);
                    if s <= e {
                        quads.push(fill(
                            Bounds::from_corners(point(x_of(s), top), point(x_of(e), top + lh)),
                            hsla(style.find_match),
                        ));
                    }
                }
            }
            for (r, d) in &decorations {
                if let DecorationStyle::Background(color) = d
                    && r.start <= row_end
                    && r.end >= row_start
                {
                    let s = r.start.max(row_start);
                    let e = r.end.min(row_end);
                    quads.push(fill(
                        Bounds::from_corners(point(x_of(s), top), point(x_of(e), top + lh)),
                        hsla(*color),
                    ));
                }
            }
            // Selections and carets.
            for sel in &selections {
                if !sel.is_empty() && sel.start() <= row_end && sel.end() > row_start {
                    let s = sel.start().max(row_start);
                    let e = sel.end().min(row_end);
                    let mut right = x_of(e);
                    if sel.end() > row_end {
                        right += char_width * 0.5; // the selected line break
                    }
                    quads.push(fill(
                        Bounds::from_corners(point(x_of(s), top), point(right, top + lh)),
                        hsla(style.selection),
                    ));
                }
                if sel.head >= row_start && sel.head <= row_end {
                    carets.push(fill(
                        Bounds::new(point(x_of(sel.head), top), size(px(2.), lh)),
                        hsla(style.caret),
                    ));
                }
            }
            let number_color = if active {
                style.line_number_active
            } else {
                style.line_number
            };
            let number_text = SharedString::from((row + 1).to_string());
            let number = text_system.shape_line(
                number_text.clone(),
                font_size,
                &[TextRun {
                    len: number_text.len(),
                    font: font.clone(),
                    color: hsla(number_color),
                    background_color: None,
                    underline: None,
                    strikethrough: None,
                }],
                None,
            );
            rows.push(RowLayout {
                row,
                text,
                line,
                number,
            });
        }
        PrepaintState {
            rows,
            quads,
            carets,
            gutter_width,
            char_width,
            scroll,
            line_height: lh,
            background: style.theme.background,
            focused: v.focus.is_focused(window),
        }
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        bounds: Bounds<Pixels>,
        _: &mut (),
        state: &mut PrepaintState,
        window: &mut Window,
        cx: &mut App,
    ) {
        let focus = self.view.read(cx).focus.clone();
        window.handle_input(
            &focus,
            ElementInputHandler::new(bounds, self.view.clone()),
            cx,
        );
        window.paint_quad(fill(bounds, hsla(state.background)));
        let lh = state.line_height;
        let text_left = bounds.left() + state.gutter_width;
        let row_top = |row: u32| bounds.top() + lh * row as f32 - state.scroll.y;
        for r in &state.rows {
            let x = text_left - state.char_width * 1.5 - r.number.width();
            r.number
                .paint(
                    point(x, row_top(r.row)),
                    lh,
                    TextAlign::Left,
                    None,
                    window,
                    cx,
                )
                .ok();
        }
        let text_bounds =
            Bounds::from_corners(point(text_left, bounds.top()), bounds.bottom_right());
        window.with_content_mask(
            Some(ContentMask {
                bounds: text_bounds,
            }),
            |window| {
                for q in state.quads.drain(..) {
                    window.paint_quad(q);
                }
                for r in &state.rows {
                    r.line
                        .paint(
                            point(text_left - state.scroll.x, row_top(r.row)),
                            lh,
                            TextAlign::Left,
                            None,
                            window,
                            cx,
                        )
                        .ok();
                }
                if state.focused {
                    for q in state.carets.drain(..) {
                        window.paint_quad(q);
                    }
                }
            },
        );
        let rows = std::mem::take(&mut state.rows)
            .into_iter()
            .map(|r| (r.row, r.text, r.line))
            .collect();
        let layout = LastLayout {
            bounds,
            text_left,
            line_height: lh,
            char_width: state.char_width,
            scroll: state.scroll,
            rows,
        };
        self.view.update(cx, |v, _| v.layout = Some(layout));
    }
}

/// Text runs for one display line: syntax colors, then decorations on top.
fn text_runs(
    text: &str,
    display: &str,
    spans: &[Span],
    decorations: &[(Range<usize>, DecorationStyle)],
    row_start: usize,
    syntax: &SyntaxTheme,
    font: &gpui::Font,
) -> Vec<TextRun> {
    let len = display.len();
    let snap = |byte_col: usize| -> usize {
        let b = crate::display::floor_char_boundary(text, byte_col.min(text.len()));
        to_display(text, b)
    };
    // Boundaries in display coordinates.
    let mut cuts = vec![0, len];
    for s in spans {
        cuts.push(snap(s.start as usize));
        cuts.push(snap(s.end as usize));
    }
    let row_end = row_start + text.len();
    let row_decorations: Vec<(Range<usize>, DecorationStyle)> = decorations
        .iter()
        .filter(|(r, _)| r.start <= row_end && r.end >= row_start)
        .map(|(r, d)| {
            let s = snap(r.start.max(row_start) - row_start);
            let e = snap(r.end.min(row_end) - row_start);
            (s..e, *d)
        })
        .collect();
    for (r, _) in &row_decorations {
        cuts.push(r.start);
        cuts.push(r.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut runs = Vec::with_capacity(cuts.len());
    let mut span_ix = 0;
    for w in cuts.windows(2) {
        let (s, e) = (w[0], w[1]);
        if s >= e || e > len {
            continue;
        }
        while span_ix < spans.len() && snap(spans[span_ix].end as usize) <= s {
            span_ix += 1;
        }
        let mut color = match spans.get(span_ix) {
            Some(sp) if snap(sp.start as usize) <= s => syntax.color(sp.kind),
            _ => syntax.default,
        };
        let mut underline = None;
        for (r, d) in &row_decorations {
            if r.start <= s && e <= r.end {
                match d {
                    DecorationStyle::Foreground(c) => color = *c,
                    DecorationStyle::Underline { color, wavy } => {
                        underline = Some(UnderlineStyle {
                            color: Some(hsla(*color)),
                            thickness: px(1.),
                            wavy: *wavy,
                        })
                    }
                    DecorationStyle::Background(_) => {}
                }
            }
        }
        runs.push(TextRun {
            len: e - s,
            font: font.clone(),
            color: hsla(color),
            background_color: None,
            underline,
            strikethrough: None,
        });
    }
    if runs.is_empty() {
        runs.push(TextRun {
            len,
            font: font.clone(),
            color: hsla(syntax.default),
            background_color: None,
            underline: None,
            strikethrough: None,
        });
    }
    runs
}
