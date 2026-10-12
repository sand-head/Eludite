//! Indentation (brief 0068): the settings ([`IndentSettings`]: tabs or spaces, sizes, the auto-indent mode, brace
//! completion), Tab and Shift+Tab over lines ([`Editor::tab`], [`Editor::outdent`]), and smart indent on Enter and on
//! typing a closer ([`Editor::newline`], [`smart_indent_after`]).
//!
//! Smart indent is lexical, by the language's [`IndentRules`]: it runs on the keystroke, on the UI thread, so it reads
//! the current and previous lines and never a syntax tree.

use crate::editor::{Editor, SelectionRange};
use crate::syntax::editing::{EditingConfig, IndentRules, Lexical, scan};

/// What Enter does with indentation (Visual Studio's Tools > Options > Text Editor > Tabs > Indenting).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum AutoIndent {
    /// A new line starts at column 0.
    None,
    /// A new line keeps the previous line's indentation.
    Block,
    /// The language's rules: one level more inside a block, closers back at their opener's level.
    #[default]
    Smart,
}

/// One document's indentation settings (from the settings and `.editorconfig`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct IndentSettings {
    /// Spaces (true) or tabs.
    pub insert_spaces: bool,
    /// Columns of one indent level.
    pub indent_size: u32,
    /// Columns of a tab character.
    pub tab_size: u32,
    pub auto_indent: AutoIndent,
    /// Brackets and quotes close themselves (`editor.autoClosingBrackets`).
    pub auto_close: bool,
}

impl Default for IndentSettings {
    fn default() -> Self {
        Self {
            insert_spaces: true,
            indent_size: 4,
            tab_size: 4,
            auto_indent: AutoIndent::Smart,
            auto_close: true,
        }
    }
}

impl IndentSettings {
    /// The white space of an indentation `width` columns wide: spaces, or tabs then spaces for the remainder.
    pub fn whitespace(&self, width: u32) -> String {
        if self.insert_spaces {
            " ".repeat(width as usize)
        } else {
            let tab = self.tab_size.max(1);
            let mut s = "\t".repeat((width / tab) as usize);
            s.push_str(&" ".repeat((width % tab) as usize));
            s
        }
    }

    /// The columns `text` (spaces and tabs) takes, tabs to the next multiple of `tab_size`.
    pub fn width(&self, text: &str) -> u32 {
        let tab = self.tab_size.max(1);
        text.chars().fold(0, |w, c| match c {
            '\t' => (w / tab + 1) * tab,
            _ => w + 1,
        })
    }

    /// One indent level.
    pub fn unit(&self) -> String {
        self.whitespace(self.indent_size.max(1))
    }
}

impl Editor {
    pub fn indent_settings(&self) -> IndentSettings {
        self.ops.indent
    }

    pub fn set_indent_settings(&mut self, settings: IndentSettings) {
        self.ops.indent = settings;
    }

    /// Tab: with a selection over more than one line, indent every touched line (Increase Line Indent); otherwise
    /// white space to the next indent stop at every caret, replacing a selection within a line.
    pub fn tab(&mut self) {
        if self.selections().iter().any(|s| {
            let (a, b) = self.touched_rows(s.to_owned());
            a != b || (!s.is_empty() && self.covers_line(*s))
        }) {
            self.indent_lines();
            return;
        }
        let settings = self.ops.indent;
        self.edit_each(|this, s| {
            if !settings.insert_spaces {
                return Some((s.range(), "\t".into()));
            }
            let p = this.buffer.offset_to_point(s.start());
            let line = this.buffer.line(p.row);
            let col = settings.width(&line[..p.column as usize]);
            let size = settings.indent_size.max(1);
            Some((s.range(), " ".repeat((size - col % size) as usize)))
        });
    }

