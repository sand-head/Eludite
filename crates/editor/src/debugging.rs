//! The debugger's marks in the editor (brief 0018): breakpoint glyphs in the margin at the far left (Visual Studio's
//! glyph margin; a tracepoint is a diamond, brief 0026), the execution point (the yellow arrow and the highlighted statement; green for a caller's frame
//! selected in the Call Stack), and the expression under the mouse for data tips. The owner decides what is shown;
//! the view only draws it and reports margin clicks ([`crate::EditorEvent::BreakpointMarginClicked`]).

use std::ops::Range;

use gpui::{Context, Pixels, Point, Rgba, point, px, rgb, rgba};

use crate::view::EditorView;

/// Width of the breakpoint margin at the far left of the gutter.
pub(crate) const BREAKPOINT_MARGIN: Pixels = px(16.);

/// How a breakpoint is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BreakpointGlyph {
    /// A filled red circle.
    Enabled,
    /// A hollow red circle.
    Disabled,
    /// A filled red circle with a white bar: it has a condition or a hit count.
    Conditional,
    /// A hollow circle: the debugger could not bind it to code.
    Unbound,
    /// A filled red diamond: a tracepoint (When Hit, print a message and continue; brief 0026).
    Tracepoint,
    /// A hollow red diamond: a disabled tracepoint.
    TracepointDisabled,
    /// A hollow gray diamond: a tracepoint the debugger could not bind.
    TracepointUnbound,
}

impl BreakpointGlyph {
    pub(crate) fn color(self) -> Rgba {
        match self {
            BreakpointGlyph::Unbound | BreakpointGlyph::TracepointUnbound => rgb(0x9C9C9C),
            _ => rgb(0xE51400),
        }
    }

    pub(crate) fn filled(self) -> bool {
        matches!(
            self,
            BreakpointGlyph::Enabled | BreakpointGlyph::Conditional | BreakpointGlyph::Tracepoint
        )
    }

    /// Drawn as Visual Studio's diamond rather than a circle.
    pub fn is_tracepoint(self) -> bool {
        matches!(
            self,
            BreakpointGlyph::Tracepoint
                | BreakpointGlyph::TracepointDisabled
                | BreakpointGlyph::TracepointUnbound
        )
    }
}

/// Which execution point is shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExecutionKind {
    /// Where the debuggee is stopped: the yellow arrow.
    Current,
    /// A caller's frame selected in the Call Stack: the green arrow.
    Frame,
}

impl ExecutionKind {
    pub(crate) fn arrow(self) -> Rgba {
        match self {
            ExecutionKind::Current => rgb(0xFFCC00),
            ExecutionKind::Frame => rgb(0x5FB760),
        }
    }

    pub(crate) fn background(self) -> Rgba {
        match self {
            ExecutionKind::Current => rgba(0xFFCC0040),
            ExecutionKind::Frame => rgba(0x5FB76038),
        }
    }
}

