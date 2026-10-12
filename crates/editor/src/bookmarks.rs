//! Bookmarks in the editor (brief 0070): Visual Studio's bookmark glyph in the indicator margin, under any
//! breakpoint on the same line. The owner (the shell's `bookmarks`) decides which lines carry one and whether it is
//! enabled; the view keeps them as anchors so they follow edits, and draws them.

use gpui::{Bounds, Context, Pixels, Rgba, Window, fill, point, px, rgb, size};

use crate::view::EditorView;

/// An enabled bookmark's color: Visual Studio's dark cyan.
pub(crate) const ENABLED: u32 = 0x3C8DBC;
/// A disabled one's: a gray outline.
pub(crate) const DISABLED: u32 = 0x8A8A8A;

impl EditorView {
    /// Show bookmark glyphs on these rows (0-based; `true` when enabled), replacing the previous ones. They stay on
    /// their lines as the text changes ([`EditorView::bookmark_rows`] reads them back).
    pub fn set_bookmark_rows(&mut self, rows: Vec<(u32, bool)>, cx: &mut Context<Self>) {
        let b = self.editor().buffer();
        let last = b.line_count().saturating_sub(1);
        let next = rows
            .into_iter()
            .map(|(row, enabled)| {
                let start = b.point_to_offset(text::Point::new(row.min(last), 0));
                (b.anchor_after(start), enabled)
            })
            .collect();
        self.bookmark_glyphs = next;
        cx.notify();
    }

    /// The rows (0-based) of the bookmark glyphs, where the text has moved them, in row order.
    pub fn bookmark_rows(&self) -> Vec<(u32, bool)> {
        let b = self.editor().buffer();
        let mut rows: Vec<(u32, bool)> = self
            .bookmark_glyphs
            .iter()
            .map(|(a, e)| (b.offset_to_point(b.offset_for_anchor(a)).row, *e))
            .collect();
        rows.sort();
        rows.dedup_by_key(|(r, _)| *r);
        rows
    }
}

/// The glyph's bounds in the margin cell of a row, and its color.
pub(crate) fn glyph(cell: Bounds<Pixels>, enabled: bool) -> (Bounds<Pixels>, Rgba, bool) {
    // A bookmark is a small flag-like rectangle, wider than tall.
    let w = cell.size.width;
    let h = (cell.size.height * 0.7).max(px(4.));
    let b = Bounds::new(
        point(cell.origin.x, cell.origin.y + (cell.size.height - h) / 2.),
        size(w, h),
    );
    (b, rgb(if enabled { ENABLED } else { DISABLED }), enabled)
}

/// Paint the glyphs.
pub(crate) fn paint(glyphs: &[(Bounds<Pixels>, Rgba, bool)], window: &mut Window) {
    for (b, color, enabled) in glyphs {
        let color: gpui::Hsla = (*color).into();
        if *enabled {
            window.paint_quad(fill(*b, color).corner_radii(px(2.)));
        } else {
            window.paint_quad(
                gpui::outline(*b, color, gpui::BorderStyle::Solid).corner_radii(px(2.)),
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_glyph_sits_inside_its_cell() {
        let cell = Bounds::new(point(px(2.), px(10.)), size(px(12.), px(12.)));
        let (b, color, enabled) = glyph(cell, true);
        assert!(enabled);
        assert_eq!(color, rgb(ENABLED));
        assert!(b.origin.y >= cell.origin.y && b.bottom() <= cell.bottom());
        assert_eq!(glyph(cell, false).1, rgb(DISABLED));
    }
}
