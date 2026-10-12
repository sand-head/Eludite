//! Inlay hints in the editor (brief 0066): parameter names and inferred types drawn inside the line as muted chips
//! that are not text.
//!
//! - **Display.** A row's shaped string is its text with tabs expanded and each hint's label inserted at its column
//!   ([`RowInlays`]). Every conversion between a buffer column and a position in that string goes through
//!   [`RowInlays::to_display`] and [`RowInlays::from_display`], so the caret, selections, find matches, decorations,
//!   popups and the IME see the buffer only: a hint never changes a line count, a line number, an offset or what is
//!   copied, and the caret moves over a hint in one step (it has no buffer position inside it).
//! - **The caret's side.** A caret at a hint's column is drawn after a parameter hint (`count:` belongs to the
//!   argument after it) and before a type hint (`: int` belongs to the name before it).
//! - **Requests.** The editor says *when* hints are wanted and for which rows ([`EditorEvent::InlayHintsRequested`]:
//!   the visible rows and [`INLAY_MARGIN`] lines on each side; when turned on, [`crate::SEMANTIC_DEBOUNCE`] after the
//!   last edit, when scrolling leaves the covered rows, on [`EditorView::refresh_inlay_hints`]). The owner answers
//!   with [`EditorView::set_inlay_hints`] quoting the id; a stale answer is dropped. The mouse resting on a hint emits
//!   [`EditorEvent::InlayHintHovered`] (the owner resolves its tooltip and shows it with the hover popup).

use std::ops::Range;

use gpui::{Context, Task};
use text::Anchor;

use crate::display::{expand_tabs, from_display, to_display};
use crate::intellisense::EditorEvent;
use crate::semantic::SEMANTIC_DEBOUNCE;
use crate::view::EditorView;

/// Lines beyond the visible range, on each side, whose hints are asked for.
pub const INLAY_MARGIN: u32 = 100;

/// What a hint stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InlayKind {
    /// An inferred type: belongs to the text before it.
    Type,
    /// A parameter name: belongs to the text after it.
    Parameter,
    /// Anything else (a chaining or closing-brace hint): drawn like a type hint.
    Other,
}

/// One hint as the owner hands it to the editor.
#[derive(Clone, Debug, PartialEq)]
pub struct InlayHint {
    /// The owner's id (quoted back in [`EditorEvent::InlayHintHovered`]).
    pub id: u64,
    /// Buffer offset the hint sits at.
    pub offset: usize,
    pub label: String,
    pub kind: InlayKind,
    pub padding_left: bool,
    pub padding_right: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct StoredHint {
    pub anchor: Anchor,
    pub hint: InlayHint,
}

/// One hint placed in a row: its byte column in the row, the text drawn (with its paddings) and whether the caret at
/// that column goes after it.
#[derive(Clone, Debug, PartialEq)]
pub struct RowInlay {
    pub column: usize,
    pub text: String,
    pub caret_after: bool,
    pub id: u64,
}

/// A row's hints, sorted by column, and the conversions between buffer columns and its display string.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct RowInlays {
    pub items: Vec<RowInlay>,
}

impl RowInlays {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }

    /// The display string: tabs expanded, hints inserted.
    pub fn display(&self, line: &str) -> String {
        let base = expand_tabs(line);
        if self.items.is_empty() {
            return base.into_owned();
        }
        let mut out = String::with_capacity(base.len() + 16 * self.items.len());
        let mut at = 0;
        for i in &self.items {
            let pos = to_display(line, i.column);
            out.push_str(&base[at..pos]);
            out.push_str(&i.text);
            at = pos;
        }
        out.push_str(&base[at..]);
        out
    }

    /// Where each hint is in the display string, in order: `(start, end, id)`.
    pub fn ranges(&self, line: &str) -> Vec<(usize, usize, u64)> {
        let mut shift = 0;
        self.items
            .iter()
            .map(|i| {
                let start = to_display(line, i.column) + shift;
                shift += i.text.len();
                (start, start + i.text.len(), i.id)
            })
            .collect()
    }

    /// Where each hint is inserted in the plain (tab-expanded) display string, with its text: for splicing runs.
    pub fn insertions(&self, line: &str) -> Vec<(usize, &RowInlay)> {
        self.items
            .iter()
            .map(|i| (to_display(line, i.column), i))
            .collect()
    }

    /// The display offset of byte column `column` (a caret, a selection edge).
    pub fn to_display(&self, line: &str, column: usize) -> usize {
        let base = to_display(line, column);
        base + self
            .items
            .iter()
            .filter(|i| i.column < column || (i.column == column && i.caret_after))
            .map(|i| i.text.len())
            .sum::<usize>()
    }

    /// The byte column at display offset `display`: a position inside a hint is the hint's column.
    pub fn from_display(&self, line: &str, display: usize) -> usize {
        let mut shift = 0;
        for (item, (start, end, _)) in self.items.iter().zip(self.ranges(line)) {
            if display < start {
                break;
            }
            if display < end {
                return item.column;
            }
            shift += end - start;
        }
        from_display(line, display - shift)
    }

    /// The hint at display offset `display`, if one is drawn there.
    pub fn hint_at(&self, line: &str, display: usize) -> Option<u64> {
        self.ranges(line)
            .into_iter()
            .find(|(s, e, _)| (*s..*e).contains(&display))
            .map(|(_, _, id)| id)
    }
}

