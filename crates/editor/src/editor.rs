//! The editing model: a buffer, its selections and the operations on them.
//! No GPUI here; `EditorView` maps input to these calls.

use std::collections::HashMap;
use std::ops::Range;
use std::time::Instant;

use text::{Anchor, Point, TransactionId};

use crate::buffer::Buffer;
use crate::display::{TAB_SIZE, byte_for_visual_column, visual_column};

/// A selection stored as anchors, so it follows edits made by anyone.
/// `tail == head` is a caret.
#[derive(Clone, Debug)]
pub struct Selection {
    pub tail: Anchor,
    pub head: Anchor,
    /// Visual column kept across vertical moves through shorter lines.
    pub goal_column: Option<u32>,
}

/// A selection resolved to byte offsets.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SelectionRange {
    pub tail: usize,
    pub head: usize,
}

impl SelectionRange {
    pub fn caret(offset: usize) -> Self {
        Self {
            tail: offset,
            head: offset,
        }
    }

    pub fn start(&self) -> usize {
        self.tail.min(self.head)
    }

    pub fn end(&self) -> usize {
        self.tail.max(self.head)
    }

    pub fn range(&self) -> Range<usize> {
        self.start()..self.end()
    }

    pub fn is_empty(&self) -> bool {
        self.tail == self.head
    }

    pub fn reversed(&self) -> bool {
        self.head < self.tail
    }
}

/// Which kind of mouse click.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClickKind {
    /// Place a caret.
    Single,
    /// Select the word.
    Double,
    /// Select the line.
    Triple,
}

/// The find-in-buffer state.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct FindQuery {
    pub text: String,
    pub case_sensitive: bool,
}

/// A buffer plus multiple selections, with Visual Studio's editing behavior.
///
/// Selections are kept sorted and non-overlapping; overlapping ones merge
/// after every operation. The "primary" selection is the one most recently
/// added (the one the view scrolls to).
#[derive(Debug)]
pub struct Editor {
    buffer: Buffer,
    selections: Vec<Selection>,
    primary: usize,
    /// Selections before and after each undo step.
    history: HashMap<TransactionId, (Vec<Selection>, Vec<Selection>)>,
    find: FindQuery,
    /// Rows moved by Page Up / Page Down. The view keeps it in sync with
    /// the viewport.
    pub page_rows: u32,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CharClass {
    Space,
    Word,
    Punct,
    Newline,
}

fn class(c: char) -> CharClass {
    if c == '\n' {
        CharClass::Newline
    } else if c.is_whitespace() {
        CharClass::Space
    } else if c.is_alphanumeric() || c == '_' {
        CharClass::Word
    } else {
        CharClass::Punct
    }
}

impl Editor {
    pub fn new(buffer: Buffer) -> Self {
        let caret = buffer.anchor_before(0);
        Self {
            buffer,
            selections: vec![Selection {
                tail: caret,
                head: caret,
                goal_column: None,
            }],
            primary: 0,
            history: HashMap::new(),
            find: FindQuery::default(),
            page_rows: 40,
        }
    }

    pub fn buffer(&self) -> &Buffer {
        &self.buffer
    }

    /// Direct buffer access for programmatic edits (an agent, a formatter).
    /// Selections are anchored, so they follow such edits.
    pub fn buffer_mut(&mut self) -> &mut Buffer {
        &mut self.buffer
    }

    pub fn text(&self) -> String {
        self.buffer.text()
    }

    // ----- selections -----

    /// All selections resolved to offsets, sorted by position.
    pub fn selections(&self) -> Vec<SelectionRange> {
        self.selections
            .iter()
            .map(|s| SelectionRange {
                tail: self.buffer.offset_for_anchor(&s.tail),
                head: self.buffer.offset_for_anchor(&s.head),
            })
            .collect()
    }

    /// The most recently added selection.
    pub fn primary_selection(&self) -> SelectionRange {
        self.selections()[self.primary]
    }

    /// Position of the primary caret (the head of the primary selection).
    pub fn primary_head(&self) -> Point {
        self.buffer.offset_to_point(self.primary_selection().head)
    }

    /// Replace all selections. `primary` indexes into `ranges`.
    pub fn set_selections(&mut self, ranges: Vec<SelectionRange>, primary: usize) {
        let with_goal = ranges.into_iter().map(|r| (r, None)).collect();
        self.set_selections_with_goals(with_goal, primary);
    }

    /// A single caret at `offset`.
    pub fn set_caret(&mut self, offset: usize) {
        let offset = self.buffer.clip_offset(offset, text::Bias::Left);
        self.set_selections(vec![SelectionRange::caret(offset)], 0);
    }

