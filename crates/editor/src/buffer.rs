//! The text buffer: Zed's `text::Buffer` plus what a file on disk
//! needs (byte-order mark, line endings, size limits) and, since brief 0070,
//! its text encoding: UTF-8, UTF-16 and UTF-32 by byte-order mark, UTF-8
//! without one, else a legacy code page through `encoding_rs`
//! (Windows-1252 unless the caller names another), re-encoded byte for byte
//! on save.

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

/// The fallback code page for files that are neither marked by a byte-order mark nor valid UTF-8 (Visual Studio's
/// default on a Western Windows system).
pub const DEFAULT_FALLBACK: &encoding_rs::Encoding = encoding_rs::WINDOWS_1252;

/// A file's text encoding (brief 0070).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextEncoding {
    Utf8,
    Utf16Le,
    Utf16Be,
    Utf32Le,
    Utf32Be,
    /// A single- or multi-byte legacy code page (`windows-1252`, `shift_jis`, ...).
    Legacy(&'static encoding_rs::Encoding),
}

impl TextEncoding {
    /// The label commands and the status bar use: `utf-8`, `utf-16le`, `utf-16be`, `utf-32le`, `utf-32be`, or the
    /// WHATWG name of a code page in lower case (`windows-1252`).
    pub fn label(self) -> String {
        match self {
            TextEncoding::Utf8 => "utf-8".into(),
            TextEncoding::Utf16Le => "utf-16le".into(),
            TextEncoding::Utf16Be => "utf-16be".into(),
            TextEncoding::Utf32Le => "utf-32le".into(),
            TextEncoding::Utf32Be => "utf-32be".into(),
            TextEncoding::Legacy(e) => e.name().to_ascii_lowercase(),
        }
    }

    /// As Visual Studio's status bar names it: `UTF-8`, `UTF-16 LE`, `Windows-1252`.
    pub fn display_name(self) -> String {
        match self {
            TextEncoding::Utf8 => "UTF-8".into(),
            TextEncoding::Utf16Le => "UTF-16 LE".into(),
            TextEncoding::Utf16Be => "UTF-16 BE".into(),
            TextEncoding::Utf32Le => "UTF-32 LE".into(),
            TextEncoding::Utf32Be => "UTF-32 BE".into(),
            TextEncoding::Legacy(e) => {
                let name = e.name();
                match name.strip_prefix("windows-") {
                    Some(rest) => format!("Windows-{rest}"),
                    None => name.to_owned(),
                }
            }
        }
    }

    /// From a label (case-insensitive): the Unicode forms above (`utf-16` is little endian, `unicode` too, as .NET
    /// names it), else any WHATWG label `encoding_rs` knows (`latin1`, `cp1252`, `shift_jis`).
    pub fn from_label(label: &str) -> Option<Self> {
        let l = label.trim().to_ascii_lowercase();
        match l.as_str() {
            "utf-8" | "utf8" => Some(TextEncoding::Utf8),
            "utf-16le" | "utf-16" | "utf16" | "unicode" => Some(TextEncoding::Utf16Le),
            "utf-16be" | "unicodefffe" => Some(TextEncoding::Utf16Be),
            "utf-32le" | "utf-32" | "utf32" => Some(TextEncoding::Utf32Le),
            "utf-32be" => Some(TextEncoding::Utf32Be),
            "cp1252" => Some(TextEncoding::Legacy(encoding_rs::WINDOWS_1252)),
            _ => {
                let e = encoding_rs::Encoding::for_label(l.as_bytes())?;
                // encoding_rs maps the UTF-16 labels to its decoder-only UTF-16 encodings: handled above.
                if e == encoding_rs::UTF_8 {
                    Some(TextEncoding::Utf8)
                } else if e == encoding_rs::UTF_16LE {
                    Some(TextEncoding::Utf16Le)
                } else if e == encoding_rs::UTF_16BE {
                    Some(TextEncoding::Utf16Be)
                } else if e == encoding_rs::REPLACEMENT || e == encoding_rs::X_USER_DEFINED {
                    None
                } else {
                    Some(TextEncoding::Legacy(e))
                }
            }
        }
    }

