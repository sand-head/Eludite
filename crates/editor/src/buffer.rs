//! The text buffer: Zed's vendored `text::Buffer` plus what a file on disk
//! needs (byte-order mark, line endings, size limits).

use std::collections::HashMap;
use std::fmt;
use std::ops::Range;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use text::{Anchor, Bias, BufferId, BufferSnapshot, Point, ReplicaId, TransactionId};

/// Files larger than this (in bytes) open as plain text: no syntax
/// highlighting. Everything else (editing, undo, find) still works. Chosen
/// well above the 10 MB first-paint budget so ordinary large sources
/// highlight, and below the point where a tree-sitter tree for the file
/// would dominate memory.
pub const LARGE_FILE_THRESHOLD: usize = 32 * 1024 * 1024;

const BOM: &str = "\u{FEFF}";

/// A line terminator.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LineEnding {
    /// `\n`
    Lf,
    /// `\r\n`
    CrLf,
    /// `\r` (classic Mac OS)
    Cr,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            LineEnding::Lf => "\n",
            LineEnding::CrLf => "\r\n",
            LineEnding::Cr => "\r",
        }
    }

    /// The ending new files get: CRLF on Windows (as Visual Studio), LF elsewhere.
    pub fn platform_default() -> Self {
        if cfg!(windows) {
            LineEnding::CrLf
        } else {
            LineEnding::Lf
        }
    }
}

/// Why a file could not be opened.
#[derive(Debug)]
pub enum LoadError {
    Io(std::io::Error),
    /// The file is not UTF-8. Other encodings are not supported yet.
    InvalidUtf8 {
        valid_up_to: usize,
    },
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
            LoadError::InvalidUtf8 { valid_up_to } => {
                write!(f, "not valid UTF-8 (first bad byte at {valid_up_to})")
            }
        }
    }
}

impl std::error::Error for LoadError {}

impl From<std::io::Error> for LoadError {
    fn from(e: std::io::Error) -> Self {
        LoadError::Io(e)
    }
}

/// A line break whose original terminator differs from the buffer's
/// dominant one. Two anchors on either side of the `\n` tell whether that
/// exact character still exists.
#[derive(Clone, Debug)]
struct EndingException {
    before: Anchor,
    after: Anchor,
    ending: LineEnding,
}

/// An editable text buffer.
///
/// Text is stored with `\n` line breaks. The file's terminators are
/// recorded on load and restored by [`Buffer::to_file_bytes`]: the dominant
/// ending is applied to every line break, and line breaks that had a
/// different ending in the file keep it for as long as they exist (mixed
/// files round-trip byte for byte). A leading UTF-8 byte-order mark is
/// stripped from the text and restored on save.
///
/// Offsets are UTF-8 byte offsets into the `\n`-normalized text.
pub struct Buffer {
    text: text::Buffer,
    line_ending: LineEnding,
    exceptions: Vec<EndingException>,
    has_bom: bool,
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buffer")
            .field("len", &self.len())
            .field("line_ending", &self.line_ending)
            .field("has_bom", &self.has_bom)
            .finish_non_exhaustive()
    }
}

impl Buffer {
    /// A buffer holding `text`, as if read from a file: a leading BOM and the
    /// line endings are detected and remembered.
    pub fn new(text: &str) -> Self {
        let (text, has_bom) = match text.strip_prefix(BOM) {
            Some(rest) => (rest, true),
            None => (text, false),
        };
        let (normalized, line_ending, breaks) = normalize(text);
        let text = text::Buffer::new_normalized(
            ReplicaId::LOCAL,
            BufferId::new(1).expect("nonzero"),
            text::LineEnding::Unix,
            normalized.as_str().into(),
        );
        let exceptions = breaks
            .into_iter()
            .map(|(offset, ending)| EndingException {
                before: text.anchor_after(offset),
                after: text.anchor_before(offset + 1),
                ending,
            })
            .collect();
        Self {
            text,
            line_ending,
            exceptions,
            has_bom,
        }
    }

    /// Decode file contents. Only UTF-8 (with or without BOM) is supported.
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LoadError> {
        let text = std::str::from_utf8(bytes).map_err(|e| LoadError::InvalidUtf8 {
            valid_up_to: e.valid_up_to(),
        })?;
        Ok(Self::new(text))
    }