    fn set_selections_with_goals(
        &mut self,
        ranges: Vec<(SelectionRange, Option<u32>)>,
        primary: usize,
    ) {
        assert!(!ranges.is_empty(), "an editor always has a selection");
        let len = self.buffer.len();
        let mut items: Vec<(usize, SelectionRange, Option<u32>)> = ranges
            .into_iter()
            .enumerate()
            .map(|(ix, (r, g))| {
                (
                    ix,
                    SelectionRange {
                        tail: r.tail.min(len),
                        head: r.head.min(len),
                    },
                    g,
                )
            })
            .collect();
        items.sort_by_key(|(_, r, _)| (r.start(), r.end()));
        // Merge overlapping or duplicate selections.
        let mut merged: Vec<(bool, SelectionRange, Option<u32>)> = Vec::new();
        for (ix, r, g) in items {
            let is_primary = ix == primary;
            if let Some((p, last, _)) = merged.last_mut()
                && (r.start() < last.end() || (r.start() == last.start() && r.end() == last.end()))
            {
                let start = last.start().min(r.start());
                let end = last.end().max(r.end());
                *last = if last.reversed() {
                    SelectionRange {
                        tail: end,
                        head: start,
                    }
                } else {
                    SelectionRange {
                        tail: start,
                        head: end,
                    }
                };
                *p |= is_primary;
                continue;
            }
            merged.push((is_primary, r, g));
        }
        self.primary = merged.iter().position(|(p, _, _)| *p).unwrap_or(0);
        self.selections = merged
            .into_iter()
            .map(|(_, r, g)| Selection {
                tail: self.buffer.anchor_before(r.tail),
                head: self.buffer.anchor_before(r.head),
                goal_column: g,
            })
            .collect();
    }

    pub fn select_all(&mut self) {
        let len = self.buffer.len();
        self.set_selections(vec![SelectionRange { tail: 0, head: len }], 0);
    }

    /// Collapse to the primary selection's caret (Escape).
    pub fn collapse_to_primary(&mut self) {
        let head = self.primary_selection().head;
        self.set_selections(vec![SelectionRange::caret(head)], 0);
    }

    /// The selected text of every selection, joined by line breaks.
    /// With no non-empty selection, the caret lines (Visual Studio copies the
    /// whole line).
    pub fn selected_text(&self) -> String {
        let sels = self.selections();
        if sels.iter().all(|s| s.is_empty()) {
            let mut out = String::new();
            for s in sels {
                let row = self.buffer.offset_to_point(s.head).row;
                out.push_str(&self.buffer.line(row));
                out.push('\n');
            }
            return out;
        }
        sels.iter()
            .filter(|s| !s.is_empty())
            .map(|s| self.buffer.text_for_range(s.range()))
            .collect::<Vec<_>>()
            .join("\n")
    }

    // ----- editing -----

    /// Run `f` as one undo step, remembering selections before and after.
    fn transact(&mut self, f: impl FnOnce(&mut Self)) {
        let before = self.selections.clone();
        let now = Instant::now();
        self.buffer.start_transaction_at(now);
        f(self);
        if let Some(id) = self.buffer.end_transaction_at(now) {
            let after = self.selections.clone();
            self.history.entry(id).or_insert((before, Vec::new())).1 = after;
        }
    }

    /// Replace, for each selection, the range `f` returns with its text, as
    /// one undo step. Carets land at the end of each inserted text.
    fn edit_each(&mut self, f: impl Fn(&Self, SelectionRange) -> Option<(Range<usize>, String)>) {
        let mut edits: Vec<(Range<usize>, String)> = self
            .selections()
            .into_iter()
            .filter_map(|s| f(self, s))
            .collect();
        edits.sort_by_key(|(r, _)| r.start);
        // Drop edits that overlap an earlier one (two carets deleting the same character).
        let mut kept: Vec<(Range<usize>, String)> = Vec::with_capacity(edits.len());
        for (r, t) in edits {
            if let Some((last, _)) = kept.last()
                && r.start < last.end
            {
                continue;
            }
            kept.push((r, normalize_newlines(&t)));
        }
        if kept.iter().all(|(r, t)| r.is_empty() && t.is_empty()) {
            return;
        }
        let primary_start = self.primary_selection().start();
        let mut carets = Vec::with_capacity(kept.len());
        let mut primary = 0;
        let mut delta: isize = 0;
        for (ix, (r, t)) in kept.iter().enumerate() {
            let start = (r.start as isize + delta) as usize;
            carets.push((SelectionRange::caret(start + t.len()), None));
            if r.start <= primary_start {
                primary = ix;
            }
            delta += t.len() as isize - r.len() as isize;
        }
        self.transact(|this| {
            this.buffer.edit(kept);
            this.set_selections_with_goals(carets, primary);
        });
    }

