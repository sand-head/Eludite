//! A naive line buffer for the spike: a `Vec` of lines, each remembering its
//! own line ending. Production uses a rope (see the audit in
//! docs/briefs/0001-report.md); this only has to be good enough to measure
//! GPUI's rendering of a 100k-line file.

use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineEnding {
    Lf,
    CrLf,
    Cr,
    /// The last line of the file has no terminator.
    None,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Line {
    pub text: String,
    pub ending: LineEnding,
}

#[derive(Debug, Clone, Default)]
pub struct Buffer {
    pub lines: Vec<Line>,
}

/// Cursor position: line index and byte offset within the line (always on a char boundary).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cursor {
    pub line: usize,
    pub col: usize,
}

const MULTIBYTE: [&str; 6] = [
    "// Grüße aus Köln: Ärger über Öl, naïve façade, coöperate",
    "// 日本語のコメント: 行番号とスクロールのテスト",
    "// Привет, мир! Проверка многобайтовых строк",
    "// Ελληνικά: γρήγορη καφέ αλεπού",
    "// emoji: 🚀 build ✅ test ❌ fail 🦀 rust",
    "// math: ∀x∈ℝ, x² ≥ 0 → √x² = |x| ≠ −x",
];

impl Buffer {
    /// Generate `n` lines of C#-like text. Roughly one line in eleven is
    /// multi-byte (Latin-1 accents, CJK, Cyrillic, Greek, emoji, math), and
    /// line endings mix LF, CRLF and CR. Deterministic.
    pub fn generate(n: usize) -> Self {
        let mut raw = String::with_capacity(n * 48);
        for i in 0..n {
            let indent = "    ".repeat(1 + (i % 4));
            if i % 11 == 5 {
                raw.push_str(&indent);
                raw.push_str(MULTIBYTE[(i / 11) % MULTIBYTE.len()]);
            } else if i % 50 == 0 {
                let _ = write!(raw, "public sealed class Generated{i:06} : IDisposable {{");
            } else if i % 50 == 49 {
                raw.push('}');
            } else {
                let _ = write!(
                    raw,
                    "{indent}var value{i} = Compute(\"line {i}\", {}, {}) + offset * {};",
                    i % 97,
                    (i * 31) % 1009,
                    i % 7
                );
            }
            if i + 1 < n {
                raw.push_str(match i % 13 {
                    0 => "\r",
                    3 | 7 => "\r\n",
                    _ => "\n",
                });
            }
        }
        Self::parse(&raw)
    }

    /// Split text into lines, recognizing `\r\n`, `\n` and lone `\r`.
    pub fn parse(text: &str) -> Self {
        let mut lines = Vec::new();
        let bytes = text.as_bytes();
        let mut start = 0;
        let mut i = 0;
        while i < bytes.len() {
            let (ending, len) = match bytes[i] {
                b'\n' => (LineEnding::Lf, 1),
                b'\r' if bytes.get(i + 1) == Some(&b'\n') => (LineEnding::CrLf, 2),
                b'\r' => (LineEnding::Cr, 1),
                _ => {
                    i += 1;
                    continue;
                }
            };
            lines.push(Line {
                text: text[start..i].to_owned(),
                ending,
            });
            i += len;
            start = i;
        }
        lines.push(Line {
            text: text[start..].to_owned(),
            ending: LineEnding::None,
        });
        Self { lines }
    }

    pub fn len(&self) -> usize {
        self.lines.len()
    }

    pub fn is_empty(&self) -> bool {
        self.lines.is_empty()
    }

    pub fn ending_counts(&self) -> (usize, usize, usize) {
        let mut c = (0, 0, 0);
        for l in &self.lines {
            match l.ending {
                LineEnding::Lf => c.0 += 1,
                LineEnding::CrLf => c.1 += 1,
                LineEnding::Cr => c.2 += 1,
                LineEnding::None => {}
            }
        }
        c
    }

