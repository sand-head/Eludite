//! The terminal's output as plain text, for agents: what `eludite.terminal.read` (`mode: since`) and
//! `eludite.terminal.wait` answer. Escape sequences are removed, `\r\n` is a newline, a lone `\r` returns to the
//! start of the line (so a progress bar or a line editor's redraw keeps its last state) and a backspace erases a
//! character.
//!
//! A position in the stream is a *mark*: the number of bytes of plain text printed so far, overwritten or not, so
//! marks only grow. A table of line starts maps a mark to the text kept: a mark inside a line that was overwritten
//! reads from that line's start, at most its current length in. The text keeps its last [`CAP`] bytes; a mark
//! before that reads from the oldest byte kept.

use std::collections::VecDeque;

/// The bytes of plain text kept.
pub const CAP: usize = 2 * 1024 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Esc {
    Ground,
    /// After ESC.
    Escape,
    /// In a CSI sequence (ESC `[`), until its final byte.
    Csi,
    /// In an OSC, DCS, SOS, PM or APC string, until BEL or ST.
    Str,
    /// ESC inside a string: `\` ends it.
    StrEscape,
    /// ESC and an intermediate (`(`, `)`, `#`, ...): one more byte.
    Charset,
}

/// The plain text of a terminal's output and its marks.
#[derive(Debug)]
pub struct Transcript {
    text: String,
    /// Bytes printed so far (the next mark).
    end: u64,
    /// Each kept line's start: (its mark, its index in `text`). The last is the current line's.
    lines: VecDeque<(u64, usize)>,
    /// After a `\r`: the next printable character overwrites the line.
    pending_cr: bool,
    state: Esc,
    /// The bytes of an unfinished UTF-8 character.
    partial: Vec<u8>,
}

impl Default for Transcript {
    fn default() -> Self {
        Self::new()
    }
}

impl Transcript {
    pub fn new() -> Self {
        Self {
            text: String::new(),
            end: 0,
            lines: VecDeque::from([(0, 0)]),
            pending_cr: false,
            state: Esc::Ground,
            partial: Vec::new(),
        }
    }

    /// The mark after everything appended so far.
    pub fn end(&self) -> u64 {
        self.end
    }

    /// The oldest mark still readable.
    pub fn start(&self) -> u64 {
        self.lines.front().map_or(self.end, |l| l.0)
    }

    /// The mark at which the line after `mark`'s begins, if one has started: where a command's output starts for a
    /// shell that marks the command's start but not its output's (bash before 4.4 has no PS0), the command's own
    /// line being what the person typed.
    pub fn next_line_mark(&self, mark: u64) -> Option<u64> {
        self.lines.iter().find(|l| l.0 > mark).map(|l| l.0)
    }

    fn current_line(&self) -> usize {
        self.lines.back().map_or(0, |l| l.1)
    }

    /// Append the PTY's bytes (escape sequences and all).
    pub fn append(&mut self, bytes: &[u8]) {
        let mut printable: Vec<u8> = Vec::new();
        for &b in bytes {
            match self.state {
                Esc::Ground => match b {
                    0x1b => {
                        self.flush(&mut printable);
                        self.state = Esc::Escape;
                    }
                    b'\n' => {
                        self.flush(&mut printable);
                        self.pending_cr = false;
                        self.text.push('\n');
                        self.end += 1;
                        self.lines.push_back((self.end, self.text.len()));
                    }
                    b'\r' => {
                        self.flush(&mut printable);
                        self.pending_cr = true;
                    }
                    0x08 => {
                        self.flush(&mut printable);
                        if self.text.len() > self.current_line() {
                            self.text.pop();
                        }
                    }
                    b'\t' => printable.push(b),
                    0x00..=0x1f | 0x7f => {}
                    _ => printable.push(b),
                },
                Esc::Escape => {
                    self.state = match b {
                        b'[' => Esc::Csi,
                        b']' | b'P' | b'X' | b'^' | b'_' => Esc::Str,
                        b'(' | b')' | b'*' | b'+' | b'#' | b'%' | b' ' => Esc::Charset,
                        _ => Esc::Ground,
                    }
                }
                Esc::Csi => {
                    if (0x40..=0x7e).contains(&b) {
                        self.state = Esc::Ground;
                    }
                }
                Esc::Str => match b {
                    0x07 => self.state = Esc::Ground,
                    0x1b => self.state = Esc::StrEscape,
                    _ => {}
                },
                Esc::StrEscape => {
                    self.state = if b == b'\\' { Esc::Ground } else { Esc::Str };
                }
                Esc::Charset => self.state = Esc::Ground,
            }
        }
        self.flush(&mut printable);
        self.trim();
    }

    /// Write printable bytes at the cursor: a pending `\r` first returns to the line's start.
    fn flush(&mut self, printable: &mut Vec<u8>) {
        if printable.is_empty() {
            return;
        }
        let mut bytes = std::mem::take(&mut self.partial);
        bytes.append(printable);
        // Keep an unfinished UTF-8 character for the next read.
        let valid = match std::str::from_utf8(&bytes) {
            Ok(_) => bytes.len(),
            Err(e) if e.error_len().is_none() => e.valid_up_to(),
            Err(_) => bytes.len(),
        };
        self.partial = bytes.split_off(valid);
        if bytes.is_empty() {
            return;
        }
        if self.pending_cr {
            self.pending_cr = false;
            let start = self.current_line();
            self.text.truncate(start);
        }
        let s = String::from_utf8_lossy(&bytes);
        self.text.push_str(&s);
        self.end += bytes.len() as u64;
    }

