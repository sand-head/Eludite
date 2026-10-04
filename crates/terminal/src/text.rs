//! The terminal's output as plain text, for agents: what `eludite.terminal.read` (`mode: since`) and
//! `eludite.terminal.wait` answer. Escape sequences are removed, `\r\n` is a newline, a lone `\r` returns to the
//! start of the line (so a progress bar keeps its last state) and a backspace erases a character. A position in the
//! stream is a *mark*: the number of bytes of plain text printed so far. The text keeps its last [`CAP`] bytes; a
//! mark before that reads from the oldest byte kept.

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

/// The plain text of a terminal's output, with its marks' base.
#[derive(Debug)]
pub struct Transcript {
    text: String,
    /// The mark of `text`'s first byte (bytes dropped from the front).
    base: u64,
    /// Where the current line starts in `text` (after the last `\n`).
    line_start: usize,
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
            base: 0,
            line_start: 0,
            pending_cr: false,
            state: Esc::Ground,
            partial: Vec::new(),
        }
    }

    /// The mark after everything appended so far.
    pub fn end(&self) -> u64 {
        self.base + self.text.len() as u64
    }

    /// The oldest mark still readable.
    pub fn start(&self) -> u64 {
        self.base
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
                        self.line_start = self.text.len();
                    }
                    b'\r' => {
                        self.flush(&mut printable);
                        self.pending_cr = true;
                    }
                    0x08 => {
                        self.flush(&mut printable);
                        if self.text.len() > self.line_start {
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
            self.text.truncate(self.line_start);
        }
        self.text.push_str(&String::from_utf8_lossy(&bytes));
    }

    fn trim(&mut self) {
        if self.text.len() <= CAP {
            return;
        }
        let mut cut = self.text.len() - CAP;
        while !self.text.is_char_boundary(cut) {
            cut += 1;
        }
        self.text.drain(..cut);
        self.base += cut as u64;
        self.line_start = self.line_start.saturating_sub(cut);
    }

    /// The text from `mark` to the end, at most its last `max` bytes: (text, whether its start was cut).
    pub fn since(&self, mark: u64, max: usize) -> (String, bool) {
        self.range(mark, self.end(), max)
    }

    /// The text from `from` to `to`, at most its last `max` bytes.
    pub fn range(&self, from: u64, to: u64, max: usize) -> (String, bool) {
        let clamp = |m: u64| (m.max(self.base).min(self.end()) - self.base) as usize;
        let (mut a, b) = (clamp(from), clamp(to));
        if a > b {
            return (String::new(), false);
        }
        let mut cut = from < self.base;
        if b - a > max {
            a = b - max;
            cut = true;
        }
        while !self.text.is_char_boundary(a) {
            a += 1;
        }
        let mut b = b;
        while !self.text.is_char_boundary(b) {
            b -= 1;
        }
        (self.text[a..b.max(a)].to_owned(), cut)
    }

    /// Forget everything printed so far (marks stay valid: the end does not move back).
    pub fn clear(&mut self) {
        self.base = self.end();
        self.text.clear();
        self.line_start = 0;
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
        assert_eq!(t.since(0, usize::MAX).0.len(), CAP);
        assert!(t.since(0, usize::MAX).1, "the start was cut");
        let end = t.end();
        t.clear();
        assert_eq!(t.end(), end);
        t.append(b"new");
        assert_eq!(t.since(end, usize::MAX), ("new".into(), false));
    }
}