    /// Whether the encoding can carry a byte-order mark (the Unicode forms).
    pub fn is_unicode(self) -> bool {
        !matches!(self, TextEncoding::Legacy(_))
    }

    fn bom(self) -> &'static [u8] {
        match self {
            TextEncoding::Utf8 => b"\xEF\xBB\xBF",
            TextEncoding::Utf16Le => b"\xFF\xFE",
            TextEncoding::Utf16Be => b"\xFE\xFF",
            TextEncoding::Utf32Le => b"\xFF\xFE\x00\x00",
            TextEncoding::Utf32Be => b"\x00\x00\xFE\xFF",
            TextEncoding::Legacy(_) => b"",
        }
    }
}

/// Why the text cannot be written in the buffer's encoding.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EncodeError {
    pub encoding: String,
    /// The first character the encoding has no byte for.
    pub character: char,
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} cannot represent {:?} (U+{:04X}); save it with another encoding (File > Advanced Save Options)",
            self.encoding, self.character, self.character as u32
        )
    }
}

impl std::error::Error for EncodeError {}

/// Detect a file's encoding: its byte-order mark, else UTF-16 by the zero bytes of mostly-ASCII text, else valid
/// UTF-8, else `fallback`. Returns the encoding and the BOM's length.
pub fn detect_encoding(
    bytes: &[u8],
    fallback: &'static encoding_rs::Encoding,
) -> (TextEncoding, usize) {
    for enc in [
        TextEncoding::Utf32Le,
        TextEncoding::Utf32Be,
        TextEncoding::Utf8,
        TextEncoding::Utf16Le,
        TextEncoding::Utf16Be,
    ] {
        if bytes.starts_with(enc.bom()) {
            return (enc, enc.bom().len());
        }
    }
    // UTF-16 without a BOM: in ASCII-heavy text every other byte is zero.
    let head = &bytes[..bytes.len().min(4096) & !1];
    if head.len() >= 4 {
        let pairs = head.len() / 2;
        let even = head.iter().step_by(2).filter(|b| **b == 0).count();
        let odd = head.iter().skip(1).step_by(2).filter(|b| **b == 0).count();
        if odd * 10 >= pairs * 7 && even == 0 {
            return (TextEncoding::Utf16Le, 0);
        }
        if even * 10 >= pairs * 7 && odd == 0 {
            return (TextEncoding::Utf16Be, 0);
        }
    }
    if std::str::from_utf8(bytes).is_ok() {
        return (TextEncoding::Utf8, 0);
    }
    let fallback = if fallback == encoding_rs::UTF_8 {
        encoding_rs::WINDOWS_1252
    } else {
        fallback
    };
    (TextEncoding::Legacy(fallback), 0)
}

/// Decode `bytes` (without their BOM) from `encoding`. Malformed sequences become U+FFFD.
fn decode(bytes: &[u8], encoding: TextEncoding) -> String {
    match encoding {
        TextEncoding::Utf8 => String::from_utf8_lossy(bytes).into_owned(),
        TextEncoding::Utf16Le | TextEncoding::Utf16Be => {
            let units = bytes.chunks_exact(2).map(|c| {
                if encoding == TextEncoding::Utf16Le {
                    u16::from_le_bytes([c[0], c[1]])
                } else {
                    u16::from_be_bytes([c[0], c[1]])
                }
            });
            let mut out: String = char::decode_utf16(units)
                .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
                .collect();
            if bytes.len() % 2 == 1 {
                out.push(char::REPLACEMENT_CHARACTER);
            }
            out
        }
        TextEncoding::Utf32Le | TextEncoding::Utf32Be => {
            let mut out: String = bytes
                .chunks_exact(4)
                .map(|c| {
                    let v = if encoding == TextEncoding::Utf32Le {
                        u32::from_le_bytes([c[0], c[1], c[2], c[3]])
                    } else {
                        u32::from_be_bytes([c[0], c[1], c[2], c[3]])
                    };
                    char::from_u32(v).unwrap_or(char::REPLACEMENT_CHARACTER)
                })
                .collect();
            if bytes.len() % 4 != 0 {
                out.push(char::REPLACEMENT_CHARACTER);
            }
            out
        }
        TextEncoding::Legacy(e) => e.decode_without_bom_handling(bytes).0.into_owned(),
    }
}

