//! EditorConfig (<https://spec.editorconfig.org>): what the `.editorconfig`
//! files above a file say about its indentation, line endings, charset and
//! trailing white space. Our own resolver on the standard library alone.
//!
//! # Public API
//!
//! - [`EditorConfig::resolve`] reads the `.editorconfig` files from the
//!   file's folder upward, stopping after the first whose preamble says
//!   `root = true` (or at the file system root). It does blocking file IO,
//!   so callers run it off the UI thread.
//! - [`EditorConfig::resolve_with`] does the same over contents already
//!   read, nearest folder first.
//! - [`glob_matches`] is the section header matcher.
//!
//! # Rules
//!
//! - Farther files apply first, then nearer ones; within a file a later
//!   section overrides an earlier one. A value of `unset` clears a property.
//! - Keys and values are case-insensitive and lowercased. A line starting
//!   with `#` or `;` (after white space) is a comment; blank lines are
//!   ignored. `root` counts only in the preamble, before any section.
//! - A section header `[pattern]` without a `/` matches the file's name at
//!   any depth; one with a `/` is anchored to the `.editorconfig`'s folder
//!   (a leading `/` is dropped). `*` matches any characters but `/`, `**`
//!   any characters including `/` (and `**/` also matches no folder at
//!   all), `?` one character but `/`, `[abc]`, `[a-z]` and `[!abc]` one
//!   character of a class, `{s1,s2}` any of the alternatives (nestable,
//!   globs allowed), `{n1..n2}` a decimal integer in the range, and `\`
//!   escapes the next character. A `{` without a matching `}`, or without a
//!   comma or `..` inside, is literal.
//! - `indent_size = tab` resolves to `tab_width`. An unset `tab_width`
//!   defaults to a numeric `indent_size`, and with `indent_style = tab` an
//!   unset `indent_size` takes `tab_width`. Invalid numbers are ignored.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// What `.editorconfig` files say about one file. `None`: not set (or `unset`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct EditorConfig {
    /// Tabs or spaces for indentation.
    pub indent_style: Option<IndentStyle>,
    /// Columns of one indent level; `indent_size = tab` resolves to `tab_width`.
    pub indent_size: Option<u32>,
    /// Columns of one tab character.
    pub tab_width: Option<u32>,
    /// The line ending new lines get.
    pub end_of_line: Option<EndOfLine>,
    /// Lowercased: `latin1`, `utf-8`, `utf-8-bom`, `utf-16be` or `utf-16le`.
    pub charset: Option<String>,
    /// Whether saving removes white space at line ends.
    pub trim_trailing_whitespace: Option<bool>,
    /// Whether saving ends the file with a newline.
    pub insert_final_newline: Option<bool>,
}

/// `indent_style`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentStyle {
    /// `tab`.
    Tab,
    /// `space`.
    Space,
}

/// `end_of_line`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndOfLine {
    /// `lf`.
    Lf,
    /// `crlf`.
    CrLf,
    /// `cr`.
    Cr,
}

const FILE_NAME: &str = ".editorconfig";

impl EditorConfig {
    /// Read `.editorconfig` from the file's folder up to the first one with
    /// `root = true` (or the file system root) and apply them for `path`
    /// (absolute). Unreadable files are skipped. Blocking file IO: callers
    /// run it off the UI thread.
    pub fn resolve(path: &Path) -> EditorConfig {
        let mut files = Vec::new();
        let mut folder = path.parent();
        while let Some(dir) = folder {
            if let Ok(contents) = std::fs::read_to_string(dir.join(FILE_NAME)) {
                let root = parse(&contents).root;
                files.push((dir.to_path_buf(), contents));
                if root {
                    break;
                }
            }
            folder = dir.parent();
        }
        Self::resolve_with(path, &files)
    }

