//! The shell integration's escape sequences, taken out of the PTY's byte stream before the emulator sees it:
//! OSC 133 (`A` prompt start, `B` command start, `C` command output start, `D[;code]` command end, the FinalTerm and
//! VS Code convention) and OSC 7 (`file://host/path`, the current folder). `alacritty_terminal` ignores both, so the
//! terminal's own loop splits a read into [`Segment`]s: the bytes for the emulator, and the marks between them in
//! the order they came. A sequence split across two reads is held until it completes.

/// A shell integration mark (OSC 133).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MarkKind {
    /// The prompt starts.
    PromptStart,
    /// The prompt ended; the person types the command.
    CommandStart,
    /// The command runs; its output follows.
    OutputStart,
    /// The command ended with this exit code (when the shell gave one).
    CommandEnd(Option<i32>),
}

/// A piece of the PTY's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Segment {
    /// Bytes for the emulator.
    Bytes(Vec<u8>),
    Mark(MarkKind),
    /// The shell's current folder (OSC 7), as a path.
    Cwd(String),
}

/// The longest integration sequence held while it is incomplete; anything longer passes through.
const MAX_HELD: usize = 4096;

/// Splits the PTY's output into [`Segment`]s across reads.
#[derive(Debug, Default)]
pub struct Scanner {
    /// The bytes of a sequence that may be an integration sequence, still incomplete.
    held: Vec<u8>,
}

impl Scanner {
    pub fn new() -> Self {
        Self::default()
    }

    /// The segments of `input`, after whatever an earlier call held.
    pub fn feed(&mut self, input: &[u8]) -> Vec<Segment> {
        let mut out = Vec::new();
        let mut plain: Vec<u8> = Vec::new();
        let mut data: Vec<u8> = std::mem::take(&mut self.held);
        data.extend_from_slice(input);
        let mut i = 0;
        while i < data.len() {
            if data[i] != 0x1b {
                // Copy the run up to the next ESC at once.
                let next = data[i..]
                    .iter()
                    .position(|&b| b == 0x1b)
                    .map_or(data.len(), |p| i + p);
                plain.extend_from_slice(&data[i..next]);
                i = next;
                continue;
            }
            match parse_osc(&data[i..]) {
                Parsed::Incomplete if data.len() - i <= MAX_HELD => {
                    self.held = data[i..].to_vec();
                    break;
                }
                Parsed::Incomplete | Parsed::Other => {
                    plain.push(0x1b);
                    i += 1;
                }
                Parsed::Ours(segment, len) => {
                    if !plain.is_empty() {
                        out.push(Segment::Bytes(std::mem::take(&mut plain)));
                    }
                    out.push(segment);
                    i += len;
                }
            }
        }
        if !plain.is_empty() {
            out.push(Segment::Bytes(plain));
        }
        out
    }
}

enum Parsed {
    /// Not an integration sequence: pass the ESC on.
    Other,
    /// It may be one; more bytes are needed.
    Incomplete,
    /// An integration sequence of this many bytes.
    Ours(Segment, usize),
}