/// Encode `text` (BOM excluded) in `encoding`.
fn encode(text: &str, encoding: TextEncoding) -> Result<Vec<u8>, EncodeError> {
    Ok(match encoding {
        TextEncoding::Utf8 => text.as_bytes().to_vec(),
        TextEncoding::Utf16Le => text.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        TextEncoding::Utf16Be => text.encode_utf16().flat_map(u16::to_be_bytes).collect(),
        TextEncoding::Utf32Le => text
            .chars()
            .flat_map(|c| (c as u32).to_le_bytes())
            .collect(),
        TextEncoding::Utf32Be => text
            .chars()
            .flat_map(|c| (c as u32).to_be_bytes())
            .collect(),
        TextEncoding::Legacy(e) => {
            let mut encoder = e.new_encoder();
            let mut out = Vec::with_capacity(
                encoder
                    .max_buffer_length_from_utf8_without_replacement(text.len())
                    .unwrap_or(text.len() * 4),
            );
            let (result, read) =
                encoder.encode_from_utf8_to_vec_without_replacement(text, &mut out, true);
            if let encoding_rs::EncoderResult::Unmappable(character) = result {
                return Err(EncodeError {
                    encoding: TextEncoding::Legacy(e).display_name(),
                    character,
                });
            }
            debug_assert_eq!(read, text.len());
            out
        }
    })
}

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
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            LoadError::Io(e) => write!(f, "{e}"),
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
/// files round-trip byte for byte). A leading byte-order mark is
/// stripped from the text and restored on save, and the text is re-encoded
/// in the file's [`TextEncoding`].
///
/// Offsets are UTF-8 byte offsets into the `\n`-normalized text.
pub struct Buffer {
    text: text::Buffer,
    line_ending: LineEnding,
    exceptions: Vec<EndingException>,
    has_bom: bool,
    encoding: TextEncoding,
}