    /// The same over given (folder, file contents) pairs, nearest folder
    /// FIRST, for tests and callers that already read them. Stops after a
    /// file whose preamble has `root = true`.
    pub fn resolve_with(path: &Path, files: &[(PathBuf, String)]) -> EditorConfig {
        let mut chain = Vec::new();
        for (folder, contents) in files {
            let Some(relative) = relative_path(path, folder) else {
                continue;
            };
            let parsed = parse(contents);
            let root = parsed.root;
            chain.push((relative, parsed));
            if root {
                break;
            }
        }
        let mut raw: HashMap<String, String> = HashMap::new();
        for (relative, parsed) in chain.iter().rev() {
            for section in &parsed.sections {
                if !glob_matches(&section.pattern, relative) {
                    continue;
                }
                for (key, value) in &section.properties {
                    raw.insert(key.clone(), value.clone());
                }
            }
        }
        from_raw(&raw)
    }
}

/// The file's path relative to `folder` with `/` separators, or `None` when
/// `folder` is not one of its ancestors.
fn relative_path(path: &Path, folder: &Path) -> Option<String> {
    let rest = path.strip_prefix(folder).ok()?;
    let parts: Vec<String> = rest
        .components()
        .map(|c| c.as_os_str().to_string_lossy().into_owned())
        .collect();
    if parts.is_empty() {
        return None;
    }
    Some(parts.join("/"))
}

/// Typed properties from the final raw values.
fn from_raw(raw: &HashMap<String, String>) -> EditorConfig {
    let get = |key: &str| raw.get(key).map(String::as_str).filter(|v| *v != "unset");
    let number = |key: &str| get(key).and_then(parse_number);
    let boolean = |key: &str| match get(key) {
        Some("true") => Some(true),
        Some("false") => Some(false),
        _ => None,
    };
    let indent_style = match get("indent_style") {
        Some("tab") => Some(IndentStyle::Tab),
        Some("space") => Some(IndentStyle::Space),
        _ => None,
    };
    let mut tab_width = number("tab_width");
    let indent_size_tab = get("indent_size") == Some("tab");
    let mut indent_size = number("indent_size");
    if indent_size_tab {
        indent_size = tab_width;
    }
    if tab_width.is_none() && !indent_size_tab {
        tab_width = indent_size;
    }
    if indent_size.is_none() && indent_style == Some(IndentStyle::Tab) {
        indent_size = tab_width;
    }
    let end_of_line = match get("end_of_line") {
        Some("lf") => Some(EndOfLine::Lf),
        Some("crlf") => Some(EndOfLine::CrLf),
        Some("cr") => Some(EndOfLine::Cr),
        _ => None,
    };
    let charset = get("charset")
        .filter(|v| {
            matches!(
                *v,
                "latin1" | "utf-8" | "utf-8-bom" | "utf-16be" | "utf-16le"
            )
        })
        .map(str::to_owned);
    EditorConfig {
        indent_style,
        indent_size,
        tab_width,
        end_of_line,
        charset,
        trim_trailing_whitespace: boolean("trim_trailing_whitespace"),
        insert_final_newline: boolean("insert_final_newline"),
    }
}

/// A positive decimal number, or `None`.
fn parse_number(value: &str) -> Option<u32> {
    value.parse::<u32>().ok().filter(|n| *n > 0)
}

/// One parsed `.editorconfig`.
struct Parsed {
    root: bool,
    sections: Vec<Section>,
}

struct Section {
    pattern: String,
    /// Lowercased keys and values, in file order.
    properties: Vec<(String, String)>,
}

fn parse(contents: &str) -> Parsed {
    let mut root = false;
    let mut sections: Vec<Section> = Vec::new();
    for line in contents.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') || line.starts_with(';') {
            continue;
        }
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            sections.push(Section {
                pattern: header.to_owned(),
                properties: Vec::new(),
            });
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let key = key.trim().to_lowercase();
        let value = value.trim().to_lowercase();
        if key.is_empty() {
            continue;
        }
        match sections.last_mut() {
            Some(section) => section.properties.push((key, value)),
            None => {
                if key == "root" {
                    root = value == "true";
                }
            }
        }
    }
    Parsed { root, sections }
}

