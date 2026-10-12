//! The view's display state and visual aids (brief 0067): what [`EditorView`] keeps for outlining, word wrap, the
//! scroll bars, sticky scroll, brace matching, white space, indent guides, the caret blink and the font and zoom, the
//! display-row caret motions (Up, Down, Page Up, Page Down, Home, End across hidden and wrapped rows), and the layer the
//! element paints over the text for them ([`AidsLayout`]).

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use gpui::{
    App, Bounds, Context, CursorStyle, Hsla, Pixels, Point, Rgba, ShapedLine, SharedString, Task,
    TextAlign, TextRun, Window, fill, point, px, size,
};

use crate::SelectionRange;
use crate::display::{TAB_SIZE, VerticalLayout, byte_for_visual_column, to_display, visual_column};
use crate::folding::{DisplayEvent, Fold, Folds};
use crate::scrollbar::{
    BarsLayout, MARKER_HEIGHT, SCROLLBAR_SIZE, ScrollMarker, ThumbDrag, marker_y, thumb,
};
use crate::view::EditorView;
use crate::wrap::{WrapCache, segment_of, segments};

/// Width of the outlining margin between the line numbers and the text.
pub const OUTLINING_MARGIN: Pixels = px(14.);
/// Visual Studio's caret blink interval.
pub const CARET_BLINK: Duration = Duration::from_millis(530);
/// Zoom bounds and step, in percent.
pub const ZOOM_MIN: u32 = 20;
pub const ZOOM_MAX: u32 = 400;
pub const ZOOM_STEP: u32 = 10;
/// The editor's default font size (Tools > Options > Fonts and Colors), in pixels.
pub const DEFAULT_FONT_SIZE: f32 = 14.;

/// Everything brief 0067 adds to the view.
pub(crate) struct DisplayState {
    pub folds: Folds,
    /// Word wrap on, the glyphs at wrapped ends, and the columns the text area holds (set each layout).
    pub wrap: bool,
    pub wrap_glyphs: bool,
    pub wrap_columns: Option<u32>,
    pub wrap_cache: RefCell<WrapCache>,
    /// One more on every change of the display map's inputs other than the folds (wrap on or off, width).
    pub epoch: u64,
    pub markers: BTreeMap<&'static str, Vec<ScrollMarker>>,
    pub scroll_markers: bool,
    pub bars: BarsLayout,
    pub drag: Option<ThumbDrag>,
    /// The widest line laid out since the text last changed (the horizontal bar's range).
    pub max_width: Pixels,
    pub sticky: bool,
    pub sticky_max: u32,
    /// The pinned lines as painted: bounds and header row.
    pub sticky_hits: Vec<(Bounds<Pixels>, u32)>,
    pub white_space: bool,
    pub indent_guides: bool,
    pub brace_matching: bool,
    pub zoom: u32,
    pub font_family: Option<SharedString>,
    pub font_size: Option<f32>,
    /// The style's font family, size and line height before the font settings and zoom.
    pub unzoomed: Option<(SharedString, Pixels, Pixels)>,
    pub caret_on: bool,
    blink: Option<Task<()>>,
    last_activity: Instant,
    pub focused: bool,
    /// Goal columns of the last display-row move, with the heads they were for.
    pub goals: Option<(Vec<usize>, Vec<u32>)>,
    /// The outlining margin's expanders as painted: bounds and fold header row.
    pub expander_hits: Vec<(Bounds<Pixels>, u32)>,
    /// The collapsed adornments as painted: bounds and fold header row.
    pub adornment_hits: Vec<(Bounds<Pixels>, u32)>,
    pub hovered_adornment: Option<u32>,
    /// The zoom box at the bottom left, as painted.
    pub zoom_box: Option<Bounds<Pixels>>,
}

impl Default for DisplayState {
    fn default() -> Self {
        Self {
            folds: Folds::default(),
            wrap: false,
            wrap_glyphs: false,
            wrap_columns: None,
            wrap_cache: RefCell::default(),
            epoch: 0,
            markers: BTreeMap::new(),
            scroll_markers: true,
            bars: BarsLayout::default(),
            drag: None,
            max_width: px(0.),
            sticky: true,
            sticky_max: crate::sticky::DEFAULT_MAX_LINES,
            sticky_hits: Vec::new(),
            white_space: false,
            indent_guides: true,
            brace_matching: true,
            zoom: 100,
            font_family: None,
            font_size: None,
            unzoomed: None,
            caret_on: true,
            blink: None,
            last_activity: Instant::now(),
            focused: false,
            goals: None,
            expander_hits: Vec::new(),
            adornment_hits: Vec::new(),
            hovered_adornment: None,
            zoom_box: None,
        }
    }
}

impl DisplayState {
    /// The key the layout cache is fresh for (with the buffer version and lens epoch).
    pub fn layout_key(&self) -> (u64, u64) {
        (self.epoch, self.folds.epoch)
    }
}

/// Whether a display row move needs the display map (a collapsed fold or wrapped rows), else the editor's own moves
/// (buffer rows, its goal columns) apply.
fn mapped(layout: &VerticalLayout, wrap: bool) -> bool {
    wrap || !layout.hidden().is_empty()
}

impl EditorView {
    /// Something the display map depends on changed: lay out again.
    pub(crate) fn display_changed(&mut self, cx: &mut Context<Self>) {
        self.display.epoch += 1;
        self.display.goals = None;
        cx.notify();
    }