    pub fn load(path: &Path) -> Result<Self, LoadError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// The file contents: BOM if the file had one, and the recorded line endings.
    pub fn to_file_bytes(&self) -> Vec<u8> {
        let mut out = String::with_capacity(self.len() + self.line_count() as usize + 3);
        if self.has_bom {
            out.push_str(BOM);
        }
        let snapshot = self.text.snapshot();
        let exceptions: HashMap<usize, LineEnding> = self
            .exceptions
            .iter()
            .filter_map(|e| {
                let at = snapshot.offset_for_anchor(&e.before);
                let after = snapshot.offset_for_anchor(&e.after);
                (after == at + 1).then_some((at, e.ending))
            })
            .collect();
        let dominant = self.line_ending.as_str();
        let mut offset = 0;
        for chunk in snapshot.as_rope().chunks() {
            let mut rest = chunk;
            while let Some(ix) = rest.find('\n') {
                out.push_str(&rest[..ix]);
                let ending = exceptions
                    .get(&(offset + ix))
                    .map_or(dominant, |e| e.as_str());
                out.push_str(ending);
                offset += ix + 1;
                rest = &rest[ix + 1..];
            }
            out.push_str(rest);
            offset += rest.len();
        }
        out.into_bytes()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        std::fs::write(path, self.to_file_bytes())
    }

    /// The `\n`-normalized text, without BOM.
    pub fn text(&self) -> String {
        self.text.snapshot().text()
    }

    /// A cheap, immutable, `Send` copy of the current text and history
    /// position; what the highlighter and any background reader use.
    pub fn snapshot(&self) -> &BufferSnapshot {
        self.text.snapshot()
    }

    pub fn version(&self) -> clock::Global {
        self.text.version()
    }

