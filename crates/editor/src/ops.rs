//! Brief 0068's shared editing state and the helpers its operations are built on: rows touched by selections, and
//! [`Editor::step`], which applies edits as one undo step of their own and sets the selections that follow.
//!
//! The operations live in their own modules ([`crate::indent`], [`crate::autoclose`], [`crate::comments`],
//! [`crate::lines`], [`crate::box_selection`], [`crate::expand_selection`], [`crate::find`]); each is an
//! `impl Editor` block over this state.

use std::collections::VecDeque;
use std::ops::Range;
use std::time::Instant;

use text::{Anchor, Point, TransactionId};

use crate::box_selection::BoxState;
use crate::editor::{Editor, SelectionRange};
use crate::find::FindState;
use crate::indent::IndentSettings;
use crate::syntax::editing::{EditingConfig, PLAIN};

/// The Clipboard Ring's size (Visual Studio keeps 20).
pub const CLIPBOARD_RING_SIZE: usize = 20;

/// Brief 0068's per-editor state.
#[derive(Debug)]
pub(crate) struct OpsState {
    pub language: &'static EditingConfig,
    pub indent: IndentSettings,
    /// Closers the editor inserted after their openers (anchored after them), which typing the closer skips.
    pub autoclosed: Vec<Anchor>,
    /// Copied and cut texts, newest first.
    pub ring: VecDeque<String>,
    /// The last Cycle Clipboard Ring paste: its undo step and the ring index it pasted.
    pub ring_paste: Option<(TransactionId, usize)>,
    pub boxed: Option<BoxState>,
    /// The text of the last box copied, which pastes as a block.
    pub box_clip: Option<String>,
    /// Expand Selection's earlier selections, innermost last, and the selections the last step produced.
    pub expand_stack: Vec<Vec<SelectionRange>>,
    pub expand_last: Vec<SelectionRange>,
    pub find: FindState,
}

impl Default for OpsState {
    fn default() -> Self {
        Self {
            language: &PLAIN,
            indent: IndentSettings::default(),
            autoclosed: Vec::new(),
            ring: VecDeque::new(),
            ring_paste: None,
            boxed: None,
            box_clip: None,
            expand_stack: Vec::new(),
            expand_last: Vec::new(),
            find: FindState::default(),
        }
    }
}

impl Editor {
    /// Use language `editing`'s comment tokens, brackets and indent rules ([`crate::syntax::Language::editing`]).
    pub fn set_language_editing(&mut self, editing: &'static EditingConfig) {
        self.ops.language = editing;
    }

    /// The editing data in use (plain text until [`Editor::set_language_editing`]).
    pub fn language_editing(&self) -> &'static EditingConfig {
        self.ops.language
    }

    pub(crate) fn row_of(&self, offset: usize) -> u32 {
        self.buffer.offset_to_point(offset).row
    }

    pub(crate) fn line_start(&self, row: u32) -> usize {
        self.buffer.point_to_offset(Point::new(row, 0))
    }

    /// The end of row `row`'s text, before its line break.
    pub(crate) fn line_end(&self, row: u32) -> usize {
        self.buffer
            .point_to_offset(Point::new(row, self.buffer.line_len(row)))
    }

    /// The start of the row after `row`, or the end of the buffer for the last row.
    pub(crate) fn line_full_end(&self, row: u32) -> usize {
        if row + 1 < self.buffer.line_count() {
            self.line_start(row + 1)
        } else {
            self.buffer.len()
        }
    }

    /// The rows a selection touches: a selection that ends at the start of a row does not touch that row.
    pub(crate) fn touched_rows(&self, s: SelectionRange) -> (u32, u32) {
        let start = self.buffer.offset_to_point(s.start());
        let end = self.buffer.offset_to_point(s.end());
        let last = if !s.is_empty() && end.column == 0 && end.row > start.row {
            end.row - 1
        } else {
            end.row
        };
        (start.row, last)
    }

    /// The touched rows of every selection, merged into sorted, disjoint blocks (adjacent blocks stay apart).
    pub(crate) fn row_blocks(&self) -> Vec<(u32, u32)> {
        let mut blocks: Vec<(u32, u32)> = self
            .selections()
            .into_iter()
            .map(|s| self.touched_rows(s))
            .collect();
        blocks.sort();
        let mut merged: Vec<(u32, u32)> = Vec::with_capacity(blocks.len());
        for (a, b) in blocks {
            if let Some(last) = merged.last_mut()
                && a <= last.1
            {
                last.1 = last.1.max(b);
                continue;
            }
            merged.push((a, b));
        }
        merged
    }

    /// Every distinct touched row, sorted.
    pub(crate) fn touched_row_list(&self) -> Vec<u32> {
        self.row_blocks()
            .into_iter()
            .flat_map(|(a, b)| a..=b)
            .collect()
    }

    /// Apply `edits` (sorted, not overlapping, against the current text) as one undo step that never merges with
    /// typing, then set `selections` (offsets in the edited text; `None` keeps the anchored selections where the
    /// edits moved them). Returns whether the text changed. Selections are set even when no edit changes anything.
    pub(crate) fn step(
        &mut self,
        edits: Vec<(Range<usize>, String)>,
        selections: Option<(Vec<SelectionRange>, usize)>,
    ) -> bool {
        let edits: Vec<(Range<usize>, String)> = edits
            .into_iter()
            .filter(|(r, t)| !(r.is_empty() && t.is_empty()))
            .collect();
        debug_assert!(edits.windows(2).all(|w| w[0].0.end <= w[1].0.start));
        if edits.is_empty() {
            if let Some((sels, primary)) = selections {
                self.set_selections(sels, primary);
            }
            return false;
        }
        self.buffer.finalize_last_transaction();
        let before = self.selections.clone();
        let now = Instant::now();
        self.buffer.start_transaction_at(now);
        self.buffer.edit(edits);
        let id = self.buffer.end_transaction_at(now);
        self.buffer.finalize_last_transaction();
        match selections {
            Some((sels, primary)) => self.set_selections(sels, primary),
            None => self.normalize_selections(),
        }
        if let Some(id) = id {
            self.history.insert(id, (before, self.selections.clone()));
        }
        true
    }

    /// The leading spaces and tabs of row `row`.
    pub(crate) fn indentation_of(&self, row: u32) -> String {
        let line = self.buffer.line(row);
        line.chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect()
    }
}