    /// The text changed (the view's `changed` hook).
    pub(crate) fn display_edited(&mut self, cx: &mut Context<Self>) {
        self.caret_activity();
        self.display.max_width = px(0.);
        self.folds_edited(cx);
    }

    /// The carets moved (the view's `moved` hook).
    pub(crate) fn display_moved(&mut self, cx: &mut Context<Self>) {
        self.caret_activity();
        self.reveal_hidden_carets(cx);
    }

    /// The display map's inputs for the current text: the collapsed folds' hidden rows and, with wrap on, every row's
    /// wrapped segments.
    pub(crate) fn display_inputs(&self) -> (Vec<Range<u32>>, Vec<u32>) {
        let hidden = self.hidden_rows();
        let columns = self
            .display
            .wrap
            .then_some(self.display.wrap_columns)
            .flatten();
        let wraps = self
            .display
            .wrap_cache
            .borrow_mut()
            .counts(self.editor.buffer().snapshot(), columns);
        (hidden, wraps)
    }

    /// The wrapped segments of `row` as byte ranges (one when wrap is off).
    pub fn row_segments(&self, row: u32) -> Vec<Range<usize>> {
        let columns = self
            .display
            .wrap
            .then_some(self.display.wrap_columns)
            .flatten();
        segments(&self.editor.buffer().line(row), columns)
    }

    // ----- word wrap -----

    /// Whether word wrap is on.
    pub fn word_wrap(&self) -> bool {
        self.display.wrap
    }

    /// Turn word wrap on or off (Edit > Advanced > Word Wrap; `editor.wordWrap`), with or without the glyphs at the
    /// end of each wrapped segment (`editor.wordWrap.glyphs`).
    pub fn set_word_wrap(&mut self, on: bool, glyphs: bool, cx: &mut Context<Self>) {
        if self.display.wrap == on && self.display.wrap_glyphs == glyphs {
            return;
        }
        // Keep the top row in place across the reflow.
        let top = self
            .vertical_layout()
            .hit(f32::from(self.scroll_position().y))
            .row;
        self.display.wrap = on;
        self.display.wrap_glyphs = glyphs;
        if on {
            self.set_scroll_x(px(0.), cx);
        }
        self.display_changed(cx);
        self.set_scroll_y_quietly(self.row_top(top));
    }

    /// Set the columns the text area holds (from the layout; tests set it directly).
    pub fn set_wrap_columns(&mut self, columns: u32) {
        if self.display.wrap_columns != Some(columns) {
            self.display.wrap_columns = Some(columns);
            if self.display.wrap {
                self.display.epoch += 1;
                self.display.goals = None;
            }
        }
    }

    // ----- white space, guides, brace matching, markers -----

    pub fn white_space_visible(&self) -> bool {
        self.display.white_space
    }

    /// View White Space (Ctrl+R, Ctrl+W).
    pub fn set_white_space_visible(&mut self, on: bool, cx: &mut Context<Self>) {
        self.display.white_space = on;
        cx.notify();
    }

    /// The indent guides (`editor.indentGuides`), brace matching (`editor.braceMatching`) and the scroll bar's
    /// markers (`editor.scrollBar.markers`).
    pub fn set_visual_aids(
        &mut self,
        indent_guides: bool,
        brace_matching: bool,
        scroll_markers: bool,
        cx: &mut Context<Self>,
    ) {
        self.display.indent_guides = indent_guides;
        self.display.brace_matching = brace_matching;
        self.display.scroll_markers = scroll_markers;
        cx.notify();
    }

    // ----- font and zoom -----

    /// The zoom level in percent.
    pub fn zoom(&self) -> u32 {
        self.display.zoom
    }

    /// Set the zoom level (clamped to 20 to 400 percent), keeping the top row in place.
    pub fn set_zoom(&mut self, level: u32, cx: &mut Context<Self>) -> u32 {
        let level = level.clamp(ZOOM_MIN, ZOOM_MAX);
        if level != self.display.zoom {
            let top = self
                .vertical_layout()
                .hit(f32::from(self.scroll_position().y))
                .row;
            self.display.zoom = level;
            self.restyle();
            self.display_changed(cx);
            self.set_scroll_y_quietly(self.row_top(top));
            cx.emit(DisplayEvent::ZoomChanged { level });
        }
        level
    }

    /// Zoom In (Ctrl+Shift+.) or Out (Ctrl+Shift+,) by one step.
    pub fn zoom_by(&mut self, steps: i32, cx: &mut Context<Self>) -> u32 {
        let level =
            (self.display.zoom as i32 + steps * ZOOM_STEP as i32).max(ZOOM_MIN as i32) as u32;
        self.set_zoom(level, cx)
    }

    /// The editor font (`editor.fontFamily`, `None` or empty: the default) and size in pixels (`editor.fontSize`).
    pub fn set_editor_font(
        &mut self,
        family: Option<SharedString>,
        size: Option<f32>,
        cx: &mut Context<Self>,
    ) {
        let family = family.filter(|f| !f.trim().is_empty());
        if self.display.font_family == family && self.display.font_size == size {
            return;
        }
        self.display.font_family = family;
        self.display.font_size = size;
        self.restyle();
        self.display_changed(cx);
    }