    pub fn insert(&mut self, cur: &mut Cursor, s: &str) {
        let line = &mut self.lines[cur.line].text;
        line.insert_str(cur.col, s);
        cur.col += s.len();
    }

    /// Split the line at the cursor. The new line inherits the old line's ending
    /// and the old line gets LF (or CRLF if it had CRLF).
    pub fn newline(&mut self, cur: &mut Cursor) {
        let line = &mut self.lines[cur.line];
        let rest = line.text.split_off(cur.col);
        let ending = line.ending;
        line.ending = if ending == LineEnding::CrLf {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        };
        self.lines.insert(cur.line + 1, Line { text: rest, ending });
        cur.line += 1;
        cur.col = 0;
    }

    pub fn backspace(&mut self, cur: &mut Cursor) {
        if cur.col > 0 {
            let text = &mut self.lines[cur.line].text;
            let prev = text[..cur.col]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
            text.replace_range(prev..cur.col, "");
            cur.col = prev;
        } else if cur.line > 0 {
            let removed = self.lines.remove(cur.line);
            cur.line -= 1;
            let prev = &mut self.lines[cur.line];
            cur.col = prev.text.len();
            prev.text.push_str(&removed.text);
            prev.ending = removed.ending;
        }
    }

    pub fn move_left(&self, cur: &mut Cursor) {
        if cur.col > 0 {
            let text = &self.lines[cur.line].text;
            cur.col = text[..cur.col]
                .char_indices()
                .next_back()
                .map_or(0, |(i, _)| i);
        } else if cur.line > 0 {
            cur.line -= 1;
            cur.col = self.lines[cur.line].text.len();
        }
    }

    pub fn move_right(&self, cur: &mut Cursor) {
        let text = &self.lines[cur.line].text;
        if let Some(c) = text[cur.col..].chars().next() {
            cur.col += c.len_utf8();
        } else if cur.line + 1 < self.lines.len() {
            cur.line += 1;
            cur.col = 0;
        }
    }

    pub fn move_vertical(&self, cur: &mut Cursor, delta: isize) {
        let target = (cur.line as isize + delta).clamp(0, self.lines.len() as isize - 1) as usize;
        let chars = self.lines[cur.line].text[..cur.col].chars().count();
        cur.line = target;
        let text = &self.lines[target].text;
        cur.col = text
            .char_indices()
            .nth(chars)
            .map_or(text.len(), |(i, _)| i);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generate_has_100k_lines_mixed_endings_and_multibyte() {
        let b = Buffer::generate(100_000);
        assert_eq!(b.len(), 100_000);
        let (lf, crlf, cr) = b.ending_counts();
        assert!(lf > 0 && crlf > 0 && cr > 0, "{lf} {crlf} {cr}");
        assert_eq!(lf + crlf + cr, 99_999);
        assert!(b.lines.iter().filter(|l| !l.text.is_ascii()).count() > 9_000);
    }

    #[test]
    fn parse_endings() {
        let b = Buffer::parse("a\r\nb\rc\nd");
        let e: Vec<_> = b
            .lines
            .iter()
            .map(|l| (l.text.as_str(), l.ending))
            .collect();
        assert_eq!(
            e,
            [
                ("a", LineEnding::CrLf),
                ("b", LineEnding::Cr),
                ("c", LineEnding::Lf),
                ("d", LineEnding::None)
            ]
        );
    }

    #[test]
    fn edit_roundtrip() {
        let mut b = Buffer::parse("héllo\nworld");
        let mut c = Cursor { line: 0, col: 3 };
        b.insert(&mut c, "X");
        assert_eq!(b.lines[0].text, "héXllo");
        b.backspace(&mut c);
        b.backspace(&mut c);
        assert_eq!(b.lines[0].text, "hllo");
        b.newline(&mut c);
        assert_eq!(b.len(), 3);
        b.backspace(&mut c);
        assert_eq!(b.lines[0].text, "hllo");
        assert_eq!(b.len(), 2);
        b.move_vertical(&mut c, 1);
        assert_eq!(c, Cursor { line: 1, col: 1 });
    }
}