    /// Replace each range with its text, all against the current text, as one undo step of its own (a workspace
    /// edit: rename, a code action, a completion's additional edits). Ranges must be sorted and must not overlap.
    /// The step never merges with the typing before or after it. Selections stay where they were in the text (they
    /// are anchors). Returns the step's id, or `None` when nothing changed.
    pub fn apply_edits(&mut self, edits: Vec<(Range<usize>, String)>) -> Option<TransactionId> {
        let edits: Vec<(Range<usize>, String)> = edits
            .into_iter()
            .filter(|(r, t)| !(r.is_empty() && t.is_empty()))
            .collect();
        if edits.is_empty() {
            return None;
        }
        debug_assert!(edits.windows(2).all(|w| w[0].0.end <= w[1].0.start));
        self.buffer.finalize_last_transaction();
        let before = self.selections.clone();
        let now = Instant::now();
        self.buffer.start_transaction_at(now);
        self.buffer.edit(edits);
        let id = self.buffer.end_transaction_at(now);
        self.buffer.finalize_last_transaction();
        self.normalize_selections();
        if let Some(id) = id {
            self.history.insert(id, (before, self.selections.clone()));
        }
        id
    }

    /// The most recent undo step, if any.
    pub fn last_transaction(&self) -> Option<TransactionId> {
        self.buffer.last_transaction()
    }

    /// Fold undo step `transaction` into `destination` (a completion's additional edits join its commit).
    pub fn merge_transactions(&mut self, transaction: TransactionId, destination: TransactionId) {
        if transaction != destination {
            self.buffer.merge_transactions(transaction, destination);
            self.history.remove(&transaction);
        }
    }

    /// Type `text` at every selection, replacing selected text.
    pub fn insert(&mut self, text: &str) {
        self.edit_each(|_, s| Some((s.range(), text.to_owned())));
    }

    /// Enter: a line break plus the current line's leading whitespace.
    pub fn newline(&mut self) {
        self.edit_each(|this, s| {
            let row = this.buffer.offset_to_point(s.start()).row;
            let line = this.buffer.line(row);
            let indent: String = line
                .chars()
                .take_while(|c| *c == ' ' || *c == '\t')
                .collect();
            Some((s.range(), format!("\n{indent}")))
        });
    }

    /// Tab: spaces to the next tab stop at every caret.
    pub fn tab(&mut self) {
        self.edit_each(|this, s| {
            let p = this.buffer.offset_to_point(s.start());
            let line = this.buffer.line(p.row);
            let col = visual_column(&line, p.column as usize);
            let n = TAB_SIZE - col % TAB_SIZE;
            Some((s.range(), " ".repeat(n as usize)))
        });
    }

    /// Backspace: delete the selection, or the character before the caret.
    pub fn backspace(&mut self) {
        self.edit_each(|this, s| {
            if !s.is_empty() {
                return Some((s.range(), String::new()));
            }
            let prev = this.prev_char_boundary(s.head)?;
            Some((prev..s.head, String::new()))
        });
    }

    /// Delete: delete the selection, or the character after the caret.
    pub fn delete(&mut self) {
        self.edit_each(|this, s| {
            if !s.is_empty() {
                return Some((s.range(), String::new()));
            }
            let next = this.next_char_boundary(s.head)?;
            Some((s.head..next, String::new()))
        });
    }

    /// Ctrl+Backspace: delete to the start of the word.
    pub fn delete_word_left(&mut self) {
        self.edit_each(|this, s| {
            if !s.is_empty() {
                return Some((s.range(), String::new()));
            }
            let to = this.word_left(s.head);
            (to < s.head).then(|| (to..s.head, String::new()))
        });
    }

    /// Ctrl+Delete: delete to the start of the next word.
    pub fn delete_word_right(&mut self) {
        self.edit_each(|this, s| {
            if !s.is_empty() {
                return Some((s.range(), String::new()));
            }
            let to = this.word_right(s.head);
            (to > s.head).then(|| (s.head..to, String::new()))
        });
    }

    /// Paste. If there is one clipboard line per selection, each selection
    /// gets its own line; otherwise every selection gets the whole text.
    pub fn paste(&mut self, text: &str) {
        let text = normalize_newlines(text);
        let lines: Vec<&str> = text
            .strip_suffix('\n')
            .unwrap_or(&text)
            .split('\n')
            .collect();
        let sels = self.selections();
        if sels.len() > 1 && lines.len() == sels.len() {
            let by_start: HashMap<usize, String> = sels
                .iter()
                .zip(lines)
                .map(|(s, l)| (s.start(), l.to_owned()))
                .collect();
            self.edit_each(|_, s| Some((s.range(), by_start[&s.start()].clone())));
        } else {
            self.insert(&text);
        }
    }

    /// Cut: copy then delete; with no selection, the whole caret line.
    pub fn cut(&mut self) -> String {
        let text = self.selected_text();
        if self.selections().iter().all(|s| s.is_empty()) {
            self.edit_each(|this, s| {
                let row = this.buffer.offset_to_point(s.head).row;
                let start = this.buffer.point_to_offset(Point::new(row, 0));
                let end = if row + 1 < this.buffer.line_count() {
                    this.buffer.point_to_offset(Point::new(row + 1, 0))
                } else {
                    this.buffer.len()
                };
                Some((start..end, String::new()))
            });
        } else {
            self.edit_each(|_, s| Some((s.range(), String::new())));
        }
        text
    }