    /// A selection that starts at its line's first character and ends at its end (or the next line's start).
    fn covers_line(&self, s: SelectionRange) -> bool {
        let (row, _) = self.touched_rows(s);
        s.start() <= self.line_start(row) + self.indentation_of(row).len()
            && s.end() >= self.line_end(row)
            && self.line_end(row) > self.line_start(row)
    }

    /// Edit > Advanced > Increase Line Indent: one level more on every touched line that is not blank. Selections keep
    /// covering the same text. One undo step.
    pub fn indent_lines(&mut self) -> bool {
        let unit = self.ops.indent.unit();
        let edits: Vec<_> = self
            .touched_row_list()
            .into_iter()
            .filter(|&row| !self.buffer.line(row).trim().is_empty())
            .map(|row| {
                let at = self.line_start(row);
                (at..at, unit.clone())
            })
            .collect();
        // A selection starting at a line's start keeps the new indentation inside it, as Visual Studio does.
        let sels = self.selections();
        let primary = self.primary;
        let shifted = shift_through(&sels, &edits);
        self.step(edits, Some((shifted, primary)))
    }

    /// Shift+Tab (Decrease Line Indent): one level less on every touched line; tabs count as `tab_size` columns, and
    /// indentation that no longer matches the setting's style is rewritten in it. One undo step.
    pub fn outdent(&mut self) -> bool {
        let settings = self.ops.indent;
        let size = settings.indent_size.max(1);
        let mut edits = Vec::new();
        for row in self.touched_row_list() {
            let ws = self.indentation_of(row);
            let width = settings.width(&ws);
            if width == 0 {
                continue;
            }
            let target = (width - 1) / size * size;
            let start = self.line_start(row);
            // Drop characters from the end of the indentation until it is narrow enough.
            let mut keep = ws.len();
            while keep > 0 && settings.width(&ws[..keep]) > target {
                keep -= 1;
            }
            // With tabs, what remains is rewritten as tabs; with spaces, a remaining tab is kept.
            let canonical = settings.insert_spaces || ws[..keep] == settings.whitespace(target);
            if settings.width(&ws[..keep]) == target && canonical {
                edits.push((start + keep..start + ws.len(), String::new()));
            } else {
                edits.push((start..start + ws.len(), settings.whitespace(target)));
            }
        }
        self.step(edits, None)
    }

    /// Enter: a line break, then the indentation [`AutoIndent`] asks for. Between a bracket pair (`{|}`) in Smart mode,
    /// the caret goes on an indented line of its own and the closer on the next, at the opener's indentation. White
    /// space before the caret and after it on the line is dropped in Smart mode.
    pub fn newline(&mut self) {
        let settings = self.ops.indent;
        let config = self.ops.language;
        self.edit_each_select(|this, s| {
            let p = this.buffer.offset_to_point(s.start());
            let line = this.buffer.line(p.row);
            let before = &line[..p.column as usize];
            let end_point = this.buffer.offset_to_point(s.end());
            let after_line = this.buffer.line(end_point.row);
            let after = &after_line[end_point.column as usize..];
            let base: String = match settings.auto_indent {
                AutoIndent::None => String::new(),
                _ => line
                    .chars()
                    .take_while(|c| *c == ' ' || *c == '\t')
                    .collect(),
            };
            if settings.auto_indent != AutoIndent::Smart {
                let text = format!("\n{base}");
                let n = text.len();
                return Some((s.range(), text, n..n));
            }
            let trimmed_before = before.trim_end_matches([' ', '\t']);
            let lead = after.len() - after.trim_start_matches([' ', '\t']).len();
            let range = s.start() - (before.len() - trimmed_before.len())..s.end() + lead;
            let after = &after[lead..];
            let unit = settings.unit();
            let deeper = smart_indent_after(config, trimmed_before, &line, &settings);
            let base = if trimmed_before.trim().is_empty() {
                // Enter on a blank line (or before the first character) keeps the caret's column.
                settings.whitespace(settings.width(trimmed_before))
            } else {
                base
            };
            let opener = trimmed_before.chars().next_back();
            let closer = after.chars().next();
            let between = match (opener, closer) {
                (Some(o), Some(c)) => {
                    config.brackets.contains(&(o, c))
                        && scan(config, trimmed_before, trimmed_before.len()) == Lexical::Code
                }
                _ => false,
            };
            if between {
                let first = format!("\n{base}{unit}");
                let caret = first.len();
                return Some((range, format!("{first}\n{base}"), caret..caret));
            }
            let text = if deeper {
                format!("\n{base}{unit}")
            } else {
                // A closer that starts the rest of the line goes back to its opener's level.
                let dedent = closer.is_some_and(|c| config.brackets.iter().any(|(_, k)| *k == c));
                if dedent {
                    format!("\n{base}")
                } else {
                    format!(
                        "\n{}",
                        outdent_after_statement(config, &line, &base, &settings)
                    )
                }
            };
            let n = text.len();
            Some((range, text, n..n))
        });
    }