/// EditorConfig glob match: `pattern` from a section header, `relative` the
/// file's path relative to the .editorconfig's folder with `/` separators.
pub fn glob_matches(pattern: &str, relative: &str) -> bool {
    let (pattern, subject) = if pattern.contains('/') {
        (pattern.strip_prefix('/').unwrap_or(pattern), relative)
    } else {
        let name = relative.rsplit('/').next().unwrap_or(relative);
        (pattern, name)
    };
    let pattern: Vec<char> = pattern.chars().collect();
    let subject: Vec<char> = subject.chars().collect();
    match_here(&pattern, &subject)
}

/// Whether `p` matches all of `t`.
fn match_here(p: &[char], t: &[char]) -> bool {
    let Some(&c) = p.first() else {
        return t.is_empty();
    };
    match c {
        '\\' => match p.get(1) {
            Some(&lit) => literal(lit, &p[2..], t),
            None => literal('\\', &p[1..], t),
        },
        '*' if p.get(1) == Some(&'*') => {
            let mut rest = &p[2..];
            while rest.first() == Some(&'*') {
                rest = &rest[1..];
            }
            if rest.first() == Some(&'/') && match_here(&rest[1..], t) {
                return true;
            }
            (0..=t.len()).any(|i| match_here(rest, &t[i..]))
        }
        '*' => {
            let rest = &p[1..];
            for i in 0..=t.len() {
                if match_here(rest, &t[i..]) {
                    return true;
                }
                if t.get(i) == Some(&'/') {
                    break;
                }
            }
            false
        }
        '?' => matches!(t.first(), Some(&ch) if ch != '/') && match_here(&p[1..], &t[1..]),
        '[' => match parse_class(&p[1..]) {
            Some((class, len)) => {
                matches!(t.first(), Some(&ch) if ch != '/' && class.matches(ch))
                    && match_here(&p[1 + len..], &t[1..])
            }
            None => literal('[', &p[1..], t),
        },
        '{' => match find_close(p) {
            Some(close) => match_brace(&p[1..close], &p[close + 1..], t),
            None => literal('{', &p[1..], t),
        },
        _ => literal(c, &p[1..], t),
    }
}

/// `t` starts with `c` and the rest matches `rest`.
fn literal(c: char, rest: &[char], t: &[char]) -> bool {
    t.first() == Some(&c) && match_here(rest, &t[1..])
}

/// A character class.
struct Class {
    negate: bool,
    ranges: Vec<(char, char)>,
}

impl Class {
    fn matches(&self, c: char) -> bool {
        self.ranges.iter().any(|&(lo, hi)| lo <= c && c <= hi) != self.negate
    }
}

/// The class after a `[`, and how many characters it took including the
/// closing `]`; `None` when it never closes.
fn parse_class(p: &[char]) -> Option<(Class, usize)> {
    let negate = p.first() == Some(&'!');
    let start = usize::from(negate);
    let mut i = start;
    let mut ranges = Vec::new();
    loop {
        let &c = p.get(i)?;
        if c == ']' && i > start {
            return Some((Class { negate, ranges }, i + 1));
        }
        let lo = if c == '\\' {
            i += 1;
            *p.get(i)?
        } else {
            c
        };
        i += 1;
        if p.get(i) == Some(&'-') && p.get(i + 1).is_some_and(|&n| n != ']') {
            i += 1;
            let mut hi = p[i];
            if hi == '\\' {
                i += 1;
                hi = *p.get(i)?;
            }
            i += 1;
            ranges.push((lo, hi));
        } else {
            ranges.push((lo, lo));
        }
    }
}