    /// Length in bytes of the normalized text.
    pub fn len(&self) -> usize {
        self.text.len()
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// Number of rows; a trailing newline starts an empty last row.
    pub fn line_count(&self) -> u32 {
        self.text.max_point().row + 1
    }

    /// The text of `row`, without its line break.
    pub fn line(&self, row: u32) -> String {
        let s = self.text.snapshot();
        let len = s.line_len(row);
        s.text_for_range(Point::new(row, 0)..Point::new(row, len))
            .collect()
    }

    pub fn line_len(&self, row: u32) -> u32 {
        self.text.line_len(row)
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    /// Change the terminator of every line break, including ones that had a
    /// different ending in the file.
    pub fn set_line_ending(&mut self, ending: LineEnding) {
        self.line_ending = ending;
        self.exceptions.clear();
    }

    /// True if the file had line breaks with more than one terminator.
    pub fn has_mixed_line_endings(&self) -> bool {
        !self.exceptions.is_empty()
    }

    pub fn has_bom(&self) -> bool {
        self.has_bom
    }

    pub fn set_bom(&mut self, has_bom: bool) {
        self.has_bom = has_bom;
    }

    /// Over [`LARGE_FILE_THRESHOLD`]: open without highlighting.
    pub fn is_large(&self) -> bool {
        self.len() > LARGE_FILE_THRESHOLD
    }

    /// Replace each range with its text, all against the current text.
    /// Ranges must be sorted and must not overlap. Inserted `\r\n` and `\r`
    /// become `\n`. Each call is one undo step unless it is inside a
    /// transaction or within the grouping interval of the previous one.
    pub fn edit<I, S>(&mut self, edits: I)
    where
        I: IntoIterator<Item = (Range<usize>, S)>,
        S: AsRef<str>,
    {
        let edits: Vec<(Range<usize>, Arc<str>)> = edits
            .into_iter()
            .map(|(range, text)| (range, normalize_insert(text.as_ref())))
            .filter(|(range, text)| !(range.is_empty() && text.is_empty()))
            .collect();
        if edits.is_empty() {
            return;
        }
        self.text.edit(edits);
    }

    /// Start a transaction: edits until the matching
    /// [`Buffer::end_transaction_at`] undo as one step.
    pub fn start_transaction_at(&mut self, now: Instant) -> Option<TransactionId> {
        self.text.start_transaction_at(now)
    }

    /// End a transaction. Returns the id of the undo step it ended up in
    /// (transactions within the grouping interval merge into one step), or
    /// `None` if nothing was edited.
    pub fn end_transaction_at(&mut self, now: Instant) -> Option<TransactionId> {
        self.text.end_transaction_at(now).map(|(id, _)| id)
    }

    /// Edits closer together than this merge into one undo step
    /// (300 ms by default, like typing a word).
    pub fn set_group_interval(&mut self, interval: Duration) {
        self.text.set_group_interval(interval);
    }

    /// Stop the last undo step from merging with the next one.
    pub fn finalize_last_transaction(&mut self) {
        self.text.finalize_last_transaction();
    }

    /// The most recent undo step, if any.
    pub fn last_transaction(&self) -> Option<TransactionId> {
        self.text.peek_undo_stack().map(|e| e.transaction_id())
    }

    /// Move the edits of undo step `transaction` into `destination`, so they undo together.
    pub fn merge_transactions(&mut self, transaction: TransactionId, destination: TransactionId) {
        self.text.merge_transactions(transaction, destination);
    }

    /// Undo the last step. Returns its transaction id, or `None` if there was nothing to undo.
    pub fn undo(&mut self) -> Option<TransactionId> {
        self.text.undo().map(|(id, _)| id)
    }

    /// Redo the last undone step.
    pub fn redo(&mut self) -> Option<TransactionId> {
        self.text.redo().map(|(id, _)| id)
    }

    pub fn can_undo(&self) -> bool {
        self.text.peek_undo_stack().is_some()
    }

    pub fn can_redo(&self) -> bool {
        self.text.peek_redo_stack().is_some()
    }

    /// An anchor that stays before text inserted at `offset`.
    pub fn anchor_before(&self, offset: usize) -> Anchor {
        self.text.anchor_before(offset)
    }

    /// An anchor that moves after text inserted at `offset`.
    pub fn anchor_after(&self, offset: usize) -> Anchor {
        self.text.anchor_after(offset)
    }

    pub fn anchor_at(&self, offset: usize, bias: Bias) -> Anchor {
        self.text.anchor_at(offset, bias)
    }

    /// Where an anchor is now.
    pub fn offset_for_anchor(&self, anchor: &Anchor) -> usize {
        self.text.offset_for_anchor(anchor)
    }

    pub fn offset_to_point(&self, offset: usize) -> Point {
        self.text.offset_to_point(offset)
    }

    pub fn point_to_offset(&self, point: Point) -> usize {
        self.text.point_to_offset(point)
    }

    /// Clip a point into the buffer and onto a character boundary.
    pub fn clip_point(&self, point: Point, bias: Bias) -> Point {
        self.text.clip_point(point, bias)
    }

    pub fn clip_offset(&self, offset: usize, bias: Bias) -> usize {
        self.text.clip_offset(offset, bias)
    }

    pub fn text_for_range(&self, range: Range<usize>) -> String {
        self.text.text_for_range(range).collect()
    }

    /// Byte ranges of every match of `query` in `range`. ASCII
    /// case-insensitive unless `case_sensitive`. Matches do not overlap.
    pub fn find_in_range(
        &self,
        query: &str,
        range: Range<usize>,
        case_sensitive: bool,
    ) -> Vec<Range<usize>> {
        let mut out = Vec::new();
        if query.is_empty() {
            return out;
        }
        let needle: Vec<u8> = if case_sensitive {
            query.as_bytes().to_vec()
        } else {
            query.as_bytes().to_ascii_lowercase()
        };
        let range = range.start.min(self.len())..range.end.min(self.len());
        // Scan chunk by chunk, carrying the last `needle.len() - 1` bytes.
        let mut window: Vec<u8> = Vec::new();
        let mut window_start = range.start;
        for chunk in self.text.text_for_range(range.clone()) {
            window.extend_from_slice(chunk.as_bytes());
            if !case_sensitive {
                let from = window.len() - chunk.len();
                window[from..].make_ascii_lowercase();
            }
            let mut i = 0;
            let mut skip_until = 0;
            while i + needle.len() <= window.len() {
                if i >= skip_until && window[i..i + needle.len()] == needle[..] {
                    let start = window_start + i;
                    if out.last().is_none_or(|r: &Range<usize>| r.end <= start) {
                        out.push(start..start + needle.len());
                    }
                    skip_until = i + needle.len();
                }
                i += 1;
            }
            let keep = (needle.len() - 1).min(window.len());
            let drop = window.len() - keep;
            window.drain(..drop);
            window_start += drop;
        }
        out
    }
}

/// `\r\n` and lone `\r` to `\n`.
fn normalize_insert(text: &str) -> Arc<str> {
    if text.contains('\r') {
        text.replace("\r\n", "\n").replace('\r', "\n").into()
    } else {
        text.into()
    }
}

/// Normalize to `\n` and pick the dominant ending. Returns the offsets (in
/// the normalized text) of line breaks whose ending is not the dominant one.
fn normalize(text: &str) -> (String, LineEnding, Vec<(usize, LineEnding)>) {
    if !text.contains('\r') {
        let ending = if text.contains('\n') {
            LineEnding::Lf
        } else {
            LineEnding::platform_default()
        };
        return (text.to_owned(), ending, Vec::new());
    }
    let bytes = text.as_bytes();
    let mut out = String::with_capacity(text.len());
    let mut breaks: Vec<(usize, LineEnding)> = Vec::new();
    let mut counts: HashMap<LineEnding, usize> = HashMap::new();
    let mut last = 0;
    let mut i = 0;
    while i < bytes.len() {
        let ending = match bytes[i] {
            b'\n' => Some((LineEnding::Lf, 1)),
            b'\r' if bytes.get(i + 1) == Some(&b'\n') => Some((LineEnding::CrLf, 2)),
            b'\r' => Some((LineEnding::Cr, 1)),
            _ => None,
        };
        if let Some((ending, len)) = ending {
            out.push_str(&text[last..i]);
            breaks.push((out.len(), ending));
            out.push('\n');
            *counts.entry(ending).or_default() += 1;
            i += len;
            last = i;
        } else {
            i += 1;
        }
    }
    out.push_str(&text[last..]);
    let dominant = [LineEnding::CrLf, LineEnding::Lf, LineEnding::Cr]
        .into_iter()
        .max_by_key(|e| {
            (
                counts.get(e).copied().unwrap_or(0),
                *e == LineEnding::platform_default(),
            )
        })
        .expect("non-empty");
    breaks.retain(|(_, e)| *e != dominant);
    (out, dominant, breaks)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now() -> Instant {
        Instant::now()
    }

    #[test]
    fn edits_and_undo_redo() {
        let mut b = Buffer::new("hello world");
        b.set_group_interval(Duration::ZERO);
        b.edit([(5..5, ",")]);
        assert_eq!(b.text(), "hello, world");
        b.edit([(0..7, "")]);
        assert_eq!(b.text(), "world");
        assert!(b.undo().is_some());
        assert_eq!(b.text(), "hello, world");
        assert!(b.undo().is_some());
        assert_eq!(b.text(), "hello world");
        assert!(b.undo().is_none());
        assert!(b.redo().is_some());
        assert_eq!(b.text(), "hello, world");
        b.edit([(0..0, ">")]);
        assert!(!b.can_redo(), "a new edit clears redo");
    }

    #[test]
    fn transactions_undo_as_one_step() {
        let mut b = Buffer::new("abc");
        b.set_group_interval(Duration::ZERO);
        let t = now();
        b.start_transaction_at(t);
        b.edit([(0..0, "1")]);
        b.edit([(4..4, "2")]);
        let id = b.end_transaction_at(t);
        assert!(id.is_some());
        assert_eq!(b.text(), "1abc2");
        b.undo();
        assert_eq!(b.text(), "abc");
    }

    #[test]
    fn edits_within_the_group_interval_merge() {
        let mut b = Buffer::new("");
        b.set_group_interval(Duration::from_millis(300));
        let t0 = now();
        for (i, c) in ["a", "b", "c"].into_iter().enumerate() {
            let t = t0 + Duration::from_millis(50 * i as u64);
            b.start_transaction_at(t);
            b.edit([(i..i, c)]);
            b.end_transaction_at(t);
        }
        let later = t0 + Duration::from_secs(5);
        b.start_transaction_at(later);
        b.edit([(3..3, " d")]);
        b.end_transaction_at(later);
        b.undo();
        assert_eq!(b.text(), "abc");
        b.undo();
        assert_eq!(b.text(), "");
    }

    #[test]
    fn anchors_survive_edits_and_undo() {
        let mut b = Buffer::new("fn main() {}\n");
        b.set_group_interval(Duration::ZERO);
        let brace = b.text().find('{').unwrap();
        let before = b.anchor_before(brace);
        let after = b.anchor_after(brace);
        b.edit([(0..0, "pub ")]);
        assert_eq!(b.offset_for_anchor(&before), brace + 4);
        // Insert exactly at the anchored position: bias decides the side.
        b.edit([(brace + 4..brace + 4, "/*x*/")]);
        assert_eq!(b.offset_for_anchor(&before), brace + 4);
        assert_eq!(b.offset_for_anchor(&after), brace + 9);
        // Delete the text around it: the anchor collapses to the deletion point.
        b.edit([(2..brace + 9, "")]);
        assert_eq!(b.offset_for_anchor(&before), 2);
        b.undo();
        b.undo();
        b.undo();
        assert_eq!(b.text(), "fn main() {}\n");
        assert_eq!(b.offset_for_anchor(&before), brace);
    }

    #[test]
    fn crlf_round_trips() {
        let src = "a\r\nb\r\n\r\nc";
        let b = Buffer::new(src);
        assert_eq!(b.text(), "a\nb\n\nc");
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        assert!(!b.has_mixed_line_endings());
        assert_eq!(b.to_file_bytes(), src.as_bytes());
    }

    #[test]
    fn mixed_endings_round_trip_and_follow_edits() {
        let src = "one\r\ntwo\nthree\r\nfour\rfive\r\n";
        let mut b = Buffer::new(src);
        assert_eq!(b.line_ending(), LineEnding::CrLf);
        assert!(b.has_mixed_line_endings());
        assert_eq!(b.line_count(), 6);
        assert_eq!(b.to_file_bytes(), src.as_bytes());
        // Insert a line at the top: the LF and CR breaks keep their endings,
        // and the new break gets the dominant CRLF.
        b.edit([(0..0, "zero\n")]);
        assert_eq!(
            String::from_utf8(b.to_file_bytes()).unwrap(),
            "zero\r\none\r\ntwo\nthree\r\nfour\rfive\r\n"
        );
        // Delete the LF break: its exception goes with it, and the joined
        // line is not given a stale ending.
        let at = b.text().find("two").unwrap() + 3;
        b.edit([(at..at + 1, "")]);
        b.edit([(at..at, "\n")]);
        assert_eq!(
            String::from_utf8(b.to_file_bytes()).unwrap(),
            "zero\r\none\r\ntwo\r\nthree\r\nfour\rfive\r\n"
        );
    }

    #[test]
    fn inserted_crlf_is_normalized() {
        let mut b = Buffer::new("x");
        b.edit([(1..1, "\r\ny\rz")]);
        assert_eq!(b.text(), "x\ny\nz");
    }

    #[test]
    fn bom_round_trips() {
        let bytes = b"\xEF\xBB\xBFclass C {}\r\n";
        let b = Buffer::from_bytes(bytes).unwrap();
        assert!(b.has_bom());
        assert_eq!(b.text(), "class C {}\n");
        assert_eq!(b.to_file_bytes(), bytes);
        let plain = Buffer::new("x\n");
        assert!(!plain.has_bom());
        assert_eq!(plain.to_file_bytes(), b"x\n");
    }

    #[test]
    fn invalid_utf8_is_rejected() {
        let err = Buffer::from_bytes(b"ok\xFF").unwrap_err();
        assert!(matches!(err, LoadError::InvalidUtf8 { valid_up_to: 2 }));
    }

    #[test]
    fn save_and_load_preserve_bytes() {
        let dir = std::env::temp_dir().join(format!("eludite-editor-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.cs");
        let bytes = b"\xEF\xBB\xBFusing System;\r\nclass A {}\n";
        std::fs::write(&path, bytes).unwrap();
        let b = Buffer::load(&path).unwrap();
        b.save(&path).unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn large_file_threshold() {
        assert!(!Buffer::new("small").is_large());
        let big = "x".repeat(LARGE_FILE_THRESHOLD + 1);
        assert!(Buffer::new(&big).is_large());
    }

    #[test]
    fn lines_and_points() {
        let b = Buffer::new("ab\ncd\n");
        assert_eq!(b.line_count(), 3);
        assert_eq!(b.line(1), "cd");
        assert_eq!(b.line(2), "");
        assert_eq!(b.offset_to_point(4), Point::new(1, 1));
    }

    #[test]
    fn find_across_chunks_and_case() {
        let mut text = "x".repeat(1000);
        text.push_str("Needle");
        text.push_str(&"y".repeat(1000));
        text.push_str("needle needleneedle");
        let b = Buffer::new(&text);
        let all = b.find_in_range("needle", 0..b.len(), false);
        assert_eq!(all.len(), 4);
        assert_eq!(all[0], 1000..1006);
        let exact = b.find_in_range("Needle", 0..b.len(), true);
        assert_eq!(exact, vec![1000..1006]);
        assert!(b.find_in_range("", 0..b.len(), true).is_empty());
        let aa = Buffer::new("aaaa");
        assert_eq!(aa.find_in_range("aa", 0..4, true), vec![0..2, 2..4]);
    }
}
