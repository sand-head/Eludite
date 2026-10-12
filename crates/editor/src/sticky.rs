//! Sticky scroll (brief 0067; Visual Studio 17.5's and Rider's): the header lines of the scopes that contain the top
//! of the view (namespace, type, member, block) pinned at the top of the text area, outermost first, at most
//! `max_lines` of them. A click on one moves the caret to that header and scrolls it into view.
//!
//! The scopes are the syntax fold ranges (the same tree-sitter query as outlining, kinds that are scopes), so sticky
//! scroll works whether outlining is on or not and whatever the language server answers.

use gpui::Context;

use crate::display::VerticalLayout;
use crate::folding::FoldRange;
use crate::view::EditorView;

/// Visual Studio's default for Maximum sticky lines.
pub const DEFAULT_MAX_LINES: u32 = 5;

/// The header rows to pin when the view's top is at content `scroll`: each scope (sorted by header row, outermost
/// first) whose header is above the first row under the pinned lines and whose last row is at or below it.
pub fn sticky_rows(
    scopes: &[FoldRange],
    layout: &VerticalLayout,
    scroll: f32,
    max_lines: u32,
) -> Vec<u32> {
    if max_lines == 0 || scroll <= 0. {
        return Vec::new();
    }
    let lh = layout.line_height();
    let mut pinned: Vec<u32> = Vec::new();
    // The pinned lines cover the rows under them: look again under the lines found until it settles.
    for _ in 0..=max_lines {
        let top = layout.hit(scroll + lh * pinned.len() as f32).row;
        let next: Vec<u32> = scopes
            .iter()
            .filter(|s| s.kind.is_scope() && s.start_row < top && top <= s.end_row)
            .filter(|s| !layout.is_hidden(s.start_row))
            .map(|s| s.start_row)
            .take(max_lines as usize)
            .collect();
        if next == pinned {
            break;
        }
        let grew = next.len() > pinned.len();
        pinned = next;
        if !grew {
            break;
        }
    }
    pinned.dedup();
    pinned
}

impl EditorView {
    /// Turn sticky scroll on or off (`editor.stickyScroll`) and set its line limit (`editor.stickyScroll.maxLines`).
    pub fn set_sticky_scroll(&mut self, enabled: bool, max_lines: u32, cx: &mut Context<Self>) {
        self.display.sticky = enabled;
        self.display.sticky_max = max_lines;
        cx.notify();
    }

    /// The header rows pinned at the top for the current scroll position.
    pub fn sticky_lines(&self) -> Vec<u32> {
        if !self.display.sticky {
            return Vec::new();
        }
        let layout = self.vertical_layout();
        sticky_rows(
            self.syntax_fold_ranges(),
            &layout,
            f32::from(self.scroll_position().y),
            self.display.sticky_max,
        )
    }

    /// A click on pinned line `index`: the caret to its header row's first character, scrolled to the top.
    pub(crate) fn sticky_clicked(&mut self, row: u32, cx: &mut Context<Self>) {
        let buffer = self.editor.buffer();
        let line = buffer.line(row);
        let indent = line.len() - line.trim_start().len();
        let offset = buffer.point_to_offset(text::Point::new(row, indent as u32));
        self.editor.set_caret(offset);
        self.scroll_to_row(row, cx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::folding::FoldKind;

    #[test]
    fn pins_the_enclosing_scopes_outermost_first() {
        // namespace 0..=100 { class 2..=90 { method 10..=40, method 50..=80 } }
        let scopes = vec![
            FoldRange::new(0, 100, FoldKind::Namespace),
            FoldRange::new(2, 90, FoldKind::Type),
            FoldRange::new(4, 6, FoldKind::Comment),
            FoldRange::new(10, 40, FoldKind::Member),
            FoldRange::new(50, 80, FoldKind::Member),
        ];
        let layout = VerticalLayout::new(10., 8., 120, vec![]);
        // At the top nothing is pinned.
        assert!(sticky_rows(&scopes, &layout, 0., 5).is_empty());
        // Row 20 at the top: namespace, class and the first method; their three lines cover rows 20 to 22, still in it.
        assert_eq!(sticky_rows(&scopes, &layout, 200., 5), vec![0, 2, 10]);
        // Row 45 (between the methods): namespace and class.
        assert_eq!(sticky_rows(&scopes, &layout, 450., 5), vec![0, 2]);
        // Row 1: the namespace's header is above, the class's header is under the pinned line.
        assert_eq!(sticky_rows(&scopes, &layout, 10., 5), vec![0]);
        // The limit.
        assert_eq!(sticky_rows(&scopes, &layout, 200., 2), vec![0, 2]);
        // Comments are not scopes; past the namespace nothing is pinned.
        assert!(sticky_rows(&scopes, &layout, 1050., 5).is_empty());
    }
}
