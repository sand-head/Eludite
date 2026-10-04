//! The display layer: tab expansion and column conversion for one line of text, and the vertical layout of the
//! rows with CodeLens rows between them ([`VerticalLayout`], brief 0052).
//!
//! Tabs are shown as spaces up to the next multiple of [`TAB_SIZE`]. The
//! shaped (display) string replaces each tab with that many spaces, so a
//! byte offset into the display string differs from one into the buffer
//! line only by the extra width of earlier tabs.
//!
//! A lens row is a display-only line above a buffer row: it takes `lens_height` of vertical space and holds no
//! buffer text, so the caret, selections, line numbers and the gutter refer to buffer rows only and are never moved
//! by it. Each buffer row's *slot* is its lens row (when it has one) followed by its text line.

use std::borrow::Cow;

/// Columns per tab stop (Visual Studio's default for C#).
pub const TAB_SIZE: u32 = 4;

/// The line with tabs expanded to spaces.
pub fn expand_tabs(line: &str) -> Cow<'_, str> {
    if !line.contains('\t') {
        return Cow::Borrowed(line);
    }
    let mut out = String::with_capacity(line.len() + 8);
    let mut col = 0u32;
    for ch in line.chars() {
        if ch == '\t' {
            let n = TAB_SIZE - col % TAB_SIZE;
            out.extend(std::iter::repeat_n(' ', n as usize));
            col += n;
        } else {
            out.push(ch);
            col += 1;
        }
    }
    Cow::Owned(out)
}

/// Visual column (characters, tabs expanded) of byte offset `byte_col`.
pub fn visual_column(line: &str, byte_col: usize) -> u32 {
    let mut col = 0u32;
    for (ix, ch) in line.char_indices() {
        if ix >= byte_col {
            break;
        }
        col += if ch == '\t' {
            TAB_SIZE - col % TAB_SIZE
        } else {
            1
        };
    }
    col
}

/// Byte offset of the character at visual column `target`, or the line
/// length if the line is shorter. A column inside a tab maps to the tab.
pub fn byte_for_visual_column(line: &str, target: u32) -> usize {
    let mut col = 0u32;
    for (ix, ch) in line.char_indices() {
        let w = if ch == '\t' {
            TAB_SIZE - col % TAB_SIZE
        } else {
            1
        };
        if col + w > target {
            return if target - col > w / 2 && ch == '\t' {
                ix + 1
            } else {
                ix
            };
        }
        col += w;
    }
    line.len()
}

/// Byte offset in the display string for byte offset `byte_col` in `line`.
pub fn to_display(line: &str, byte_col: usize) -> usize {
    if !line.contains('\t') {
        return byte_col.min(line.len());
    }
    let mut col = 0u32;
    let mut display = 0usize;
    for (ix, ch) in line.char_indices() {
        if ix >= byte_col {
            return display;
        }
        if ch == '\t' {
            let n = TAB_SIZE - col % TAB_SIZE;
            display += n as usize;
            col += n;
        } else {
            display += ch.len_utf8();
            col += 1;
        }
    }
    display
}

/// Byte offset in `line` for byte offset `display_col` in its display string.
pub fn from_display(line: &str, display_col: usize) -> usize {
    if !line.contains('\t') {
        return floor_char_boundary(line, display_col.min(line.len()));
    }
    let mut col = 0u32;
    let mut display = 0usize;
    for (ix, ch) in line.char_indices() {
        let w = if ch == '\t' {
            (TAB_SIZE - col % TAB_SIZE) as usize
        } else {
            ch.len_utf8()
        };
        if display + w > display_col {
            return if ch == '\t' && display_col - display > w / 2 {
                ix + 1
            } else {
                ix
            };
        }
        display += w;
        col += if ch == '\t' { w as u32 } else { 1 };
    }
    line.len()
}