    pub fn undo(&mut self) -> bool {
        let Some(id) = self.buffer.undo() else {
            return false;
        };
        if let Some((before, _)) = self.history.get(&id) {
            self.selections = before.clone();
            self.primary = self.primary.min(self.selections.len() - 1);
        }
        self.normalize_selections();
        true
    }

    pub fn redo(&mut self) -> bool {
        let Some(id) = self.buffer.redo() else {
            return false;
        };
        if let Some((_, after)) = self.history.get(&id)
            && !after.is_empty()
        {
            self.selections = after.clone();
            self.primary = self.primary.min(self.selections.len() - 1);
        }
        self.normalize_selections();
        true
    }

    fn normalize_selections(&mut self) {
        let sels = self.selections();
        self.set_selections(sels, self.primary);
    }

    // ----- movement -----

    fn prev_char_boundary(&self, offset: usize) -> Option<usize> {
        let c = self.buffer.snapshot().reversed_chars_at(offset).next()?;
        Some(offset - c.len_utf8())
    }

    fn next_char_boundary(&self, offset: usize) -> Option<usize> {
        let c = self.buffer.snapshot().chars_at(offset).next()?;
        Some(offset + c.len_utf8())
    }

    /// Start of the word at or before `offset` (Ctrl+Left).
    pub fn word_left(&self, offset: usize) -> usize {
        let snapshot = self.buffer.snapshot();
        let mut chars = snapshot.reversed_chars_at(offset).peekable();
        let mut pos = offset;
        // A line break is a stop of its own.
        if let Some('\n') = chars.peek() {
            return pos - 1;
        }
        while let Some(&c) = chars.peek() {
            if class(c) != CharClass::Space {
                break;
            }
            pos -= c.len_utf8();
            chars.next();
        }
        if let Some(&c) = chars.peek() {
            let k = class(c);
            if k == CharClass::Newline {
                return pos;
            }
            while let Some(&c) = chars.peek() {
                if class(c) != k {
                    break;
                }
                pos -= c.len_utf8();
                chars.next();
            }
        }
        pos
    }

    /// Start of the next word after `offset` (Ctrl+Right).
    pub fn word_right(&self, offset: usize) -> usize {
        let snapshot = self.buffer.snapshot();
        let mut chars = snapshot.chars_at(offset).peekable();
        let mut pos = offset;
        if let Some('\n') = chars.peek() {
            return pos + 1;
        }
        if let Some(&c) = chars.peek() {
            let k = class(c);
            while let Some(&c) = chars.peek() {
                if class(c) != k {
                    break;
                }
                pos += c.len_utf8();
                chars.next();
            }
        }
        while let Some(&c) = chars.peek() {
            if class(c) != CharClass::Space {
                break;
            }
            pos += c.len_utf8();
            chars.next();
        }
        pos
    }

    /// The word around `offset`, for double-click and Shift+Alt+.
    pub fn word_at(&self, offset: usize) -> Range<usize> {
        let snapshot = self.buffer.snapshot();
        let after = snapshot.chars_at(offset).next();
        let before = snapshot.reversed_chars_at(offset).next();
        let k = match (before, after) {
            (_, Some(c)) if class(c) == CharClass::Word => CharClass::Word,
            (Some(c), _) if class(c) == CharClass::Word => CharClass::Word,
            (_, Some(c)) if class(c) != CharClass::Newline => class(c),
            _ => return offset..offset,
        };
        let mut start = offset;
        for c in snapshot.reversed_chars_at(offset) {
            if class(c) != k {
                break;
            }
            start -= c.len_utf8();
        }
        let mut end = offset;
        for c in snapshot.chars_at(offset) {
            if class(c) != k {
                break;
            }
            end += c.len_utf8();
        }
        start..end
    }

    /// Move every selection's head with `f`. Without `extend`, selections
    /// collapse to carets at the new heads.
    fn move_heads(
        &mut self,
        extend: bool,
        f: impl Fn(&Self, SelectionRange, Option<u32>) -> (usize, Option<u32>),
    ) {
        let moved: Vec<(SelectionRange, Option<u32>)> = self
            .selections
            .iter()
            .zip(self.selections())
            .map(|(sel, r)| {
                let (head, goal) = f(self, r, sel.goal_column);
                let tail = if extend { r.tail } else { head };
                (SelectionRange { tail, head }, goal)
            })
            .collect();
        let primary = self.primary;
        self.set_selections_with_goals(moved, primary);
    }

    /// Left: collapse a selection to its start, or move one character.
    pub fn move_left(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| {
            if !extend && !r.is_empty() {
                (r.start(), None)
            } else {
                (this.prev_char_boundary(r.head).unwrap_or(r.head), None)
            }
        });
    }

