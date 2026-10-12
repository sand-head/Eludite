//! The search engine behind the editor's Quick Find and Quick Replace
//! (brief 0068), with Visual Studio's semantics.
//!
//! [`Matcher::new`] compiles a query under [`SearchOptions`] (Match case,
//! Match whole word, Use regular expressions) into one [`regex::Regex`]:
//! plain text is escaped, a case-insensitive search folds Unicode case, and
//! a regular expression runs in multi-line mode (`^` and `$` per line).
//! Searches take the whole `\n`-normalized buffer text and a byte range to
//! search in (the document, or the selection under "Current selection"), so
//! the text around the range still counts for whole-word checks and anchors.
//!
//! Matches are byte ranges into the text, sorted and non-overlapping, never
//! empty: a zero-length regex match (`^`, `\b`, `x*`) is skipped one char on.
//! The whole-word rule is Visual Studio's: a match is rejected when it starts
//! with a word char right after a word char, or ends with a word char right
//! before one (a word char is alphanumeric or `_`); a match whose edge is not
//! a word char (`.x`) is not rejected on that edge. Only the chars at the
//! boundaries are looked at, and nothing is allocated per match beyond the
//! result `Vec`.
//!
//! [`Matcher::replacement`] gives Quick Replace's text for one match: with
//! regular expressions on, `$1`, `${1}`, `${name}` and `$$` expand as
//! [`regex::Captures::expand`] does; otherwise it is literal.

use std::ops::Range;

use regex::{Regex, RegexBuilder};

/// The options of one search.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SearchOptions {
    /// Match case: off folds Unicode case.
    pub match_case: bool,
    /// Match whole word, Visual Studio's rule (see the module docs).
    pub whole_word: bool,
    /// Use regular expressions (the `regex` crate's syntax, multi-line).
    pub regex: bool,
}

/// A compiled query.
#[derive(Clone, Debug)]
pub struct Matcher {
    regex: Regex,
    whole_word: bool,
    is_regex: bool,
}

impl Matcher {
    /// Compiles `query` under `options`. Plain text is escaped; a
    /// case-insensitive search folds Unicode case; a regular expression runs
    /// in multi-line mode. The error is the regex crate's message on one line.
    pub fn new(query: &str, options: &SearchOptions) -> Result<Matcher, String> {
        let pattern = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let regex = RegexBuilder::new(&pattern)
            .case_insensitive(!options.match_case)
            .multi_line(options.regex)
            .unicode(true)
            .build()
            .map_err(|error| one_line(&error.to_string()))?;
        Ok(Matcher {
            regex,
            whole_word: options.whole_word,
            is_regex: options.regex,
        })
    }

    /// Every non-empty match within `range` of `text`, sorted and
    /// non-overlapping, stopping after `limit` matches when one is given.
    pub fn find_all(
        &self,
        text: &str,
        range: Range<usize>,
        limit: Option<usize>,
    ) -> Vec<Range<usize>> {
        let mut matches = Vec::new();
        if limit == Some(0) {
            return matches;
        }
        self.scan(text, range.start, range.end, |m| {
            matches.push(m);
            limit.is_none_or(|limit| matches.len() < limit)
        });
        matches
    }

    /// The first match starting at or after `from`, within `range`. Wrapping
    /// around is the caller's job.
    pub fn find_next(&self, text: &str, from: usize, range: Range<usize>) -> Option<Range<usize>> {
        let mut found = None;
        self.scan(text, from.max(range.start), range.end, |m| {
            found = Some(m);
            false
        });
        found
    }

    /// The last match whose start is before `before`, within `range`. The
    /// matches are the ones [`Matcher::find_all`] gives for `range`, so this
    /// scans forward from `range.start`.
    pub fn find_previous(
        &self,
        text: &str,
        before: usize,
        range: Range<usize>,
    ) -> Option<Range<usize>> {
        let mut found = None;
        self.scan(text, range.start, range.end, |m| {
            if m.start >= before {
                return false;
            }
            found = Some(m);
            true
        });
        found
    }

    /// The replacement text for the match `m` of `text`: with regular
    /// expressions on, `$1`, `${1}`, `${name}` and `$$` expand; otherwise
    /// `replacement` is literal.
    pub fn replacement(&self, text: &str, m: Range<usize>, replacement: &str) -> String {
        if !self.is_regex {
            return replacement.to_owned();
        }
        let Some(captures) = self.regex.captures_at(text, m.start) else {
            return replacement.to_owned();
        };
        let mut expanded = String::new();
        captures.expand(replacement, &mut expanded);
        expanded
    }