    /// After typing `typed` at the carets: a closer typed as the first character of its line goes to the indentation
    /// of the line holding its opener; Visual Basic's block ends (`End If`, `Next`, `Loop`, `Else`...) go one level
    /// less than the block's lines. Smart mode only; one edit per caret line, merged into the typing's undo step.
    pub(crate) fn reindent_after_typing(&mut self, typed: &str) {
        if self.ops.indent.auto_indent != AutoIndent::Smart {
            return;
        }
        let config = self.ops.language;
        let settings = self.ops.indent;
        let mut edits = Vec::new();
        for s in self.selections() {
            if !s.is_empty() {
                continue;
            }
            let p = self.buffer.offset_to_point(s.head);
            let line = self.buffer.line(p.row);
            let ws_len = line.len() - line.trim_start_matches([' ', '\t']).len();
            let rest = &line[ws_len..];
            let target = if config.brackets.iter().any(|(_, c)| typed == c.to_string())
                && rest.len() == typed.len()
                && p.column as usize == line.len()
            {
                let closer = typed.chars().next().expect("one char");
                self.opener_indentation(p.row, closer)
            } else if config.indent == IndentRules::VisualBasic
                && typed == " "
                && p.column as usize == line.len()
            {
                vb_block_end_indent(self, p.row, rest.trim_end(), &settings)
            } else {
                None
            };
            if let Some(target) = target
                && target != line[..ws_len]
            {
                let start = self.line_start(p.row);
                edits.push((start..start + ws_len, target));
            }
        }
        if edits.is_empty() {
            return;
        }
        let primary = self.primary;
        let sels = self.selections();
        let shifted = shift_through(&sels, &edits);
        self.transact(|this| {
            this.buffer.edit(edits);
            this.set_selections(shifted, primary);
        });
    }

    /// The indentation of the line holding the opener that matches a `closer` typed at the end of row `row`
    /// (brackets inside strings and comments on scanned lines are skipped). Looks back at most 2,000 rows.
    fn opener_indentation(&self, row: u32, closer: char) -> Option<String> {
        let config = self.ops.language;
        let opener = config.brackets.iter().find(|(_, c)| *c == closer)?.0;
        let mut depth = 0i32;
        let first = row.saturating_sub(2000);
        for r in (first..=row).rev() {
            let line = self.buffer.line(r);
            // The closer just typed is the last character of `row`.
            let end = if r == row {
                line.len() - closer.len_utf8()
            } else {
                line.len()
            };
            for (i, c) in line[..end].char_indices().rev() {
                if c != opener && c != closer {
                    continue;
                }
                if scan(config, &line, i) != Lexical::Code {
                    continue;
                }
                if c == closer {
                    depth += 1;
                } else if depth == 0 {
                    return Some(self.indentation_of(r));
                } else {
                    depth -= 1;
                }
            }
        }
        None
    }

