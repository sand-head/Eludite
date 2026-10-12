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
///
/// Brief 0067 makes it the editor's vertical display map ([`VerticalLayout::with_display`]): a buffer row can be
/// hidden (inside a collapsed outlining region: no height, no lens row) or wrapped into several display rows (each
/// `line_height` tall). With either, the slot tops are prefix sums rebuilt in one pass over the rows, and every lookup
/// is a binary search over them.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct VerticalLayout {
    line_height: f32,
    lens_height: f32,
    line_count: u32,
    /// Buffer rows with a lens row above them: sorted, unique, all `< line_count`.
    lens_rows: Vec<u32>,
    // ----- brief 0067: the display map -----
    /// Hidden buffer rows: sorted, disjoint, non-adjacent, within `0..line_count`.
    hidden: Vec<std::ops::Range<u32>>,
    /// Display rows per buffer row when any row wraps (`len == line_count`, each at least 1); empty when none does.
    wraps: std::sync::Arc<[u32]>,
    /// `tops[row]`: the top of `row`'s slot, `tops[line_count]` the total height; empty on the plain grid.
    tops: std::sync::Arc<[f32]>,
}

/// What a content y falls on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowHit {
    pub row: u32,
    /// The lens row above `row`, not its text.
    pub in_lens: bool,
    /// The wrapped display row of `row` (0 for its first, and whenever it does not wrap; brief 0067).
    pub segment: u32,
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
            ..Self::default()
        }
    }

    /// The same layout with `hidden` buffer rows (any order, overlapping or not) and `wraps[row]` display rows per
    /// buffer row (empty: none wraps; missing or zero entries count as 1). Brief 0067.
    pub fn with_display(mut self, hidden: Vec<std::ops::Range<u32>>, wraps: Vec<u32>) -> Self {
        let n = self.line_count;
        let mut hidden: Vec<std::ops::Range<u32>> = hidden
            .into_iter()
            .map(|r| r.start.min(n)..r.end.min(n))
            .filter(|r| r.start < r.end)
            .collect();
        hidden.sort_unstable_by_key(|r| r.start);
        let mut merged: Vec<std::ops::Range<u32>> = Vec::with_capacity(hidden.len());
        for r in hidden {
            match merged.last_mut() {
                Some(last) if r.start <= last.end => last.end = last.end.max(r.end),
                _ => merged.push(r),
            }
        }
        let wraps: Vec<u32> = if !wraps.iter().any(|&w| w > 1) {
            Vec::new()
        } else if wraps.len() == n as usize && !wraps.contains(&0) {
            wraps
        } else {
            (0..n as usize)
                .map(|i| wraps.get(i).copied().unwrap_or(1).max(1))
                .collect()
        };
        self.hidden = merged;
        self.wraps = wraps.into();
        self.tops = if self.hidden.is_empty() && self.wraps.is_empty() {
            std::sync::Arc::from(Vec::new())
        } else {
            self.build_tops().into()
        };
        self
    }

    fn build_tops(&self) -> Vec<f32> {
        let n = self.line_count as usize;
        let mut tops = Vec::with_capacity(n + 1);
        let (mut y, mut lens, mut hide) = (0f32, 0usize, 0usize);
        for row in 0..n as u32 {
            tops.push(y);
            while hide < self.hidden.len() && self.hidden[hide].end <= row {
                hide += 1;
            }
            let hidden = self.hidden.get(hide).is_some_and(|r| r.contains(&row));
            while lens < self.lens_rows.len() && self.lens_rows[lens] < row {
                lens += 1;
            }
            if hidden {
                continue;
            }
            if self.lens_rows.get(lens) == Some(&row) {
                y += self.lens_height;
            }
            y += self.line_height * self.wraps.get(row as usize).copied().unwrap_or(1) as f32;
        }
        tops.push(y);
        tops
    }

    fn mapped(&self) -> bool {
        !self.tops.is_empty()
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

    /// Whether `row` has a lens row above it that takes space (a hidden row's lens row is hidden with it).
    pub fn has_lens(&self, row: u32) -> bool {
        self.lens_rows.binary_search(&row).is_ok() && !self.is_hidden(row)
    }

    /// The hidden row ranges, sorted and disjoint.
    pub fn hidden(&self) -> &[std::ops::Range<u32>] {
        &self.hidden
    }

    /// Whether `row` is inside a collapsed region.
    pub fn is_hidden(&self, row: u32) -> bool {
        let i = self.hidden.partition_point(|r| r.end <= row);
        self.hidden.get(i).is_some_and(|r| r.contains(&row))
    }

    /// Display rows `row` takes: 0 when hidden, else its wrapped segments (1 when it does not wrap).
    pub fn segments(&self, row: u32) -> u32 {
        if self.is_hidden(row) {
            0
        } else {
            self.wraps.get(row as usize).copied().unwrap_or(1)
        }
    }

    /// The height of `row`'s text (all its wrapped segments; 0 when hidden).
    pub fn row_height(&self, row: u32) -> f32 {
        self.line_height * self.segments(row) as f32
    }

    /// The first row at or after `row` that is not hidden, if any.
    pub fn next_visible(&self, row: u32) -> Option<u32> {
        let i = self.hidden.partition_point(|r| r.end <= row);
        let row = match self.hidden.get(i) {
            Some(r) if r.contains(&row) => r.end,
            _ => row,
        };
        (row < self.line_count).then_some(row)
    }

    /// The last row at or before `row` that is not hidden, if any.
    pub fn prev_visible(&self, row: u32) -> Option<u32> {
        let i = self.hidden.partition_point(|r| r.end <= row);
        match self.hidden.get(i) {
            Some(r) if r.contains(&row) => r.start.checked_sub(1),
            _ => Some(row.min(self.line_count - 1)),
        }
    }

    /// Display rows in the whole document (wrapped segments counted, hidden rows not, lens rows not).
    pub fn display_row_count(&self) -> u32 {
        let hidden: u32 = self.hidden.iter().map(|r| r.end - r.start).sum();
        if self.wraps.is_empty() {
            return self.line_count - hidden;
        }
        (0..self.line_count).map(|r| self.segments(r)).sum()
    }

    /// Lens rows above buffer rows before `row`.
    fn lenses_before(&self, row: u32) -> usize {
        self.lens_rows.partition_point(|&r| r < row)
    }

    /// The top of `row`'s slot: its lens row when it has one, else its text line. A hidden row's slot is empty and
    /// sits where the next shown row starts.
    pub fn slot_top(&self, row: u32) -> f32 {
        if self.mapped() {
            return self.tops[(row as usize).min(self.line_count as usize)];
        }
        self.line_height * row as f32 + self.lens_height * self.lenses_before(row) as f32
    }

    /// The top of `row`'s text line (its first wrapped segment).
    pub fn line_top(&self, row: u32) -> f32 {
        if self.mapped() {
            let lens = if self.has_lens(row) {
                self.lens_height
            } else {
                0.
            };
            return self.slot_top(row) + lens;
        }
        self.line_height * row as f32 + self.lens_height * self.lenses_before(row + 1) as f32
    }

    /// The top of wrapped segment `segment` of `row`.
    pub fn segment_top(&self, row: u32, segment: u32) -> f32 {
        self.line_top(row) + self.line_height * segment as f32
    }

    /// The top of the lens row above `row`, if it has one.
    pub fn lens_top(&self, row: u32) -> Option<f32> {
        self.has_lens(row).then(|| self.slot_top(row))
    }

    /// The height of the whole document.
    pub fn total_height(&self) -> f32 {
        if self.mapped() {
            return self.tops[self.line_count as usize];
        }
        self.line_height * self.line_count as f32 + self.lens_height * self.lens_rows.len() as f32
    }

    /// The largest scroll offset: the last shown row's slot at the top of the viewport.
    pub fn max_scroll(&self) -> f32 {
        let last = self.prev_visible(self.line_count - 1).unwrap_or(0);
        if self.mapped() {
            // Its last wrapped segment at the top, as Visual Studio scrolls a wrapped last line.
            return self.segment_top(last, self.segments(last).saturating_sub(1));
        }
        self.slot_top(last)
    }

    /// The row (and whether its lens row, and which wrapped segment) at content `y`; above the top is row 0, below
    /// the end the last shown row.
    pub fn hit(&self, y: f32) -> RowHit {
        if self.mapped() {
            let n = self.line_count as usize;
            let mut row = self.tops[..n]
                .partition_point(|&t| t <= y)
                .saturating_sub(1) as u32;
            row = self
                .prev_visible(row)
                .or_else(|| self.next_visible(0))
                .unwrap_or(0);
            let line_top = self.line_top(row);
            let in_lens = self.has_lens(row) && y >= self.slot_top(row) && y < line_top;
            let segment = if in_lens || y < line_top {
                0
            } else {
                (((y - line_top) / self.line_height) as u32).min(self.segments(row).max(1) - 1)
            };
            return RowHit {
                row,
                in_lens,
                segment,
            };
        }
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
            segment: 0,
        }
    }

    /// The rows whose slots intersect the content range `top..top + height` (hidden rows inside it included: callers
    /// skip them with [`Self::is_hidden`]).
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
                in_lens: false,
                segment: 0
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
                in_lens: false,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(37.9),
            RowHit {
                row: 1,
                in_lens: false,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(38.),
            RowHit {
                row: 2,
                in_lens: true,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(52.9),
            RowHit {
                row: 2,
                in_lens: true,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(53.),
            RowHit {
                row: 2,
                in_lens: false,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(124.),
            RowHit {
                row: 5,
                in_lens: true,
                segment: 0
            }
        );
        assert_eq!(
            l.hit(10_000.),
            RowHit {
                row: 9,
                in_lens: false,
                segment: 0
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
                        in_lens: true,
                        segment: 0
                    }
                );
                y += 15.;
            }
            assert_eq!(big.line_top(r), y);
            assert_eq!(
                big.hit(y + 1.),
                RowHit {
                    row: r,
                    in_lens: false,
                    segment: 0
                }
            );
            y += 19.;
        }
    }

    #[test]
    fn hidden_and_wrapped_rows_with_lenses_match_a_linear_walk() {
        // 2,000 rows: lens rows every 7th, a fold hiding 10..20 and one at 500..1500 (nested 600..700 inside it),
        // every 3rd row wrapped into 2 segments and every 11th into 3.
        let n = 2000u32;
        let lenses: Vec<u32> = (0..n).step_by(7).collect();
        let wraps: Vec<u32> = (0..n)
            .map(|r| {
                if r % 11 == 0 {
                    3
                } else if r % 3 == 0 {
                    2
                } else {
                    1
                }
            })
            .collect();
        let l = VerticalLayout::new(19., 15., n, lenses)
            .with_display(vec![600..700, 10..20, 500..1500], wraps.clone());
        assert_eq!(l.hidden(), [10..20, 500..1500]);
        let hidden = |r: u32| (10..20).contains(&r) || (500..1500).contains(&r);
        let mut y = 0f32;
        let mut display_rows = 0;
        for r in 0..n {
            assert_eq!(l.is_hidden(r), hidden(r), "row {r}");
            assert_eq!(l.slot_top(r), y, "row {r}");
            if hidden(r) {
                assert_eq!(l.segments(r), 0);
                assert!(!l.has_lens(r));
                continue;
            }
            if r % 7 == 0 {
                assert!(l.has_lens(r));
                assert_eq!(
                    l.hit(y + 1.),
                    RowHit {
                        row: r,
                        in_lens: true,
                        segment: 0
                    }
                );
                y += 15.;
            }
            assert_eq!(l.line_top(r), y, "row {r}");
            for seg in 0..wraps[r as usize] {
                assert_eq!(l.segment_top(r, seg), y);
                assert_eq!(
                    l.hit(y + 1.),
                    RowHit {
                        row: r,
                        in_lens: false,
                        segment: seg
                    },
                    "row {r}"
                );
                y += 19.;
                display_rows += 1;
            }
        }
        assert_eq!(l.total_height(), y);
        assert_eq!(l.display_row_count(), display_rows);
        // Navigation across the folds.
        assert_eq!(l.next_visible(10), Some(20));
        assert_eq!(l.next_visible(9), Some(9));
        assert_eq!(l.prev_visible(19), Some(9));
        assert_eq!(l.prev_visible(1499), Some(499));
        assert_eq!(l.next_visible(1999), Some(1999));
        // A y in a fold's place is the next shown row; past the end the last row.
        assert_eq!(l.hit(l.line_top(20) + 1.).row, 20);
        assert_eq!(l.hit(1e9).row, 1999);
        assert_eq!(l.hit(-5.).row, 0);
        // The last row's last segment can reach the top.
        assert_eq!(l.max_scroll(), l.segment_top(1999, wraps[1999] - 1));
        // Folds at the end of the document: the last shown row is before them.
        let end = VerticalLayout::new(10., 0., 10, vec![]).with_display(vec![5..10], vec![]);
        assert_eq!(end.max_scroll(), 40.);
        assert_eq!(end.hit(1e6).row, 4);
        assert_eq!(end.total_height(), 50.);
        // No folds and no wraps: the plain grid.
        let plain = VerticalLayout::new(10., 0., 10, vec![]).with_display(vec![], vec![1; 10]);
        assert_eq!(plain, VerticalLayout::new(10., 0., 10, vec![]));
    }

    #[test]
    fn a_50k_row_map_with_1000_folds_rebuilds_within_budget() {
        let n = 50_000u32;
        let hidden: Vec<std::ops::Range<u32>> =
            (0..1000).map(|i| i * 50 + 1..i * 50 + 40).collect();
        let wraps: Vec<u32> = (0..n).map(|r| 1 + (r % 4 == 0) as u32).collect();
        let lenses: Vec<u32> = (0..n).step_by(25).collect();
        let mut took = std::time::Duration::ZERO;
        let mut total = 0.;
        for _ in 0..10 {
            let (h, w) = (hidden.clone(), wraps.clone());
            let base = VerticalLayout::new(19., 15., n, lenses.clone());
            let start = std::time::Instant::now();
            let l = base.with_display(h, w);
            took += start.elapsed();
            total += l.total_height();
        }
        assert!(total > 0.);
        // The budget is the optimized build's; an unoptimized test build gets four times as long.
        let budget = if cfg!(debug_assertions) { 8 } else { 2 };
        eludite_test_support::assert_budget(
            "display map rebuild, 50k rows, 1,000 folds, wrap",
            took / 10,
            std::time::Duration::from_millis(budget),
        );
    }

    #[test]
    fn multibyte_columns() {
        let line = "añb";
        assert_eq!(visual_column(line, 3), 2);
        assert_eq!(byte_for_visual_column(line, 2), 3);
        assert_eq!(from_display(line, 2), 1);
    }
}