fn is_identifier_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl EditorView {
    /// Show breakpoint glyphs on these rows (0-based), replacing the previous ones. They stay on their lines as the
    /// text changes ([`EditorView::breakpoint_glyphs`] reads them back).
    pub fn set_breakpoint_glyphs(
        &mut self,
        rows: Vec<(u32, BreakpointGlyph)>,
        cx: &mut Context<Self>,
    ) {
        let b = self.editor.buffer();
        let last = b.line_count().saturating_sub(1);
        let next: Vec<_> = rows
            .into_iter()
            .map(|(row, glyph)| {
                let start = b.point_to_offset(text::Point::new(row.min(last), 0));
                (b.anchor_before(start), glyph)
            })
            .collect();
        self.breakpoint_glyphs = next;
        cx.notify();
    }

    /// The rows (0-based) of the breakpoint glyphs, where the text has moved them, in row order.
    pub fn breakpoint_glyphs(&self) -> Vec<(u32, BreakpointGlyph)> {
        let b = self.editor.buffer();
        let mut rows: Vec<(u32, BreakpointGlyph)> = self
            .breakpoint_glyphs
            .iter()
            .map(|(a, g)| (b.offset_to_point(b.offset_for_anchor(a)).row, *g))
            .collect();
        rows.sort_by_key(|(r, _)| *r);
        rows
    }

    /// Show the execution point over `range` (byte offsets: the statement), or hide it.
    pub fn set_execution_point(
        &mut self,
        at: Option<(Range<usize>, ExecutionKind)>,
        cx: &mut Context<Self>,
    ) {
        let b = self.editor.buffer();
        let len = b.len();
        self.execution = at.map(|(r, kind)| {
            (
                b.anchor_before(r.start.min(len))..b.anchor_after(r.end.min(len)),
                kind,
            )
        });
        cx.notify();
    }

    /// The row (0-based) and kind of the execution point shown.
    pub fn execution_point(&self) -> Option<(u32, ExecutionKind)> {
        let (r, kind) = self.execution.as_ref()?;
        let b = self.editor.buffer();
        Some((b.offset_to_point(b.offset_for_anchor(&r.start)).row, *kind))
    }

    /// The window position of the breakpoint margin on `row` (0-based), if the row was in the last frame: where a
    /// click toggles a breakpoint.
    pub fn breakpoint_margin_point(&self, row: u32) -> Option<Point<Pixels>> {
        let l = self.layout.as_ref()?;
        let y = l.bounds.top() + px(l.vertical.line_top(row)) - l.scroll.y + l.line_height / 2.;
        (y >= l.bounds.top() && y <= l.bounds.bottom())
            .then(|| point(l.bounds.left() + BREAKPOINT_MARGIN / 2., y))
    }

    /// The row whose breakpoint margin is at `position`, if it is in the margin.
    pub(crate) fn margin_row(&self, position: Point<Pixels>) -> Option<u32> {
        let l = self.layout.as_ref()?;
        if !l.bounds.contains(&position) || position.x >= l.bounds.left() + BREAKPOINT_MARGIN {
            return None;
        }
        let y = position.y - l.bounds.top() + l.scroll.y;
        let hit = l.vertical.hit(f32::from(y));
        (!hit.in_lens && hit.row < self.editor.buffer().line_count()).then_some(hit.row)
    }

    /// The expression a data tip shows for the identifier at `offset`: the identifier with the member accesses
    /// before it (`this._timeProvider`, `order.Customer.Name` up to the hovered member), and its byte range.
    pub fn expression_at(&self, offset: usize) -> Option<(Range<usize>, String)> {
        let buffer = self.editor.buffer();
        let point = buffer.offset_to_point(offset.min(buffer.len()));
        let line = buffer.line(point.row);
        let line_start = buffer.point_to_offset(text::Point::new(point.row, 0));
        let col = (point.column as usize).min(line.len());
        let start_of = |end: usize| {
            line[..end]
                .char_indices()
                .rev()
                .take_while(|(_, c)| is_identifier_char(*c))
                .last()
                .map_or(end, |(i, _)| i)
        };
        let mut start = start_of(col);
        let end = line[col..]
            .char_indices()
            .find(|(_, c)| !is_identifier_char(*c))
            .map_or(line.len(), |(i, _)| col + i);
        if start >= end || line[start..].starts_with(|c: char| c.is_ascii_digit()) {
            return None;
        }
        // Walk back over `.member` links.
        while start > 0 && line[..start].ends_with('.') {
            let prev = start_of(start - 1);
            if prev == start - 1 {
                break;
            }
            start = prev;
        }
        Some((
            line_start + start..line_start + end,
            line[start..end].to_owned(),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tracepoints_are_diamonds_filled_when_enabled() {
        use BreakpointGlyph::*;
        for g in [Tracepoint, TracepointDisabled, TracepointUnbound] {
            assert!(g.is_tracepoint(), "{g:?}");
        }
        for g in [Enabled, Disabled, Conditional, Unbound] {
            assert!(!g.is_tracepoint(), "{g:?}");
        }
        assert!(Tracepoint.filled() && !TracepointDisabled.filled() && !TracepointUnbound.filled());
        assert_eq!(TracepointUnbound.color(), Unbound.color());
        assert_eq!(Tracepoint.color(), Enabled.color());
    }
}