    /// Apply the font settings and the zoom to the style.
    pub(crate) fn restyle(&mut self) {
        if self.display.unzoomed.is_none() {
            let s = self.style();
            self.display.unzoomed = Some((s.font_family.clone(), s.font_size, s.line_height));
        }
        let base = self.display.unzoomed.clone().expect("set above");
        let (family, size, line_height) = base;
        let family = self.display.font_family.clone().unwrap_or(family);
        let (size, line_height) = match self.display.font_size {
            // Visual Studio's line height for a size: 19 px at 14 px.
            Some(s) => (s, (s * 19. / 14.).round()),
            None => (f32::from(size), f32::from(line_height)),
        };
        let zoom = self.display.zoom as f32 / 100.;
        let style = self.style_mut();
        style.font_family = family;
        style.font_size = px((size * zoom).max(1.));
        style.line_height = px((line_height * zoom).round().max(2.));
    }

    // ----- the caret blink -----

    /// The caret moved or the text changed: show the caret and restart the blink.
    pub(crate) fn caret_activity(&mut self) {
        self.display.caret_on = true;
        self.display.last_activity = Instant::now();
    }

    /// Whether the caret is drawn in this frame.
    pub fn caret_visible(&self) -> bool {
        self.display.caret_on
    }

    /// Start blinking while focused (called by the element after each paint).
    pub(crate) fn ensure_blink(&mut self, focused: bool, cx: &mut Context<Self>) {
        self.display.focused = focused;
        if !focused {
            self.display.caret_on = true;
            self.display.blink = None;
            return;
        }
        if self.display.blink.is_some() {
            return;
        }
        self.display.blink = Some(cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor().timer(CARET_BLINK).await;
                let go_on = this
                    .update(cx, |v, cx| {
                        if !v.display.focused {
                            v.display.blink = None;
                            return false;
                        }
                        if v.display.last_activity.elapsed() < CARET_BLINK {
                            v.display.caret_on = true;
                        } else {
                            v.display.caret_on = !v.display.caret_on;
                            cx.notify();
                        }
                        true
                    })
                    .unwrap_or(false);
                if !go_on {
                    break;
                }
            }
        }));
    }

    // ----- display-row motion -----

    /// Up (negative) or Down by `rows` display rows: hidden rows are skipped and wrapped rows count one each, keeping
    /// the goal column within the display row.
    pub fn move_display_rows(&mut self, rows: i64, extend: bool, cx: &mut Context<Self>) {
        let layout = self.vertical_layout().clone();
        if !mapped(&layout, self.display.wrap) {
            self.editor.move_vertical(rows, extend);
            self.moved_by_display(cx);
            return;
        }
        let buffer = self.editor.buffer();
        let selections = self.editor.selections();
        let heads: Vec<usize> = selections.iter().map(|s| s.head).collect();
        let goals_in = match &self.display.goals {
            Some((h, g)) if *h == heads => Some(g.clone()),
            _ => None,
        };
        let mut goals = Vec::with_capacity(heads.len());
        let mut out = Vec::with_capacity(heads.len());
        for (i, sel) in selections.iter().enumerate() {
            let p = buffer.offset_to_point(sel.head);
            let mut row = p.row;
            let mut line = buffer.line(row);
            let mut segs = self.row_segments(row);
            let mut si = segment_of(&segs, p.column as usize);
            let goal = goals_in
                .as_ref()
                .and_then(|g| g.get(i).copied())
                .unwrap_or_else(|| {
                    visual_column(&line, p.column as usize) - visual_column(&line, segs[si].start)
                });
            let mut head = None;
            for _ in 0..rows.unsigned_abs() {
                if rows > 0 {
                    if si + 1 < segs.len() {
                        si += 1;
                    } else if let Some(r) = layout.next_visible(row + 1) {
                        row = r;
                        line = buffer.line(row);
                        segs = self.row_segments(row);
                        si = 0;
                    } else {
                        head = Some(buffer.len());
                        break;
                    }
                } else if si > 0 {
                    si -= 1;
                } else if let Some(r) = row.checked_sub(1).and_then(|r| layout.prev_visible(r)) {
                    row = r;
                    line = buffer.line(row);
                    segs = self.row_segments(row);
                    si = segs.len() - 1;
                } else {
                    head = Some(0);
                    break;
                }
            }
            let head = head.unwrap_or_else(|| {
                let seg = &segs[si];
                let base = visual_column(&line, seg.start);
                let mut col = byte_for_visual_column(&line, base + goal).max(seg.start);
                if si + 1 < segs.len() && col >= seg.end {
                    // Not past the wrapped row's end, which is the next row's start.
                    col = line[..seg.end]
                        .char_indices()
                        .next_back()
                        .map_or(seg.start, |(i, _)| i);
                }
                buffer.point_to_offset(text::Point::new(row, col as u32))
            });
            goals.push(goal);
            out.push(SelectionRange {
                tail: if extend { sel.tail } else { head },
                head,
            });
        }
        let primary = self.primary_index();
        self.editor.set_selections(out, primary);
        let heads = self.editor.selections().iter().map(|s| s.head).collect();
        self.moved_by_display(cx);
        self.display.goals = Some((heads, goals));
    }

    /// Page Up or Page Down by display rows.
    pub fn page_display_rows(&mut self, down: bool, extend: bool, cx: &mut Context<Self>) {
        let rows = self.editor.page_rows.max(1) as i64;
        self.move_display_rows(if down { rows } else { -rows }, extend, cx);
    }

    /// Home and End by display row with word wrap on (the wrapped row's start or end first); else the editor's own.
    pub fn display_home_end(&mut self, end: bool, extend: bool, cx: &mut Context<Self>) {
        if !self.display.wrap {
            if end {
                self.editor.move_end(extend);
            } else {
                self.editor.move_home(extend);
            }
            self.moved_by_display(cx);
            return;
        }
        let buffer = self.editor.buffer();
        let selections = self.editor.selections();
        let mut fallback = false;
        let mut out = Vec::new();
        for sel in &selections {
            let p = buffer.offset_to_point(sel.head);
            let segs = self.row_segments(p.row);
            let si = segment_of(&segs, p.column as usize);
            let line = buffer.line(p.row);
            let seg = &segs[si];
            let col = p.column as usize;
            let target = if end {
                if si + 1 < segs.len() {
                    let last = line[..seg.end]
                        .char_indices()
                        .next_back()
                        .map_or(seg.start, |(i, _)| i);
                    (col != last).then_some(last)
                } else {
                    None
                }
            } else if si > 0 && col != seg.start {
                Some(seg.start)
            } else {
                None
            };
            match target {
                Some(c) => {
                    let head = buffer.point_to_offset(text::Point::new(p.row, c as u32));
                    out.push(SelectionRange {
                        tail: if extend { sel.tail } else { head },
                        head,
                    });
                }
                None => {
                    fallback = true;
                    break;
                }
            }
        }
        if fallback {
            if end {
                self.editor.move_end(extend);
            } else {
                self.editor.move_home(extend);
            }
        } else {
            let primary = self.primary_index();
            self.editor.set_selections(out, primary);
        }
        self.moved_by_display(cx);
    }
}