/// Where every buffer row and lens row sits vertically (brief 0052), in content pixels from the top of the document:
/// each text line is `line_height` tall, and a buffer row with a lens row above it is preceded by `lens_height` more.
/// With no lens rows this is the plain grid `row * line_height`. Lookups are binary searches over the sorted lens
/// rows, so a frame with 200 lens rows costs the same as one with none.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerticalLayout {
    line_height: f32,
    lens_height: f32,
    line_count: u32,
    /// Buffer rows with a lens row above them: sorted, unique, all `< line_count`.
    lens_rows: Vec<u32>,
}

/// What a content y falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowHit {
    pub row: u32,
    /// The lens row above `row`, not its text.
    pub in_lens: bool,
}

impl VerticalLayout {
    pub fn new(
        line_height: f32,
        lens_height: f32,
        line_count: u32,
        mut lens_rows: Vec<u32>,
    ) -> Self {
        lens_rows.retain(|&r| r < line_count);
        lens_rows.sort_unstable();
        lens_rows.dedup();
        Self {
            line_height,
            lens_height,
            line_count: line_count.max(1),
            lens_rows,
        }
    }

    pub fn line_height(&self) -> f32 {
        self.line_height
    }

    pub fn lens_height(&self) -> f32 {
        self.lens_height
    }

    pub fn line_count(&self) -> u32 {
        self.line_count
    }

    /// The buffer rows with a lens row above them, in order.
    pub fn lens_rows(&self) -> &[u32] {
        &self.lens_rows
    }

    pub fn has_lens(&self, row: u32) -> bool {
        self.lens_rows.binary_search(&row).is_ok()
    }

    /// Lens rows above buffer rows before `row`.
    fn lenses_before(&self, row: u32) -> usize {
        self.lens_rows.partition_point(|&r| r < row)
    }

    /// The top of `row`'s slot: its lens row when it has one, else its text line.
    pub fn slot_top(&self, row: u32) -> f32 {
        self.line_height * row as f32 + self.lens_height * self.lenses_before(row) as f32
    }

    /// The top of `row`'s text line.
    pub fn line_top(&self, row: u32) -> f32 {
        self.line_height * row as f32 + self.lens_height * self.lenses_before(row + 1) as f32
    }

    /// The top of the lens row above `row`, if it has one.
    pub fn lens_top(&self, row: u32) -> Option<f32> {
        self.has_lens(row).then(|| self.slot_top(row))
    }

    /// The height of the whole document.
    pub fn total_height(&self) -> f32 {
        self.line_height * self.line_count as f32 + self.lens_height * self.lens_rows.len() as f32
    }

    /// The largest scroll offset: the last row's slot at the top of the viewport.
    pub fn max_scroll(&self) -> f32 {
        self.slot_top(self.line_count - 1)
    }