impl fmt::Debug for Buffer {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Buffer")
            .field("len", &self.len())
            .field("line_ending", &self.line_ending)
            .field("has_bom", &self.has_bom)
            .field("encoding", &self.encoding)
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
            encoding: TextEncoding::Utf8,
        }
    }

    /// Decode file contents: the encoding is detected ([`detect_encoding`], Windows-1252 as the fallback).
    pub fn from_bytes(bytes: &[u8]) -> Result<Self, LoadError> {
        Ok(Self::from_bytes_with(bytes, DEFAULT_FALLBACK))
    }

    /// Decode file contents, reading bytes that are neither Unicode nor UTF-8 as `fallback`.
    pub fn from_bytes_with(bytes: &[u8], fallback: &'static encoding_rs::Encoding) -> Self {
        let (encoding, bom) = detect_encoding(bytes, fallback);
        Self::decoded(bytes, encoding, bom)
    }

    /// Decode file contents as `encoding` (Reopen With Encoding); a BOM of that encoding is kept.
    pub fn from_bytes_as(bytes: &[u8], encoding: TextEncoding) -> Self {
        let bom = if bytes.starts_with(encoding.bom()) {
            encoding.bom().len()
        } else {
            0
        };
        Self::decoded(bytes, encoding, bom)
    }

    fn decoded(bytes: &[u8], encoding: TextEncoding, bom: usize) -> Self {
        let mut buffer = match encoding {
            // The common case decodes nothing: the bytes are the text.
            TextEncoding::Utf8 if bom == 0 => match std::str::from_utf8(bytes) {
                Ok(text) => Self::new(text),
                Err(_) => Self::new(&decode(bytes, encoding)),
            },
            _ => Self::new(&decode(&bytes[bom..], encoding)),
        };
        buffer.has_bom = bom > 0;
        buffer.encoding = encoding;
        buffer
    }

    pub fn load(path: &Path) -> Result<Self, LoadError> {
        Self::from_bytes(&std::fs::read(path)?)
    }

    /// As [`Buffer::load`], with the fallback code page.
    pub fn load_with(
        path: &Path,
        fallback: &'static encoding_rs::Encoding,
    ) -> Result<Self, LoadError> {
        Ok(Self::from_bytes_with(&std::fs::read(path)?, fallback))
    }

    /// The file's encoding.
    pub fn encoding(&self) -> TextEncoding {
        self.encoding
    }

    /// Save in `encoding` from now on (File > Advanced Save Options). A legacy code page carries no BOM.
    pub fn set_encoding(&mut self, encoding: TextEncoding) {
        self.encoding = encoding;
        if !encoding.is_unicode() {
            self.has_bom = false;
        }
    }

    /// The file contents: BOM if the file had one, the recorded line endings, in the file's encoding. A character
    /// the encoding cannot carry is written as UTF-8 would have it; [`Buffer::try_to_file_bytes`] refuses instead.
    pub fn to_file_bytes(&self) -> Vec<u8> {
        self.try_to_file_bytes().unwrap_or_else(|_| {
            let mut b = if self.has_bom {
                BOM.as_bytes().to_vec()
            } else {
                Vec::new()
            };
            b.extend(self.file_text().into_bytes());
            b
        })
    }

    /// The file contents, or why the encoding cannot carry the text.
    pub fn try_to_file_bytes(&self) -> Result<Vec<u8>, EncodeError> {
        let text = self.file_text();
        if self.encoding == TextEncoding::Utf8 {
            let mut out = Vec::with_capacity(text.len() + 3);
            if self.has_bom {
                out.extend_from_slice(BOM.as_bytes());
            }
            out.extend(text.into_bytes());
            return Ok(out);
        }
        let mut out = Vec::new();
        if self.has_bom {
            out.extend_from_slice(self.encoding.bom());
        }
        out.extend(encode(&text, self.encoding)?);
        Ok(out)
    }

    /// The text with the file's line endings, without BOM.
    fn file_text(&self) -> String {
        let mut out = String::with_capacity(self.len() + self.line_count() as usize + 3);
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
        out
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        let bytes = self
            .try_to_file_bytes()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, bytes)
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
    fn invalid_utf8_reads_as_the_fallback_code_page() {
        let b = Buffer::from_bytes(b"ok\xFF").unwrap();
        assert_eq!(
            b.encoding(),
            TextEncoding::Legacy(encoding_rs::WINDOWS_1252)
        );
        assert_eq!(b.text(), "ok\u{FF}");
        assert_eq!(b.to_file_bytes(), b"ok\xFF");
    }

    /// Brief 0070's fixtures: each opens and saves unchanged byte for byte, and an edit is written in the file's
    /// encoding.
    #[test]
    fn encodings_round_trip_byte_for_byte() {
        let text = "// caf\u{e9} \u{20ac}\r\nclass C { }\r\n";
        let utf16le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_le_bytes))
            .collect();
        let utf16be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain(text.encode_utf16().flat_map(u16::to_be_bytes))
            .collect();
        let utf32le: Vec<u8> = [0xFF, 0xFE, 0, 0]
            .into_iter()
            .chain(text.chars().flat_map(|c| (c as u32).to_le_bytes()))
            .collect();
        let utf32be: Vec<u8> = [0, 0, 0xFE, 0xFF]
            .into_iter()
            .chain(text.chars().flat_map(|c| (c as u32).to_be_bytes()))
            .collect();
        // Windows-1252: é is E9, € is 80; the five undefined bytes (81, 8D, 8F, 90, 9D) survive too.
        let cp1252 = b"// caf\xE9 \x80 \x81\x8D\x8F\x90\x9D\r\nclass C { }\r\n".to_vec();
        let utf8_plain = text.as_bytes().to_vec();
        let utf16_no_bom: Vec<u8> = "class C { }\r\n"
            .encode_utf16()
            .flat_map(u16::to_le_bytes)
            .collect();
        for (bytes, encoding, bom) in [
            (&utf16le, TextEncoding::Utf16Le, true),
            (&utf16be, TextEncoding::Utf16Be, true),
            (&utf32le, TextEncoding::Utf32Le, true),
            (&utf32be, TextEncoding::Utf32Be, true),
            (
                &cp1252,
                TextEncoding::Legacy(encoding_rs::WINDOWS_1252),
                false,
            ),
            (&utf8_plain, TextEncoding::Utf8, false),
            (&utf16_no_bom, TextEncoding::Utf16Le, false),
        ] {
            let mut b = Buffer::from_bytes(bytes).unwrap();
            assert_eq!(b.encoding(), encoding, "{}", encoding.label());
            assert_eq!(b.has_bom(), bom, "{}", encoding.label());
            assert_eq!(b.line_ending(), LineEnding::CrLf);
            assert!(b.text().contains("class C"), "{}", encoding.label());
            assert_eq!(&b.to_file_bytes(), bytes, "{}", encoding.label());
            // An edit is written in the same encoding.
            b.edit([(0..0, "x")]);
            let again = Buffer::from_bytes(&b.to_file_bytes()).unwrap();
            assert_eq!(again.encoding(), encoding);
            assert_eq!(again.text(), b.text());
        }
    }

    #[test]
    fn a_code_page_refuses_characters_it_cannot_carry() {
        let mut b = Buffer::from_bytes(b"caf\xE9\n").unwrap();
        b.edit([(0..0, "\u{4e2d}")]);
        let err = b.try_to_file_bytes().unwrap_err();
        assert_eq!(err.character, '\u{4e2d}');
        assert!(err.to_string().contains("Windows-1252"), "{err}");
        // Save With Encoding: UTF-8 with a BOM carries it.
        b.set_encoding(TextEncoding::Utf8);
        b.set_bom(true);
        assert_eq!(
            b.try_to_file_bytes().unwrap(),
            "\u{FEFF}\u{4e2d}caf\u{e9}\n".as_bytes()
        );
    }

    #[test]
    fn encoding_labels() {
        assert_eq!(TextEncoding::from_label("UTF-8"), Some(TextEncoding::Utf8));
        assert_eq!(
            TextEncoding::from_label("unicode"),
            Some(TextEncoding::Utf16Le)
        );
        assert_eq!(
            TextEncoding::from_label("utf-16be"),
            Some(TextEncoding::Utf16Be)
        );
        assert_eq!(
            TextEncoding::from_label("cp1252")
                .map(TextEncoding::label)
                .as_deref(),
            Some("windows-1252")
        );
        assert_eq!(
            TextEncoding::from_label("latin1")
                .map(TextEncoding::display_name)
                .as_deref(),
            Some("Windows-1252")
        );
        assert_eq!(
            TextEncoding::from_label("shift_jis")
                .map(TextEncoding::label)
                .as_deref(),
            Some("shift_jis")
        );
        assert_eq!(TextEncoding::from_label("nope"), None);
        // A fallback of UTF-8 cannot fail over to itself.
        let b = Buffer::from_bytes_with(b"\xFF", encoding_rs::UTF_8);
        assert_eq!(
            b.encoding(),
            TextEncoding::Legacy(encoding_rs::WINDOWS_1252)
        );
        let b = Buffer::from_bytes_with(b"\x82\xA0", encoding_rs::SHIFT_JIS);
        assert_eq!(b.text(), "\u{3042}");
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
