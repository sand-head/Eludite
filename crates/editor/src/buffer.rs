use std::ops::Range;

use ropey::{Rope, RopeSlice};

/// A reversible edit, recorded for undo/redo. Offsets are in chars.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Edit {
    Insert { at: usize, text: String },
    Remove { at: usize, text: String },
}

impl Edit {
    fn inverse(&self) -> Edit {
        match self {
            Edit::Insert { at, text } => Edit::Remove {
                at: *at,
                text: text.clone(),
            },
            Edit::Remove { at, text } => Edit::Insert {
                at: *at,
                text: text.clone(),
            },
        }
    }
}

/// A text buffer backed by a rope.
#[derive(Debug, Clone, Default)]
pub struct Buffer {
    rope: Rope,
    undo: Vec<Edit>,
    redo: Vec<Edit>,
}

impl Buffer {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(clippy::should_implement_trait)]
    pub fn from_str(text: &str) -> Self {
        Self {
            rope: Rope::from_str(text),
            ..Self::default()
        }
    }

    /// Length in chars.
    pub fn len_chars(&self) -> usize {
        self.rope.len_chars()
    }

    /// Number of lines. A trailing newline yields an empty final line, as in ropey.
    pub fn len_lines(&self) -> usize {
        self.rope.len_lines()
    }

    /// Line `idx` including its line ending. Panics if out of bounds.
    pub fn line(&self, idx: usize) -> RopeSlice<'_> {
        self.rope.line(idx)
    }

    pub fn insert(&mut self, char_idx: usize, text: &str) {
        if text.is_empty() {
            return;
        }
        self.apply(Edit::Insert {
            at: char_idx,
            text: text.to_owned(),
        });
    }

    pub fn remove(&mut self, range: Range<usize>) {
        if range.is_empty() {
            return;
        }
        let text = self.rope.slice(range.clone()).to_string();
        self.apply(Edit::Remove {
            at: range.start,
            text,
        });
    }

    /// Undo the most recent edit. Returns false if there was nothing to undo.
    pub fn undo(&mut self) -> bool {
        let Some(edit) = self.undo.pop() else {
            return false;
        };
        self.apply_raw(&edit.inverse());
        self.redo.push(edit);
        true
    }

    /// Redo the most recently undone edit. Returns false if there was nothing to redo.
    pub fn redo(&mut self) -> bool {
        let Some(edit) = self.redo.pop() else {
            return false;
        };
        self.apply_raw(&edit);
        self.undo.push(edit);
        true
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    fn apply(&mut self, edit: Edit) {
        self.apply_raw(&edit);
        self.undo.push(edit);
        self.redo.clear();
    }

    fn apply_raw(&mut self, edit: &Edit) {
        match edit {
            Edit::Insert { at, text } => self.rope.insert(*at, text),
            Edit::Remove { at, text } => self.rope.remove(*at..*at + text.chars().count()),
        }
    }
}

impl std::fmt::Display for Buffer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        for chunk in self.rope.chunks() {
            f.write_str(chunk)?;
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lines() {
        let b = Buffer::from_str("one\ntwo\r\nthree");
        assert_eq!(b.len_lines(), 3);
        assert_eq!(b.line(1).to_string(), "two\r\n");
        assert_eq!(b.line(2).to_string(), "three");
    }

    #[test]
    fn insert_remove_undo_redo() {
        let mut b = Buffer::from_str("hello world");
        b.insert(5, ",");
        assert_eq!(b.to_string(), "hello, world");
        b.remove(0..7);
        assert_eq!(b.to_string(), "world");

        assert!(b.undo());
        assert_eq!(b.to_string(), "hello, world");
        assert!(b.undo());
        assert_eq!(b.to_string(), "hello world");
        assert!(!b.undo());

        assert!(b.redo());
        assert_eq!(b.to_string(), "hello, world");
        b.insert(0, ">");
        assert!(!b.can_redo(), "new edit clears redo");
    }

    #[test]
    fn unicode_offsets_are_chars() {
        let mut b = Buffer::from_str("añb");
        b.remove(1..2);
        assert_eq!(b.to_string(), "ab");
        b.undo();
        assert_eq!(b.to_string(), "añb");
        assert_eq!(b.len_chars(), 3);
    }

    #[test]
    fn empty_edits_are_not_recorded() {
        let mut b = Buffer::new();
        b.insert(0, "");
        b.remove(0..0);
        assert!(!b.can_undo());
    }
}