    /// Replace, for each selection, the range `f` returns with its text, as one undo step that may merge with
    /// typing, and put the selection at the range (relative to the inserted text's start) `f` returns.
    pub(crate) fn edit_each_select(
        &mut self,
        f: impl Fn(
            &Self,
            SelectionRange,
        ) -> Option<(std::ops::Range<usize>, String, std::ops::Range<usize>)>,
    ) {
        let mut edits: Vec<_> = self
            .selections()
            .into_iter()
            .filter_map(|s| f(self, s))
            .collect();
        edits.sort_by_key(|(r, _, _)| r.start);
        let mut kept: Vec<(std::ops::Range<usize>, String, std::ops::Range<usize>)> = Vec::new();
        for e in edits {
            if let Some(last) = kept.last()
                && e.0.start < last.0.end
            {
                continue;
            }
            kept.push(e);
        }
        if kept.is_empty() {
            return;
        }
        let primary_start = self.primary_selection().start();
        let mut sels = Vec::with_capacity(kept.len());
        let mut primary = 0;
        let mut delta: isize = 0;
        for (ix, (r, t, sel)) in kept.iter().enumerate() {
            let start = (r.start as isize + delta) as usize;
            sels.push(SelectionRange {
                tail: start + sel.start,
                head: start + sel.end,
            });
            if r.start <= primary_start {
                primary = ix;
            }
            delta += t.len() as isize - r.len() as isize;
        }
        let edits: Vec<_> = kept
            .into_iter()
            .map(|(r, t, _)| (r, t))
            .filter(|(r, t)| !(r.is_empty() && t.is_empty()))
            .collect();
        if edits.is_empty() {
            self.set_selections(sels, primary);
            return;
        }
        self.transact(|this| {
            this.buffer.edit(edits);
            this.set_selections(sels, primary);
        });
    }
}

/// `sels` moved through `edits` (sorted, against the same text): a selection starting exactly at an insertion point
/// stays before the insertion, and its end moves past insertions at or before it.
pub(crate) fn shift_through(
    sels: &[SelectionRange],
    edits: &[(std::ops::Range<usize>, String)],
) -> Vec<SelectionRange> {
    shift_with(sels, edits, false)
}

/// [`shift_through`], but an insertion at a selection's start goes before it and one at its end after it: the
/// selection keeps exactly the text it had (wrapping it in a comment).
pub(crate) fn shift_inside(
    sels: &[SelectionRange],
    edits: &[(std::ops::Range<usize>, String)],
) -> Vec<SelectionRange> {
    shift_with(sels, edits, true)
}

fn shift_with(
    sels: &[SelectionRange],
    edits: &[(std::ops::Range<usize>, String)],
    inside: bool,
) -> Vec<SelectionRange> {
    let map = |offset: usize, at_start: bool| -> usize {
        let mut delta: isize = 0;
        for (r, t) in edits {
            if r.start > offset || (at_start && r.start == offset && r.is_empty()) {
                break;
            }
            if r.end <= offset {
                delta += t.len() as isize - r.len() as isize;
            } else {
                // Inside a replaced range: to its start.
                return (r.start as isize + delta) as usize;
            }
        }
        (offset as isize + delta) as usize
    };
    sels.iter()
        .map(|s| {
            let (start, end) = if inside {
                (map(s.start(), false), map(s.end(), !s.is_empty()))
            } else {
                (map(s.start(), !s.is_empty()), map(s.end(), false))
            };
            if s.reversed() {
                SelectionRange {
                    tail: end,
                    head: start,
                }
            } else {
                SelectionRange {
                    tail: start,
                    head: end,
                }
            }
        })
        .collect()
}