// ----- painting -----

/// One wrapped segment of a row as laid out by the element (brief 0067's view of `view::RowLayout`).
pub(crate) struct LaidSegment<'a> {
    pub row: u32,
    pub segment: u32,
    pub last_segment: bool,
    pub top: Pixels,
    /// The whole buffer line, and the segment's byte range of it.
    pub text: &'a str,
    pub range: Range<usize>,
    pub line: &'a ShapedLine,
}

/// What the element paints for brief 0067, beside the text.
#[derive(Default)]
pub(crate) struct AidsLayout {
    quads: Vec<gpui::PaintQuad>,
    /// Text drawn over the text area (white space, wrap glyphs, adornment labels): origin and line.
    overlays: Vec<(Point<Pixels>, ShapedLine)>,
    /// Paths for the expanders' signs and lines drawn over the gutter, outside the text's mask.
    gutter_quads: Vec<gpui::PaintQuad>,
    /// The sticky lines: their panel, then each line's number and text.
    sticky: Vec<gpui::PaintQuad>,
    sticky_lines: Vec<(Point<Pixels>, ShapedLine)>,
    /// The adornment's tooltip: panel and lines.
    tooltip: Vec<gpui::PaintQuad>,
    tooltip_lines: Vec<(Point<Pixels>, ShapedLine)>,
    bars: Vec<gpui::PaintQuad>,
    bar_text: Vec<(Point<Pixels>, ShapedLine)>,
    hitboxes: Vec<(gpui::Hitbox, CursorStyle)>,
    // The layout written back to the view.
    pub expander_hits: Vec<(Bounds<Pixels>, u32)>,
    pub adornment_hits: Vec<(Bounds<Pixels>, u32)>,
    pub sticky_hits: Vec<(Bounds<Pixels>, u32)>,
    pub bars_layout: BarsLayout,
    pub zoom_box: Option<Bounds<Pixels>>,
    pub max_width: Pixels,
    line_height: Pixels,
}

/// The geometry the element computed for this frame.
pub(crate) struct FrameGeometry {
    pub bounds: Bounds<Pixels>,
    pub text_left: Pixels,
    /// The text area's right and bottom edges (before the scroll bars).
    pub text_right: Pixels,
    pub text_bottom: Pixels,
    pub char_width: Pixels,
    pub scroll: Point<Pixels>,
    pub font: gpui::Font,
}

fn hsla(c: Rgba) -> Hsla {
    c.into()
}

fn with_alpha(c: Rgba, a: f32) -> Rgba {
    Rgba { a, ..c }
}

fn run(len: usize, font: &gpui::Font, color: Hsla) -> TextRun {
    TextRun {
        len,
        font: font.clone(),
        color,
        background_color: None,
        underline: None,
        strikethrough: None,
    }
}

