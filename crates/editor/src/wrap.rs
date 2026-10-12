//! Word wrap (brief 0067): Edit > Advanced > Word Wrap (Ctrl+E, Ctrl+W).
//!
//! The editor font is monospaced, so a line wraps by visual columns (tabs expanded to [`TAB_SIZE`] stops) at the text
//! area's width: at the last break opportunity before the edge (after a space, or after `,`, `;`, `.`, `(`, `)`, `{`,
//! `}`, `[`, `]`, `<`, `>`, `-`, `/` or `=`), else at the edge itself. A line's segments are byte ranges of the buffer
//! line, so the caret, selections and highlights map into them without another layout. [`WrapCache`] keeps every row's
//! segment count for the display map ([`crate::display::VerticalLayout::with_display`]) and re-measures only the rows
//! an edit touched.

use std::ops::Range;

use text::BufferSnapshot;

use crate::display::TAB_SIZE;

/// The fewest columns a wrapped line gets, however narrow the editor.
pub const MIN_COLUMNS: u32 = 8;

fn breaks_after(c: char) -> bool {
    matches!(
        c,
        ' ' | '\t'
            | ','
            | ';'
            | '.'
            | '('
            | ')'
            | '{'
            | '}'
            | '['
            | ']'
            | '<'
            | '>'
            | '-'
            | '/'
            | '='
    )
}

/// The byte offsets where each wrapped segment of `line` starts (the first is always 0) at `columns` columns.
pub fn segment_starts(line: &str, columns: u32) -> Vec<usize> {
    let columns = columns.max(MIN_COLUMNS);
    let mut starts = vec![0];
    // Visual column of the current segment's start in the line (tab stops are the line's, not the segment's).
    let mut col = 0u32;
    let mut seg_col = 0u32;
    let mut last_break: Option<(usize, u32)> = None;
    let mut seg_start = 0usize;
    // Breaks inside the line's indentation would leave a blank row.
    let mut seen_text = false;
    for (ix, ch) in line.char_indices() {
        let w = if ch == '\t' {
            TAB_SIZE - col % TAB_SIZE
        } else {
            1
        };
        if col + w - seg_col > columns && ix > seg_start {
            // Break at the last opportunity in this segment, else here.
            let (at, at_col) = match last_break {
                Some((b, bc)) if b > seg_start => (b, bc),
                _ => (ix, col),
            };
            starts.push(at);
            seg_start = at;
            seg_col = at_col;
            last_break = None;
        }
        col += w;
        seen_text |= !ch.is_whitespace();
        if seen_text && breaks_after(ch) {
            last_break = Some((ix + ch.len_utf8(), col));
        }
    }
    starts
}

/// The segments of `line` as byte ranges.
pub fn segments(line: &str, columns: Option<u32>) -> Vec<Range<usize>> {
    let Some(columns) = columns else {
        return vec![0..line.len()];
    };
    let starts = segment_starts(line, columns);
    starts
        .iter()
        .enumerate()
        .map(|(i, &s)| s..starts.get(i + 1).copied().unwrap_or(line.len()))
        .collect()
}

/// The segment of `segments` holding byte column `col` (the end of a segment belongs to the next one, except at the
/// line's end).
pub fn segment_of(segments: &[Range<usize>], col: usize) -> usize {
    segments.iter().rposition(|s| s.start <= col).unwrap_or(0)
}

/// Every row's segment count at the current width, kept across edits.
#[derive(Default)]
pub(crate) struct WrapCache {
    columns: Option<u32>,
    version: Option<clock::Global>,
    counts: Vec<u32>,
}