/// Whether the line after `before` (the text before the caret, trimmed at its end) goes one level deeper, by the
/// language's rules. `line` is the whole line.
pub fn smart_indent_after(
    config: &EditingConfig,
    before: &str,
    line: &str,
    _settings: &IndentSettings,
) -> bool {
    let code = strip_line_comment(config, before);
    let code = code.trim_end();
    if code.is_empty() {
        return false;
    }
    if scan(config, code, code.len()) != Lexical::Code {
        return false;
    }
    let last = code.chars().next_back().expect("not empty");
    let opens_bracket = config.brackets.iter().any(|(o, _)| *o == last);
    match config.indent {
        IndentRules::None => false,
        IndentRules::Braces => opens_bracket,
        IndentRules::Razor => opens_bracket || markup_opens(code),
        IndentRules::Markup => markup_opens(code),
        IndentRules::VisualBasic => vb_opens_block(code),
        IndentRules::FSharp => fsharp_opens(code, line),
    }
}

/// `before` without a trailing line comment (outside strings).
fn strip_line_comment<'a>(config: &EditingConfig, before: &'a str) -> &'a str {
    let Some(token) = config.line_comment else {
        return before;
    };
    let mut from = 0;
    while let Some(i) = before[from..].find(token) {
        let at = from + i;
        if scan(config, before, at) == Lexical::Code {
            return &before[..at];
        }
        from = at + token.len();
    }
    before
}

/// For a statement continued from a deeper line: nothing yet (the base indentation stays).
fn outdent_after_statement(
    _config: &EditingConfig,
    _line: &str,
    base: &str,
    _settings: &IndentSettings,
) -> String {
    base.to_owned()
}

const VOID_ELEMENTS: &[&str] = &[
    "area", "base", "br", "col", "embed", "hr", "img", "input", "link", "meta", "param", "source",
    "track", "wbr", "!doctype",
];

/// The line ends inside or after an opening tag that is not closed on the line.
fn markup_opens(code: &str) -> bool {
    let Some(open) = code.rfind('<') else {
        return false;
    };
    let tag = &code[open + 1..];
    if tag.starts_with('/') || tag.starts_with('!') || tag.starts_with('?') {
        return false;
    }
    let name: String = tag
        .chars()
        .take_while(|c| c.is_alphanumeric() || *c == '-' || *c == ':' || *c == '.')
        .collect();
    if name.is_empty() || VOID_ELEMENTS.contains(&name.to_ascii_lowercase().as_str()) {
        return false;
    }
    if tag.ends_with("/>") {
        return false;
    }
    // `<div>` with nothing after it, or a tag still open (`<div class="a"`).
    tag.ends_with('>') || !tag.contains('>')
}

fn words(code: &str) -> Vec<String> {
    code.split(|c: char| !(c.is_alphanumeric() || c == '_'))
        .filter(|w| !w.is_empty())
        .map(str::to_ascii_lowercase)
        .collect()
}

/// Visual Basic: the line opens a block.
fn vb_opens_block(code: &str) -> bool {
    let w = words(code);
    let Some(first) = w.first().map(String::as_str) else {
        return false;
    };
    if first == "end" || w.contains(&"mustoverride".to_owned()) || w.contains(&"declare".to_owned())
    {
        return false;
    }
    let modifiers = [
        "public",
        "private",
        "protected",
        "friend",
        "shared",
        "overrides",
        "overridable",
        "overloads",
        "notoverridable",
        "mustinherit",
        "notinheritable",
        "partial",
        "readonly",
        "writeonly",
        "async",
        "iterator",
        "default",
        "static",
        "shadows",
        "widening",
        "narrowing",
    ];
    let head = w
        .iter()
        .map(String::as_str)
        .find(|w| !modifiers.contains(w))
        .unwrap_or("");
    let trimmed = code.trim_end().to_ascii_lowercase();
    match head {
        "sub" | "function" | "operator" => !trimmed.contains(" = ") || code.contains("=>"),
        "property" => {
            // An auto property (`Property Name As String`) is one line; one with `Get`/`Set` blocks follows with a
            // `Get`, which this cannot know: auto properties are the common case.
            false
        }
        "class" | "module" | "structure" | "interface" | "enum" | "namespace" => true,
        "get" | "set" | "else" | "try" | "finally" | "do" | "while" | "for" | "using" | "with"
        | "synclock" | "select" | "case" | "catch" | "addhandler" | "removehandler"
        | "raiseevent" => true,
        "if" | "elseif" => trimmed.ends_with("then"),
        _ => false,
    }
}

