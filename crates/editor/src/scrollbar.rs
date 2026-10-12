//! The editor's scroll bars (brief 0067): a vertical bar at the right and a horizontal one at the bottom, with Visual
//! Studio's "bar mode" overview markers on the vertical bar.
//!
//! - **Geometry** ([`thumb`], [`scroll_for_thumb`], [`marker_y`]) is plain arithmetic over the display map's heights,
//!   so it is tested without a window.
//! - **Markers** are layers like decorations: [`EditorView::set_scroll_markers`] takes `(offset, kind)` pairs, anchored
//!   so they follow edits. The view adds its own: the caret line, find matches, breakpoints, and every wavy-underline
//!   decoration (errors and warnings, in their squiggle's color). Changes (added, modified, deleted) sit in the bar's
//!   left third, diagnostics in its right third, the rest across it, as Visual Studio draws them.
//! - **Mouse.** Dragging the thumb scrolls; a click on the track pages toward the click; a click on a marker scrolls
//!   its row to the middle of the view and puts the caret there.

use gpui::{Bounds, Context, MouseDownEvent, MouseMoveEvent, Pixels, Point, Rgba, px, rgb};
use text::Anchor;

use crate::view::EditorView;

/// The bars' thickness.
pub const SCROLLBAR_SIZE: Pixels = px(14.);
/// The smallest thumb.
pub const MIN_THUMB: f32 = 18.;
/// A marker's height on the bar, and how close a click must be to hit it.
pub const MARKER_HEIGHT: f32 = 3.;
const MARKER_SLOP: f32 = 3.;

/// What a marker on the vertical bar stands for.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MarkerKind {
    Error,
    Warning,
    Info,
    FindMatch,
    Added,
    Modified,
    Deleted,
    Breakpoint,
    Bookmark,
    Caret,
    /// A diagnostic or any other layer in its own color.
    Custom(Rgba),
}

impl MarkerKind {
    pub fn color(self) -> Rgba {
        match self {
            MarkerKind::Error => rgb(0xE51400),
            MarkerKind::Warning => rgb(0x9DA02E),
            MarkerKind::Info => rgb(0x1BA1E2),
            MarkerKind::FindMatch => rgb(0xC0802F),
            MarkerKind::Added => rgb(0x57A64A),
            MarkerKind::Modified => rgb(0x2E73B8),
            MarkerKind::Deleted => rgb(0xC5453C),
            MarkerKind::Breakpoint => rgb(0x9E2B2B),
            MarkerKind::Bookmark => rgb(0x8A8A8A),
            MarkerKind::Caret => rgb(0x3D7EFF),
            MarkerKind::Custom(c) => c,
        }
    }

    /// The marker's horizontal band on the bar: (start, end) as fractions of its width.
    pub fn band(self) -> (f32, f32) {
        match self {
            MarkerKind::Added | MarkerKind::Modified | MarkerKind::Deleted => (0., 0.33),
            MarkerKind::Error | MarkerKind::Warning | MarkerKind::Info | MarkerKind::Custom(_) => {
                (0.67, 1.)
            }
            MarkerKind::Breakpoint | MarkerKind::Bookmark => (0.33, 0.67),
            MarkerKind::FindMatch | MarkerKind::Caret => (0., 1.),
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            MarkerKind::Error => "error",
            MarkerKind::Warning => "warning",
            MarkerKind::Info => "info",
            MarkerKind::FindMatch => "find",
            MarkerKind::Added => "added",
            MarkerKind::Modified => "modified",
            MarkerKind::Deleted => "deleted",
            MarkerKind::Breakpoint => "breakpoint",
            MarkerKind::Bookmark => "bookmark",
            MarkerKind::Caret => "caret",
            MarkerKind::Custom(_) => "custom",
        }
    }
}

/// A marker: the start of its row (anchored), and what it is.
#[derive(Debug, Clone)]
pub struct ScrollMarker {
    pub at: Anchor,
    pub kind: MarkerKind,
}

/// The thumb on a track: its top (from the track's start) and length, in pixels.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Thumb {
    pub top: f32,
    pub length: f32,
}

/// The thumb for a `track`-long bar over a view of `viewport` pixels scrolled to `scroll` of `max_scroll`.
pub fn thumb(track: f32, viewport: f32, max_scroll: f32, scroll: f32) -> Thumb {
    let content = (max_scroll + viewport).max(1.);
    let length = (track * viewport / content).clamp(MIN_THUMB.min(track), track);
    let top = if max_scroll > 0. {
        (track - length) * (scroll / max_scroll).clamp(0., 1.)
    } else {
        0.
    };
    Thumb { top, length }
}

/// The scroll offset that puts the thumb's top at `top`.
pub fn scroll_for_thumb(track: f32, length: f32, max_scroll: f32, top: f32) -> f32 {
    let free = track - length;
    if free <= 0. {
        return 0.;
    }
    (top / free).clamp(0., 1.) * max_scroll
}

