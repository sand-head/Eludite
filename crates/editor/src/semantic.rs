//! Semantic highlighting in the editor (brief 0066): the language server's classifications painted over the
//! tree-sitter colors.
//!
//! - **Layering.** Tree-sitter highlighting is the first paint and never waits. [`SemanticRows`] holds the server's
//!   colored spans per buffer row; a span's color replaces the syntax color under it, and decorations (diagnostics,
//!   references, the execution point) paint over both. A span with no color leaves the syntax color (and may still
//!   strike the text through, for a deprecated symbol).
//! - **Edits.** The rows move through edits like [`crate::syntax::LineHighlights`]: spans left of an edit stay, spans
//!   right of it shift with the text, spans inside it go. They stay until a fresh answer replaces them.
//! - **Requests.** The editor knows nothing about LSP: it says *when* tokens are wanted
//!   ([`EditorEvent::SemanticTokensRequested`]: when turned on, [`SEMANTIC_DEBOUNCE`] after the last edit, on
//!   [`EditorView::refresh_semantic_tokens`]); the owner answers with [`EditorView::set_semantic_tokens`] quoting the
//!   id, once or several times (the visible rows first, then the whole document). An answer for another id, or made on
//!   text the person changed since, is dropped (CLAUDE.md invariant 12) and the debounce asks again.

use std::ops::Range;
use std::sync::Arc;

use gpui::{Context, Rgba, Task};
use text::{BufferSnapshot, Point};

use crate::intellisense::EditorEvent;
use crate::view::EditorView;

/// How long after the last edit the tokens are asked for again (as CodeLens, brief 0052).
pub const SEMANTIC_DEBOUNCE: std::time::Duration = std::time::Duration::from_millis(150);

/// A server-classified byte range within one row. Columns are UTF-8 byte offsets from the start of the row, end
/// exclusive.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SemanticSpan {
    pub start: u32,
    pub end: u32,
    /// The color painted over the syntax color, or `None` to keep it.
    pub color: Option<Rgba>,
    /// A deprecated symbol is struck through.
    pub strikethrough: bool,
}

/// The server's spans for every row of one buffer version (rows share their span slices when cloned).
#[derive(Clone, Debug, Default)]
pub struct SemanticRows {
    rows: Vec<Option<Arc<[SemanticSpan]>>>,
}

impl SemanticRows {
    /// `row_count` rows with no spans.
    pub fn new(row_count: u32) -> Self {
        Self {
            rows: vec![None; row_count as usize],
        }
    }

    pub fn row_count(&self) -> u32 {
        self.rows.len() as u32
    }

    /// The spans of `row`, sorted and non-overlapping; empty when out of range.
    pub fn spans(&self, row: u32) -> &[SemanticSpan] {
        self.rows
            .get(row as usize)
            .and_then(|r| r.as_deref())
            .unwrap_or(&[])
    }

    /// Every span, for tests and counts.
    pub fn span_count(&self) -> usize {
        self.rows.iter().flatten().map(|r| r.len()).sum()
    }

    /// Replace `row`'s spans (sorted by start; overlapping ones are clipped).
    pub fn set_row(&mut self, row: u32, mut spans: Vec<SemanticSpan>) {
        let Some(r) = self.rows.get_mut(row as usize) else {
            return;
        };
        spans.sort_by_key(|s| s.start);
        let mut end = 0;
        spans.retain_mut(|s| {
            s.start = s.start.max(end);
            if s.start >= s.end {
                return false;
            }
            end = s.end;
            true
        });
        *r = (!spans.is_empty()).then(|| spans.into());
    }

    /// Move spans from `old`'s coordinates to `new`'s.
    pub fn interpolate(&mut self, old: &BufferSnapshot, new: &BufferSnapshot) {
        if old.version() == new.version() {
            return;
        }
        let edits: Vec<(Range<Point>, Range<Point>)> = new
            .edits_since::<usize>(old.version())
            .map(|e| {
                (
                    old.offset_to_point(e.old.start)..old.offset_to_point(e.old.end),
                    new.offset_to_point(e.new.start)..new.offset_to_point(e.new.end),
                )
            })
            .collect();
        // Last edit first: everything before an edit is still in old coordinates when it is applied.
        for (old_range, new_range) in edits.iter().rev() {
            let (r0, c0) = (old_range.start.row, old_range.start.column);
            let (r1, c1) = (old_range.end.row, old_range.end.column);
            let new_rows = new_range.end.row - new_range.start.row;
            let tail_col = if new_rows == 0 {
                c0 + (new_range.end.column - new_range.start.column)
            } else {
                new_range.end.column
            };
            let head: Vec<SemanticSpan> = self
                .spans(r0)
                .iter()
                .filter(|s| s.end <= c0)
                .copied()
                .collect();
            let tail: Vec<SemanticSpan> = self
                .spans(r1)
                .iter()
                .filter(|s| s.start >= c1)
                .map(|s| SemanticSpan {
                    start: s.start - c1 + tail_col,
                    end: s.end - c1 + tail_col,
                    ..*s
                })
                .collect();
            let row = |spans: Vec<SemanticSpan>| (!spans.is_empty()).then(|| spans.into());
            let mut replacement: Vec<Option<Arc<[SemanticSpan]>>> =
                Vec::with_capacity(new_rows as usize + 1);
            if new_rows == 0 {
                let mut spans = head;
                spans.extend(tail);
                replacement.push(row(spans));
            } else {
                replacement.push(row(head));
                replacement.extend(std::iter::repeat_n(None, new_rows as usize - 1));
                replacement.push(row(tail));
            }
            let end = ((r1 + 1) as usize).min(self.rows.len());
            let start = (r0 as usize).min(end);
            self.rows.splice(start..end, replacement);
        }
    }
}