impl EditorView {
    /// Lay out brief 0067's layer for this frame.
    pub(crate) fn layout_aids(
        &self,
        g: &FrameGeometry,
        laid: &[LaidSegment<'_>],
        vertical: &VerticalLayout,
        window: &mut Window,
    ) -> AidsLayout {
        let style = self.style();
        let theme = &style.theme;
        let lh = style.line_height;
        let font_size = style.font_size;
        let text_system = window.text_system().clone();
        let mut a = AidsLayout {
            line_height: lh,
            ..AidsLayout::default()
        };
        let x_of = |s: &LaidSegment<'_>, col: usize| -> Pixels {
            let ds = to_display(s.text, s.range.start);
            let d = to_display(s.text, col.clamp(s.range.start, s.range.end));
            g.text_left - g.scroll.x + s.line.x_for_index(d - ds)
        };
        let muted = hsla(with_alpha(theme.text_muted, 0.75));
        for s in laid {
            a.max_width = a.max_width.max(s.line.width());
        }

        // Brace matching: a box behind each bracket.
        if let Some((open, close)) = self.brace_pair() {
            let snapshot = self.editor.buffer().snapshot();
            for r in [open, close] {
                let p = snapshot.offset_to_point(r.start);
                if let Some(s) = laid.iter().find(|s| {
                    s.row == p.row
                        && s.range.start <= p.column as usize
                        && ((p.column as usize) < s.range.end || s.last_segment)
                }) {
                    let x0 = x_of(s, p.column as usize);
                    let x1 = x_of(s, p.column as usize + 1);
                    a.quads.push(fill(
                        Bounds::from_corners(point(x0, s.top), point(x1, s.top + lh)),
                        hsla(with_alpha(theme.text_muted, 0.3)),
                    ));
                }
            }
        }

        // Indent guides: a dotted line at each tab stop inside the indentation (blank lines take the next line's).
        if self.display.indent_guides {
            let buffer = self.editor.buffer();
            let indent_of = |text: &str| -> Option<u32> {
                (!text.trim().is_empty())
                    .then(|| visual_column(text, text.len() - text.trim_start().len()))
            };
            let mut next_indent: Option<u32> = None;
            for s in laid.iter().rev() {
                if s.segment != 0 {
                    continue;
                }
                let indent = match indent_of(s.text) {
                    Some(i) => {
                        next_indent = Some(i);
                        i
                    }
                    None => next_indent.unwrap_or_else(|| {
                        let below = vertical.next_visible(s.row + 1);
                        below.and_then(|r| indent_of(&buffer.line(r))).unwrap_or(0)
                    }),
                };
                let rows = vertical.segments(s.row).max(1) as f32;
                // A guide at each indentation level the line is inside.
                for col in (0..indent).step_by(TAB_SIZE as usize) {
                    let x = g.text_left - g.scroll.x + g.char_width * col as f32;
                    if x < g.text_left {
                        continue;
                    }
                    let mut y = s.top;
                    let bottom = s.top + lh * rows;
                    while y < bottom {
                        a.quads.push(fill(
                            Bounds::new(point(x, y), size(px(1.), px(2.))),
                            hsla(with_alpha(theme.guide, 0.8)),
                        ));
                        y += px(4.);
                    }
                }
            }
        }

        // White space: `·` for spaces, `→` for tabs, over the text in the muted color.
        if self.display.white_space {
            for s in laid {
                let seg = &s.text[s.range.clone()];
                if !seg.contains([' ', '\t']) {
                    continue;
                }
                for (i, ch) in seg.char_indices() {
                    let glyph = match ch {
                        ' ' => "\u{b7}",
                        '\t' => "\u{2192}",
                        _ => continue,
                    };
                    let col = s.range.start + i;
                    let x = x_of(s, col);
                    let line = text_system.shape_line(
                        SharedString::from(glyph),
                        font_size,
                        &[run(glyph.len(), &g.font, muted)],
                        None,
                    );
                    a.overlays.push((point(x, s.top), line));
                }
            }
        }

        // Wrap glyphs at the end of each wrapped segment but the last.
        if self.display.wrap && self.display.wrap_glyphs {
            for s in laid.iter().filter(|s| !s.last_segment) {
                let glyph = "\u{21b5}";
                let line = text_system.shape_line(
                    SharedString::from(glyph),
                    font_size,
                    &[run(glyph.len(), &g.font, muted)],
                    None,
                );
                a.overlays.push((
                    point(x_of(s, s.range.end) + g.char_width * 0.5, s.top),
                    line,
                ));
            }
        }

        // Outlining: the margin's expanders and extent lines, and each collapsed fold's adornment.
        let folds = self.folds();
        if self.outlining_enabled() {
            let margin_left = g.text_left - OUTLINING_MARGIN;
            let line_color = hsla(with_alpha(theme.text_muted, 0.6));
            let first_top = laid.first().map_or(g.bounds.top(), |s| s.top);
            let last_bottom = laid.last().map_or(g.bounds.top(), |s| s.top + lh);
            for f in folds.iter() {
                let header = laid.iter().find(|s| s.row == f.start_row && s.segment == 0);
                // The extent line of an expanded fold, from under its box to its last row.
                if !f.collapsed {
                    let top = match header {
                        Some(s) => s.top + lh * 0.75,
                        None if f.start_row < laid.first().map_or(0, |s| s.row) => first_top,
                        None => continue,
                    };
                    let end_seg = laid.iter().rev().find(|s| s.row == f.end_row);
                    let bottom = match end_seg {
                        Some(s) => s.top + lh * 0.5,
                        None if f.end_row > laid.last().map_or(0, |s| s.row) => last_bottom,
                        None => continue,
                    };
                    if bottom > top {
                        let x = margin_left + OUTLINING_MARGIN / 2.;
                        a.gutter_quads.push(fill(
                            Bounds::from_corners(point(x, top), point(x + px(1.), bottom)),
                            line_color,
                        ));
                        if end_seg.is_some() {
                            a.gutter_quads.push(fill(
                                Bounds::from_corners(
                                    point(x, bottom),
                                    point(x + OUTLINING_MARGIN * 0.35, bottom + px(1.)),
                                ),
                                line_color,
                            ));
                        }
                    }
                }
                let Some(s) = header else {
                    continue;
                };
                let side = (lh * 0.55).min(px(11.)).max(px(7.));
                let b = Bounds::new(
                    point(
                        margin_left + (OUTLINING_MARGIN - side) / 2.,
                        s.top + (lh - side) / 2.,
                    ),
                    size(side, side),
                );
                a.gutter_quads.push(fill(b, hsla(theme.background)));
                a.gutter_quads
                    .push(gpui::outline(b, line_color, gpui::BorderStyle::Solid));
                let mid_y = b.top() + side / 2.;
                a.gutter_quads.push(fill(
                    Bounds::from_corners(
                        point(b.left() + px(2.), mid_y - px(0.5)),
                        point(b.right() - px(2.), mid_y + px(0.5)),
                    ),
                    hsla(theme.text),
                ));
                if f.collapsed {
                    let mid_x = b.left() + side / 2.;
                    a.gutter_quads.push(fill(
                        Bounds::from_corners(
                            point(mid_x - px(0.5), b.top() + px(2.)),
                            point(mid_x + px(0.5), b.bottom() - px(2.)),
                        ),
                        hsla(theme.text),
                    ));
                }
                a.expander_hits.push((b.dilate(px(2.)), f.start_row));
                a.hitboxes.push((
                    window.insert_hitbox(b.dilate(px(2.)), gpui::HitboxBehavior::Normal),
                    CursorStyle::PointingHand,
                ));
                if f.collapsed {
                    self.layout_adornment(f, s, g, &mut a, window);
                }
            }
        }

        // Sticky scroll: the pinned header lines over the top of the text area.
        let sticky = self.sticky_lines();
        if !sticky.is_empty() {
            let panel = Bounds::from_corners(
                point(g.bounds.left(), g.bounds.top()),
                point(g.text_right, g.bounds.top() + lh * sticky.len() as f32),
            );
            a.sticky.push(fill(panel, hsla(theme.background)));
            a.sticky.push(fill(
                Bounds::from_corners(
                    point(g.bounds.left(), panel.bottom()),
                    point(g.text_right, panel.bottom() + px(1.)),
                ),
                hsla(theme.border),
            ));
            let buffer = self.editor.buffer();
            for (i, &row) in sticky.iter().enumerate() {
                let top = g.bounds.top() + lh * i as f32;
                let text = buffer.line(row);
                let display = crate::display::expand_tabs(&text).into_owned();
                let runs = crate::view::sticky_runs(self, row, &text, &display, &g.font);
                let line =
                    text_system.shape_line(SharedString::from(display), font_size, &runs, None);
                a.sticky_lines
                    .push((point(g.text_left - g.scroll.x, top), line));
                let number = SharedString::from((row + 1).to_string());
                let n = text_system.shape_line(
                    number.clone(),
                    font_size,
                    &[run(number.len(), &g.font, hsla(style.line_number))],
                    None,
                );
                let x = g.text_left
                    - g.char_width * 1.5
                    - n.width()
                    - if self.outlining_enabled() {
                        OUTLINING_MARGIN
                    } else {
                        px(0.)
                    };
                a.sticky_lines.push((point(x, top), n));
                let hit = Bounds::from_corners(
                    point(g.bounds.left(), top),
                    point(g.text_right, top + lh),
                );
                a.sticky_hits.push((hit, row));
                a.hitboxes.push((
                    window.insert_hitbox(hit, gpui::HitboxBehavior::Normal),
                    CursorStyle::PointingHand,
                ));
            }
        }

        // The adornment's tooltip: the hidden text in a panel under the adornment.
        if let Some(row) = self.display.hovered_adornment
            && let Some(f) = folds.iter().find(|f| f.collapsed && f.start_row == row)
            && let Some((b, _)) = self.display.adornment_hits.iter().find(|(_, r)| *r == row)
        {
            let text = self.fold_tooltip(f);
            let lines: Vec<ShapedLine> = text
                .lines()
                .map(|l| {
                    let d = crate::display::expand_tabs(l).into_owned();
                    let len = d.len();
                    text_system.shape_line(
                        SharedString::from(d),
                        font_size,
                        &[run(len, &g.font, hsla(theme.text))],
                        None,
                    )
                })
                .collect();
            let width = lines.iter().map(|l| l.width()).fold(px(0.), Pixels::max) + px(12.);
            let origin = point(b.left(), b.bottom() + px(2.));
            let panel = Bounds::new(origin, size(width, lh * lines.len() as f32 + px(8.)));
            a.tooltip.push(fill(panel, hsla(theme.popup_background)));
            a.tooltip.push(gpui::outline(
                panel,
                hsla(theme.popup_border),
                gpui::BorderStyle::Solid,
            ));
            for (i, l) in lines.into_iter().enumerate() {
                a.tooltip_lines.push((
                    point(origin.x + px(6.), origin.y + px(4.) + lh * i as f32),
                    l,
                ));
            }
        }

        self.layout_bars(g, vertical, &mut a, window);
        a
    }

