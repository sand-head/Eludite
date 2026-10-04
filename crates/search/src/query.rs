//! The query: Visual Studio's switches compiled into ripgrep's matcher and a line regex.

use std::fmt;
use std::ops::Range;

use grep_regex::{RegexMatcher, RegexMatcherBuilder};

/// What to find, with Visual Studio's Find in Files switches.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Query {
    pub text: String,
    /// Use regular expressions (Rust regex syntax); otherwise `text` is literal.
    pub regex: bool,
    pub case_sensitive: bool,
    /// Match whole word.
    pub whole_word: bool,
}

/// Why a query does not compile.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QueryError(pub String);

impl fmt::Display for QueryError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for QueryError {}

/// An identifier character for Match whole word: a letter, a digit (Unicode) or `_`.
fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

impl Query {
    pub fn new(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            ..Self::default()
        }
    }

    /// The regular expression this query searches for (before case folding).
    pub fn pattern(&self) -> String {
        if self.regex {
            return if self.whole_word {
                format!(r"\b(?:{})\b", self.text)
            } else {
                self.text.clone()
            };
        }
        let escaped = regex::escape(&self.text);
        if !self.whole_word {
            return escaped;
        }
        // Visual Studio's rule for literal text: the characters around the match are not identifier characters where
        // the text's own first or last character is one (so `.Foo` still matches `x.Foo`).
        let first = self.text.chars().next().is_some_and(is_word_char);
        let last = self.text.chars().last().is_some_and(is_word_char);
        format!(
            "{}{escaped}{}",
            if first { r"\b" } else { "" },
            if last { r"\b" } else { "" }
        )
    }

    /// Compile for searching. An empty query, a pattern that does not parse, or one that would match a line ending
    /// (multiline is off) is an error.
    pub fn compile(&self) -> Result<Compiled, QueryError> {
        if self.text.is_empty() {
            return Err(QueryError("the query is empty".into()));
        }
        let pattern = self.pattern();
        let grep = RegexMatcherBuilder::new()
            .case_insensitive(!self.case_sensitive)
            .line_terminator(Some(b'\n'))
            .crlf(true)
            .build(&pattern)
            .map_err(|e| QueryError(clean(&e.to_string())))?;
        let line = regex::bytes::RegexBuilder::new(&pattern)
            .case_insensitive(!self.case_sensitive)
            .build()
            .map_err(|e| QueryError(clean(&e.to_string())))?;
        Ok(Compiled {
            grep,
            line,
            expand: self.regex,
        })
    }
}

/// The regex crates' multi-line error texts, on one line.
fn clean(message: &str) -> String {
    let lines: Vec<&str> = message
        .lines()
        .map(str::trim_end)
        .filter(|l| !l.is_empty())
        .collect();
    lines.join(" ")
}

/// A compiled query: ripgrep's matcher for finding the matching lines, and the same pattern for the matches on a
/// line and their groups.
#[derive(Debug, Clone)]
pub struct Compiled {
    pub(crate) grep: RegexMatcher,
    pub(crate) line: regex::bytes::Regex,
    /// A regular expression: replacements expand `$1` groups.
    pub(crate) expand: bool,
}

impl Compiled {
    /// The byte ranges of every match in `line` (a line without its ending).
    pub fn ranges(&self, line: &[u8]) -> Vec<Range<usize>> {
        self.line.find_iter(line).map(|m| m.range()).collect()
    }

    /// The replacement text for each match in `line`, by its range: `$1`, `${name}` and `$0` expand for a regular
    /// expression; literal text is inserted as it is.
    pub fn replacements(&self, line: &[u8], replacement: &str) -> Vec<(Range<usize>, String)> {
        if !self.expand {
            return self
                .ranges(line)
                .into_iter()
                .map(|r| (r, replacement.to_owned()))
                .collect();
        }
        self.line
            .captures_iter(line)
            .filter_map(|caps| {
                let whole = caps.get(0)?;
                let mut out = Vec::new();
                caps.expand(replacement.as_bytes(), &mut out);
                Some((whole.range(), String::from_utf8_lossy(&out).into_owned()))
            })
            .collect()
    }
}

#[cfg(test)]
#[allow(clippy::single_range_in_vec_init)]
mod tests {
    use super::*;

    fn q(text: &str) -> Query {
        Query::new(text)
    }

    #[test]
    fn literal_text_is_escaped_and_case_folds_by_default() {
        let c = q("a.b(").compile().unwrap();
        assert_eq!(c.ranges(b"xA.B(y a.b("), [1..5, 7..11]);
        assert!(c.ranges(b"axb(").is_empty());
        let cased = Query {
            case_sensitive: true,
            ..q("Order")
        }
        .compile()
        .unwrap();
        assert_eq!(cased.ranges(b"order Order"), [6..11]);
    }

    #[test]
    fn whole_word_is_visual_studios_rule() {
        let w = |text: &str| Query {
            whole_word: true,
            ..q(text)
        };
        assert_eq!(w("Order").pattern(), r"\bOrder\b");
        let c = w("Order").compile().unwrap();
        assert_eq!(
            c.ranges(b"Order Orders _Order Order_1 (Order)"),
            [0..5, 29..34]
        );
        // Identifier characters include Unicode letters and digits.
        assert!(c.ranges("éOrder Order9".as_bytes()).is_empty());
        // Only the ends that are identifier characters need a boundary.
        let dot = w(".Foo").compile().unwrap();
        assert_eq!(dot.ranges(b"x.Foo x.Foos"), [1..5]);
        // A regular expression gets a boundary on both ends.
        let re = Query {
            regex: true,
            whole_word: true,
            ..q("a|bc")
        };
        assert_eq!(re.pattern(), r"\b(?:a|bc)\b");
        assert_eq!(re.compile().unwrap().ranges(b"a ab bc abc"), [0..1, 5..7]);
    }

    #[test]
    fn regular_expressions_expand_groups_and_reject_line_endings() {
        let re = Query {
            regex: true,
            case_sensitive: true,
            ..q(r"(\w+)\.Count\(\)")
        }
        .compile()
        .unwrap();
        assert_eq!(
            re.replacements(b"if (items.Count() > 0 && xs.Count())", "$1.Length"),
            [
                (4..17, "items.Length".to_owned()),
                (25..35, "xs.Length".to_owned())
            ]
        );
        let named = Query {
            regex: true,
            ..q(r"(?<a>\d+)-(?<b>\d+)")
        }
        .compile()
        .unwrap();
        assert_eq!(
            named.replacements(b"1-2", "${b}-${a} $0 $$"),
            [(0..3, "2-1 1-2 $".to_owned())]
        );
        // Literal replacements are not expanded.
        let lit = q("x").compile().unwrap();
        assert_eq!(lit.replacements(b"x", "$1"), [(0..1, "$1".to_owned())]);
        assert!(
            Query {
                regex: true,
                ..q("a\\nb")
            }
            .compile()
            .is_err()
        );
        assert!(
            Query {
                regex: true,
                ..q("(")
            }
            .compile()
            .unwrap_err()
            .0
            .contains("unclosed")
        );
        assert!(q("").compile().is_err());
    }
}