/// Where content y `y` sits on a `track`-long bar over `content` pixels of document.
pub fn marker_y(track: f32, content: f32, y: f32) -> f32 {
    if content <= 0. {
        return 0.;
    }
    (track * y / content).clamp(0., (track - MARKER_HEIGHT).max(0.))
}

/// The bars as last painted (window coordinates).
#[derive(Debug, Clone, Default)]
pub struct BarsLayout {
    pub vertical: Option<Bounds<Pixels>>,
    pub vertical_thumb: Option<Bounds<Pixels>>,
    pub horizontal: Option<Bounds<Pixels>>,
    pub horizontal_thumb: Option<Bounds<Pixels>>,
    /// Each marker drawn: its bounds and its buffer row.
    pub markers: Vec<(Bounds<Pixels>, u32)>,
    /// The vertical scroll range and viewport the thumb was drawn for.
    pub max_scroll_y: f32,
    pub max_scroll_x: f32,
    pub viewport: gpui::Size<Pixels>,
}

/// A drag of a thumb in progress.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct ThumbDrag {
    pub vertical: bool,
    /// Where the mouse went down, and the thumb's top then.
    pub start: f32,
    pub thumb_top: f32,
}

impl EditorView {
    /// Replace one layer of scroll bar markers (for example `"git"` or `"bookmarks"`) with `(offset, kind)` pairs.
    pub fn set_scroll_markers(
        &mut self,
        layer: &'static str,
        markers: Vec<(usize, MarkerKind)>,
        cx: &mut Context<Self>,
    ) {
        let buffer = self.editor.buffer();
        let markers = markers
            .into_iter()
            .map(|(offset, kind)| {
                let row = buffer.offset_to_point(offset.min(buffer.len())).row;
                ScrollMarker {
                    at: buffer.anchor_before(buffer.point_to_offset(text::Point::new(row, 0))),
                    kind,
                }
            })
            .collect();
        self.display.markers.insert(layer, markers);
        cx.notify();
    }

    pub fn clear_scroll_markers(&mut self, layer: &'static str, cx: &mut Context<Self>) {
        self.display.markers.remove(layer);
        cx.notify();
    }