    fn layout_adornment(
        &self,
        f: &Fold,
        s: &LaidSegment<'_>,
        g: &FrameGeometry,
        a: &mut AidsLayout,
        window: &mut Window,
    ) {
        let style = self.style();
        let theme = &style.theme;
        let lh = style.line_height;
        // After the header's last wrapped segment.
        let x = g.text_left - g.scroll.x + s.line.width() + g.char_width;
        let label = f.adornment().to_owned();
        let small = (style.font_size * 0.9).round();
        let line = window.text_system().shape_line(
            SharedString::from(label.clone()),
            small,
            &[run(label.len(), &g.font, hsla(theme.text_muted))],
            None,
        );
        let b = Bounds::new(
            point(x, s.top + px(2.)),
            size(line.width() + px(8.), lh - px(4.)),
        );
        a.quads.push(gpui::outline(
            b,
            hsla(with_alpha(theme.text_muted, 0.8)),
            gpui::BorderStyle::Solid,
        ));
        a.overlays.push((
            point(b.left() + px(4.), s.top + (lh - small * 1.3) / 2.),
            line,
        ));
        a.adornment_hits.push((b, f.start_row));
        a.hitboxes.push((
            window.insert_hitbox(b, gpui::HitboxBehavior::Normal),
            CursorStyle::Arrow,
        ));
    }