    /// Whether the query is a regular expression.
    pub fn is_regex(&self) -> bool {
        self.is_regex
    }

    /// Calls `found` with each accepted match starting at or after `start`
    /// and ending at or before `end`, until it returns false.
    fn scan(
        &self,
        text: &str,
        start: usize,
        end: usize,
        mut found: impl FnMut(Range<usize>) -> bool,
    ) {
        let end = end.min(text.len());
        let mut at = start;
        while at <= end {
            let Some(m) = self.regex.find_at(text, at) else {
                return;
            };
            if m.end() > end {
                return;
            }
            if m.is_empty() {
                at = next_char(text, m.start());
                continue;
            }
            if self.whole_word && !is_whole_word(text, m.start(), m.end()) {
                at = next_char(text, m.start());
                continue;
            }
            if !found(m.range()) {
                return;
            }
            at = m.end();
        }
    }
}

/// The offset one char after `at`, or past the end of `text`.
fn next_char(text: &str, at: usize) -> usize {
    at + text[at..].chars().next().map_or(1, char::len_utf8)
}

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Visual Studio's whole-word rule for the non-empty match `start..end`.
fn is_whole_word(text: &str, start: usize, end: usize) -> bool {
    let found = &text[start..end];
    let starts_in_word = found.chars().next().is_some_and(is_word)
        && text[..start].chars().next_back().is_some_and(is_word);
    let ends_in_word = found.chars().next_back().is_some_and(is_word)
        && text[end..].chars().next().is_some_and(is_word);
    !starts_in_word && !ends_in_word
}

/// The regex crate's message on one line: its `error: ` line when it has
/// one (a syntax error), else its lines joined.
fn one_line(message: &str) -> String {
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if let Some(error) = lines.iter().find_map(|l| l.strip_prefix("error: ")) {
        return format!("regex parse error: {error}");
    }
    lines.join(" ")
}

// ----- the editor's Quick Find state (brief 0068) -----

use std::cell::RefCell;

use text::Anchor;

use crate::editor::{Editor, SelectionRange};

/// The matches of the current query, for one buffer version.
#[derive(Debug)]
struct MatchCache {
    version: clock::Global,
    key: (String, SearchOptions, Vec<Range<usize>>),
    result: Result<Vec<Range<usize>>, String>,
}

/// Quick Find's options beyond [`crate::FindQuery`], its scope, and the cached matches.
#[derive(Debug, Default)]
pub(crate) struct FindState {
    pub whole_word: bool,
    pub regex: bool,
    /// Find in Selection: the selections when it was turned on.
    pub scope: Option<Vec<Range<Anchor>>>,
    cache: RefCell<Option<MatchCache>>,
}

impl Editor {
    /// Match Case, Match Whole Word and Use Regular Expressions.
    pub fn find_options(&self) -> SearchOptions {
        SearchOptions {
            match_case: self.find.case_sensitive,
            whole_word: self.ops.find.whole_word,
            regex: self.ops.find.regex,
        }
    }

    pub fn set_find_options(&mut self, options: SearchOptions) {
        self.find.case_sensitive = options.match_case;
        self.ops.find.whole_word = options.whole_word;
        self.ops.find.regex = options.regex;
    }

    /// Find in Selection: on limits the search to the current non-empty selections (off when there are none).
    pub fn set_find_in_selection(&mut self, on: bool) {
        self.ops.find.scope = if on {
            let ranges: Vec<Range<Anchor>> = self
                .selections()
                .into_iter()
                .filter(|s| !s.is_empty())
                .map(|s| self.buffer.anchor_before(s.start())..self.buffer.anchor_after(s.end()))
                .collect();
            (!ranges.is_empty()).then_some(ranges)
        } else {
            None
        };
    }

    pub fn find_in_selection(&self) -> bool {
        self.ops.find.scope.is_some()
    }

    fn find_scopes(&self) -> Vec<Range<usize>> {
        match &self.ops.find.scope {
            Some(ranges) => ranges
                .iter()
                .map(|r| {
                    self.buffer.offset_for_anchor(&r.start)..self.buffer.offset_for_anchor(&r.end)
                })
                .collect(),
            None => std::iter::once(0..self.buffer.len()).collect(),
        }
    }