    /// Every marker the vertical bar shows: the layers set by the owner, then the view's own (caret, find matches,
    /// breakpoints, wavy underlines). Rows, with kinds; one per row and kind.
    pub fn scroll_markers(&self) -> Vec<(u32, MarkerKind)> {
        let snapshot = self.editor.buffer().snapshot();
        let row = |a: &Anchor| snapshot.offset_to_point(snapshot.offset_for_anchor(a)).row;
        let mut out: Vec<(u32, MarkerKind)> = self
            .display
            .markers
            .values()
            .flatten()
            .map(|m| (row(&m.at), m.kind))
            .collect();
        for (layer, decorations) in self.decoration_layers() {
            let _ = layer;
            for d in decorations {
                if let crate::DecorationStyle::Underline { color, wavy: true } = d.style {
                    out.push((row(&d.range.start), MarkerKind::Custom(color)));
                }
            }
        }
        for (anchor, _) in &self.breakpoint_glyphs {
            out.push((row(anchor), MarkerKind::Breakpoint));
        }
        if self.is_find_bar_open() && !self.editor.find_query().text.is_empty() {
            for m in self
                .editor
                .find_matches(0..snapshot.len())
                .iter()
                .take(10_000)
            {
                out.push((snapshot.offset_to_point(m.start).row, MarkerKind::FindMatch));
            }
        }
        out.push((self.editor.primary_head().row, MarkerKind::Caret));
        out.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.as_str().cmp(b.1.as_str())));
        out.dedup_by(|b, a| a.0 == b.0 && a.1 == b.1);
        out
    }

    /// The scroll bars as last painted.
    pub fn scroll_bars(&self) -> &BarsLayout {
        &self.display.bars
    }

    /// A press on a scroll bar: a marker jumps, the thumb starts a drag, the track pages. True when it was on a bar.
    pub(crate) fn scrollbar_mouse_down(
        &mut self,
        event: &MouseDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let p = event.position;
        let bars = self.display.bars.clone();
        if let Some(track) = bars.vertical
            && track.contains(&p)
        {
            let y = f32::from(p.y);
            // The thumb first (a drag), then a marker (a jump), else the track (a page).
            if let Some(t) = bars.vertical_thumb
                && t.contains(&p)
            {
                self.display.drag = Some(ThumbDrag {
                    vertical: true,
                    start: y,
                    thumb_top: f32::from(t.top() - track.top()),
                });
                return true;
            }
            if let Some((_, row)) = bars
                .markers
                .iter()
                .filter(|(b, _)| {
                    y >= f32::from(b.top()) - MARKER_SLOP
                        && y <= f32::from(b.bottom()) + MARKER_SLOP
                })
                .min_by(|a, b| {
                    let d = |r: &Bounds<Pixels>| (f32::from(r.center().y) - y).abs();
                    d(&a.0).total_cmp(&d(&b.0))
                })
                .copied()
            {
                self.jump_to_marker_row(row, cx);
                return true;
            }
            if let Some(t) = bars.vertical_thumb {
                // Page toward the click.
                let page = f32::from(bars.viewport.height - self.style().line_height).max(1.);
                let dir = if p.y < t.top() { -1. } else { 1. };
                let y = self.scroll_position().y + px(page * dir);
                self.set_scroll_y(y, cx);
            }
            return true;
        }
        if let Some(track) = bars.horizontal
            && track.contains(&p)
        {
            if let Some(t) = bars.horizontal_thumb {
                let x = f32::from(p.x);
                if t.contains(&p) {
                    self.display.drag = Some(ThumbDrag {
                        vertical: false,
                        start: x,
                        thumb_top: f32::from(t.left() - track.left()),
                    });
                } else {
                    let page = f32::from(bars.viewport.width) * 0.8;
                    let dir = if p.x < t.left() { -1. } else { 1. };
                    let to = (f32::from(self.scroll_position().x) + page * dir)
                        .clamp(0., bars.max_scroll_x);
                    self.set_scroll_x(px(to), cx);
                }
            }
            return true;
        }
        false
    }

    /// A drag of a thumb. True while one is in progress.
    pub(crate) fn scrollbar_mouse_move(
        &mut self,
        event: &MouseMoveEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(drag) = self.display.drag else {
            return false;
        };
        if event.pressed_button != Some(gpui::MouseButton::Left) {
            self.display.drag = None;
            return false;
        }
        let bars = self.display.bars.clone();
        if drag.vertical {
            let (Some(track), Some(t)) = (bars.vertical, bars.vertical_thumb) else {
                return true;
            };
            let top = drag.thumb_top + f32::from(event.position.y) - drag.start;
            let y = scroll_for_thumb(
                f32::from(track.size.height),
                f32::from(t.size.height),
                bars.max_scroll_y,
                top,
            );
            self.set_scroll_y(px(y), cx);
        } else {
            let (Some(track), Some(t)) = (bars.horizontal, bars.horizontal_thumb) else {
                return true;
            };
            let left = drag.thumb_top + f32::from(event.position.x) - drag.start;
            let x = scroll_for_thumb(
                f32::from(track.size.width),
                f32::from(t.size.width),
                bars.max_scroll_x,
                left,
            );
            self.set_scroll_x(px(x), cx);
        }
        true
    }

    pub(crate) fn scrollbar_mouse_up(&mut self) {
        self.display.drag = None;
    }

    /// Scroll `row` to the middle of the view and put the caret at its start (a marker click).
    pub fn jump_to_marker_row(&mut self, row: u32, cx: &mut Context<Self>) {
        let buffer = self.editor.buffer();
        let row = row.min(buffer.line_count() - 1);
        let offset = buffer.point_to_offset(text::Point::new(row, 0));
        self.editor.set_caret(offset);
        self.reveal_hidden_carets(cx);
        let height = self.display.bars.viewport.height;
        let top = self.row_top(row) - (height - self.style().line_height) / 2.;
        self.set_scroll_y(top, cx);
    }

    /// Whether `position` is over a scroll bar (the cursor there is an arrow, not an I-beam).
    pub fn over_scroll_bar(&self, position: Point<Pixels>) -> bool {
        let b = &self.display.bars;
        b.vertical.is_some_and(|r| r.contains(&position))
            || b.horizontal.is_some_and(|r| r.contains(&position))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thumb_geometry_follows_the_viewport_and_scroll() {
        // 1,000 px of document in a 100 px view: the thumb is a tenth of the track.
        let t = thumb(1000., 100., 900., 0.);
        assert_eq!(
            t,
            Thumb {
                top: 0.,
                length: 100.
            }
        );
        // Scrolled to the end, the thumb ends at the track's end.
        let t = thumb(100., 100., 900., 900.);
        assert_eq!(t.top + t.length, 100.);
        // Half way.
        let t = thumb(200., 100., 900., 450.);
        assert_eq!(t.length, 20.);
        assert_eq!(t.top, 90.);
        // A very long document: the thumb keeps its minimum.
        let t = thumb(100., 100., 1_000_000., 500_000.);
        assert_eq!(t.length, MIN_THUMB);
        assert_eq!(t.top, (100. - MIN_THUMB) / 2.);
        // Everything fits: the thumb fills the track.
        assert_eq!(
            thumb(100., 300., 0., 0.),
            Thumb {
                top: 0.,
                length: 100.
            }
        );
        // Dragging the thumb back gives the scroll offset.
        assert_eq!(scroll_for_thumb(200., 20., 900., 90.), 450.);
        assert_eq!(scroll_for_thumb(200., 20., 900., -5.), 0.);
        assert_eq!(scroll_for_thumb(200., 20., 900., 999.), 900.);
        // Markers map the document onto the track.
        assert_eq!(marker_y(100., 1000., 500.), 50.);
        assert_eq!(marker_y(100., 1000., 1000.), 100. - MARKER_HEIGHT);
    }
}
