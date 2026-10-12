//! Box (rectangular) selection and the multi-caret commands (brief 0068).
//!
//! A box is two corners, each a row and a visual column, columns past a line's end included (virtual space). It is
//! held as one selection per row, so typing, Backspace, Delete and copy work per row as for any multi-selection; a row
//! shorter than the box's left column gets a caret at its end, and typing there pads the row with spaces to the
//! column first. A box copied is remembered and pastes as a block: its rows go on consecutive rows at the caret's
//! column. Any selection change other than a box command ends the box.

use crate::display::{byte_for_visual_column, visual_column};
use crate::editor::{Editor, SelectionRange};

/// A box selection's corners: (row, visual column).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct BoxState {
    pub anchor: (u32, u32),
    pub head: (u32, u32),
}

impl BoxState {
    fn left(&self) -> u32 {
        self.anchor.1.min(self.head.1)
    }

    fn right(&self) -> u32 {
        self.anchor.1.max(self.head.1)
    }

    fn rows(&self) -> std::ops::RangeInclusive<u32> {
        self.anchor.0.min(self.head.0)..=self.anchor.0.max(self.head.0)
    }
}

/// The visual width of a line (tabs to their stops).
fn line_width(line: &str) -> u32 {
    visual_column(line, line.len())
}

impl Editor {
    /// The box selection, if the selections are one.
    pub fn box_selection(&self) -> Option<BoxState> {
        self.ops.boxed
    }

    /// Select the box between `anchor` and `head` ((row, visual column) each; rows clamped to the buffer).
    pub fn select_box(&mut self, anchor: (u32, u32), head: (u32, u32)) {
        let last = self.buffer.line_count() - 1;
        let state = BoxState {
            anchor: (anchor.0.min(last), anchor.1),
            head: (head.0.min(last), head.1),
        };
        let reversed = state.head.1 < state.anchor.1;
        let mut ranges = Vec::new();
        let mut primary = 0;
        for row in state.rows() {
            let line = self.buffer.line(row);
            let start = self.line_start(row);
            let l = start + byte_for_visual_column(&line, state.left());
            let r = start + byte_for_visual_column(&line, state.right());
            if row == state.head.0 {
                primary = ranges.len();
            }
            ranges.push(if reversed {
                SelectionRange { tail: r, head: l }
            } else {
                SelectionRange { tail: l, head: r }
            });
        }
        self.set_selections(ranges, primary);
        self.ops.boxed = Some(state);
    }

    /// Shift+Alt+arrows: grow or shrink the box by `rows` and `columns` from its head (starting a box at the primary
    /// caret). Shift+Alt+Up and Down from a caret are carets on consecutive rows.
    pub fn extend_box(&mut self, rows: i64, columns: i64) {
        let state = self.ops.boxed.unwrap_or_else(|| {
            let p = self.primary_head();
            let line = self.buffer.line(p.row);
            let col = visual_column(&line, p.column as usize);
            BoxState {
                anchor: (p.row, col),
                head: (p.row, col),
            }
        });
        let last = self.buffer.line_count() as i64 - 1;
        let row = (state.head.0 as i64 + rows).clamp(0, last) as u32;
        let col = (state.head.1 as i64 + columns).max(0) as u32;
        self.select_box(state.anchor, (row, col));
    }

    /// Pad the box's rows that end left of its left column with spaces, as one step that the typing after it joins.
    pub(crate) fn pad_box_rows(&mut self) {
        let Some(state) = self.ops.boxed else {
            return;
        };
        let mut edits = Vec::new();
        for row in state.rows() {
            let line = self.buffer.line(row);
            let width = line_width(&line);
            if width < state.left() {
                let at = self.line_end(row);
                edits.push((at..at, " ".repeat((state.left() - width) as usize)));
            }
        }
        if edits.is_empty() {
            return;
        }
        self.transact(|this| {
            this.buffer.edit(edits);
        });
        self.select_box(state.anchor, state.head);
    }