/// The index of the `}` matching the `{` at `p[0]`, skipping escapes and
/// nested braces.
fn find_close(p: &[char]) -> Option<usize> {
    let mut depth = 0usize;
    let mut i = 0;
    while i < p.len() {
        match p[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// A `{...}` group with body `body`, followed by `rest`.
fn match_brace(body: &[char], rest: &[char], t: &[char]) -> bool {
    let text: String = body.iter().collect();
    if let Some((lo, hi)) = parse_range(&text) {
        let (lo, hi) = (lo.min(hi), lo.max(hi));
        let mut end = usize::from(matches!(t.first(), Some('+' | '-')));
        let digits_start = end;
        while t.get(end).is_some_and(char::is_ascii_digit) {
            end += 1;
        }
        if end == digits_start {
            return false;
        }
        let number: String = t[..end].iter().collect();
        return number
            .parse::<i64>()
            .is_ok_and(|n| lo <= n && n <= hi && match_here(rest, &t[end..]));
    }
    let alternatives = split_alternatives(body);
    if alternatives.len() < 2 {
        let mut literal_p = vec!['\\', '{'];
        literal_p.extend_from_slice(body);
        literal_p.extend_from_slice(&['\\', '}']);
        literal_p.extend_from_slice(rest);
        return match_here(&literal_p, t);
    }
    alternatives.into_iter().any(|alt| {
        let mut joined = alt.to_vec();
        joined.extend_from_slice(rest);
        match_here(&joined, t)
    })
}

/// `n1..n2` with optional signs.
fn parse_range(text: &str) -> Option<(i64, i64)> {
    let (a, b) = text.split_once("..")?;
    let valid = |s: &str| {
        let digits = s.strip_prefix(['+', '-']).unwrap_or(s);
        !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit())
    };
    if !valid(a) || !valid(b) {
        return None;
    }
    Some((a.parse().ok()?, b.parse().ok()?))
}

/// The top-level comma-separated pieces of a brace body.
fn split_alternatives(body: &[char]) -> Vec<&[char]> {
    let mut pieces = Vec::new();
    let mut depth = 0usize;
    let mut start = 0;
    let mut i = 0;
    while i < body.len() {
        match body[i] {
            '\\' => i += 1,
            '{' => depth += 1,
            '}' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                pieces.push(&body[start..i]);
                start = i + 1;
            }
            _ => {}
        }
        i += 1;
    }
    pieces.push(&body[start.min(body.len())..]);
    pieces
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(folder: &str, contents: &str) -> (PathBuf, String) {
        (PathBuf::from(folder), contents.to_owned())
    }

    #[test]
    fn nearer_file_overrides_farther() {
        let files = [
            at("/r/src", "[*.cs]\nindent_size = 2\n"),
            at("/r", "[*]\nindent_style = space\nindent_size = 4\n"),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/src/a.cs"), &files);
        assert_eq!(config.indent_style, Some(IndentStyle::Space));
        assert_eq!(config.indent_size, Some(2));
        assert_eq!(config.tab_width, Some(2));
    }

    #[test]
    fn later_section_overrides_earlier() {
        let files = [at(
            "/r",
            "[*]\nend_of_line = crlf\n[*.sh]\nend_of_line = lf\n[*]\ninsert_final_newline = true\n",
        )];
        let sh = EditorConfig::resolve_with(Path::new("/r/run.sh"), &files);
        assert_eq!(sh.end_of_line, Some(EndOfLine::Lf));
        assert_eq!(sh.insert_final_newline, Some(true));
        let cs = EditorConfig::resolve_with(Path::new("/r/a.cs"), &files);
        assert_eq!(cs.end_of_line, Some(EndOfLine::CrLf));
    }

    #[test]
    fn root_stops_the_walk() {
        let files = [
            at("/r/a", "root = true\n[*]\nindent_size = 3\n"),
            at("/r", "[*]\ncharset = utf-8\nindent_size = 8\n"),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/a/x.txt"), &files);
        assert_eq!(config.indent_size, Some(3));
        assert_eq!(config.charset, None);
    }

    #[test]
    fn root_counts_only_in_the_preamble() {
        let files = [
            at("/r/a", "[*]\nroot = true\nindent_size = 3\n"),
            at("/r", "[*]\ncharset = UTF-8\n"),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/a/x.txt"), &files);
        assert_eq!(config.charset.as_deref(), Some("utf-8"));
    }

    #[test]
    fn folders_that_are_not_ancestors_are_skipped() {
        let files = [
            at("/other", "root = true\n[*]\nindent_size = 3\n"),
            at("/r", "[*]\nindent_size = 5\n"),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/x.txt"), &files);
        assert_eq!(config.indent_size, Some(5));
    }

    #[test]
    fn unset_clears() {
        let files = [
            at(
                "/r/a",
                "[*]\nindent_style = unset\ntrim_trailing_whitespace = UNSET\n",
            ),
            at(
                "/r",
                "[*]\nindent_style = tab\ntrim_trailing_whitespace = true\n",
            ),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/a/x.txt"), &files);
        assert_eq!(config, EditorConfig::default());
    }

    #[test]
    fn indent_size_tab_takes_tab_width() {
        let files = [at("/r", "[*]\nindent_size = tab\ntab_width = 6\n")];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.indent_size, Some(6));
        assert_eq!(config.tab_width, Some(6));
        let files = [at("/r", "[*]\nindent_size = tab\n")];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.indent_size, None);
        assert_eq!(config.tab_width, None);
    }

    #[test]
    fn tab_width_defaults_and_tab_style_takes_it() {
        let files = [at("/r", "[*]\nindent_size = 3\n")];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.tab_width, Some(3));
        let files = [at("/r", "[*]\nindent_style = tab\ntab_width = 8\n")];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.indent_size, Some(8));
        let files = [at("/r", "[*]\nindent_size = 2\ntab_width = 8\n")];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!((config.indent_size, config.tab_width), (Some(2), Some(8)));
    }

    #[test]
    fn invalid_values_are_ignored() {
        let files = [at(
            "/r",
            "[*]\nindent_size = four\ntab_width = -2\nend_of_line = nl\ncharset = ebcdic\ninsert_final_newline = yes\nindent_style = both\n",
        )];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config, EditorConfig::default());
    }

    #[test]
    fn keys_and_values_are_case_insensitive() {
        let files = [at(
            "/r",
            "[*]\nIndent_Style = SPACE\nEND_OF_LINE = CRLF\nCharset = UTF-8-BOM\nTrim_Trailing_Whitespace = True\n",
        )];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.indent_style, Some(IndentStyle::Space));
        assert_eq!(config.end_of_line, Some(EndOfLine::CrLf));
        assert_eq!(config.charset.as_deref(), Some("utf-8-bom"));
        assert_eq!(config.trim_trailing_whitespace, Some(true));
    }

    #[test]
    fn comments_and_blank_lines() {
        let files = [at(
            "/r",
            "# top\n  ; also a comment\n\nroot = true\n\n[*]\n# indent_size = 9\n   ;tab_width = 9\nindent_size = 4\n",
        )];
        let config = EditorConfig::resolve_with(Path::new("/r/x"), &files);
        assert_eq!(config.indent_size, Some(4));
        assert_eq!(config.tab_width, Some(4));
    }

    #[test]
    fn star_and_extension_globs() {
        assert!(glob_matches("*", "a.cs"));
        assert!(glob_matches("*", "deep/down/a.cs"));
        assert!(glob_matches("*.cs", "a.cs"));
        assert!(glob_matches("*.cs", "src/x/a.cs"));
        assert!(!glob_matches("*.cs", "a.csx"));
        assert!(glob_matches("**.cs", "src/x/a.cs"));
        assert!(glob_matches("a?.txt", "ab.txt"));
        assert!(!glob_matches("a?.txt", "a.txt"));
    }

    #[test]
    fn alternatives() {
        assert!(glob_matches("{*.json,*.yml}", "a.json"));
        assert!(glob_matches("{*.json,*.yml}", "conf/b.yml"));
        assert!(!glob_matches("{*.json,*.yml}", "c.yaml"));
        assert!(glob_matches("*.{cs,{vb,fs}}", "a.fs"));
        assert!(glob_matches("{a,}b", "b"));
        // No comma: literal.
        assert!(glob_matches("{single}", "{single}"));
        assert!(!glob_matches("{single}", "single"));
        // Never closed: literal.
        assert!(glob_matches("{a,b", "{a,b"));
    }

    #[test]
    fn character_classes() {
        assert!(glob_matches("[Mm]akefile", "Makefile"));
        assert!(glob_matches("[Mm]akefile", "sub/makefile"));
        assert!(!glob_matches("[Mm]akefile", "xakefile"));
        assert!(glob_matches("[a-c].txt", "b.txt"));
        assert!(!glob_matches("[a-c].txt", "d.txt"));
        assert!(glob_matches("[!a]x", "bx"));
        assert!(!glob_matches("[!a]x", "ax"));
        // Never closed: literal.
        assert!(glob_matches("[ab", "[ab"));
    }

    #[test]
    fn anchored_patterns() {
        assert!(glob_matches("src/**/*.cs", "src/a/b/c.cs"));
        assert!(glob_matches("src/**/*.cs", "src/c.cs"));
        assert!(!glob_matches("src/**/*.cs", "test/src/c.cs"));
        assert!(glob_matches("/src/*.cs", "src/c.cs"));
        assert!(glob_matches("lib/*.js", "lib/a.js"));
        assert!(!glob_matches("lib/*.js", "lib/a/b.js"));
        assert!(glob_matches("lib/**.js", "lib/a/b.js"));
    }

    #[test]
    fn number_ranges() {
        assert!(glob_matches("file{1..3}.txt", "file1.txt"));
        assert!(glob_matches("file{1..3}.txt", "file3.txt"));
        assert!(!glob_matches("file{1..3}.txt", "file4.txt"));
        assert!(!glob_matches("file{1..3}.txt", "file12.txt"));
        assert!(glob_matches("n{-5..5}", "n-3"));
        assert!(!glob_matches("n{-5..5}", "n-6"));
        assert!(glob_matches("n{+1..10}", "n10"));
        assert!(!glob_matches("n{1..3}", "nx"));
    }

    #[test]
    fn escapes() {
        assert!(glob_matches("\\*.txt", "*.txt"));
        assert!(!glob_matches("\\*.txt", "a.txt"));
        assert!(glob_matches("a\\{b,c\\}", "a{b,c}"));
        assert!(glob_matches("[\\]]", "]"));
    }

    #[test]
    fn sections_use_paths_relative_to_their_file() {
        let files = [
            at("/r/src", "[lib/*.cs]\nindent_size = 2\n"),
            at(
                "/r",
                "[src/lib/*.cs]\ntab_width = 7\n[lib/*.cs]\ncharset = latin1\n",
            ),
        ];
        let config = EditorConfig::resolve_with(Path::new("/r/src/lib/a.cs"), &files);
        assert_eq!(config.indent_size, Some(2));
        assert_eq!(config.tab_width, Some(7));
        assert_eq!(config.charset, None);
    }

    #[test]
    fn resolve_reads_real_files() {
        let unique = format!(
            "eludite-editorconfig-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_nanos())
                .unwrap_or(0)
        );
        let base = std::env::temp_dir().join(unique);
        let inner = base.join("proj").join("src");
        std::fs::create_dir_all(&inner).unwrap();
        // Above the root: must not apply.
        std::fs::write(base.join(FILE_NAME), "[*]\ncharset = latin1\n").unwrap();
        std::fs::write(
            base.join("proj").join(FILE_NAME),
            "root = true\n[*]\nindent_style = space\nindent_size = 4\nend_of_line = lf\n",
        )
        .unwrap();
        std::fs::write(inner.join(FILE_NAME), "[*.cs]\nindent_size = 2\n").unwrap();
        let config = EditorConfig::resolve(&inner.join("a.cs"));
        let _ = std::fs::remove_dir_all(&base);
        assert_eq!(
            config,
            EditorConfig {
                indent_style: Some(IndentStyle::Space),
                indent_size: Some(2),
                tab_width: Some(2),
                end_of_line: Some(EndOfLine::Lf),
                charset: None,
                trim_trailing_whitespace: None,
                insert_final_newline: None,
            }
        );
    }
}