    pub fn move_right(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| {
            if !extend && !r.is_empty() {
                (r.end(), None)
            } else {
                (this.next_char_boundary(r.head).unwrap_or(r.head), None)
            }
        });
    }

    pub fn move_word_left(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| (this.word_left(r.head), None));
    }

    pub fn move_word_right(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| (this.word_right(r.head), None));
    }

    /// Up (negative) or down by `rows`, keeping the goal column.
    pub fn move_vertical(&mut self, rows: i64, extend: bool) {
        self.move_heads(extend, |this, r, goal| {
            let p = this.buffer.offset_to_point(r.head);
            let line = this.buffer.line(p.row);
            let goal = goal.unwrap_or_else(|| visual_column(&line, p.column as usize));
            let last = this.buffer.line_count() as i64 - 1;
            let target = p.row as i64 + rows;
            if target < 0 {
                return (0, Some(goal));
            }
            if target > last {
                return (this.buffer.len(), Some(goal));
            }
            let row = target as u32;
            let col = byte_for_visual_column(&this.buffer.line(row), goal);
            (
                this.buffer.point_to_offset(Point::new(row, col as u32)),
                Some(goal),
            )
        });
    }

    pub fn move_up(&mut self, extend: bool) {
        self.move_vertical(-1, extend);
    }

    pub fn move_down(&mut self, extend: bool) {
        self.move_vertical(1, extend);
    }

    pub fn page_up(&mut self, extend: bool) {
        self.move_vertical(-(self.page_rows.max(1) as i64), extend);
    }

    pub fn page_down(&mut self, extend: bool) {
        self.move_vertical(self.page_rows.max(1) as i64, extend);
    }

    /// Home: to the first non-blank character, or to column 0 if already there.
    pub fn move_home(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| {
            let p = this.buffer.offset_to_point(r.head);
            let line = this.buffer.line(p.row);
            let indent = line.len() - line.trim_start_matches([' ', '\t']).len();
            let col = if p.column as usize == indent {
                0
            } else {
                indent
            };
            (
                this.buffer.point_to_offset(Point::new(p.row, col as u32)),
                None,
            )
        });
    }

    pub fn move_end(&mut self, extend: bool) {
        self.move_heads(extend, |this, r, _| {
            let row = this.buffer.offset_to_point(r.head).row;
            (
                this.buffer
                    .point_to_offset(Point::new(row, this.buffer.line_len(row))),
                None,
            )
        });
    }

    pub fn move_to_start(&mut self, extend: bool) {
        self.move_heads(extend, |_, _, _| (0, None));
    }

    pub fn move_to_end(&mut self, extend: bool) {
        self.move_heads(extend, |this, _, _| (this.buffer.len(), None));
    }

    /// Add a caret one row above (negative) or below the primary caret, at
    /// the same visual column (Alt+Shift+Up/Down). The new caret is primary.
    pub fn add_caret_vertical(&mut self, rows: i64) {
        let primary = self.primary_selection();
        let p = self.buffer.offset_to_point(primary.head);
        let target = p.row as i64 + rows;
        if target < 0 || target >= self.buffer.line_count() as i64 {
            return;
        }
        let goal = self.selections[self.primary]
            .goal_column
            .unwrap_or_else(|| visual_column(&self.buffer.line(p.row), p.column as usize));
        let row = target as u32;
        let col = byte_for_visual_column(&self.buffer.line(row), goal);
        let offset = self.buffer.point_to_offset(Point::new(row, col as u32));
        let mut ranges: Vec<(SelectionRange, Option<u32>)> = self
            .selections
            .iter()
            .zip(self.selections())
            .map(|(s, r)| (r, s.goal_column))
            .collect();
        ranges.push((SelectionRange::caret(offset), Some(goal)));
        let primary = ranges.len() - 1;
        self.set_selections_with_goals(ranges, primary);
    }

    /// Shift+Alt+.: select the word at the caret, or add the next occurrence
    /// of the primary selection's text as a new selection.
    pub fn select_next_occurrence(&mut self) {
        let primary = self.primary_selection();
        if primary.is_empty() {
            let word = self.word_at(primary.head);
            if !word.is_empty() {
                let mut ranges = self.selections();
                ranges[self.primary] = SelectionRange {
                    tail: word.start,
                    head: word.end,
                };
                let p = self.primary;
                self.set_selections(ranges, p);
            }
            return;
        }
        let needle = self.buffer.text_for_range(primary.range());
        let last_end = self.selections().iter().map(|s| s.end()).max().unwrap_or(0);
        let len = self.buffer.len();
        let found = self
            .buffer
            .find_in_range(&needle, last_end..len, true)
            .into_iter()
            .chain(self.buffer.find_in_range(&needle, 0..last_end, true))
            .find(|m| !self.selections().iter().any(|s| s.range() == *m));
        if let Some(m) = found {
            let mut ranges = self.selections();
            ranges.push(SelectionRange {
                tail: m.start,
                head: m.end,
            });
            let p = ranges.len() - 1;
            self.set_selections(ranges, p);
        }
    }

    // ----- mouse -----

    /// A mouse press at `offset`. `add` (Ctrl+Alt+click) adds a caret,
    /// `extend` (Shift+click) moves the primary head.
    pub fn click(&mut self, offset: usize, kind: ClickKind, add: bool, extend: bool) {
        let offset = self.buffer.clip_offset(offset, text::Bias::Left);
        let range = match kind {
            ClickKind::Single => SelectionRange::caret(offset),
            ClickKind::Double => {
                let w = self.word_at(offset);
                SelectionRange {
                    tail: w.start,
                    head: w.end,
                }
            }
            ClickKind::Triple => {
                let row = self.buffer.offset_to_point(offset).row;
                let start = self.buffer.point_to_offset(Point::new(row, 0));
                let end = if row + 1 < self.buffer.line_count() {
                    self.buffer.point_to_offset(Point::new(row + 1, 0))
                } else {
                    self.buffer.len()
                };
                SelectionRange {
                    tail: start,
                    head: end,
                }
            }
        };
        if extend {
            let mut ranges = self.selections();
            ranges[self.primary].head = offset;
            let p = self.primary;
            self.set_selections(ranges, p);
        } else if add {
            let mut ranges = self.selections();
            ranges.push(range);
            let p = ranges.len() - 1;
            self.set_selections(ranges, p);
        } else {
            self.set_selections(vec![range], 0);
        }
    }

    /// Mouse drag with the button held: move the primary selection's head.
    pub fn drag_to(&mut self, offset: usize) {
        let offset = self.buffer.clip_offset(offset, text::Bias::Left);
        let mut ranges = self.selections();
        ranges[self.primary].head = offset;
        let p = self.primary;
        self.set_selections(ranges, p);
    }

    // ----- find -----

    pub fn find_query(&self) -> &FindQuery {
        &self.find
    }

    pub fn set_find_query(&mut self, query: FindQuery) {
        self.find = query;
    }

    /// Select the next match after the primary selection, wrapping at the
    /// end. Returns false if there is no match.
    pub fn find_next(&mut self) -> bool {
        let from = self.primary_selection().end();
        let len = self.buffer.len();
        let q = &self.find;
        let m = self
            .buffer
            .find_in_range(&q.text, from..len, q.case_sensitive)
            .into_iter()
            .next()
            .or_else(|| {
                self.buffer
                    .find_in_range(&q.text, 0..from.min(len), q.case_sensitive)
                    .into_iter()
                    .next()
            });
        self.select_match(m)
    }

    /// Select the previous match before the primary selection, wrapping.
    pub fn find_previous(&mut self) -> bool {
        let to = self.primary_selection().start();
        let len = self.buffer.len();
        let q = &self.find;
        let m = self
            .buffer
            .find_in_range(&q.text, 0..to, q.case_sensitive)
            .pop()
            .or_else(|| {
                self.buffer
                    .find_in_range(&q.text, to..len, q.case_sensitive)
                    .pop()
            });
        self.select_match(m)
    }

    fn select_match(&mut self, m: Option<Range<usize>>) -> bool {
        match m {
            Some(m) => {
                self.set_selections(
                    vec![SelectionRange {
                        tail: m.start,
                        head: m.end,
                    }],
                    0,
                );
                true
            }
            None => false,
        }
    }

    /// Matches of the current query in `range`, for highlighting.
    pub fn find_matches(&self, range: Range<usize>) -> Vec<Range<usize>> {
        self.buffer
            .find_in_range(&self.find.text, range, self.find.case_sensitive)
    }
}