/// Visual Basic: typing the space after a block end (`End`, `Next`, `Loop`, `Else`...) re-indents its line one level
/// less than the line before, or to that line's level when it opened the block: a simple, lexical rule.
fn vb_block_end_indent(
    editor: &Editor,
    row: u32,
    rest: &str,
    settings: &IndentSettings,
) -> Option<String> {
    if row == 0 || rest.is_empty() || rest.contains(' ') {
        return None;
    }
    let word = rest.to_ascii_lowercase();
    if !matches!(
        word.as_str(),
        "end" | "next" | "loop" | "wend" | "else" | "elseif" | "case" | "catch" | "finally"
    ) {
        return None;
    }
    let r = (0..row)
        .rev()
        .find(|&r| !editor.buffer.line(r).trim().is_empty())?;
    let width = settings.width(&editor.indentation_of(r));
    let line = editor.buffer.line(r);
    let target = if vb_opens_block(line.trim()) {
        if word == "case" {
            return None;
        }
        width
    } else {
        width.saturating_sub(settings.indent_size)
    };
    Some(settings.whitespace(target))
}

/// F#'s offside rule: the line ends with something whose body follows on the next line.
fn fsharp_opens(code: &str, _line: &str) -> bool {
    let trimmed = code.trim_end();
    for end in [
        "=",
        "->",
        "(",
        "[",
        "[|",
        "{",
        "begin",
        "struct",
        "class",
        "interface",
    ] {
        if trimmed.ends_with(end) {
            return true;
        }
    }
    let last = words(trimmed).pop().unwrap_or_default();
    matches!(
        last.as_str(),
        "then" | "else" | "do" | "with" | "try" | "finally" | "function" | "lazy"
    ) && trimmed.ends_with(last.as_str())
}

#[cfg(test)]
pub(crate) mod tests {
    use crate::Buffer;
    use crate::editor::{Editor, SelectionRange};
    use crate::indent::{AutoIndent, IndentSettings};
    use crate::syntax::editing::for_language;
    use std::time::Duration;

    pub(crate) fn editor(marked: &str) -> Editor {
        let (text, sels) = parse(marked);
        let mut b = Buffer::new(&text);
        b.set_group_interval(Duration::ZERO);
        let mut e = Editor::new(b);
        let n = sels.len();
        e.set_selections(sels, n - 1);
        e
    }

    pub(crate) fn lang(marked: &str, id: &str) -> Editor {
        let mut e = editor(marked);
        e.set_language_editing(for_language(id));
        e
    }

    pub(crate) fn parse(marked: &str) -> (String, Vec<SelectionRange>) {
        let mut text = String::new();
        let mut sels = Vec::new();
        let mut open = None;
        for c in marked.chars() {
            match c {
                '|' => sels.push(SelectionRange::caret(text.len())),
                '[' => open = Some(text.len()),
                ']' => sels.push(SelectionRange {
                    tail: open.take().unwrap(),
                    head: text.len(),
                }),
                c => text.push(c),
            }
        }
        if sels.is_empty() {
            sels.push(SelectionRange::caret(0));
        }
        (text, sels)
    }

    pub(crate) fn marked(e: &Editor) -> String {
        let text = e.text();
        let mut marks: Vec<(usize, char)> = Vec::new();
        for s in e.selections() {
            if s.is_empty() {
                marks.push((s.head, '|'));
            } else {
                marks.push((s.start(), if s.reversed() { ']' } else { '[' }));
                marks.push((s.end(), if s.reversed() { '[' } else { ']' }));
            }
        }
        marks.sort_by_key(|(at, _)| *at);
        let mut out = String::new();
        let mut last = 0;
        for (at, c) in marks {
            out.push_str(&text[last..at]);
            out.push(c);
            last = at;
        }
        out.push_str(&text[last..]);
        out
    }