    /// The box's text: each row's part, one per line, empty rows included.
    pub fn box_text(&self) -> Option<String> {
        self.ops.boxed?;
        Some(
            self.selections()
                .iter()
                .map(|s| self.buffer.text_for_range(s.range()))
                .collect::<Vec<_>>()
                .join("\n"),
        )
    }

    /// Copy, with a box's rows remembered as a block for [`Editor::paste_text`].
    pub fn copy_selection(&mut self) -> String {
        match self.box_text() {
            Some(text) => {
                self.ops.box_clip = Some(text.clone());
                self.remember_clipboard(&text);
                text
            }
            None => {
                self.ops.box_clip = None;
                self.copy()
            }
        }
    }

    /// Paste as the keyboard does: a block copied from a box goes on consecutive rows at a single caret's column;
    /// anything else is [`Editor::paste`].
    pub fn paste_text(&mut self, text: &str) {
        self.ops.ring_paste = None;
        let block = self.ops.box_clip.as_deref() == Some(text)
            && self.selections().len() == 1
            && text.contains('\n');
        if !block {
            if self.ops.boxed.is_some() && self.selections().len() > 1 {
                self.pad_box_rows();
            }
            self.paste(text);
            return;
        }
        let caret = self.primary_selection();
        let p = self.buffer.offset_to_point(caret.start());
        let col = visual_column(&self.buffer.line(p.row), p.column as usize);
        let last = self.buffer.line_count() - 1;
        let mut edits = Vec::new();
        let mut end_caret = 0;
        let mut delta = 0usize;
        let rows: Vec<&str> = text.split('\n').collect();
        for (i, part) in rows.iter().enumerate() {
            let row = p.row + i as u32;
            let (at, inserted) = if row <= last {
                let line = self.buffer.line(row);
                let width = line_width(&line);
                if width < col {
                    let pad = " ".repeat((col - width) as usize);
                    (self.line_end(row), format!("{pad}{part}"))
                } else {
                    (
                        self.line_start(row) + byte_for_visual_column(&line, col),
                        (*part).to_owned(),
                    )
                }
            } else {
                (
                    self.buffer.len(),
                    format!("\n{}{part}", " ".repeat(col as usize)),
                )
            };
            // Rows past the end all append at the end, in order.
            let at_final = at + delta;
            delta += inserted.len();
            end_caret = at_final + inserted.len();
            edits.push((at..at, inserted));
        }
        let range = caret.range();
        if !range.is_empty() {
            edits.insert(0, (range.clone(), String::new()));
            end_caret -= range.len();
        }
        // Appends at the end share an offset; merge them so the edits stay ordered.
        let mut merged: Vec<(std::ops::Range<usize>, String)> = Vec::new();
        for (r, t) in edits {
            if let Some(last) = merged.last_mut()
                && last.0.start == r.start
                && r.is_empty()
                && last.0.is_empty()
            {
                last.1.push_str(&t);
                continue;
            }
            merged.push((r, t));
        }
        self.step(merged, Some((vec![SelectionRange::caret(end_caret)], 0)));
    }

    /// Edit.InsertCaretsAtAllMatching (Shift+Alt+;): select every occurrence of the primary selection's text (the word
    /// at a caret).
    pub fn select_all_occurrences(&mut self) {
        let primary = self.primary_selection();
        let range = if primary.is_empty() {
            self.word_at(primary.head)
        } else {
            primary.range()
        };
        if range.is_empty() {
            return;
        }
        let needle = self.buffer.text_for_range(range.clone());
        let len = self.buffer.len();
        let found = self.buffer.find_in_range(&needle, 0..len, true);
        let primary_ix = found.iter().position(|m| *m == range).unwrap_or(0);
        let ranges = found
            .into_iter()
            .map(|m| SelectionRange {
                tail: m.start,
                head: m.end,
            })
            .collect::<Vec<_>>();
        if !ranges.is_empty() {
            self.set_selections(ranges, primary_ix);
        }
    }