/// An answer to [`EditorEvent::SemanticTokensRequested`]: the spans of `rows` (rows outside it keep what they show),
/// by row. A whole-document answer covers every row.
#[derive(Clone, Debug, Default)]
pub struct SemanticUpdate {
    pub rows: Range<u32>,
    pub spans: Vec<(u32, Vec<SemanticSpan>)>,
}

/// The view's semantic state.
#[derive(Default)]
pub(crate) struct SemanticState {
    pub enabled: bool,
    next_id: u64,
    /// The request in flight: its id and the buffer version it was made on.
    pending: Option<(u64, clock::Global)>,
    stale: bool,
    pub rows: SemanticRows,
    /// The snapshot `rows` is aligned with.
    snapshot: Option<BufferSnapshot>,
    debounce: Option<Task<()>>,
    /// Answers applied (tests and the report's numbers).
    pub applied: usize,
}

impl EditorView {
    /// Whether the editor shows the server's classifications and asks for them.
    pub fn semantic_highlighting_enabled(&self) -> bool {
        self.semantic.enabled
    }

    /// Turn semantic highlighting on (tokens are asked for at once) or off (the server's colors go; the syntax colors
    /// stay).
    pub fn set_semantic_highlighting_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.semantic.enabled == enabled {
            return;
        }
        self.semantic.enabled = enabled;
        if enabled {
            self.refresh_semantic_tokens(cx);
        } else {
            self.semantic.pending = None;
            self.semantic.debounce = None;
            self.semantic.rows = SemanticRows::default();
            self.semantic.snapshot = None;
            cx.notify();
        }
    }

    /// Ask for the tokens again now (a refresh, a new generation, a setting). Nothing while it is off.
    pub fn refresh_semantic_tokens(&mut self, cx: &mut Context<Self>) {
        if !self.semantic.enabled {
            return;
        }
        self.semantic.next_id += 1;
        let id = self.semantic.next_id;
        let version = self.editor.buffer().snapshot().version().clone();
        self.semantic.pending = Some((id, version));
        self.semantic.stale = false;
        self.semantic.debounce = None;
        cx.emit(EditorEvent::SemanticTokensRequested { id });
    }

    /// The answer (or part of it) to request `id`. Dropped (returns false) when another request superseded it or the
    /// text changed since it was made. Several answers may quote one id (the visible rows, then the document).
    pub fn set_semantic_tokens(
        &mut self,
        id: u64,
        update: SemanticUpdate,
        cx: &mut Context<Self>,
    ) -> bool {
        let snapshot = self.editor.buffer().snapshot().clone();
        let current =
            matches!(&self.semantic.pending, Some((p, v)) if *p == id && v == snapshot.version());
        if !self.semantic.enabled || !current {
            return false;
        }
        let line_count = snapshot.max_point().row + 1;
        let mut rows = match &self.semantic.snapshot {
            Some(old) if self.semantic.rows.row_count() > 0 => {
                let mut r = self.semantic.rows.clone();
                r.interpolate(old, &snapshot);
                r
            }
            _ => SemanticRows::new(line_count),
        };
        if rows.row_count() != line_count {
            rows = SemanticRows::new(line_count);
        }
        let covered = update.rows.start.min(line_count)..update.rows.end.min(line_count);
        let mut by_row = update.spans.into_iter().peekable();
        for row in covered {
            while by_row.peek().is_some_and(|(r, _)| *r < row) {
                by_row.next();
            }
            let spans = match by_row.peek() {
                Some((r, _)) if *r == row => by_row.next().map(|(_, s)| s).unwrap_or_default(),
                _ => Vec::new(),
            };
            rows.set_row(row, spans);
        }
        self.semantic.rows = rows;
        self.semantic.snapshot = Some(snapshot);
        self.semantic.applied += 1;
        cx.notify();
        true
    }

    /// The server's spans shown for the current text (moved through edits since they arrived).
    pub fn semantic_rows(&self) -> &SemanticRows {
        &self.semantic.rows
    }

    /// How many answers were applied.
    pub fn semantic_updates(&self) -> usize {
        self.semantic.applied
    }

    /// Keep the shown spans aligned with the current text.
    pub(crate) fn sync_semantic(&mut self) {
        let current = self.editor.buffer().snapshot();
        if let Some(old) = &self.semantic.snapshot
            && old.version() != current.version()
        {
            self.semantic.rows.interpolate(old, current);
            self.semantic.snapshot = Some(current.clone());
        }
    }

    /// The text changed: move the spans, and ask again [`SEMANTIC_DEBOUNCE`] after the last edit.
    pub(crate) fn semantic_edited(&mut self, cx: &mut Context<Self>) {
        self.sync_semantic();
        if !self.semantic.enabled {
            return;
        }
        self.semantic.stale = true;
        self.semantic.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEMANTIC_DEBOUNCE).await;
            this.update(cx, |v, cx| {
                v.semantic.debounce = None;
                if v.semantic.stale {
                    v.refresh_semantic_tokens(cx);
                }
            })
            .ok();
        }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::rgb;
    use text::{Buffer as TextBuffer, BufferId, ReplicaId};

    fn span(start: u32, end: u32) -> SemanticSpan {
        SemanticSpan {
            start,
            end,
            color: Some(rgb(0x4EC9B0)),
            strikethrough: false,
        }
    }

    #[test]
    fn spans_move_through_edits() {
        let mut b = TextBuffer::new(
            ReplicaId::LOCAL,
            BufferId::new(1).unwrap(),
            "var list = new List();\nFoo(list);\n",
        );
        let old = b.snapshot().clone();
        let mut rows = SemanticRows::new(3);
        rows.set_row(0, vec![span(15, 19), span(4, 8)]);
        rows.set_row(1, vec![span(4, 8)]);
        // Typing before `list` on row 0 shifts both spans; an edit inside `List` drops it.
        b.edit([(0..0, "  ")]);
        b.edit([(18..18, "X")]);
        rows.interpolate(&old, b.snapshot());
        assert_eq!(rows.spans(0), &[span(6, 10)]);
        assert_eq!(rows.spans(1), &[span(4, 8)]);
        // A new line splits a row; joining moves the tail.
        let old = b.snapshot().clone();
        b.edit([(0..0, "\n")]);
        rows.interpolate(&old, b.snapshot());
        assert_eq!(rows.row_count(), 4);
        assert_eq!(rows.spans(1), &[span(6, 10)]);
        assert_eq!(rows.spans(2), &[span(4, 8)]);
        assert_eq!(rows.span_count(), 2);
    }

    #[test]
    fn overlapping_spans_are_clipped() {
        let mut rows = SemanticRows::new(1);
        rows.set_row(0, vec![span(0, 5), span(3, 8), span(4, 5)]);
        assert_eq!(rows.spans(0), &[span(0, 5), span(5, 8)]);
    }

    /// Syntax colors first, the server's over them, decorations over both; a faded decoration lowers the opacity.
    #[test]
    fn colors_layer_syntax_then_semantic_then_decorations() {
        use crate::syntax::{HighlightKind, Span, SyntaxTheme};
        use crate::view::{DecorationStyle, text_runs};
        let theme = SyntaxTheme::vs_dark(&eludite_ui::Theme::dark());
        let font = gpui::font("Mono");
        let line = "Foo bar baz";
        let syntax = [Span {
            start: 0,
            end: 11,
            kind: HighlightKind::Type,
        }];
        let semantic = [
            span(4, 7),
            SemanticSpan {
                start: 8,
                end: 11,
                color: None,
                strikethrough: true,
            },
        ];
        let red = rgb(0xFF0000);
        let decorations = [
            (8usize..11usize, DecorationStyle::Foreground(red)),
            (0..3, DecorationStyle::Faded(0.5)),
        ];
        let runs = text_runs(
            line,
            line,
            &syntax,
            &semantic,
            &decorations,
            0,
            &theme,
            &font,
        );
        let colors: Vec<(usize, gpui::Hsla, bool)> = runs
            .iter()
            .map(|r| (r.len, r.color, r.strikethrough.is_some()))
            .collect();
        let mut faded = theme.color(HighlightKind::Type);
        faded.a *= 0.5;
        assert_eq!(
            colors,
            [
                (3, faded.into(), false),
                (1, theme.color(HighlightKind::Type).into(), false),
                (3, rgb(0x4EC9B0).into(), false),
                (1, theme.color(HighlightKind::Type).into(), false),
                (3, red.into(), true),
            ]
        );
    }
}
