//! Highlight spans stored per buffer row.

use std::ops::Range;
use std::sync::Arc;

use text::{BufferSnapshot, Point};

use super::HighlightKind;

/// A highlighted byte range within one row. Columns are UTF-8 byte offsets
/// from the start of the row, end exclusive.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    pub start: u32,
    pub end: u32,
    pub kind: HighlightKind,
}

#[derive(Clone, Debug, Default)]
struct Row {
    spans: Option<Arc<[Span]>>,
    /// Edited or structurally changed since it was last highlighted.
    dirty: bool,
}

/// Highlight spans for every row of one buffer version.
///
/// Rows are stored separately so that an edit only touches the rows it
/// covers: [`LineHighlights::interpolate`] moves spans through edits in
/// O(edited rows), which is what lets the UI thread keep showing slightly
/// stale highlights, correctly placed, while a background parse catches up.
/// Cloning shares each row's span slice.
#[derive(Clone, Debug, Default)]
pub struct LineHighlights {
    rows: Vec<Row>,
}

/// One edit in row/column coordinates: `old` in the old text, `new` in the
/// new text (`new.start == old.start` after earlier edits are accounted for).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct PointEdit {
    pub old: Range<Point>,
    pub new: Range<Point>,
}

/// The edits that turn `old` into `new`, in ascending order.
pub(crate) fn edits_between(old: &BufferSnapshot, new: &BufferSnapshot) -> Vec<PointEdit> {
    new.edits_since::<usize>(old.version())
        .map(|e| PointEdit {
            old: old.offset_to_point(e.old.start)..old.offset_to_point(e.old.end),
            new: new.offset_to_point(e.new.start)..new.offset_to_point(e.new.end),
        })
        .collect()
}

impl LineHighlights {
    /// `row_count` rows, all unhighlighted and dirty.
    pub fn dirty(row_count: u32) -> Self {
        Self {
            rows: vec![
                Row {
                    spans: None,
                    dirty: true,
                };
                row_count as usize
            ],
        }
    }

    pub fn row_count(&self) -> u32 {
        self.rows.len() as u32
    }

    /// Spans of `row`, sorted and non-overlapping. Empty if out of range.
    pub fn spans(&self, row: u32) -> &[Span] {
        self.rows
            .get(row as usize)
            .and_then(|r| r.spans.as_deref())
            .unwrap_or(&[])
    }

    /// The kind at a position, if any span covers it.
    pub fn kind_at(&self, point: Point) -> Option<HighlightKind> {
        self.spans(point.row)
            .iter()
            .find(|s| s.start <= point.column && point.column < s.end)
            .map(|s| s.kind)
    }

    pub fn is_dirty(&self, row: u32) -> bool {
        self.rows.get(row as usize).is_some_and(|r| r.dirty)
    }

    pub fn dirty_row_count(&self) -> usize {
        self.rows.iter().filter(|r| r.dirty).count()
    }

    pub(crate) fn mark_dirty(&mut self, rows: Range<u32>) {
        let end = (rows.end as usize).min(self.rows.len());
        let start = (rows.start as usize).min(end);
        for row in &mut self.rows[start..end] {
            row.dirty = true;
        }
    }

    pub(crate) fn set_row(&mut self, row: u32, spans: Vec<Span>) {
        if let Some(r) = self.rows.get_mut(row as usize) {
            r.spans = (!spans.is_empty()).then(|| spans.into());
            r.dirty = false;
        }
    }

    /// Ranges of consecutive dirty rows, in order.
    pub(crate) fn dirty_ranges(&self) -> Vec<Range<u32>> {
        let mut out = Vec::new();
        let mut start = None;
        for (ix, row) in self.rows.iter().enumerate() {
            match (row.dirty, start) {
                (true, None) => start = Some(ix as u32),
                (false, Some(s)) => {
                    out.push(s..ix as u32);
                    start = None;
                }
                _ => {}
            }
        }
        if let Some(s) = start {
            out.push(s..self.rows.len() as u32);
        }
        out
    }

    /// Move spans from `old`'s coordinates to `new`'s. Spans left of an edit
    /// keep their place, spans right of it shift with the text, spans inside
    /// it are dropped, and every row an edit touches is marked dirty.
    pub fn interpolate(&mut self, old: &BufferSnapshot, new: &BufferSnapshot) {
        if old.version() == new.version() {
            return;
        }
        let edits = edits_between(old, new);
        self.apply_edits(&edits);
        debug_assert_eq!(self.row_count(), new.max_point().row + 1);
    }