    /// Edit.RemoveLastSecondaryCaret (Shift+Alt+,): drop the most recently added selection (the primary) when there
    /// are several.
    pub fn remove_last_caret(&mut self) {
        let mut ranges = self.selections();
        if ranges.len() < 2 {
            return;
        }
        ranges.remove(self.primary);
        let p = self.primary.saturating_sub(1).min(ranges.len() - 1);
        self.set_selections(ranges, p);
    }

    /// Edit.InsertNextMatchingCaret's skip (Ctrl+Shift+Alt+.): the newest selection moves to the next occurrence of
    /// its text.
    pub fn skip_occurrence(&mut self) {
        let primary = self.primary_selection();
        if primary.is_empty() || self.selections().len() < 2 {
            self.select_next_occurrence();
            return;
        }
        let ix = self.primary;
        self.select_next_occurrence();
        if self.selections().len() > 1 && self.primary_selection() != primary {
            let mut ranges = self.selections();
            if let Some(old) = ranges.iter().position(|r| *r == primary) {
                ranges.remove(old);
                let p = ranges
                    .iter()
                    .position(|r| *r == self.primary_selection())
                    .unwrap_or(ix.min(ranges.len() - 1));
                self.set_selections(ranges, p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::indent::tests::{editor, marked};

    #[test]
    fn a_box_types_on_every_row_with_virtual_space() {
        let mut e = editor("abcdef\nab\nabcdef");
        e.select_box((0, 4), (2, 4));
        assert_eq!(e.selections().len(), 3);
        e.type_text("X");
        assert_eq!(e.text(), "abcdXef\nab  X\nabcdXef");
        assert!(e.undo());
        assert_eq!(e.text(), "abcdef\nab\nabcdef");
    }

    #[test]
    fn a_box_selects_columns_and_replaces_them() {
        let mut e = editor("abcdef\nab\nabcdef");
        e.select_box((0, 1), (2, 3));
        assert_eq!(marked(&e), "a[bc]def\na[b]\na[bc]def");
        assert_eq!(e.box_text().as_deref(), Some("bc\nb\nbc"));
        e.type_text("-");
        assert_eq!(e.text(), "a-def\na-\na-def");
    }

    #[test]
    fn shift_alt_arrows_grow_the_box() {
        let mut e = editor("ab|cd\nabcd\nabcd");
        e.extend_box(1, 0);
        e.extend_box(1, 0);
        assert_eq!(marked(&e), "ab|cd\nab|cd\nab|cd");
        e.extend_box(0, 1);
        assert_eq!(marked(&e), "ab[c]d\nab[c]d\nab[c]d");
        e.extend_box(-1, 0);
        assert_eq!(marked(&e), "ab[c]d\nab[c]d\nabcd");
        e.move_right(false);
        assert!(e.box_selection().is_none());
    }

    #[test]
    fn a_box_copies_and_pastes_as_a_block() {
        let mut e = editor("1234\n5678\nxy");
        e.select_box((0, 1), (1, 3));
        let text = e.copy_selection();
        assert_eq!(text, "23\n67");
        e.set_caret(e.buffer().len());
        // At row 2 col 2: the block goes on rows 2 and 3 (added) at column 2.
        e.paste_text(&text);
        assert_eq!(e.text(), "1234\n5678\nxy23\n  67");
        assert!(e.undo());
        assert_eq!(e.text(), "1234\n5678\nxy");
        let mut e = editor("ab\nabcdef\n|");
        e.ops.box_clip = Some("X\nY".into());
        e.set_caret(1);
        e.paste_text("X\nY");
        assert_eq!(e.text(), "aXb\naYbcdef\n");
    }

    #[test]
    fn multi_caret_commands() {
        let mut e = editor("var x|1 = x1 + x1;");
        e.select_all_occurrences();
        assert_eq!(marked(&e), "var [x1] = [x1] + [x1];");
        e.remove_last_caret();
        assert_eq!(e.selections().len(), 2);
        let mut e = editor("a [b] b b");
        e.select_next_occurrence();
        assert_eq!(marked(&e), "a [b] [b] b");
        e.skip_occurrence();
        assert_eq!(marked(&e), "a [b] b [b]");
    }
}
