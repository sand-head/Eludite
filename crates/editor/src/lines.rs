//! Edit > Advanced's line and case commands (brief 0068): Duplicate, Move Selected Lines Up and Down, Delete Line,
//! Line Cut, Open Line Above and Below, Transpose Line, Make Uppercase and Lowercase, Delete Horizontal White Space,
//! and the Clipboard Ring. Each works on every selection and is one undo step of its own.

use std::ops::Range;

use crate::editor::{Editor, SelectionRange};
use crate::ops::CLIPBOARD_RING_SIZE;

impl Editor {
    /// Edit.Duplicate (Ctrl+D): a caret's line is duplicated below and the caret moves to the copy; a selection's
    /// text is duplicated after it and the copy is selected.
    pub fn duplicate(&mut self) -> bool {
        let sels = self.selections();
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        // For each selection: the index of its edit and its offsets relative to the edited text.
        let mut placed: Vec<(usize, usize, usize, bool)> = Vec::new();
        let mut done_rows: Vec<u32> = Vec::new();
        for s in &sels {
            if s.is_empty() {
                let row = self.row_of(s.head);
                let line = self.buffer.line(row);
                if !done_rows.contains(&row) {
                    done_rows.push(row);
                    let at = self.line_end(row);
                    edits.push((at..at, format!("\n{line}")));
                }
                let shift = line.len() + 1;
                placed.push((edits.len() - 1, s.tail + shift, s.head + shift, false));
            } else {
                let text = self.buffer.text_for_range(s.range());
                let n = text.len();
                edits.push((s.end()..s.end(), text));
                placed.push((edits.len() - 1, s.end(), s.end() + n, s.reversed()));
            }
        }
        // Deltas before each edit, in the order they were pushed (sorted, since selections are).
        let mut before = Vec::with_capacity(edits.len());
        let mut delta = 0usize;
        for (_, t) in &edits {
            before.push(delta);
            delta += t.len();
        }
        let new: Vec<SelectionRange> = placed
            .into_iter()
            .map(|(ix, a, b, reversed)| {
                let d = before[ix];
                if reversed {
                    SelectionRange {
                        tail: b + d,
                        head: a + d,
                    }
                } else {
                    SelectionRange {
                        tail: a + d,
                        head: b + d,
                    }
                }
            })
            .collect();
        let primary = self.primary;
        self.step(edits, Some((new, primary)))
    }

    /// The touched rows of every selection, with blocks that touch merged (for moving lines).
    fn move_blocks(&self) -> Vec<(u32, u32)> {
        let mut merged: Vec<(u32, u32)> = Vec::new();
        for (a, b) in self.row_blocks() {
            if let Some(last) = merged.last_mut()
                && a <= last.1 + 1
            {
                last.1 = last.1.max(b);
                continue;
            }
            merged.push((a, b));
        }
        merged
    }

    /// Edit.MoveSelectedLinesUp / Down (Alt+Up, Alt+Down): each block of touched lines swaps with the line above or
    /// below; the selections move with their text. A block at the top (bottom) stays.
    pub fn move_lines(&mut self, up: bool) -> bool {
        let blocks = self.move_blocks();
        self.move_row_blocks(&blocks, up)
    }