impl WrapCache {
    /// The segment counts for `snapshot` at `columns` (`None`: wrap is off, an empty vector). Re-measures every row when
    /// the width changed, else only the rows edited since the last call.
    pub fn counts(&mut self, snapshot: &BufferSnapshot, columns: Option<u32>) -> Vec<u32> {
        let Some(cols) = columns else {
            self.columns = None;
            self.version = None;
            self.counts.clear();
            return Vec::new();
        };
        let rows = snapshot.max_point().row + 1;
        let measure = |row: u32| -> u32 {
            let len = snapshot.line_len(row);
            // A line that fits needs no scan.
            if len <= cols {
                return 1;
            }
            let start = snapshot.point_to_offset(text::Point::new(row, 0));
            let line: String = snapshot
                .text_for_range(start..start + len as usize)
                .collect();
            segment_starts(&line, cols).len() as u32
        };
        let full = self.columns != Some(cols) || self.version.is_none();
        if full {
            self.counts = (0..rows).map(measure).collect();
        } else if let Some(since) = self.version.as_ref()
            && snapshot.version() != since
        {
            let mut counts = std::mem::take(&mut self.counts);
            // Edits in order: before each, `counts` is in new coordinates up to the edit's start, so splice there.
            for edit in snapshot.edits_since::<text::Point>(since) {
                let old_rows = (edit.old.end.row - edit.old.start.row + 1) as usize;
                let start = edit.new.start.row;
                let fresh: Vec<u32> = (start..=edit.new.end.row).map(measure).collect();
                let at = (start as usize).min(counts.len());
                let end = (at + old_rows).min(counts.len());
                counts.splice(at..end, fresh);
            }
            counts.resize(rows as usize, 1);
            self.counts = counts;
        }
        self.columns = Some(cols);
        self.version = Some(snapshot.version().clone());
        self.counts.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wraps_at_the_last_break_before_the_edge() {
        assert_eq!(segment_starts("short", 10), vec![0]);
        // "aaaa bbbb cccc" at 10 columns: after "aaaa bbbb " (10 columns fit exactly), then "cccc".
        assert_eq!(segment_starts("aaaa bbbb cccc", 10), vec![0, 10]);
        // No break opportunity: at the edge.
        assert_eq!(
            segment_starts("abcdefghijklmnopqrstuvwxyz", 10),
            vec![0, 10, 20]
        );
        // Punctuation is a break opportunity.
        assert_eq!(segment_starts("Foo(alpha,beta,gamma)", 12), vec![0, 10]);
        // The indentation is not a break opportunity.
        assert_eq!(segment_starts("        abcdefghij", 12), vec![0, 12]);
        let segs = segments("aaaa bbbb cccc", Some(10));
        assert_eq!(segs, vec![0..10, 10..14]);
        assert_eq!(segment_of(&segs, 9), 0);
        assert_eq!(segment_of(&segs, 10), 1);
        assert_eq!(segment_of(&segs, 14), 1);
        assert_eq!(segments("x", None), vec![0..1]);
    }

    #[test]
    fn tabs_and_multibyte_count_by_columns() {
        // A tab at column 0 is four columns: "\tab" is 6 columns.
        assert_eq!(segment_starts("\tabcdefgh", 8), vec![0, 5]);
        // Each ñ is one column, two bytes.
        let line = "ññññññññññ";
        assert_eq!(segment_starts(line, 8), vec![0, 16]);
    }

    #[test]
    fn the_cache_follows_edits_without_measuring_every_row() {
        let mut buffer = crate::Buffer::new(&("short\n".repeat(100) + "a b c d e f g h i j k l"));
        let mut cache = WrapCache::default();
        let counts = cache.counts(buffer.snapshot(), Some(8));
        assert_eq!(counts.len(), 101);
        assert_eq!(counts[100], 3);
        assert!(counts[..100].iter().all(|&c| c == 1));
        // Make row 3 long, insert two rows before row 50.
        let at = buffer.point_to_offset(text::Point::new(3, 5));
        buffer.edit([(at..at, " and much more text here")]);
        let at = buffer.point_to_offset(text::Point::new(50, 0));
        buffer.edit([(at..at, "x\ny\n")]);
        let counts = cache.counts(buffer.snapshot(), Some(8));
        assert_eq!(counts.len(), 103);
        assert!(counts[3] > 1);
        assert_eq!(counts[102], 3);
        let fresh = WrapCache::default().counts(buffer.snapshot(), Some(8));
        assert_eq!(counts, fresh);
        // Off: nothing.
        assert!(cache.counts(buffer.snapshot(), None).is_empty());
    }
}
