//! What editing needs to know about a language (brief 0068): its comment tokens, the brackets and quotes that close
//! themselves, how its strings are written, and its smart indent rules. Plain data by language id, beside the
//! highlighting registration ([`super::LanguageConfig`]), so a language without an entry edits as plain text.
//!
//! [`scan`] is a lexical scan of one line: where code is, and whether a position is inside a string or a comment. It
//! knows only the current line (a block comment or a raw string opened on an earlier line is not seen), which is what
//! brace completion and comment commands need and fits the keystroke budget; nothing here touches a syntax tree.

/// How a language indents on Enter, lexically.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum IndentRules {
    /// Keep the previous line's indentation only.
    None,
    /// C-like: one level more after a line ending in an open bracket.
    Braces,
    /// Visual Basic's block keywords.
    VisualBasic,
    /// F#'s offside rule: one level more after `=`, `->`, `then`, `do` and the like.
    FSharp,
    /// HTML: one level more after an opening tag left open on its line.
    Markup,
    /// Razor: markup's rules and the braces of its code.
    Razor,
}

/// How a language writes its strings, for [`scan`].
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct StringForms {
    /// `\` escapes the next character inside `"..."` and `'...'`.
    pub backslash: bool,
    /// C#'s and F#'s verbatim strings `@"..."`, with `""` for a quote.
    pub verbatim: bool,
    /// C#'s raw strings and F#'s triple-quoted strings: `"""..."""`.
    pub triple: bool,
    /// Visual Basic: `""` inside a string is a quote, and there is no escape character.
    pub doubled: bool,
    /// Rust's raw strings `r"..."`, `r#"..."#`.
    pub rust_raw: bool,
    /// `'` starts a character literal only when one closes it on the same line (`'a'`, `'\n'`): Rust's lifetimes and
    /// F#'s type parameters are code.
    pub char_literal_only: bool,
}

/// One language's editing data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EditingConfig {
    /// The language id this entry is for (`""` for plain text).
    pub id: &'static str,
    /// The line comment token, if the language has one.
    pub line_comment: Option<&'static str>,
    /// The block comment's start and end, if the language has one.
    pub block_comment: Option<(&'static str, &'static str)>,
    /// Razor: the comment of its markup (`@* *@`) where [`Self::block_comment`] is its code's.
    pub markup_comment: Option<(&'static str, &'static str)>,
    /// Bracket pairs that close themselves and that Enter opens.
    pub brackets: &'static [(char, char)],
    /// Quotes that close themselves.
    pub quotes: &'static [char],
    /// The characters that open strings, for [`scan`].
    pub string_quotes: &'static [char],
    pub strings: StringForms,
    pub indent: IndentRules,
}

const C_BRACKETS: &[(char, char)] = &[('(', ')'), ('[', ']'), ('{', '}')];

/// Plain text: brackets and quotes close, no comments, Enter keeps the indentation.
pub const PLAIN: EditingConfig = EditingConfig {
    id: "",
    line_comment: None,
    block_comment: None,
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"'],
    string_quotes: &['"'],
    strings: StringForms {
        backslash: true,
        verbatim: false,
        triple: false,
        doubled: false,
        rust_raw: false,
        char_literal_only: false,
    },
    indent: IndentRules::None,
};