    fn layout_bars(
        &self,
        g: &FrameGeometry,
        vertical: &VerticalLayout,
        a: &mut AidsLayout,
        window: &mut Window,
    ) {
        let style = self.style();
        let theme = &style.theme;
        let viewport = size(g.text_right - g.text_left, g.text_bottom - g.bounds.top());
        let track_color = hsla(theme.panel);
        let thumb_color = hsla(with_alpha(theme.text_muted, 0.55));
        // Vertical.
        let vbar = Bounds::from_corners(
            point(g.text_right, g.bounds.top()),
            point(g.bounds.right(), g.text_bottom),
        );
        let max_y = vertical.max_scroll();
        let track = f32::from(vbar.size.height);
        let t = thumb(
            track,
            f32::from(viewport.height),
            max_y,
            f32::from(g.scroll.y),
        );
        a.bars.push(fill(vbar, track_color));
        let vthumb = Bounds::new(
            point(vbar.left() + px(3.), vbar.top() + px(t.top)),
            size(vbar.size.width - px(6.), px(t.length)),
        );
        a.bars.push(fill(vthumb, thumb_color).corner_radii(px(3.)));
        if self.display.scroll_markers {
            let content = (max_y + f32::from(viewport.height)).max(vertical.total_height());
            for (row, kind) in self.scroll_markers() {
                if vertical.is_hidden(row) {
                    continue;
                }
                let y = marker_y(track, content, vertical.line_top(row));
                let (b0, b1) = kind.band();
                let w = f32::from(vbar.size.width);
                let height = if kind == crate::MarkerKind::Caret {
                    2.
                } else {
                    MARKER_HEIGHT
                };
                let b = Bounds::new(
                    point(vbar.left() + px(w * b0), vbar.top() + px(y)),
                    size(px(w * (b1 - b0)), px(height)),
                );
                a.bars.push(fill(b, hsla(kind.color())));
                if kind != crate::MarkerKind::Caret {
                    a.bars_layout.markers.push((b, row));
                }
            }
        }
        a.hitboxes.push((
            window.insert_hitbox(vbar, gpui::HitboxBehavior::Normal),
            CursorStyle::Arrow,
        ));
        // Horizontal: the zoom box at the left, then the bar when a line is wider than the view (wrap off).
        let hbar_top = g.text_bottom;
        let zoom_text = SharedString::from(format!("{} %", self.display.zoom));
        let small = (theme.typography.ui).min(px(12.));
        let ui_font = window.text_style().font();
        let zoom_line = window.text_system().shape_line(
            zoom_text.clone(),
            small,
            &[run(zoom_text.len(), &ui_font, hsla(theme.text_muted))],
            None,
        );
        let zoom_box = Bounds::new(
            point(g.bounds.left(), hbar_top),
            size(zoom_line.width() + px(12.), SCROLLBAR_SIZE),
        );
        a.bars.push(fill(
            Bounds::from_corners(
                point(g.bounds.left(), hbar_top),
                point(g.bounds.right(), g.bounds.bottom()),
            ),
            track_color,
        ));
        a.bar_text.push((
            point(
                zoom_box.left() + px(6.),
                hbar_top + (SCROLLBAR_SIZE - small * 1.25) / 2.,
            ),
            zoom_line,
        ));
        a.zoom_box = Some(zoom_box);
        a.hitboxes.push((
            window.insert_hitbox(zoom_box, gpui::HitboxBehavior::Normal),
            CursorStyle::PointingHand,
        ));
        let content_w = (self.display.max_width.max(a.max_width) + g.char_width * 4.).max(px(1.));
        let view_w = viewport.width;
        let max_x = f32::from(content_w - view_w)
            .max(f32::from(g.scroll.x))
            .max(0.);
        if !self.display.wrap && max_x > 0. {
            let hbar = Bounds::from_corners(
                point(zoom_box.right() + px(4.), hbar_top),
                point(g.text_right, g.bounds.bottom()),
            );
            let track = f32::from(hbar.size.width);
            let t = thumb(track, f32::from(view_w), max_x, f32::from(g.scroll.x));
            let hthumb = Bounds::new(
                point(hbar.left() + px(t.top), hbar.top() + px(3.)),
                size(px(t.length), hbar.size.height - px(6.)),
            );
            a.bars.push(fill(hthumb, thumb_color).corner_radii(px(3.)));
            a.bars_layout.horizontal = Some(hbar);
            a.bars_layout.horizontal_thumb = Some(hthumb);
            a.hitboxes.push((
                window.insert_hitbox(hbar, gpui::HitboxBehavior::Normal),
                CursorStyle::Arrow,
            ));
        }
        a.bars_layout.vertical = Some(vbar);
        a.bars_layout.vertical_thumb = Some(vthumb);
        a.bars_layout.max_scroll_y = max_y;
        a.bars_layout.max_scroll_x = max_x;
        a.bars_layout.viewport = viewport;
    }
}