    /// The row (and whether its lens row) at content `y`; above the top is row 0, below the end the last row.
    pub fn hit(&self, y: f32) -> RowHit {
        // The last row whose slot starts at or above y.
        let (mut lo, mut hi) = (0u32, self.line_count);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.slot_top(mid) <= y {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        RowHit {
            row: lo,
            in_lens: self.has_lens(lo) && y >= self.slot_top(lo) && y < self.line_top(lo),
        }
    }

    /// The rows whose slots intersect the content range `top..top + height`.
    pub fn visible_rows(&self, top: f32, height: f32) -> std::ops::Range<u32> {
        let first = self.hit(top).row;
        let last = self.hit(top + height).row;
        first..(last + 1).min(self.line_count)
    }
}

pub fn floor_char_boundary(s: &str, mut ix: usize) -> usize {
    ix = ix.min(s.len());
    while !s.is_char_boundary(ix) {
        ix -= 1;
    }
    ix
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_expand_to_stops() {
        assert_eq!(expand_tabs("\tx"), "    x");
        assert_eq!(expand_tabs("ab\tc"), "ab  c");
        assert_eq!(visual_column("ab\tc", 3), 4);
        assert_eq!(to_display("ab\tc", 3), 4);
        assert_eq!(from_display("ab\tc", 4), 3);
        assert_eq!(from_display("ab\tc", 2), 2);
        assert_eq!(byte_for_visual_column("ab\tc", 4), 3);
        assert_eq!(byte_for_visual_column("ab", 9), 2);
    }

    #[test]
    fn lens_rows_take_height_and_never_move_buffer_rows_relative_to_each_other() {
        // No lenses: the plain grid.
        let plain = VerticalLayout::new(19., 15., 10, vec![]);
        assert_eq!(plain.line_top(3), 57.);
        assert_eq!(plain.slot_top(3), 57.);
        assert_eq!(plain.total_height(), 190.);
        assert_eq!(
            plain.hit(56.9),
            RowHit {
                row: 2,
                in_lens: false
            }
        );
        // Lenses above rows 2 and 5 (and an out-of-range row, a duplicate, unsorted).
        let l = VerticalLayout::new(19., 15., 10, vec![5, 2, 2, 42]);
        assert_eq!(l.lens_rows(), [2, 5]);
        assert_eq!(l.line_top(1), 19.);
        assert_eq!(l.lens_top(2), Some(38.));
        assert_eq!(l.line_top(2), 53.);
        assert_eq!(l.slot_top(3), 72.);
        assert_eq!(l.lens_top(5), Some(110.));
        assert_eq!(l.line_top(5), 125.);
        assert_eq!(l.lens_top(4), None);
        assert_eq!(l.total_height(), 220.);
        // Every text line is still line_height tall: lens rows only push later rows down.
        for r in 0..9 {
            let gap = l.line_top(r + 1) - l.line_top(r);
            assert_eq!(gap, if l.has_lens(r + 1) { 34. } else { 19. });
        }
        // Hit testing: text, lens row, and the edges.
        assert_eq!(
            l.hit(-5.),
            RowHit {
                row: 0,
                in_lens: false
            }
        );
        assert_eq!(
            l.hit(37.9),
            RowHit {
                row: 1,
                in_lens: false
            }
        );
        assert_eq!(
            l.hit(38.),
            RowHit {
                row: 2,
                in_lens: true
            }
        );
        assert_eq!(
            l.hit(52.9),
            RowHit {
                row: 2,
                in_lens: true
            }
        );
        assert_eq!(
            l.hit(53.),
            RowHit {
                row: 2,
                in_lens: false
            }
        );
        assert_eq!(
            l.hit(124.),
            RowHit {
                row: 5,
                in_lens: true
            }
        );
        assert_eq!(
            l.hit(10_000.),
            RowHit {
                row: 9,
                in_lens: false
            }
        );
        assert_eq!(l.max_scroll(), l.slot_top(9));
        assert_eq!(l.visible_rows(40., 30.), 2..3);
        assert_eq!(l.visible_rows(40., 40.), 2..4);
        // 200 lens rows: the lookups stay consistent with a linear walk.
        let rows: Vec<u32> = (0..1000).step_by(5).collect();
        let big = VerticalLayout::new(19., 15., 1000, rows);
        let mut y = 0.;
        for r in 0..1000 {
            if big.has_lens(r) {
                assert_eq!(
                    big.hit(y + 1.),
                    RowHit {
                        row: r,
                        in_lens: true
                    }
                );
                y += 15.;
            }
            assert_eq!(big.line_top(r), y);
            assert_eq!(
                big.hit(y + 1.),
                RowHit {
                    row: r,
                    in_lens: false
                }
            );
            y += 19.;
        }
    }

    #[test]
    fn multibyte_columns() {
        let line = "añb";
        assert_eq!(visual_column(line, 3), 2);
        assert_eq!(byte_for_visual_column(line, 2), 3);
        assert_eq!(from_display(line, 2), 1);
    }
}