    #[test]
    fn tab_indents_three_selected_lines_and_keeps_the_selection() {
        let mut e = editor("a[aa\nbbb\ncc]c\nddd");
        e.tab();
        assert_eq!(marked(&e), "    a[aa\n    bbb\n    cc]c\nddd");
        assert!(e.undo());
        assert_eq!(marked(&e), "a[aa\nbbb\ncc]c\nddd");
        // A selection of whole lines from column 0 keeps the new indentation inside.
        let mut e = editor("[one\ntwo\n]three");
        e.tab();
        assert_eq!(marked(&e), "[    one\n    two\n]three");
        e.tab();
        assert_eq!(marked(&e), "[        one\n        two\n]three");
    }

    #[test]
    fn tab_skips_blank_lines_and_uses_tabs_when_asked() {
        let mut e = editor("[a\n\nb]");
        e.set_indent_settings(IndentSettings {
            insert_spaces: false,
            ..IndentSettings::default()
        });
        e.tab();
        assert_eq!(marked(&e), "[\ta\n\n\tb]");
    }

    #[test]
    fn tab_within_a_line_goes_to_the_next_stop() {
        let mut e = editor("ab|c\nx[y]z");
        e.tab();
        assert_eq!(marked(&e), "ab  |c\nx   |z");
        let mut e = editor("\t|x");
        e.tab();
        // A tab is four columns: the next stop is eight.
        assert_eq!(marked(&e), "\t    |x");
    }

    #[test]
    fn shift_tab_outdents_tabs_spaces_and_both() {
        let mut e = editor("[        eight\n\tone tab\n  two\n\t  tab and two\nnone]");
        e.outdent();
        assert_eq!(marked(&e), "[    eight\none tab\ntwo\n\ttab and two\nnone]");
        e.outdent();
        assert_eq!(marked(&e), "[eight\none tab\ntwo\ntab and two\nnone]");
        assert!(e.undo());
        assert!(e.undo());
        assert_eq!(
            marked(&e),
            "[        eight\n\tone tab\n  two\n\t  tab and two\nnone]"
        );
        // Shift+Tab with a caret outdents its line.
        let mut e = editor("    x|y\n        z|");
        e.outdent();
        assert_eq!(marked(&e), "x|y\n    z|");
        // With tabs, mixed indentation is rewritten in tabs.
        let mut e = editor("  \t\tx|");
        e.set_indent_settings(IndentSettings {
            insert_spaces: false,
            ..IndentSettings::default()
        });
        e.outdent();
        assert_eq!(marked(&e), "\tx|");
    }

    #[test]
    fn enter_between_braces_opens_an_indented_body() {
        let mut e = lang("    void F() {|}", "csharp");
        e.newline();
        assert_eq!(marked(&e), "    void F() {\n        |\n    }");
        let mut e = lang("call(|)", "csharp");
        e.newline();
        assert_eq!(marked(&e), "call(\n    |\n)");
        // Inside a string, no.
        let mut e = lang("var s = \"{|}\";", "csharp");
        e.newline();
        assert_eq!(marked(&e), "var s = \"{\n|}\";");
    }

    #[test]
    fn enter_after_an_opener_indents_and_before_a_closer_does_not() {
        let mut e = lang("if (x) {   |", "csharp");
        e.newline();
        assert_eq!(marked(&e), "if (x) {\n    |");
        let mut e = lang("    if (x) { // note|", "csharp");
        e.newline();
        assert_eq!(marked(&e), "    if (x) { // note\n        |");
        let mut e = lang("    x();|", "csharp");
        e.newline();
        assert_eq!(marked(&e), "    x();\n    |");
        let mut e = lang("fn main() {|", "rust");
        e.newline();
        assert_eq!(marked(&e), "fn main() {\n    |");
        let mut e = lang("a {|", "css");
        e.newline();
        assert_eq!(marked(&e), "a {\n    |");
        let mut e = lang("{\"a\": {|", "json");
        e.newline();
        assert_eq!(marked(&e), "{\"a\": {\n    |");
    }