impl AidsLayout {
    /// Paint the layer under the text's mask (`in_text`), or over everything else.
    pub(crate) fn paint_in_text(&mut self, window: &mut Window, cx: &mut App) {
        for q in self.quads.drain(..) {
            window.paint_quad(q);
        }
        for (o, l) in &self.overlays {
            l.paint(*o, self.line_height, TextAlign::Left, None, window, cx)
                .ok();
        }
    }

    pub(crate) fn paint_over(&mut self, window: &mut Window, cx: &mut App) {
        for q in self.gutter_quads.drain(..) {
            window.paint_quad(q);
        }
        for q in self.sticky.drain(..) {
            window.paint_quad(q);
        }
        for (o, l) in &self.sticky_lines {
            l.paint(*o, self.line_height, TextAlign::Left, None, window, cx)
                .ok();
        }
        for q in self.bars.drain(..) {
            window.paint_quad(q);
        }
        for (o, l) in &self.bar_text {
            l.paint(*o, SCROLLBAR_SIZE, TextAlign::Left, None, window, cx)
                .ok();
        }
        for q in self.tooltip.drain(..) {
            window.paint_quad(q);
        }
        for (o, l) in &self.tooltip_lines {
            l.paint(*o, self.line_height, TextAlign::Left, None, window, cx)
                .ok();
        }
        for (h, cursor) in &self.hitboxes {
            window.set_cursor_style(*cursor, h);
        }
    }
}

impl EditorView {
    /// Write the painted layer's geometry back for the mouse.
    pub(crate) fn store_aids(&mut self, a: &mut AidsLayout) {
        self.display.expander_hits = std::mem::take(&mut a.expander_hits);
        self.display.adornment_hits = std::mem::take(&mut a.adornment_hits);
        self.display.sticky_hits = std::mem::take(&mut a.sticky_hits);
        self.display.bars = std::mem::take(&mut a.bars_layout);
        self.display.zoom_box = a.zoom_box.take();
        self.display.max_width = self.display.max_width.max(a.max_width);
    }

    /// A press on brief 0067's layer: an expander toggles its fold, an adornment expands it, a sticky line goes to
    /// its header, the zoom box resets the zoom, a scroll bar scrolls. True when handled.
    pub(crate) fn aids_mouse_down(
        &mut self,
        event: &gpui::MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if event.button != gpui::MouseButton::Left {
            return false;
        }
        let p = event.position;
        if let Some((_, row)) = self
            .display
            .sticky_hits
            .iter()
            .find(|(b, _)| b.contains(&p))
            .copied()
        {
            self.sticky_clicked(row, cx);
            return true;
        }
        if let Some((_, row)) = self
            .display
            .expander_hits
            .iter()
            .find(|(b, _)| b.contains(&p))
            .copied()
        {
            self.toggle_outlining_expansion(Some(row), cx);
            return true;
        }
        if let Some((_, row)) = self
            .display
            .adornment_hits
            .iter()
            .find(|(b, _)| b.contains(&p))
            .copied()
        {
            // A double click (or a single one, as Visual Studio's adornment does) expands the fold.
            self.display.hovered_adornment = None;
            self.toggle_outlining_expansion(Some(row), cx);
            return true;
        }
        if self.display.zoom_box.is_some_and(|b| b.contains(&p)) {
            self.set_zoom(100, cx);
            return true;
        }
        self.scrollbar_mouse_down(event, cx)
    }

    /// The mouse moved: the adornment under it shows its tooltip. True while a thumb is dragged.
    pub(crate) fn aids_mouse_move(
        &mut self,
        event: &gpui::MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let hovered = self
            .display
            .adornment_hits
            .iter()
            .find(|(b, _)| b.contains(&event.position))
            .map(|(_, r)| *r);
        if hovered != self.display.hovered_adornment {
            self.display.hovered_adornment = hovered;
            cx.notify();
        }
        self.scrollbar_mouse_move(event, cx)
    }

    /// Ctrl+wheel zooms (Visual Studio): true when it did.
    pub(crate) fn aids_scroll(
        &mut self,
        event: &gpui::ScrollWheelEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        if !event.modifiers.control {
            return false;
        }
        let dy = f32::from(event.delta.pixel_delta(self.style().line_height).y);
        if dy == 0. {
            return true;
        }
        self.zoom_by(if dy > 0. { 1 } else { -1 }, cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mapped_only_with_folds_or_wrap() {
        let plain = VerticalLayout::new(10., 8., 10, vec![]);
        assert!(!mapped(&plain, false));
        assert!(mapped(&plain, true));
        let folded = VerticalLayout::new(10., 8., 10, vec![]).with_display(vec![2..5], vec![]);
        assert!(mapped(&folded, false));
    }
}