    fn move_row_blocks(&mut self, blocks: &[(u32, u32)], up: bool) -> bool {
        let last_row = self.buffer.line_count() - 1;
        let sels = self.selections();
        let mut edits = Vec::new();
        // (rows, shift) for selections in each moved block.
        let mut shifts: Vec<(u32, u32, isize)> = Vec::new();
        for &(a, b) in blocks {
            if (up && a == 0) || (!up && b >= last_row) {
                continue;
            }
            let (other, first, last) = if up {
                (a - 1, a - 1, b)
            } else {
                (b + 1, a, b + 1)
            };
            let start = self.line_start(first);
            let end = self.line_full_end(last);
            let trailing = end > self.line_end(last);
            let block = self
                .buffer
                .text_for_range(self.line_start(a)..self.line_end(b));
            let neighbor = self.buffer.line(other);
            let mut text = if up {
                format!("{block}\n{neighbor}")
            } else {
                format!("{neighbor}\n{block}")
            };
            if trailing {
                text.push('\n');
            }
            edits.push((start..end, text));
            let by = neighbor.len() as isize + 1;
            shifts.push((a, b, if up { -by } else { by }));
        }
        if edits.is_empty() {
            return false;
        }
        // Edits keep each range's length, so offsets outside moved blocks stay.
        let map = |offset: usize| -> usize {
            let row = self.row_of(offset);
            shifts
                .iter()
                .find(|(a, b, _)| row >= *a && row <= *b)
                .map_or(offset, |(_, _, d)| (offset as isize + d) as usize)
        };
        let new: Vec<SelectionRange> = sels
            .iter()
            .map(|s| SelectionRange {
                tail: map(s.tail),
                head: map(s.head),
            })
            .collect();
        let primary = self.primary;
        self.step(edits, Some((new, primary)))
    }

    /// Edit.LineTranspose (Shift+Alt+T): each caret's line swaps with the line below, and the caret follows it.
    pub fn transpose_line(&mut self) -> bool {
        let mut rows: Vec<u32> = self
            .selections()
            .iter()
            .map(|s| self.row_of(s.head))
            .collect();
        rows.dedup();
        let blocks: Vec<(u32, u32)> = rows.into_iter().map(|r| (r, r)).collect();
        self.move_row_blocks(&blocks, false)
    }

    /// The ranges of the touched lines' blocks, each with its line break (the line break before, for a block ending
    /// at the last line).
    fn line_ranges(&self) -> Vec<(Range<usize>, u32)> {
        let last_row = self.buffer.line_count() - 1;
        self.row_blocks()
            .into_iter()
            .map(|(a, b)| {
                if b == last_row && a > 0 {
                    (self.line_end(a - 1)..self.buffer.len(), a)
                } else {
                    (self.line_start(a)..self.line_full_end(b), a)
                }
            })
            .collect()
    }

    /// Edit.LineDelete (Ctrl+Shift+L): the touched lines go; carets keep their column on the next line.
    pub fn delete_line(&mut self) -> bool {
        self.delete_lines()
    }

    /// Edit.LineCut (Ctrl+L): the touched lines go and their text (with line breaks) is returned for the clipboard.
    pub fn line_cut(&mut self) -> String {
        let text: String = self
            .row_blocks()
            .into_iter()
            .map(|(a, b)| {
                let mut t = self
                    .buffer
                    .text_for_range(self.line_start(a)..self.line_full_end(b));
                if !t.ends_with('\n') {
                    t.push('\n');
                }
                t
            })
            .collect();
        self.delete_lines();
        self.remember_clipboard(&text);
        text
    }

    fn delete_lines(&mut self) -> bool {
        let primary = self.primary_selection();
        let column = if primary.is_empty() {
            self.buffer.offset_to_point(primary.head).column
        } else {
            0
        };
        let ranges = self.line_ranges();
        let mut delta = 0usize;
        let mut carets = Vec::new();
        for (r, _) in &ranges {
            carets.push(r.start - delta);
            delta += r.len();
        }
        let edits: Vec<_> = ranges
            .into_iter()
            .map(|(r, _)| (r, String::new()))
            .collect();
        let changed = self.step(edits, None);
        // Each caret at its column on the line that took the deleted lines' place.
        let new: Vec<SelectionRange> = carets
            .into_iter()
            .map(|at| {
                let at = at.min(self.buffer.len());
                let p = self.buffer.offset_to_point(at);
                let col = column.min(self.buffer.line_len(p.row));
                SelectionRange::caret(if p.column > 0 {
                    at
                } else {
                    self.line_start(p.row) + col as usize
                })
            })
            .collect();
        let n = new.len();
        self.set_selections(new, n - 1);
        changed
    }