    #[test]
    fn block_and_none_modes() {
        let mut e = lang("    if (x) {|", "csharp");
        e.set_indent_settings(IndentSettings {
            auto_indent: AutoIndent::Block,
            ..IndentSettings::default()
        });
        e.newline();
        assert_eq!(marked(&e), "    if (x) {\n    |");
        e.set_indent_settings(IndentSettings {
            auto_indent: AutoIndent::None,
            ..IndentSettings::default()
        });
        e.newline();
        assert_eq!(marked(&e), "    if (x) {\n    \n|");
    }

    #[test]
    fn closer_typed_first_on_a_line_goes_to_its_opener() {
        let mut e = lang("    if (x) {\n        y();\n        |", "csharp");
        e.type_text("}");
        assert_eq!(marked(&e), "    if (x) {\n        y();\n    }|");
        // Not when the closer is not first on its line.
        let mut e = lang("    if (x) {\n        y(|", "csharp");
        e.type_text(")");
        assert_eq!(marked(&e), "    if (x) {\n        y()|");
    }

    #[test]
    fn visual_basic_blocks() {
        let mut e = lang("    Public Sub Main()|", "vb");
        e.newline();
        assert_eq!(marked(&e), "    Public Sub Main()\n        |");
        let mut e = lang("If x > 1 Then|", "vb");
        e.newline();
        assert_eq!(marked(&e), "If x > 1 Then\n    |");
        let mut e = lang("If x > 1 Then Return|", "vb");
        e.newline();
        assert_eq!(marked(&e), "If x > 1 Then Return\n|");
        let mut e = lang("Public MustOverride Sub F()|", "vb");
        e.newline();
        assert_eq!(marked(&e), "Public MustOverride Sub F()\n|");
        let mut e = lang("Class C|", "vb");
        e.newline();
        assert_eq!(marked(&e), "Class C\n    |");
        // `End` typed on an indented line goes back one level.
        let mut e = lang("    If x Then\n        y()\n        End|", "vb");
        e.type_text(" ");
        assert_eq!(marked(&e), "    If x Then\n        y()\n    End |");
    }

    #[test]
    fn fsharp_offside() {
        for line in [
            "let f x =|",
            "match x with|",
            "    if a then|",
            "let xs = (|",
            "fun x ->|",
        ] {
            let mut e = lang(line, "fsharp");
            e.newline();
            let text = e.text();
            let indent = line.len() - line.trim_start().len();
            let expect = format!("{}\n{}", line.replace('|', ""), " ".repeat(indent + 4));
            assert_eq!(text, expect, "{line}");
        }
        let mut e = lang("let x = 1|", "fsharp");
        e.newline();
        assert_eq!(marked(&e), "let x = 1\n|");
    }

    #[test]
    fn markup_tags() {
        let mut e = lang("<div>|", "html");
        e.newline();
        assert_eq!(marked(&e), "<div>\n    |");
        let mut e = lang("<br>|", "html");
        e.newline();
        assert_eq!(marked(&e), "<br>\n|");
        let mut e = lang("<p>text</p>|", "html");
        e.newline();
        assert_eq!(marked(&e), "<p>text</p>\n|");
        let mut e = lang("@code {|", "razor");
        e.newline();
        assert_eq!(marked(&e), "@code {\n    |");
    }

    #[test]
    fn settings_widths() {
        let s = IndentSettings {
            insert_spaces: false,
            indent_size: 4,
            tab_size: 8,
            ..IndentSettings::default()
        };
        assert_eq!(s.width("\t  "), 10);
        assert_eq!(s.whitespace(12), "\t    ");
        assert_eq!(s.unit(), "    ");
    }
}