fn normalize_newlines(text: &str) -> String {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n")
    } else {
        text.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    /// An editor over `marked`, where `|` marks carets and `[`…`]` selections
    /// (tail at `[`, head at `]`).
    fn editor(marked: &str) -> Editor {
        let (text, sels) = parse_marked(marked);
        let mut b = Buffer::new(&text);
        b.set_group_interval(Duration::ZERO);
        let mut e = Editor::new(b);
        let n = sels.len();
        e.set_selections(sels, n - 1);
        e
    }

    fn parse_marked(marked: &str) -> (String, Vec<SelectionRange>) {
        let mut text = String::new();
        let mut sels = Vec::new();
        let mut open = None;
        for c in marked.chars() {
            match c {
                '|' => sels.push(SelectionRange::caret(text.len())),
                '[' => open = Some(text.len()),
                ']' => sels.push(SelectionRange {
                    tail: open.take().unwrap(),
                    head: text.len(),
                }),
                c => text.push(c),
            }
        }
        if sels.is_empty() {
            sels.push(SelectionRange::caret(0));
        }
        (text, sels)
    }

    /// Render text with selections marked, the inverse of `parse_marked`.
    fn marked(e: &Editor) -> String {
        let text = e.text();
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in e.selections() {
            if s.is_empty() {
                marks.push((s.head, '|'));
            } else {
                marks.push((s.start(), if s.reversed() { ']' } else { '[' }));
                marks.push((s.end(), if s.reversed() { '[' } else { ']' }));
            }
        }
        marks.sort_by_key(|(at, _)| *at);
        let mut out = String::new();
        let mut last = 0;
        for (at, c) in marks {
            out.push_str(&text[last..at]);
            out.push(c);
            last = at;
        }
        out.push_str(&text[last..]);
        out
    }

    #[test]
    fn workspace_edits_are_one_undo_step_that_never_merges_with_typing() {
        let mut e = editor("class A { void Ping() { Ping(); } }|");
        // Typing within the group interval would merge; workspace edits never do.
        e.buffer.set_group_interval(Duration::from_secs(60));
        e.insert(" ");
        let text = e.text();
        let at: Vec<usize> = text.match_indices("Ping").map(|(i, _)| i).collect();
        let id = e
            .apply_edits(at.iter().map(|&i| (i..i + 4, "Pong".to_owned())).collect())
            .unwrap();
        assert_eq!(e.text(), "class A { void Pong() { Pong(); } } ");
        assert_eq!(marked(&e), "class A { void Pong() { Pong(); } } |");
        assert_eq!(e.last_transaction(), Some(id));
        e.insert("x");
        assert!(e.undo());
        assert_eq!(e.text(), "class A { void Pong() { Pong(); } } ");
        assert!(e.undo());
        assert_eq!(e.text(), "class A { void Ping() { Ping(); } } ");
        assert!(e.redo());
        assert_eq!(e.text(), "class A { void Pong() { Pong(); } } ");
        assert_eq!(e.apply_edits(vec![(0..0, String::new())]), None);
    }

    #[test]
    fn merged_steps_undo_together() {
        let mut e = editor("List<int> x;|");
        e.insert("!");
        let commit = e.last_transaction().unwrap();
        let using = e
            .apply_edits(vec![(0..0, "using System.Collections.Generic;\n".into())])
            .unwrap();
        e.merge_transactions(using, commit);
        assert_eq!(e.text(), "using System.Collections.Generic;\nList<int> x;!");
        assert!(e.undo());
        assert_eq!(e.text(), "List<int> x;");
    }

    #[test]
    fn typing_replaces_selections_and_moves_carets() {
        let mut e = editor("one [two] three|");
        e.insert("X");
        assert_eq!(marked(&e), "one X| threeX|");
    }

    #[test]
    fn multi_caret_insert_backspace_and_undo() {
        let mut e = editor("a|\nb|\nc|");
        e.insert("xy");
        assert_eq!(marked(&e), "axy|\nbxy|\ncxy|");
        e.backspace();
        assert_eq!(marked(&e), "ax|\nbx|\ncx|");
        assert!(e.undo());
        assert_eq!(marked(&e), "axy|\nbxy|\ncxy|");
        assert!(e.undo());
        assert_eq!(marked(&e), "a|\nb|\nc|");
        assert!(!e.undo());
        assert!(e.redo());
        assert_eq!(marked(&e), "axy|\nbxy|\ncxy|");
    }

    #[test]
    fn adjacent_carets_merge_after_backspace() {
        let mut e = editor("ab|c|d");
        e.backspace();
        assert_eq!(marked(&e), "a|d");
        e.backspace();
        assert_eq!(marked(&e), "|d");
    }

    #[test]
    fn delete_and_word_deletes() {
        let mut e = editor("foo| bar.baz");
        e.delete();
        assert_eq!(marked(&e), "foo|bar.baz");
        e.delete_word_right();
        assert_eq!(marked(&e), "foo|.baz");
        e.move_to_end(false);
        e.delete_word_left();
        assert_eq!(marked(&e), "foo.|");
    }

    #[test]
    fn newline_keeps_indentation() {
        let mut e = editor("    if (x)|");
        e.newline();
        assert_eq!(marked(&e), "    if (x)\n    |");
        e.tab();
        assert_eq!(marked(&e), "    if (x)\n        |");
    }

    #[test]
    fn horizontal_movement_and_selection() {
        let mut e = editor("ab|cd");
        e.move_right(true);
        e.move_right(true);
        assert_eq!(marked(&e), "ab[cd]");
        e.move_left(false);
        assert_eq!(marked(&e), "ab|cd");
        e.move_left(true);
        assert_eq!(marked(&e), "a]b[cd");
        e.move_right(false);
        assert_eq!(marked(&e), "ab|cd");
    }

    #[test]
    fn vertical_movement_keeps_the_goal_column() {
        let mut e = editor("long line|\nab\nlonger line");
        e.move_down(false);
        assert_eq!(marked(&e), "long line\nab|\nlonger line");
        e.move_down(false);
        assert_eq!(marked(&e), "long line\nab\nlonger li|ne");
        e.move_up(true);
        e.move_up(true);
        assert_eq!(marked(&e), "long line]\nab\nlonger li[ne");
    }

    #[test]
    fn home_toggles_between_indent_and_column_zero() {
        let mut e = editor("    code|");
        e.move_home(false);
        assert_eq!(marked(&e), "    |code");
        e.move_home(false);
        assert_eq!(marked(&e), "|    code");
        e.move_end(true);
        assert_eq!(marked(&e), "[    code]");
    }

    #[test]
    fn word_movement() {
        let mut e = editor("|foo.bar  baz\nnext");
        e.move_word_right(false);
        assert_eq!(marked(&e), "foo|.bar  baz\nnext");
        e.move_word_right(false);
        e.move_word_right(false);
        assert_eq!(marked(&e), "foo.bar  |baz\nnext");
        e.move_word_right(false);
        assert_eq!(marked(&e), "foo.bar  baz|\nnext");
        e.move_word_right(false);
        assert_eq!(marked(&e), "foo.bar  baz\n|next");
        e.move_word_left(false);
        assert_eq!(marked(&e), "foo.bar  baz|\nnext");
        e.move_word_left(false);
        assert_eq!(marked(&e), "foo.bar  |baz\nnext");
    }

    #[test]
    fn add_carets_vertically_and_type() {
        let mut e = editor("ab|c\nabc\nabc");
        e.add_caret_vertical(1);
        e.add_caret_vertical(1);
        assert_eq!(e.selections().len(), 3);
        e.insert("-");
        assert_eq!(marked(&e), "ab-|c\nab-|c\nab-|c");
        e.collapse_to_primary();
        assert_eq!(marked(&e), "ab-c\nab-c\nab-|c");
    }

    #[test]
    fn select_next_occurrence() {
        let mut e = editor("var x|1 = x1 + x1;");
        e.select_next_occurrence();
        assert_eq!(marked(&e), "var [x1] = x1 + x1;");
        e.select_next_occurrence();
        e.select_next_occurrence();
        assert_eq!(marked(&e), "var [x1] = [x1] + [x1];");
        e.insert("y");
        assert_eq!(marked(&e), "var y| = y| + y|;");
    }

    #[test]
    fn overlapping_selections_merge() {
        let mut e = editor("abcdef");
        e.set_selections(
            vec![
                SelectionRange { tail: 0, head: 3 },
                SelectionRange { tail: 2, head: 5 },
            ],
            1,
        );
        assert_eq!(marked(&e), "[abcde]f");
    }

    #[test]
    fn clicks() {
        let mut e = editor("hello world\nline two");
        e.click(8, ClickKind::Single, false, false);
        assert_eq!(marked(&e), "hello wo|rld\nline two");
        e.click(2, ClickKind::Single, false, true);
        assert_eq!(marked(&e), "he]llo wo[rld\nline two");
        e.click(8, ClickKind::Double, false, false);
        assert_eq!(marked(&e), "hello [world]\nline two");
        e.click(14, ClickKind::Triple, false, false);
        assert_eq!(marked(&e), "hello world\n[line two]");
        e.click(1, ClickKind::Single, false, false);
        e.click(13, ClickKind::Single, true, false);
        assert_eq!(marked(&e), "h|ello world\nl|ine two");
        e.drag_to(16);
        assert_eq!(marked(&e), "h|ello world\nl[ine] two");
    }

    #[test]
    fn copy_cut_paste() {
        let mut e = editor("one\ntw|o\nthree");
        assert_eq!(e.selected_text(), "two\n");
        let cut = e.cut();
        assert_eq!(cut, "two\n");
        assert_eq!(marked(&e), "one\n|three");
        let mut e = editor("a|\nb|");
        e.paste("1\n2");
        assert_eq!(marked(&e), "a1|\nb2|");
        e.paste("z");
        assert_eq!(marked(&e), "a1z|\nb2z|");
        let mut e = editor("[ab]c");
        assert_eq!(e.selected_text(), "ab");
        e.paste("x\r\ny");
        assert_eq!(marked(&e), "x\ny|c");
    }

    #[test]
    fn find_next_and_previous_wrap() {
        let mut e = editor("|Foo foo FOO");
        e.set_find_query(FindQuery {
            text: "foo".into(),
            case_sensitive: false,
        });
        assert!(e.find_next());
        assert_eq!(marked(&e), "[Foo] foo FOO");
        assert!(e.find_next());
        assert!(e.find_next());
        assert_eq!(marked(&e), "Foo foo [FOO]");
        assert!(e.find_next());
        assert_eq!(marked(&e), "[Foo] foo FOO");
        assert!(e.find_previous());
        assert_eq!(marked(&e), "Foo foo [FOO]");
        assert_eq!(e.find_matches(0..e.buffer().len()).len(), 3);
        e.set_find_query(FindQuery {
            text: "foo".into(),
            case_sensitive: true,
        });
        assert!(e.find_next());
        assert_eq!(marked(&e), "Foo [foo] FOO");
        e.set_find_query(FindQuery {
            text: "zzz".into(),
            case_sensitive: false,
        });
        assert!(!e.find_next());
    }

    #[test]
    fn undo_restores_selections_after_external_edit() {
        let mut e = editor("abc|");
        e.insert("d");
        // Someone else edits the start of the buffer; the caret follows.
        e.buffer_mut().edit([(0..0, ">> ")]);
        assert_eq!(marked(&e), ">> abcd|");
        e.undo();
        assert_eq!(marked(&e), "abcd|");
        e.undo();
        assert_eq!(marked(&e), "abc|");
    }
}
