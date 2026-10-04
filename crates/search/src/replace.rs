//! The replacement engine: the edits a replacement makes in a file's matches.
//!
//! Matches never span lines (multiline is off), so each edit is on one line. An edit is given twice: as a byte range
//! in the searched text (`(path, Vec<(Range, String)>)` with [`file_edits`]) and as a line with UTF-16 columns, the
//! position the workspace-edit applier takes (LSP), which does not depend on the file's line endings or byte order
//! mark.

use std::ops::Range;
use std::path::PathBuf;

use crate::engine::FileMatches;
use crate::query::Compiled;
use crate::text::utf16_len;

/// One replacement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    /// 1-based.
    pub line: u64,
    /// The match's byte range in the searched text.
    pub range: Range<usize>,
    /// Its columns on the line, 0-based, in UTF-16 code units (LSP's), end exclusive.
    pub start_utf16: u32,
    pub end_utf16: u32,
    /// The replacement, groups expanded.
    pub text: String,
}

/// The edits replacing every match of `file` (as searched by `compiled`) with `replacement`: `$1`, `${name}` and
/// `$0` expand for a regular expression, `$$` is a dollar; literal text is inserted as it is.
pub fn replacements(compiled: &Compiled, file: &FileMatches, replacement: &str) -> Vec<Edit> {
    let mut out = Vec::new();
    for line in &file.lines {
        let wanted: Vec<&Range<usize>> = line.ranges.iter().collect();
        for (range, text) in compiled.replacements(line.text.as_bytes(), replacement) {
            // Only the matches the search kept (the cap may have cut a line short).
            if !wanted.contains(&&range) {
                continue;
            }
            let (Some(head), Some(body)) =
                (line.text.get(..range.start), line.text.get(range.clone()))
            else {
                continue;
            };
            let start = utf16_len(head) as u32;
            out.push(Edit {
                line: line.line,
                range: line.offset as usize + range.start..line.offset as usize + range.end,
                start_utf16: start,
                end_utf16: start + utf16_len(body) as u32,
                text,
            });
        }
    }
    out
}

/// The edits of `file` as `(path, [(byte range in the searched text, replacement)])`.
pub fn file_edits(
    compiled: &Compiled,
    file: &FileMatches,
    replacement: &str,
) -> (PathBuf, Vec<(Range<usize>, String)>) {
    let edits = replacements(compiled, file, replacement)
        .into_iter()
        .map(|e| (e.range, e.text))
        .collect();
    (file.path.clone(), edits)
}

/// `text` with `edits` (sorted, not overlapping byte ranges) applied: the preview of a replacement.
pub fn apply(text: &str, edits: &[(Range<usize>, String)]) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for (r, new) in edits {
        out.push_str(&text[at..r.start]);
        out.push_str(new);
        at = r.end;
    }
    out.push_str(&text[at..]);
    out
}