/// The view's hint state.
#[derive(Default)]
pub(crate) struct InlayState {
    pub enabled: bool,
    next_id: u64,
    /// The request in flight: its id, the version it was made on and the rows it covers.
    pending: Option<(u64, clock::Global, Range<u32>)>,
    stale: bool,
    pub hints: Vec<StoredHint>,
    /// The rows the shown hints were asked for.
    covered: Option<Range<u32>>,
    debounce: Option<Task<()>>,
    /// The hint under the mouse.
    pub hovered: Option<u64>,
    pub applied: usize,
}

impl EditorView {
    pub fn inlay_hints_enabled(&self) -> bool {
        self.inlays.enabled
    }

    /// Turn hints on (asked for at once) or off (removed at once).
    pub fn set_inlay_hints_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if self.inlays.enabled == enabled {
            return;
        }
        self.inlays.enabled = enabled;
        if enabled {
            self.refresh_inlay_hints(cx);
        } else {
            self.inlays.pending = None;
            self.inlays.debounce = None;
            self.inlays.hints.clear();
            self.inlays.covered = None;
            self.inlays.hovered = None;
            cx.notify();
        }
    }

    /// The rows hints are asked for: the visible ones and [`INLAY_MARGIN`] around them.
    fn inlay_window(&self) -> Range<u32> {
        let visible = self.visible_rows();
        let count = self.editor.buffer().line_count().max(1);
        visible.start.saturating_sub(INLAY_MARGIN)
            ..visible.end.saturating_add(INLAY_MARGIN).min(count)
    }

    /// Ask for the hints of the visible window now. Nothing while hints are off.
    pub fn refresh_inlay_hints(&mut self, cx: &mut Context<Self>) {
        if !self.inlays.enabled {
            return;
        }
        self.inlays.next_id += 1;
        let id = self.inlays.next_id;
        let rows = self.inlay_window();
        let version = self.editor.buffer().snapshot().version().clone();
        self.inlays.pending = Some((id, version, rows.clone()));
        self.inlays.stale = false;
        self.inlays.debounce = None;
        cx.emit(EditorEvent::InlayHintsRequested { id, rows });
    }

    /// The answer to request `id`: the hints of its rows, replacing those shown. Dropped (false) when superseded or
    /// the text changed since the request.
    pub fn set_inlay_hints(
        &mut self,
        id: u64,
        hints: Vec<InlayHint>,
        cx: &mut Context<Self>,
    ) -> bool {
        let rows = match &self.inlays.pending {
            Some((p, v, rows)) if *p == id && v == self.editor.buffer().snapshot().version() => {
                rows.clone()
            }
            _ => return false,
        };
        if !self.inlays.enabled {
            return false;
        }
        self.inlays.pending = None;
        let buffer = self.editor.buffer();
        let mut stored: Vec<StoredHint> = hints
            .into_iter()
            .filter(|h| h.offset <= buffer.len() && !h.label.is_empty())
            .map(|h| StoredHint {
                anchor: if h.kind == InlayKind::Parameter {
                    buffer.anchor_after(h.offset)
                } else {
                    buffer.anchor_before(h.offset)
                },
                hint: h,
            })
            .collect();
        stored.sort_by_key(|h| h.hint.offset);
        self.inlays.hints = stored;
        self.inlays.covered = Some(rows);
        self.inlays.applied += 1;
        cx.notify();
        true
    }

    /// The hints shown, with their current offsets (they move with the text).
    pub fn inlay_hints(&self) -> Vec<(usize, InlayHint)> {
        let b = self.editor.buffer();
        self.inlays
            .hints
            .iter()
            .map(|h| (b.offset_for_anchor(&h.anchor), h.hint.clone()))
            .collect()
    }

    /// How many answers were applied.
    pub fn inlay_updates(&self) -> usize {
        self.inlays.applied
    }

    /// The hints of each row in `rows`, placed (only rows that have some).
    pub(crate) fn row_inlays(&self, rows: Range<u32>) -> Vec<(u32, RowInlays)> {
        if self.inlays.hints.is_empty() {
            return Vec::new();
        }
        let snapshot = self.editor.buffer().snapshot();
        let mut out: Vec<(u32, RowInlays)> = Vec::new();
        for h in &self.inlays.hints {
            let offset = snapshot.offset_for_anchor(&h.anchor);
            let p = snapshot.offset_to_point(offset);
            if !rows.contains(&p.row) {
                continue;
            }
            let mut text = String::new();
            if h.hint.padding_left {
                text.push(' ');
            }
            text.push_str(&h.hint.label);
            if h.hint.padding_right {
                text.push(' ');
            }
            let item = RowInlay {
                column: p.column as usize,
                text,
                caret_after: h.hint.kind == InlayKind::Parameter,
                id: h.hint.id,
            };
            match out.last_mut() {
                Some((r, inlays)) if *r == p.row => inlays.items.push(item),
                _ => out.push((p.row, RowInlays { items: vec![item] })),
            }
        }
        for (_, r) in &mut out {
            r.items.sort_by_key(|i| (i.column, !i.caret_after));
        }
        out.sort_by_key(|(r, _)| *r);
        out
    }

    /// The text changed: ask again [`SEMANTIC_DEBOUNCE`] after the last edit (the hints move with the text meanwhile).
    pub(crate) fn inlay_edited(&mut self, cx: &mut Context<Self>) {
        if !self.inlays.enabled {
            return;
        }
        self.inlays.stale = true;
        self.inlays.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(SEMANTIC_DEBOUNCE).await;
            this.update(cx, |v, cx| {
                v.inlays.debounce = None;
                if v.inlays.stale {
                    v.refresh_inlay_hints(cx);
                }
            })
            .ok();
        }));
    }

    /// The view scrolled: ask for the new window's hints when it left the covered rows.
    pub(crate) fn check_inlay_window(&mut self, cx: &mut Context<Self>) {
        if !self.inlays.enabled || self.inlays.debounce.is_some() {
            return;
        }
        let visible = self.visible_rows();
        let covered = match &self.inlays.pending {
            Some((_, _, rows)) => Some(rows.clone()),
            None => self.inlays.covered.clone(),
        };
        let inside = covered.is_some_and(|c| c.start <= visible.start && visible.end <= c.end);
        if !inside {
            self.refresh_inlay_hints(cx);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inlays(items: &[(usize, &str, bool)]) -> RowInlays {
        RowInlays {
            items: items
                .iter()
                .enumerate()
                .map(|(i, (c, t, after))| RowInlay {
                    column: *c,
                    text: (*t).into(),
                    caret_after: *after,
                    id: i as u64,
                })
                .collect(),
        }
    }

    #[test]
    fn hints_are_inserted_and_skipped_by_conversions() {
        // `Foo(1, x)` with a parameter hint before `1` and before `x`.
        let line = "Foo(1, x)";
        let r = inlays(&[(4, "count: ", true), (7, "name: ", true)]);
        let shown = r.display(line);
        assert_eq!(shown, "Foo(count: 1, name: x)");
        assert_eq!(r.ranges(line), [(4, 11, 0), (14, 20, 1)]);
        // The caret before `1` is drawn after the parameter hint; columns after it shift by the hint.
        assert_eq!(r.to_display(line, 4), 11);
        assert_eq!(r.to_display(line, 3), 3);
        assert_eq!(r.to_display(line, 7), 20);
        assert_eq!(r.to_display(line, 9), 22);
        // Back: positions inside a chip are its column; after it, the text.
        assert_eq!(r.from_display(line, 6), 4);
        assert_eq!(r.from_display(line, 11), 4);
        assert_eq!(r.from_display(line, 12), 5);
        assert_eq!(r.from_display(line, 21), 8);
        assert_eq!(r.hint_at(line, 5), Some(0));
        assert_eq!(r.hint_at(line, 12), None);
    }

    #[test]
    fn a_type_hint_keeps_the_caret_before_it_and_tabs_still_expand() {
        let line = "\tvar x = 1;";
        let r = inlays(&[(6, ": int", false)]);
        assert_eq!(r.display(line), "    var x: int = 1;");
        // The caret right after `x` stays before the hint; one more column jumps over it.
        assert_eq!(r.to_display(line, 6), 9);
        assert_eq!(r.to_display(line, 7), 15);
        assert_eq!(r.from_display(line, 9), 6);
        assert_eq!(r.from_display(line, 15), 7);
        assert_eq!(inlays(&[]).display(line), "    var x = 1;");
    }
}