const CSHARP: EditingConfig = EditingConfig {
    id: "csharp",
    line_comment: Some("//"),
    block_comment: Some(("/*", "*/")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"', '\''],
    string_quotes: &['"', '\''],
    strings: StringForms {
        backslash: true,
        verbatim: true,
        triple: true,
        doubled: false,
        rust_raw: false,
        char_literal_only: false,
    },
    indent: IndentRules::Braces,
};

const RUST: EditingConfig = EditingConfig {
    id: "rust",
    line_comment: Some("//"),
    block_comment: Some(("/*", "*/")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"'],
    string_quotes: &['"', '\''],
    strings: StringForms {
        backslash: true,
        verbatim: false,
        triple: false,
        doubled: false,
        rust_raw: true,
        char_literal_only: true,
    },
    indent: IndentRules::Braces,
};

const JAVASCRIPT: EditingConfig = EditingConfig {
    id: "javascript",
    line_comment: Some("//"),
    block_comment: Some(("/*", "*/")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"', '\'', '`'],
    string_quotes: &['"', '\'', '`'],
    strings: StringForms {
        backslash: true,
        verbatim: false,
        triple: false,
        doubled: false,
        rust_raw: false,
        char_literal_only: false,
    },
    indent: IndentRules::Braces,
};

const CSS: EditingConfig = EditingConfig {
    id: "css",
    line_comment: None,
    block_comment: Some(("/*", "*/")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"', '\''],
    string_quotes: &['"', '\''],
    strings: JAVASCRIPT.strings,
    indent: IndentRules::Braces,
};

const JSON: EditingConfig = EditingConfig {
    id: "json",
    line_comment: None,
    block_comment: None,
    markup_comment: None,
    brackets: &[('[', ']'), ('{', '}')],
    quotes: &['"'],
    string_quotes: &['"'],
    strings: JAVASCRIPT.strings,
    indent: IndentRules::Braces,
};

const HTML: EditingConfig = EditingConfig {
    id: "html",
    line_comment: None,
    block_comment: Some(("<!--", "-->")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"', '\''],
    string_quotes: &[],
    strings: PLAIN.strings,
    indent: IndentRules::Markup,
};

const RAZOR: EditingConfig = EditingConfig {
    id: "razor",
    line_comment: Some("//"),
    block_comment: Some(("/*", "*/")),
    markup_comment: Some(("@*", "*@")),
    brackets: C_BRACKETS,
    quotes: &['"', '\''],
    string_quotes: &['"'],
    strings: CSHARP.strings,
    indent: IndentRules::Razor,
};

const VISUAL_BASIC: EditingConfig = EditingConfig {
    id: "vb",
    line_comment: Some("'"),
    block_comment: None,
    markup_comment: None,
    brackets: &[('(', ')'), ('{', '}')],
    quotes: &['"'],
    string_quotes: &['"'],
    strings: StringForms {
        backslash: false,
        verbatim: false,
        triple: false,
        doubled: true,
        rust_raw: false,
        char_literal_only: false,
    },
    indent: IndentRules::VisualBasic,
};

const FSHARP: EditingConfig = EditingConfig {
    id: "fsharp",
    line_comment: Some("//"),
    block_comment: Some(("(*", "*)")),
    markup_comment: None,
    brackets: C_BRACKETS,
    quotes: &['"'],
    string_quotes: &['"', '\''],
    strings: StringForms {
        backslash: true,
        verbatim: true,
        triple: true,
        doubled: false,
        rust_raw: false,
        char_literal_only: true,
    },
    indent: IndentRules::FSharp,
};

/// The editing data of language `id` (a [`super::LanguageConfig::id`]); [`PLAIN`] for an unknown id.
pub fn for_language(id: &str) -> &'static EditingConfig {
    match id {
        "csharp" => &CSHARP,
        "rust" => &RUST,
        "javascript" | "typescript" | "tsx" => &JAVASCRIPT,
        "jsonc" => &JSONC,
        "json" => &JSON,
        "css" => &CSS,
        "html" => &HTML,
        "razor" => &RAZOR,
        "vb" => &VISUAL_BASIC,
        "fsharp" | "fsharp-signature" => &FSHARP,
        _ => &PLAIN,
    }
}

const JSONC: EditingConfig = EditingConfig {
    id: "jsonc",
    line_comment: Some("//"),
    block_comment: Some(("/*", "*/")),
    ..JSON
};

/// What a position on a line is in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Lexical {
    Code,
    String,
    Comment,
}

/// The line `line` scanned up to byte `at`: what `at` is in. Strings and comments that start on earlier lines are
/// not seen.
pub fn scan(config: &EditingConfig, line: &str, at: usize) -> Lexical {
    let at = at.min(line.len());
    let mut i = 0;
    let starts = |i: usize, token: &str| line[i..].starts_with(token);
    while i < at {
        if let Some(t) = config.line_comment
            && starts(i, t)
        {
            return Lexical::Comment;
        }
        for (open, close) in config
            .block_comment
            .iter()
            .chain(config.markup_comment.iter())
        {
            if starts(i, open) {
                match line[i + open.len()..].find(close) {
                    Some(end) => {
                        let past = i + open.len() + end + close.len();
                        if at < past {
                            return Lexical::Comment;
                        }
                        i = past;
                    }
                    None => return Lexical::Comment,
                }
                break;
            }
        }
        if i >= at {
            break;
        }
        let c = line[i..].chars().next().expect("i < len");
        let s = &config.strings;
        // Rust's raw strings: r"..." and r#"..."#.
        if s.rust_raw && c == 'r' && !prev_is_word(line, i) {
            let hashes = line[i + 1..].bytes().take_while(|&b| b == b'#').count();
            if line[i + 1 + hashes..].starts_with('"') {
                let body = i + 2 + hashes;
                let close: String = std::iter::once('"')
                    .chain(std::iter::repeat_n('#', hashes))
                    .collect();
                match line[body..].find(&close) {
                    Some(end) if at >= body + end + close.len() => {
                        i = body + end + close.len();
                        continue;
                    }
                    _ => return Lexical::String,
                }
            }
        }
        if s.triple && starts(i, "\"\"\"") {
            let body = i + 3;
            match line[body..].find("\"\"\"") {
                Some(end) if at >= body + end + 3 => {
                    i = body + end + 3;
                    continue;
                }
                _ => return Lexical::String,
            }
        }
        if s.verbatim && (starts(i, "@\"") || starts(i, "$@\"") || starts(i, "@$\"")) {
            let body = i + if starts(i, "@\"") { 2 } else { 3 };
            match verbatim_end(line, body) {
                Some(end) if at >= end => {
                    i = end;
                    continue;
                }
                _ => return Lexical::String,
            }
        }
        if config.string_quotes.contains(&c) {
            if c == '\'' && s.char_literal_only && !char_literal_at(line, i) {
                i += 1;
                continue;
            }
            match quoted_end(line, i, c, s) {
                Some(end) if at >= end => {
                    i = end;
                    continue;
                }
                _ => return Lexical::String,
            }
        }
        i += c.len_utf8();
    }
    Lexical::Code
}

fn prev_is_word(line: &str, i: usize) -> bool {
    line[..i]
        .chars()
        .next_back()
        .is_some_and(|c| c.is_alphanumeric() || c == '_')
}

/// The end (past the closing quote) of the string opened by the quote at `i`, if it closes on the line.
fn quoted_end(line: &str, i: usize, quote: char, s: &StringForms) -> Option<usize> {
    let mut j = i + quote.len_utf8();
    let mut chars = line[j..].chars();
    while let Some(c) = chars.next() {
        if s.backslash && c == '\\' {
            j += 1;
            if let Some(n) = chars.next() {
                j += n.len_utf8();
            }
            continue;
        }
        j += c.len_utf8();
        if c == quote {
            if s.doubled && line[j..].starts_with(quote) {
                chars.next();
                j += quote.len_utf8();
                continue;
            }
            return Some(j);
        }
    }
    None
}

fn verbatim_end(line: &str, body: usize) -> Option<usize> {
    let b = line.as_bytes();
    let mut j = body;
    while j < b.len() {
        if b[j] == b'"' {
            if b.get(j + 1) == Some(&b'"') {
                j += 2;
                continue;
            }
            return Some(j + 1);
        }
        j += 1;
    }
    None
}

/// A character literal starts at `i` (`'a'`, `'\n'`, `'\u{1F600}'`), as opposed to a lifetime or type parameter.
fn char_literal_at(line: &str, i: usize) -> bool {
    let rest = &line[i + 1..];
    let mut chars = rest.chars();
    match chars.next() {
        Some('\\') => rest[1..].contains('\''),
        Some(_) => chars.next() == Some('\''),
        None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn at(id: &str, marked: &str) -> Lexical {
        let i = marked.find('|').unwrap();
        let line = marked.replace('|', "");
        scan(for_language(id), &line, i)
    }

    #[test]
    fn strings_and_comments_by_language() {
        assert_eq!(at("csharp", "var x = |1;"), Lexical::Code);
        assert_eq!(at("csharp", "var x = \"a|b\";"), Lexical::String);
        assert_eq!(at("csharp", "var x = \"a\\\"|b\";"), Lexical::String);
        assert_eq!(at("csharp", "var x = \"ab\"|;"), Lexical::Code);
        assert_eq!(at("csharp", "var p = @\"C:\\|x\";"), Lexical::String);
        assert_eq!(at("csharp", "var p = @\"C:\\\"|;"), Lexical::Code);
        assert_eq!(at("csharp", "var p = @\"a\"\"|b\";"), Lexical::String);
        assert_eq!(at("csharp", "var p = $@\"{x}|\";"), Lexical::String);
        assert_eq!(
            at("csharp", "var r = \"\"\"a \" |b\"\"\";"),
            Lexical::String
        );
        assert_eq!(at("csharp", "var r = \"\"\"a\"\"\" |;"), Lexical::Code);
        assert_eq!(at("csharp", "x(); // a |comment"), Lexical::Comment);
        assert_eq!(at("csharp", "x(/* a |*/);"), Lexical::Comment);
        assert_eq!(at("csharp", "x(/* a */|);"), Lexical::Code);
        assert_eq!(at("csharp", "var c = '|x';"), Lexical::String);
        assert_eq!(at("vb", "Dim s = \"a\"\"|b\""), Lexical::String);
        assert_eq!(at("vb", "Dim s = \"a\\\"|"), Lexical::Code);
        assert_eq!(at("vb", "Dim s = 1 ' |note"), Lexical::Comment);
        assert_eq!(at("rust", "fn f<'a>(x: &'a |str)"), Lexical::Code);
        assert_eq!(at("rust", "let c = '|x';"), Lexical::String);
        assert_eq!(at("rust", "let s = r#\"a \" |b\"#;"), Lexical::String);
        assert_eq!(at("rust", "let s = r#\"a\"#|;"), Lexical::Code);
        assert_eq!(at("fsharp", "let f (x: 'T) = |x"), Lexical::Code);
        assert_eq!(at("fsharp", "(* a |*)"), Lexical::Comment);
        assert_eq!(at("javascript", "const s = `a|`;"), Lexical::String);
        assert_eq!(at("css", "a { color: red; /* |x */ }"), Lexical::Comment);
        assert_eq!(at("razor", "@* a |*@"), Lexical::Comment);
        assert_eq!(at("json", "{\"a|\": 1}"), Lexical::String);
        assert_eq!(at("", "plain \"a|\""), Lexical::String);
    }

    #[test]
    fn every_builtin_language_has_an_entry() {
        for config in super::super::BUILTINS {
            assert!(
                !for_language(config.id).id.is_empty(),
                "{} has no editing entry",
                config.id
            );
        }
        assert_eq!(for_language("plaintext"), &PLAIN);
        assert_eq!(for_language("vb").line_comment, Some("'"));
        assert_eq!(for_language("fsharp").block_comment, Some(("(*", "*)")));
        assert_eq!(for_language("razor").markup_comment, Some(("@*", "*@")));
        assert_eq!(for_language("jsonc").line_comment, Some("//"));
        assert_eq!(for_language("json").line_comment, None);
    }
}