    /// Edit.LineOpenAbove (Ctrl+Enter) and Edit.LineOpenBelow (Ctrl+Shift+Enter): a new line with the line's
    /// indentation (one level more below a line that opens a block, in Smart mode), and the caret on it.
    pub fn open_line(&mut self, above: bool) -> bool {
        let mut rows: Vec<u32> = self
            .selections()
            .iter()
            .map(|s| self.row_of(s.head))
            .collect();
        rows.dedup();
        let settings = self.ops.indent;
        let mut edits = Vec::new();
        let mut carets = Vec::new();
        let mut delta = 0usize;
        for row in rows {
            let indent = self.indentation_of(row);
            let line = self.buffer.line(row);
            let (at, text, caret) = if above {
                let at = self.line_start(row);
                (at, format!("{indent}\n"), indent.len())
            } else {
                let deeper = settings.auto_indent == crate::indent::AutoIndent::Smart
                    && crate::indent::smart_indent_after(
                        self.ops.language,
                        line.trim_end(),
                        &line,
                        &settings,
                    );
                let indent = if deeper {
                    format!("{indent}{}", settings.unit())
                } else {
                    indent
                };
                let at = self.line_end(row);
                let text = format!("\n{indent}");
                let n = text.len();
                (at, text, n)
            };
            carets.push(SelectionRange::caret(at + delta + caret));
            delta += text.len();
            edits.push((at..at, text));
        }
        let n = carets.len();
        self.step(edits, Some((carets, n - 1)))
    }

    /// Edit.MakeUppercase (Ctrl+Shift+U) and Edit.MakeLowercase (Ctrl+U): the selections' text, or the character
    /// after each caret (which then moves past it).
    pub fn change_case(&mut self, upper: bool) -> bool {
        let snapshot = self.buffer.snapshot().clone();
        let sels = self.selections();
        let mut edits = Vec::new();
        let mut new = Vec::new();
        let mut delta: isize = 0;
        for s in &sels {
            let range = if s.is_empty() {
                match snapshot.chars_at(s.head).next() {
                    Some(c) if c != '\n' => s.head..s.head + c.len_utf8(),
                    _ => {
                        new.push(SelectionRange::caret((s.head as isize + delta) as usize));
                        continue;
                    }
                }
            } else {
                s.range()
            };
            let text = self.buffer.text_for_range(range.clone());
            let mapped = if upper {
                text.to_uppercase()
            } else {
                text.to_lowercase()
            };
            let start = (range.start as isize + delta) as usize;
            let end = start + mapped.len();
            new.push(if s.is_empty() {
                SelectionRange::caret(end)
            } else if s.reversed() {
                SelectionRange {
                    tail: end,
                    head: start,
                }
            } else {
                SelectionRange {
                    tail: start,
                    head: end,
                }
            });
            delta += mapped.len() as isize - range.len() as isize;
            if mapped != text {
                edits.push((range, mapped));
            }
        }
        let primary = self.primary;
        self.step(edits, Some((new, primary)))
    }

