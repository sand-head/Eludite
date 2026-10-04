//! Links in a terminal line: urls (`http://`, `https://`, `file://`) and paths, absolute or relative, with a
//! position in MSBuild's and the compilers' forms: `path(line,col)`, `path(line)`, `path:line:col` and `path:line`.
//! A bare word is a path only when it has a folder separator or looks like a file name (`lib.rs`); whether it exists
//! is the caller's question (it knows the terminal's folder). Ranges are in characters of the line, so they map to
//! grid columns for text without wide characters.

use std::ops::Range;
use std::sync::OnceLock;

use regex::Regex;

/// What a link points at.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    Url(String),
    Path {
        path: String,
        line: Option<u32>,
        column: Option<u32>,
    },
}

/// A link and where it is in its line (characters).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Link {
    pub range: Range<usize>,
    pub target: Target,
}

fn url_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r#"(?:https?|file)://[^\s<>"'`]+"#).expect("valid"))
}

fn path_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"(?x)
            (?P<path>(?:[A-Za-z]:[\\/])?[\w.~@+\-\\/$]*[\w~@+\-\\/$])
            (?:
                \((?P<l1>\d+)(?:,\s?(?P<c1>\d+))?\)
              | :(?P<l2>\d+)(?::(?P<c2>\d+))?
            )?",
        )
        .expect("valid")
    })
}

/// Whether `word` reads as a path: it has a folder separator, or a file name with an extension (`lib.rs`).
fn looks_like_path(word: &str) -> bool {
    if word.contains('/') || word.contains('\\') {
        // Not `/`, `//` or a lone `~`.
        return word
            .chars()
            .any(|c| c.is_alphanumeric() || c == '.' || c == '~');
    }
    match word.rsplit_once('.') {
        Some((stem, ext)) => {
            !stem.is_empty()
                && !ext.is_empty()
                && ext.len() <= 10
                && ext.chars().all(|c| c.is_ascii_alphanumeric())
                && ext.chars().any(|c| c.is_ascii_alphabetic())
                && stem.chars().any(|c| c.is_alphanumeric())
        }
        None => false,
    }
}

/// The links in `line`, left to right.
pub fn find_links(line: &str) -> Vec<Link> {
    let char_of = |byte: usize| line[..byte].chars().count();
    let mut links = Vec::new();
    let mut taken: Vec<Range<usize>> = Vec::new();
    for m in url_re().find_iter(line) {
        let url = m
            .as_str()
            .trim_end_matches(['.', ',', ';', ':', ')', ']', '}', '!', '?']);
        let range = m.start()..m.start() + url.len();
        taken.push(range.clone());
        links.push(Link {
            range: char_of(range.start)..char_of(range.end),
            target: Target::Url(url.to_owned()),
        });
    }
    for c in path_re().captures_iter(line) {
        let whole = c.get(0).expect("match");
        if taken
            .iter()
            .any(|t| t.start < whole.end() && whole.start() < t.end)
        {
            continue;
        }
        // A path starts at a word boundary: not in the middle of `a=b/c`'s `=` side or after a letter.
        if let Some(prev) = line[..whole.start()].chars().next_back()
            && (prev.is_alphanumeric() || prev == '_')
        {
            continue;
        }
        let path_m = c.name("path").expect("path");
        let mut path = path_m.as_str();
        let number = |name: &str| c.name(name).and_then(|m| m.as_str().parse::<u32>().ok());
        let (line_no, column) = (
            number("l1").or_else(|| number("l2")),
            number("c1").or_else(|| number("c2")),
        );
        let mut end = whole.end();
        if line_no.is_none() {
            // A sentence's full stop is not part of the path.
            let trimmed = path.trim_end_matches(['.', ',']);
            end -= path.len() - trimmed.len();
            path = trimmed;
        }
        if !looks_like_path(path) {
            continue;
        }
        links.push(Link {
            range: char_of(whole.start())..char_of(end),
            target: Target::Path {
                path: path.to_owned(),
                line: line_no,
                column,
            },
        });
    }
    links.sort_by_key(|l| l.range.start);
    links
}

/// The link under character `column` of `line`.
pub fn link_at(line: &str, column: usize) -> Option<Link> {
    find_links(line)
        .into_iter()
        .find(|l| l.range.contains(&column))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn path(p: &str, line: Option<u32>, column: Option<u32>) -> Target {
        Target::Path {
            path: p.into(),
            line,
            column,
        }
    }

    fn targets(line: &str) -> Vec<Target> {
        find_links(line).into_iter().map(|l| l.target).collect()
    }

    #[test]
    fn the_compilers_and_msbuilds_forms() {
        assert_eq!(
            targets("src/Program.cs(12,5): error CS1002: ; expected"),
            [path("src/Program.cs", Some(12), Some(5))]
        );
        assert_eq!(
            targets("  --> src/lib.rs:3:1"),
            [path("src/lib.rs", Some(3), Some(1))]
        );
        assert_eq!(
            targets("main.go:7: undefined: x"),
            [path("main.go", Some(7), None)]
        );
        assert_eq!(targets("Foo.cs(4)"), [path("Foo.cs", Some(4), None)]);
        assert_eq!(
            targets(r"C:\src\App\Program.cs(1,2): warning"),
            [path(r"C:\src\App\Program.cs", Some(1), Some(2))]
        );
        assert_eq!(
            targets("/home/me/x.txt and ./a/b and ~/notes.md."),
            [
                path("/home/me/x.txt", None, None),
                path("./a/b", None, None),
                path("~/notes.md", None, None)
            ]
        );
    }

    #[test]
    fn urls_and_what_is_not_a_link() {
        assert_eq!(
            targets("see https://example.com/a?b=1, then http://localhost:5000/."),
            [
                Target::Url("https://example.com/a?b=1".into()),
                Target::Url("http://localhost:5000/".into())
            ]
        );
        assert!(targets("hello world 1.5 v2 / // $ ").is_empty());
    }

    #[test]
    fn ranges_are_characters_and_link_at_finds_the_one_under_a_column() {
        let line = "\u{e9}\u{e9} src/lib.rs:3:1 x";
        let links = find_links(line);
        assert_eq!(links[0].range, 3..17);
        assert_eq!(
            link_at(line, 10).map(|l| l.target),
            Some(path("src/lib.rs", Some(3), Some(1)))
        );
        assert!(link_at(line, 1).is_none());
        assert!(link_at(line, 17).is_none());
    }
}