    fn with_matches<R>(&self, f: impl FnOnce(&Result<Vec<Range<usize>>, String>) -> R) -> R {
        let version = self.buffer.version();
        let key = (
            self.find.text.clone(),
            self.find_options(),
            self.find_scopes(),
        );
        let mut cache = self.ops.find.cache.borrow_mut();
        let fresh = cache
            .as_ref()
            .is_some_and(|c| c.version == version && c.key == key);
        if !fresh {
            let result = if key.0.is_empty() {
                Ok(Vec::new())
            } else {
                Matcher::new(&key.0, &key.1).map(|m| {
                    let text = self.buffer.text();
                    key.2
                        .iter()
                        .flat_map(|scope| m.find_all(&text, scope.clone(), None))
                        .collect()
                })
            };
            *cache = Some(MatchCache {
                version,
                key,
                result,
            });
        }
        f(&cache.as_ref().expect("filled above").result)
    }

    /// The pattern's error, when Use Regular Expressions is on and the query does not compile.
    pub fn find_error(&self) -> Option<String> {
        self.with_matches(|r| r.as_ref().err().cloned())
    }

    /// How many matches the query has in scope.
    pub fn find_match_count(&self) -> usize {
        self.with_matches(|r| r.as_ref().map_or(0, Vec::len))
    }

    /// Which match the primary selection is (for "3 of 17").
    pub fn find_match_index(&self) -> Option<usize> {
        let sel = self.primary_selection().range();
        self.with_matches(|r| {
            let m = r.as_ref().ok()?;
            m.binary_search_by_key(&sel.start, |m| m.start)
                .ok()
                .filter(|&i| m[i] == sel)
        })
    }

    /// Matches of the current query in `range`, for highlighting.
    pub fn find_matches(&self, range: Range<usize>) -> Vec<Range<usize>> {
        self.with_matches(|r| {
            let Ok(m) = r else {
                return Vec::new();
            };
            let from = m.partition_point(|m| m.end <= range.start);
            m[from..]
                .iter()
                .take_while(|m| m.start < range.end)
                .cloned()
                .collect()
        })
    }

    /// Select the next match after the primary selection, wrapping at the end. False when there is no match.
    pub fn find_next(&mut self) -> bool {
        let from = self.primary_selection().end();
        let m = self.with_matches(|r| {
            let m = r.as_ref().ok()?;
            let i = m.partition_point(|m| m.start < from);
            m.get(i).or_else(|| m.first()).cloned()
        });
        self.select_found(m)
    }

    /// Select the previous match before the primary selection, wrapping.
    pub fn find_previous(&mut self) -> bool {
        let to = self.primary_selection().start();
        let m = self.with_matches(|r| {
            let m = r.as_ref().ok()?;
            let i = m.partition_point(|m| m.start < to);
            (if i > 0 { m.get(i - 1) } else { m.last() }).cloned()
        });
        self.select_found(m)
    }

    fn select_found(&mut self, m: Option<Range<usize>>) -> bool {
        match m {
            Some(m) => {
                self.set_selections(
                    vec![SelectionRange {
                        tail: m.start,
                        head: m.end,
                    }],
                    0,
                );
                true
            }
            None => false,
        }
    }

    /// Replace Next: replace the match the primary selection is (with `$1` groups when the query is a regular
    /// expression), then select the next match. False when there was nothing to replace.
    pub fn replace_next(&mut self, replacement: &str) -> bool {
        let Some(i) = self.find_match_index() else {
            self.find_next();
            return false;
        };
        let m = self.with_matches(|r| r.as_ref().map(|m| m[i].clone()).ok());
        let Some(m) = m else {
            return false;
        };
        let Ok(matcher) = Matcher::new(&self.find.text, &self.find_options()) else {
            return false;
        };
        let text = self.buffer.text();
        let with = matcher.replacement(&text, m.clone(), replacement);
        let end = m.start + with.len();
        self.step(vec![(m, with)], Some((vec![SelectionRange::caret(end)], 0)));
        self.find_next();
        true
    }