    /// Edit.DeleteHorizontalWhiteSpace (Ctrl+K, Ctrl+\): the spaces and tabs around each caret go; in a selection,
    /// each run of them after a line's first character becomes one space.
    pub fn delete_horizontal_white_space(&mut self) -> bool {
        let is_ws = |c: char| c == ' ' || c == '\t';
        let mut edits: Vec<(Range<usize>, String)> = Vec::new();
        for s in self.selections() {
            if s.is_empty() {
                let row = self.row_of(s.head);
                let start = self.line_start(row);
                let line = self.buffer.line(row);
                let col = s.head - start;
                let left = line[..col].len() - line[..col].trim_end_matches(is_ws).len();
                let right = line[col..].len() - line[col..].trim_start_matches(is_ws).len();
                if left + right > 0 {
                    edits.push((s.head - left..s.head + right, String::new()));
                }
                continue;
            }
            let (a, b) = self.touched_rows(s);
            for row in a..=b {
                let start = self.line_start(row);
                let line = self.buffer.line(row);
                let indent = line.len() - line.trim_start_matches(is_ws).len();
                let from = s.start().max(start + indent).saturating_sub(start);
                let to = s.end().min(start + line.len()).saturating_sub(start);
                let mut i = from;
                let bytes = line.as_bytes();
                while i < to {
                    if bytes[i] == b' ' || bytes[i] == b'\t' {
                        let mut j = i;
                        while j < to && (bytes[j] == b' ' || bytes[j] == b'\t') {
                            j += 1;
                        }
                        let at_end = j == line.len();
                        if at_end {
                            edits.push((start + i..start + j, String::new()));
                        } else if j - i > 1 || bytes[i] == b'\t' {
                            edits.push((start + i..start + j, " ".into()));
                        }
                        i = j;
                    } else {
                        i += 1;
                    }
                }
            }
        }
        edits.sort_by_key(|(r, _)| r.start);
        edits.dedup_by(|b, a| b.0.start < a.0.end);
        self.step(edits, None)
    }

    /// Edit.GoTo's line: the caret at the first non-blank character of 1-based `line` (clamped to the document).
    pub fn go_to_line(&mut self, line: u32) {
        let row = line.saturating_sub(1).min(self.buffer.line_count() - 1);
        let indent = self.indentation_of(row).len();
        self.set_caret(self.line_start(row) + indent);
    }

    // ----- the Clipboard Ring -----

    /// Remember `text` as the newest Clipboard Ring entry (copy and cut call it).
    pub fn remember_clipboard(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let ring = &mut self.ops.ring;
        if ring.front().map(String::as_str) != Some(text) {
            ring.retain(|t| t != text);
            ring.push_front(text.to_owned());
            ring.truncate(CLIPBOARD_RING_SIZE);
        }
    }

    /// The Clipboard Ring, newest first.
    pub fn clipboard_ring(&self) -> impl Iterator<Item = &str> {
        self.ops.ring.iter().map(String::as_str)
    }

    /// Copy: the selected text (or the caret lines), remembered in the Clipboard Ring.
    pub fn copy(&mut self) -> String {
        let text = self.selected_text();
        self.remember_clipboard(&text);
        text
    }

    /// Edit.CycleClipboardRing (Ctrl+Shift+V): paste the newest ring entry; right after such a paste, replace it with
    /// the next older entry (wrapping). Returns the text pasted.
    pub fn paste_ring(&mut self) -> Option<String> {
        if self.ops.ring.is_empty() {
            return None;
        }
        let again = self
            .ops
            .ring_paste
            .filter(|(id, _)| self.buffer.last_transaction() == Some(*id));
        let index = match again {
            Some((_, ix)) if self.undo() => (ix + 1) % self.ops.ring.len(),
            _ => 0,
        };
        let text = self.ops.ring[index].clone();
        self.buffer.finalize_last_transaction();
        self.paste(&text);
        self.buffer.finalize_last_transaction();
        self.ops.ring_paste = self.buffer.last_transaction().map(|id| (id, index));
        Some(text)
    }
}

#[cfg(test)]
mod tests {
    use crate::indent::tests::{editor, lang, marked};

    #[test]
    fn duplicate_lines_and_selections_with_several_carets() {
        let mut e = editor("a|b\ncd\ne|f");
        assert!(e.duplicate());
        assert_eq!(marked(&e), "ab\na|b\ncd\nef\ne|f");
        assert!(e.undo());
        assert_eq!(marked(&e), "a|b\ncd\ne|f");
        let mut e = editor("x [ab] y [c]");
        e.duplicate();
        assert_eq!(marked(&e), "x ab[ab] y c[c]");
    }