    pub(crate) fn apply_edits(&mut self, edits: &[PointEdit]) {
        // Last edit first: everything before an edit is still in old
        // coordinates when it is applied.
        for edit in edits.iter().rev() {
            let (r0, c0) = (edit.old.start.row, edit.old.start.column);
            let (r1, c1) = (edit.old.end.row, edit.old.end.column);
            let new_rows = edit.new.end.row - edit.new.start.row;
            // Column where the old row's tail lands, relative to this edit's start.
            let tail_col = if new_rows == 0 {
                c0 + (edit.new.end.column - edit.new.start.column)
            } else {
                edit.new.end.column
            };
            let first = self.spans(r0).to_vec();
            let last = self.spans(r1).to_vec();
            let head: Vec<Span> = first
                .iter()
                .filter(|s| s.start < c0)
                .map(|s| Span {
                    end: s.end.min(c0),
                    ..*s
                })
                .collect();
            let tail: Vec<Span> = last
                .iter()
                .filter(|s| s.end > c1)
                .map(|s| Span {
                    start: s.start.max(c1) - c1 + tail_col,
                    end: s.end - c1 + tail_col,
                    kind: s.kind,
                })
                .collect();
            let mut replacement = Vec::with_capacity(new_rows as usize + 1);
            if new_rows == 0 {
                let mut spans = head;
                spans.extend(tail);
                replacement.push(row_from(spans));
            } else {
                replacement.push(row_from(head));
                for _ in 1..new_rows {
                    replacement.push(Row {
                        spans: None,
                        dirty: true,
                    });
                }
                replacement.push(row_from(tail));
            }
            let end = ((r1 + 1) as usize).min(self.rows.len());
            let start = (r0 as usize).min(end);
            self.rows.splice(start..end, replacement);
        }
    }
}

fn row_from(spans: Vec<Span>) -> Row {
    Row {
        spans: (!spans.is_empty()).then(|| spans.into()),
        dirty: true,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use text::{Buffer as TextBuffer, BufferId, ReplicaId};

    fn span(start: u32, end: u32, kind: HighlightKind) -> Span {
        Span { start, end, kind }
    }

    fn buffer(text: &str) -> TextBuffer {
        TextBuffer::new(ReplicaId::LOCAL, BufferId::new(1).unwrap(), text)
    }

    #[test]
    fn interpolation_shifts_spans_right_of_an_edit() {
        let mut b = buffer("let x = 1;\nfoo();\n");
        let old = b.snapshot().clone();
        let mut h = LineHighlights::dirty(3);
        h.set_row(
            0,
            vec![
                span(0, 3, HighlightKind::Keyword),
                span(8, 9, HighlightKind::Number),
            ],
        );
        h.set_row(1, vec![span(0, 3, HighlightKind::Function)]);
        b.edit([(4..5, "value")]);
        h.interpolate(&old, b.snapshot());
        assert_eq!(
            h.spans(0),
            &[
                span(0, 3, HighlightKind::Keyword),
                span(12, 13, HighlightKind::Number)
            ]
        );
        assert!(h.is_dirty(0));
        assert_eq!(h.spans(1), &[span(0, 3, HighlightKind::Function)]);
        assert!(!h.is_dirty(1));
    }

    #[test]
    fn interpolation_splits_and_joins_rows() {
        let mut b = buffer("aaa bbb\nccc\n");
        let old = b.snapshot().clone();
        let mut h = LineHighlights::dirty(3);
        h.set_row(
            0,
            vec![
                span(0, 3, HighlightKind::Keyword),
                span(4, 7, HighlightKind::Type),
            ],
        );
        h.set_row(1, vec![span(0, 3, HighlightKind::String)]);
        // Split row 0 before "bbb".
        b.edit([(4..4, "\n")]);
        h.interpolate(&old, b.snapshot());
        assert_eq!(h.row_count(), 4);
        assert_eq!(h.spans(0), &[span(0, 3, HighlightKind::Keyword)]);
        assert_eq!(h.spans(1), &[span(0, 3, HighlightKind::Type)]);
        assert_eq!(h.spans(2), &[span(0, 3, HighlightKind::String)]);
        // Join it back.
        let old = b.snapshot().clone();
        b.edit([(4..5, "")]);
        h.interpolate(&old, b.snapshot());
        assert_eq!(h.row_count(), 3);
        assert_eq!(
            h.spans(0),
            &[
                span(0, 3, HighlightKind::Keyword),
                span(4, 7, HighlightKind::Type)
            ]
        );
    }

    #[test]
    fn interpolation_handles_several_edits_on_one_row() {
        let mut b = buffer("ab cd ef");
        let old = b.snapshot().clone();
        let mut h = LineHighlights::dirty(1);
        h.set_row(
            0,
            vec![
                span(0, 2, HighlightKind::Keyword),
                span(3, 5, HighlightKind::Type),
                span(6, 8, HighlightKind::String),
            ],
        );
        b.edit([(0..0, "X"), (3..3, "YY")]);
        assert_eq!(b.snapshot().text(), "Xab YYcd ef");
        h.interpolate(&old, b.snapshot());
        assert_eq!(
            h.spans(0),
            &[
                span(1, 3, HighlightKind::Keyword),
                span(6, 8, HighlightKind::Type),
                span(9, 11, HighlightKind::String),
            ]
        );
    }

    #[test]
    fn dirty_ranges_are_coalesced() {
        let mut h = LineHighlights::dirty(6);
        for row in [0, 1, 4] {
            h.set_row(row, vec![]);
        }
        assert_eq!(h.dirty_ranges(), vec![2..4, 5..6]);
        assert_eq!(h.dirty_row_count(), 3);
    }
}