    /// Replace All: every match in scope, as one undo step. Returns how many were replaced.
    pub fn replace_all(&mut self, replacement: &str) -> usize {
        let matches = self.with_matches(|r| r.as_ref().cloned().unwrap_or_default());
        if matches.is_empty() {
            return 0;
        }
        let Ok(matcher) = Matcher::new(&self.find.text, &self.find_options()) else {
            return 0;
        };
        let edits: Vec<(Range<usize>, String)> = if matcher.is_regex() {
            let text = self.buffer.text();
            matches
                .iter()
                .map(|m| {
                    (
                        m.clone(),
                        matcher.replacement(&text, m.clone(), replacement),
                    )
                })
                .collect()
        } else {
            matches
                .iter()
                .map(|m| (m.clone(), replacement.to_owned()))
                .collect()
        };
        let n = edits.len();
        self.step(edits, None);
        n
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn options(match_case: bool, whole_word: bool, regex: bool) -> SearchOptions {
        SearchOptions {
            match_case,
            whole_word,
            regex,
        }
    }

    fn matcher(query: &str, options: SearchOptions) -> Matcher {
        Matcher::new(query, &options).expect("the query compiles")
    }

    fn all<'a>(matcher: &Matcher, text: &'a str) -> Vec<&'a str> {
        matcher
            .find_all(text, 0..text.len(), None)
            .into_iter()
            .map(|m| &text[m])
            .collect()
    }

    #[test]
    fn case_insensitive_search_folds_unicode_case() {
        let text = "une école, UNE ÉCOLE, Une École";
        assert_eq!(
            all(&matcher("ÉCOLE", options(false, false, false)), text),
            ["école", "ÉCOLE", "École"]
        );
        let greek = "σας ΣΑΣ Σας";
        assert_eq!(
            all(&matcher("ΣΑΣ", options(false, false, false)), greek),
            ["σας", "ΣΑΣ", "Σας"]
        );
    }

    #[test]
    fn match_case_matches_only_the_same_case() {
        let text = "école ÉCOLE École";
        assert_eq!(
            all(&matcher("École", options(true, false, false)), text),
            ["École"]
        );
        assert_eq!(
            all(&matcher("foo", options(true, false, false)), "Foo foo FOO"),
            ["foo"]
        );
    }

    #[test]
    fn plain_text_is_literal() {
        let text = "a.b axb (a.b) $1 a\\nb";
        assert_eq!(
            all(&matcher("a.b", options(false, false, false)), text),
            ["a.b", "a.b"]
        );
        assert_eq!(
            all(&matcher("(a.b)", options(false, false, false)), text),
            ["(a.b)"]
        );
        assert_eq!(
            all(&matcher("a\\nb", options(false, false, false)), text),
            ["a\\nb"]
        );
        assert_eq!(
            all(&matcher("^", options(false, false, false)), "^ a ^"),
            ["^", "^"]
        );
    }

    #[test]
    fn whole_word_follows_visual_studios_rule() {
        let text = "Foo FooBar _Foo Foo.";
        let m = matcher("Foo", options(true, true, false));
        assert_eq!(m.find_all(text, 0..text.len(), None), [0..3, 16..19]);
        // A query whose first char is not a word char is not rejected for
        // the word char before it.
        assert_eq!(
            all(&matcher(".x", options(true, true, false)), "a.x a.xy"),
            [".x"]
        );
        // Nor one whose last char is not a word char, for the one after it.
        assert_eq!(
            all(&matcher("x.", options(true, true, false)), "x.a ax."),
            ["x."]
        );
        // A rejected match does not hide a later one starting inside it.
        assert_eq!(
            all(&matcher("aa", options(true, true, false)), "aaa aa"),
            ["aa"]
        );
    }

    #[test]
    fn regex_groups_and_replacement_expand() {
        let text = "let x = 1;\nlet yy = 22;";
        let m = matcher(r"let (?<name>\w+) = (\d+);", options(true, false, true));
        let found = m.find_all(text, 0..text.len(), None);
        assert_eq!(found, [0..10, 11..23]);
        assert_eq!(m.replacement(text, found[1].clone(), "$2"), "22");
        assert_eq!(m.replacement(text, found[1].clone(), "${2}0"), "220");
        assert_eq!(m.replacement(text, found[1].clone(), "${name}=$2"), "yy=22");
        assert_eq!(m.replacement(text, found[0].clone(), "$$${name}"), "$x");
        assert!(m.is_regex());
    }

    #[test]
    fn plain_replacement_is_literal() {
        let text = "a.b";
        let m = matcher("a.b", options(false, false, false));
        assert_eq!(m.replacement(text, 0..3, "$1 ${name} $$"), "$1 ${name} $$");
        assert!(!m.is_regex());
    }

    #[test]
    fn an_invalid_regex_is_a_one_line_error() {
        let error = Matcher::new("(abc", &options(false, false, true)).expect_err("unclosed group");
        assert!(!error.contains('\n'), "{error}");
        assert!(error.contains("unclosed group"), "{error}");
        // The same text is fine as plain text.
        assert!(Matcher::new("(abc", &options(false, false, false)).is_ok());
    }

    #[test]
    fn a_range_limits_the_search_but_its_context_counts_for_whole_words() {
        let text = "foo foobar foo";
        let m = matcher("foo", options(true, false, false));
        assert_eq!(m.find_all(text, 4..text.len(), None), [4..7, 11..14]);
        // A match crossing the range's end is outside it.
        assert_eq!(m.find_all(text, 0..9, None), [0..3, 4..7]);
        assert!(m.find_all(text, 0..2, None).is_empty());
        // "foo" in "foobar" is whole-word inside the range 4..7 alone, but
        // not with the "bar" after the range.
        let whole = matcher("foo", options(true, true, false));
        assert!(whole.find_all(text, 4..7, None).is_empty());
        assert_eq!(
            whole.find_all(text, 11..14, None),
            [Range { start: 11, end: 14 }]
        );
        // Nor "bar" at a range starting after "foo".
        let bar = matcher("bar", options(true, true, false));
        assert!(bar.find_all(text, 7..10, None).is_empty());
        // Anchors see the text before the range: ^ is not at 2.
        let start = matcher("^o", options(true, false, true));
        assert_eq!(
            start.find_all("foo\noo", 2..6, None),
            [Range { start: 4, end: 5 }]
        );
    }

    #[test]
    fn find_next_and_find_previous() {
        let text = "ab ab ab";
        let m = matcher("ab", options(true, false, false));
        let range = 0..text.len();
        assert_eq!(m.find_next(text, 0, range.clone()), Some(0..2));
        assert_eq!(m.find_next(text, 1, range.clone()), Some(3..5));
        assert_eq!(m.find_next(text, 3, range.clone()), Some(3..5));
        assert_eq!(m.find_next(text, 7, range.clone()), None);
        assert_eq!(m.find_previous(text, 8, range.clone()), Some(6..8));
        assert_eq!(m.find_previous(text, 6, range.clone()), Some(3..5));
        assert_eq!(m.find_previous(text, 4, range.clone()), Some(3..5));
        assert_eq!(m.find_previous(text, 0, range.clone()), None);
        // Within a range.
        assert_eq!(m.find_next(text, 0, 2..text.len()), Some(3..5));
        assert_eq!(m.find_previous(text, 8, 0..5), Some(3..5));
    }

    #[test]
    fn a_limit_stops_the_search() {
        let text = "x x x x";
        let m = matcher("x", options(true, false, false));
        assert_eq!(m.find_all(text, 0..text.len(), Some(2)), [0..1, 2..3]);
        assert!(m.find_all(text, 0..text.len(), Some(0)).is_empty());
        assert_eq!(m.find_all(text, 0..text.len(), Some(10)).len(), 4);
    }

    #[test]
    fn empty_regex_matches_are_skipped() {
        let text = "one\ntwo\n";
        assert_eq!(
            all(&matcher("^", options(false, false, true)), text),
            Vec::<&str>::new()
        );
        assert_eq!(
            all(&matcher("x*", options(false, false, true)), "axxb"),
            ["xx"]
        );
        assert_eq!(
            all(&matcher("", options(false, false, false)), "abc"),
            Vec::<&str>::new()
        );
        // Multi-byte chars are stepped over whole.
        assert_eq!(
            all(&matcher("é*", options(true, false, true)), "aéébé"),
            ["éé", "é"]
        );
    }

    #[test]
    fn regexes_are_multi_line() {
        let text = "fn a() {\n    x;\n\ty;\nz;\n";
        let m = matcher(r"^[ \t]+", options(false, false, true));
        assert_eq!(m.find_all(text, 0..text.len(), None), [9..13, 16..17]);
        let ends = matcher(r";$", options(false, false, true));
        assert_eq!(ends.find_all(text, 0..text.len(), None).len(), 3);
    }

    #[test]
    fn a_large_text_finds_every_match() {
        let mut text = String::with_capacity(2_100_000);
        for line in 0..50_000 {
            if line % 5 == 0 {
                text.push_str("    let needle = compute(value, other_value); // a comment\n");
            } else {
                text.push_str("    let haystack = compute(value, other_value); // comment\n");
            }
        }
        let m = matcher("needle", options(false, true, false));
        assert_eq!(m.find_all(&text, 0..text.len(), None).len(), 10_000);
    }
}
