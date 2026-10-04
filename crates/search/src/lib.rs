//! Find in Files and Replace in Files for Eludite (brief 0042, PLAN.md 4.11): ripgrep's engines behind a small API
//! the shell and `eludite.search.*` call.
//!
//! - **Walking** ([`search`]): the `ignore` crate's parallel walker, one thread per available core, over the
//!   scope's folders, honoring `.gitignore` (and `.git/info/exclude`, the global excludes file and `.ignore`) when
//!   [`Filters::use_gitignore`] is on, the excluded globs ([`Filters::excludes`], the setting `search.excludes`, and
//!   [`Filters::exclude`]) as overrides that also prune folders, the include globs ([`Filters::include`], Visual
//!   Studio's File types: `!` excludes), the largest file size and symbolic links ([`Filters::follow_symlinks`]).
//! - **Matching** ([`Query`]): `grep-regex`'s matcher (literal fast paths, `\n` never matched, `$` before `\r\n`)
//!   drives `grep-searcher` line by line, multiline off; binary files are skipped by ripgrep's NUL rule; a UTF-8 or
//!   UTF-16 byte order mark decides the encoding, UTF-8 otherwise. The ranges of the matches on a matching line come
//!   from the same pattern in the `regex` crate. Match case, Match whole word (Visual Studio's: a word boundary where
//!   the query's own end is an identifier character; `\b` on both ends of a regular expression) and regular
//!   expressions (Rust syntax) are the query's switches.
//! - **Streaming**: each matching file is handed to the caller's sink as one [`FileMatches`] (its lines in order) as
//!   soon as it is searched, from the walker's threads; a [`CancelToken`] stops the walk and the current file at its
//!   next match, and [`Request::max_results`] stops everything at the cap ([`Summary::truncated`]).
//! - **The open-documents overlay** ([`Overlay`]): a document open in an editor is searched from the text the shell
//!   supplies (unsaved edits included) instead of its file, under its path, once.
//! - **Replacement** ([`replace::replacements`]): the edits a replacement makes in a file's matches, with `$1`
//!   groups for regular expressions, as byte ranges in the searched text and as line and UTF-16 columns for LSP.
//!   [`fingerprint`] lets the caller check that a file still has the text that was searched before editing it.
//!
//! The boundary: paths in and out are absolute and as the caller gave them (normalize before); nothing here knows
//! about the shell, the command bus or the settings store, and nothing here writes files.

mod engine;
mod query;
pub mod replace;
mod text;

pub use engine::{
    CancelToken, DEFAULT_EXCLUDES, FileMatches, Filters, LineMatch, Overlay, Request, Scope,
    SearchError, Summary, search,
};
pub use query::{Compiled, Query, QueryError};
pub use text::{Encoding, fingerprint, utf16_len};