/// `s` starts with ESC: an OSC 133 or OSC 7 sequence ended by BEL or ESC `\`?
fn parse_osc(s: &[u8]) -> Parsed {
    const PREFIXES: [&[u8]; 2] = [b"\x1b]133;", b"\x1b]7;"];
    let Some(prefix) = PREFIXES.iter().find(|p| {
        let n = p.len().min(s.len());
        s[..n] == p[..n]
    }) else {
        return Parsed::Other;
    };
    if s.len() < prefix.len() {
        return Parsed::Incomplete;
    }
    let body = &s[prefix.len()..];
    let mut end = None;
    for (j, &b) in body.iter().enumerate() {
        match b {
            0x07 => {
                end = Some((j, 1));
                break;
            }
            0x1b => {
                if j + 1 >= body.len() {
                    return Parsed::Incomplete;
                }
                if body[j + 1] == b'\\' {
                    end = Some((j, 2));
                    break;
                }
                // Another escape inside: not a sequence we understand.
                return Parsed::Other;
            }
            _ => {}
        }
    }
    let Some((j, terminator)) = end else {
        return Parsed::Incomplete;
    };
    let payload = String::from_utf8_lossy(&body[..j]);
    let len = prefix.len() + j + terminator;
    let segment = if prefix.starts_with(b"\x1b]133") {
        let mut parts = payload.split(';');
        match parts.next() {
            Some("A") => Segment::Mark(MarkKind::PromptStart),
            Some("B") => Segment::Mark(MarkKind::CommandStart),
            Some("C") => Segment::Mark(MarkKind::OutputStart),
            Some("D") => Segment::Mark(MarkKind::CommandEnd(
                parts.next().and_then(|c| c.trim().parse().ok()),
            )),
            // Other OSC 133 kinds (`P`, `E`, ...) carry nothing we use: drop them.
            _ => return Parsed::Ours(Segment::Bytes(Vec::new()), len),
        }
    } else {
        match file_url_path(&payload) {
            Some(path) => Segment::Cwd(path),
            None => return Parsed::Ours(Segment::Bytes(Vec::new()), len),
        }
    };
    Parsed::Ours(segment, len)
}

/// The path of a `file://host/path` url, percent-decoded (`file:///C:/x` is `C:/x`).
pub fn file_url_path(url: &str) -> Option<String> {
    let rest = url.strip_prefix("file://")?;
    let path = &rest[rest.find('/')?..];
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%'
            && i + 2 < bytes.len()
            && let Ok(v) = u8::from_str_radix(&path[i + 1..i + 3], 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(bytes[i]);
        i += 1;
    }
    let path = String::from_utf8(out).ok()?;
    // `/C:/Users/x` on Windows.
    let b = path.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' {
        return Some(path[1..].to_owned());
    }
    Some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Segment {
        Segment::Bytes(s.as_bytes().to_vec())
    }

    #[test]
    fn marks_are_taken_out_in_order() {
        let mut s = Scanner::new();
        let out = s.feed(b"out\r\n\x1b]133;D;3\x07\x1b]133;A\x07$ \x1b]133;B\x1b\\x");
        assert_eq!(
            out,
            [
                bytes("out\r\n"),
                Segment::Mark(MarkKind::CommandEnd(Some(3))),
                Segment::Mark(MarkKind::PromptStart),
                bytes("$ "),
                Segment::Mark(MarkKind::CommandStart),
                bytes("x"),
            ]
        );
    }

    #[test]
    fn a_sequence_split_across_reads_is_held() {
        let mut s = Scanner::new();
        assert_eq!(s.feed(b"ab\x1b]13"), [bytes("ab")]);
        assert_eq!(s.feed(b"3;C"), []);
        assert_eq!(
            s.feed(b"\x07cd"),
            [Segment::Mark(MarkKind::OutputStart), bytes("cd")]
        );
        // ESC at the very end, then the backslash of ST in the next read.
        assert_eq!(s.feed(b"\x1b]133;D\x1b"), []);
        assert_eq!(s.feed(b"\\"), [Segment::Mark(MarkKind::CommandEnd(None))]);
    }

    #[test]
    fn other_sequences_pass_through() {
        let mut s = Scanner::new();
        let input = b"\x1b[31mred\x1b[0m \x1b]0;title\x07 \x1b]1337;x\x07";
        assert_eq!(s.feed(input), [Segment::Bytes(input.to_vec())]);
        // A lone ESC at the end may start an integration sequence: held, then released.
        assert_eq!(s.feed(b"a\x1b"), [bytes("a")]);
        assert_eq!(s.feed(b"[1m"), [bytes("\x1b[1m")]);
    }

    #[test]
    fn osc_7_reports_the_folder() {
        let mut s = Scanner::new();
        assert_eq!(
            s.feed(b"\x1b]7;file://host/home/me/my%20dir\x07"),
            [Segment::Cwd("/home/me/my dir".into())]
        );
        assert_eq!(file_url_path("file:///C:/x/y").as_deref(), Some("C:/x/y"));
        assert_eq!(file_url_path("http://x/y"), None);
    }
}