    #[test]
    fn move_lines_with_several_carets() {
        let mut e = editor("one\ntw|o\nthree\nfo|ur\nfive");
        assert!(e.move_lines(true));
        assert_eq!(marked(&e), "tw|o\none\nfo|ur\nthree\nfive");
        // At the top, the first block stays; the other moves.
        assert!(e.move_lines(true));
        assert_eq!(marked(&e), "tw|o\nfo|ur\none\nthree\nfive");
        assert!(e.move_lines(false));
        assert_eq!(marked(&e), "one\ntw|o\nfo|ur\nthree\nfive");
        let mut e = editor("a\n[b\nc]\nd");
        e.move_lines(false);
        assert_eq!(marked(&e), "a\nd\n[b\nc]");
        assert!(!e.move_lines(false));
        e.move_lines(true);
        assert_eq!(marked(&e), "a\n[b\nc]\nd");
        assert!(e.undo());
        assert_eq!(marked(&e), "a\nd\n[b\nc]");
    }

    #[test]
    fn delete_line_line_cut_and_transpose() {
        let mut e = editor("one\ntw|o\nthree");
        assert!(e.delete_line());
        assert_eq!(marked(&e), "one\nth|ree");
        let mut e = editor("one\n[two\nthr]ee\nfour");
        assert_eq!(e.line_cut(), "two\nthree\n");
        assert_eq!(marked(&e), "one\n|four");
        assert_eq!(e.clipboard_ring().next(), Some("two\nthree\n"));
        let mut e = editor("one\ntwo|");
        assert_eq!(e.line_cut(), "two\n");
        assert_eq!(marked(&e), "one|");
        let mut e = editor("o|ne\ntwo\nthree");
        assert!(e.transpose_line());
        assert_eq!(marked(&e), "two\no|ne\nthree");
    }

    #[test]
    fn open_line_above_and_below() {
        let mut e = lang("    if (x) {|\n    }", "csharp");
        e.open_line(false);
        assert_eq!(marked(&e), "    if (x) {\n        |\n    }");
        let mut e = lang("a\n    b|", "csharp");
        e.open_line(true);
        assert_eq!(marked(&e), "a\n    |\n    b");
    }

    #[test]
    fn case_commands() {
        let mut e = editor("[hello] wörld|");
        assert!(e.change_case(true));
        assert_eq!(marked(&e), "[HELLO] wörld|");
        let mut e = editor("|élan [ÉCOLE]");
        e.change_case(false);
        assert_eq!(e.text(), "élan école");
        e.change_case(true);
        assert_eq!(marked(&e), "éL|an [ÉCOLE]");
    }

    #[test]
    fn horizontal_white_space() {
        let mut e = editor("a   |  b\nc\t|d");
        assert!(e.delete_horizontal_white_space());
        assert_eq!(marked(&e), "a|b\nc|d");
        let mut e = editor("    [x   =  1;   ]");
        e.delete_horizontal_white_space();
        assert_eq!(e.text(), "    x = 1;");
    }

    #[test]
    fn go_to_line_lands_on_the_first_character() {
        let mut e = editor("a\n    b\nc|");
        e.go_to_line(2);
        assert_eq!(marked(&e), "a\n    |b\nc");
        e.go_to_line(99);
        assert_eq!(marked(&e), "a\n    b\n|c");
        e.go_to_line(0);
        assert_eq!(marked(&e), "|a\n    b\nc");
    }

    #[test]
    fn the_clipboard_ring_cycles() {
        let mut e = editor("[one] two three");
        e.copy();
        e.set_selections(vec![crate::SelectionRange { tail: 4, head: 7 }], 0);
        e.copy();
        e.set_caret(e.buffer().len());
        assert_eq!(e.paste_ring().as_deref(), Some("two"));
        assert_eq!(e.text(), "one two threetwo");
        assert_eq!(e.paste_ring().as_deref(), Some("one"));
        assert_eq!(e.text(), "one two threeone");
        assert_eq!(e.paste_ring().as_deref(), Some("two"));
        assert_eq!(e.text(), "one two threetwo");
        e.type_text("!");
        assert_eq!(e.paste_ring().as_deref(), Some("two"));
        assert_eq!(e.text(), "one two threetwo!two");
    }
}