    /// Keep the last [`CAP`] bytes (cut to three quarters of it, so trimming is rare).
    fn trim(&mut self) {
        if self.text.len() <= CAP {
            return;
        }
        let mut cut = self.text.len() - CAP * 3 / 4;
        while !self.text.is_char_boundary(cut) {
            cut += 1;
        }
        self.text.drain(..cut);
        while self.lines.len() > 1 && self.lines[1].1 <= cut {
            self.lines.pop_front();
        }
        if let Some(first) = self.lines.front_mut()
            && first.1 < cut
        {
            first.0 += (cut - first.1) as u64;
            first.1 = cut;
        }
        for l in self.lines.iter_mut() {
            l.1 -= cut;
        }
    }

    /// The index in `text` of `mark`, and whether text before it was dropped.
    fn index(&self, mark: u64) -> (usize, bool) {
        if mark >= self.end {
            return (self.text.len(), false);
        }
        let i = self.lines.partition_point(|l| l.0 <= mark);
        if i == 0 {
            return (0, true);
        }
        let (line_mark, start) = self.lines[i - 1];
        let line_end = self.lines.get(i).map_or(self.text.len(), |l| l.1);
        let mut idx = (start + (mark - line_mark) as usize).min(line_end);
        while !self.text.is_char_boundary(idx) {
            idx -= 1;
        }
        (idx, false)
    }

    /// The text from `mark` to the end, at most its last `max` bytes: (text, whether its start was cut).
    pub fn since(&self, mark: u64, max: usize) -> (String, bool) {
        self.range(mark, self.end, max)
    }

    /// The text from `from` to `to`, at most its last `max` bytes.
    pub fn range(&self, from: u64, to: u64, max: usize) -> (String, bool) {
        let (mut a, mut cut) = self.index(from);
        let (b, _) = self.index(to);
        if a > b {
            return (String::new(), false);
        }
        if b - a > max {
            a = b - max;
            cut = true;
        }
        while !self.text.is_char_boundary(a) {
            a += 1;
        }
        (self.text[a..b.max(a)].to_owned(), cut)
    }

    /// Forget everything printed so far (marks stay valid: the end does not move back).
    pub fn clear(&mut self) {
        self.text.clear();
        self.lines = VecDeque::from([(self.end, 0)]);
        self.pending_cr = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn text(input: &[u8]) -> String {
        let mut t = Transcript::new();
        t.append(input);
        t.since(0, usize::MAX).0
    }

    #[test]
    fn the_next_line_mark_is_where_a_commands_output_starts() {
        let mut t = Transcript::new();
        t.append(b"$ echo hi");
        let typed = t.end();
        assert_eq!(t.next_line_mark(typed), None, "the output has not started");
        t.append(b"\r\nhi\r\n$ ");
        let out = t.next_line_mark(typed).unwrap();
        assert!(t.range(out, t.end(), 100).0.starts_with("hi"));
    }

    #[test]
    fn escapes_are_removed_and_lines_kept() {
        assert_eq!(
            text(b"\x1b[1;32mok\x1b[0m\r\n\x1b]0;title\x07next\r\n"),
            "ok\nnext\n"
        );
        assert_eq!(text(b"\x1b(Bplain\x1bP+q\x1b\\ text"), "plain text");
    }

    #[test]
    fn carriage_returns_and_backspaces_overwrite() {
        assert_eq!(text(b"10%\r50%\r100%\r\ndone"), "100%\ndone");
        assert_eq!(text(b"ab\x08c"), "ac");
        // A `\r` with nothing after it changes nothing yet.
        assert_eq!(text(b"prompt$ \r"), "prompt$ ");
    }

    #[test]
    fn marks_only_grow_when_a_line_is_redrawn() {
        // A line editor redraws its long prompt and the typed command: the text shrinks, the marks do not.
        let mut t = Transcript::new();
        t.append(b"root@host:/a/long/folder# ec");
        let typed = t.end();
        t.append(b"\rroot@host:/a/long/folder# echo hi\r\n");
        assert!(t.end() > typed, "the end only grows");
        let before_output = t.end();
        t.append(b"hi\r\n");
        assert_eq!(t.since(before_output, usize::MAX).0, "hi\n");
        // A mark inside the redrawn line reads from inside that line, never past its end.
        let (inside, _) = t.since(typed, usize::MAX);
        assert_eq!(inside, "ho hi\nhi\n", "the same column of the redrawn line");
        assert_eq!(
            t.since(0, usize::MAX).0,
            "root@host:/a/long/folder# echo hi\nhi\n"
        );
    }

    #[test]
    fn marks_count_bytes_and_survive_the_cap() {
        let mut t = Transcript::new();
        t.append(b"one\n");
        let mark = t.end();
        assert_eq!(mark, 4);
        t.append("t\u{e9}st\n".as_bytes());
        assert_eq!(t.since(mark, usize::MAX), ("t\u{e9}st\n".into(), false));
        // A character split across two reads.
        let e = "\u{e9}".as_bytes();
        t.append(&e[..1]);
        t.append(&e[1..]);
        assert!(t.since(0, usize::MAX).0.ends_with('\u{e9}'));
        // At most the last `max` bytes.
        assert_eq!(t.since(0, 3), ("\n\u{e9}".into(), true));
        let big = vec![b'x'; CAP + 10];
        t.append(&big);
        let (all, cut) = t.since(0, usize::MAX);
        assert!(all.len() <= CAP && all.len() >= CAP / 2);
        assert!(cut, "the start was cut");
        assert!(t.start() > 0);
        let end = t.end();
        t.clear();
        assert_eq!(t.end(), end);
        t.append(b"new");
        assert_eq!(t.since(end, usize::MAX), ("new".into(), false));
    }
}
