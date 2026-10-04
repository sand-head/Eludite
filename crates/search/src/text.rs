//! Decoding a file's bytes for searching, and the small text helpers the matches need.

use std::borrow::Cow;
use std::hash::{DefaultHasher, Hasher};

/// How a file was decoded for searching, from its byte order mark.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Encoding {
    /// UTF-8 without a byte order mark (the default for a file without one; invalid bytes are searched as they are).
    #[default]
    Utf8,
    /// UTF-8 with its byte order mark (stripped for searching).
    Utf8Bom,
    /// UTF-16, little-endian, by its byte order mark (decoded to UTF-8 for searching).
    Utf16Le,
    /// UTF-16, big-endian, by its byte order mark.
    Utf16Be,
}

impl Encoding {
    /// UTF-16: the editor and the workspace-edit applier read UTF-8 only, so such a file is searched but not replaced
    /// in.
    pub fn is_utf16(self) -> bool {
        matches!(self, Encoding::Utf16Le | Encoding::Utf16Be)
    }
}

/// The UTF-8 text to search in `bytes`, by its byte order mark.
pub(crate) fn decode(bytes: &[u8]) -> (Cow<'_, [u8]>, Encoding) {
    if let Some(rest) = bytes.strip_prefix(b"\xEF\xBB\xBF") {
        return (Cow::Borrowed(rest), Encoding::Utf8Bom);
    }
    let utf16 = |rest: &[u8], le: bool| {
        let units = rest.as_chunks::<2>().0.iter().map(|c| {
            if le {
                u16::from_le_bytes([c[0], c[1]])
            } else {
                u16::from_be_bytes([c[0], c[1]])
            }
        });
        let text: String = char::decode_utf16(units)
            .map(|r| r.unwrap_or(char::REPLACEMENT_CHARACTER))
            .collect();
        Cow::Owned(text.into_bytes())
    };
    if let Some(rest) = bytes.strip_prefix(b"\xFF\xFE") {
        return (utf16(rest, true), Encoding::Utf16Le);
    }
    if let Some(rest) = bytes.strip_prefix(b"\xFE\xFF") {
        return (utf16(rest, false), Encoding::Utf16Be);
    }
    (Cow::Borrowed(bytes), Encoding::Utf8)
}

/// A line without its line ending (`\n` or `\r\n`).
pub(crate) fn strip_ending(line: &[u8]) -> &[u8] {
    let line = line.strip_suffix(b"\n").unwrap_or(line);
    line.strip_suffix(b"\r").unwrap_or(line)
}

/// The length of `s` in UTF-16 code units (LSP's columns).
pub fn utf16_len(s: &str) -> usize {
    s.chars().map(char::len_utf16).sum()
}

/// A stable hash of `bytes`: what a file had when it was searched, to check before editing it that it still has it.
/// For a file it is of the file's bytes; for an open document of the text the overlay supplied.
pub fn fingerprint(bytes: &[u8]) -> u64 {
    let mut h = DefaultHasher::new();
    h.write(bytes);
    h.write_usize(bytes.len());
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn byte_order_marks_decide_the_encoding() {
        assert_eq!(decode(b"abc"), (Cow::Borrowed(&b"abc"[..]), Encoding::Utf8));
        assert_eq!(decode(b"\xEF\xBB\xBFabc").0.as_ref(), b"abc");
        let le: Vec<u8> = [0xFF, 0xFE]
            .into_iter()
            .chain("h\u{e9}!".encode_utf16().flat_map(|u| u.to_le_bytes()))
            .collect();
        assert_eq!(
            decode(&le),
            (
                Cow::Owned("h\u{e9}!".as_bytes().to_vec()),
                Encoding::Utf16Le
            )
        );
        let be: Vec<u8> = [0xFE, 0xFF]
            .into_iter()
            .chain("x\n".encode_utf16().flat_map(|u| u.to_be_bytes()))
            .collect();
        assert_eq!(decode(&be).0.as_ref(), b"x\n");
        assert!(decode(&be).1.is_utf16());
    }

    #[test]
    fn endings_columns_and_fingerprints() {
        assert_eq!(strip_ending(b"a\r\n"), b"a");
        assert_eq!(strip_ending(b"a\n"), b"a");
        assert_eq!(strip_ending(b"a"), b"a");
        assert_eq!(utf16_len("a\u{1F600}b"), 4);
        assert_eq!(fingerprint(b"x"), fingerprint(b"x"));
        assert_ne!(fingerprint(b"x"), fingerprint(b"y"));
    }
}
