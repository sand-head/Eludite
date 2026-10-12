//! Snippet sessions (brief 0069): LSP snippet syntax parsed into text and tab stops, inserted as a session the
//! person moves through with Tab and Shift+Tab, as Visual Studio's code snippets and VS Code's snippets behave.
//!
//! - [`parse`] reads `$1`, `${1}`, `${1:text}` (placeholders nest), `${1|a,b|}` (the first choice is inserted), `$0`,
//!   variables (`$name`, `${name}`, `${name:default}`, resolved by the caller; an unknown one inserts its default, or
//!   nothing) and the escapes `\$`, `\}`, `\\`, `\,` and `\|`. A bare `$1` repeating a placeholder mirrors its text.
//! - [`crate::EditorView::insert_snippet`] replaces a range with the text as one undo step, indents its following lines
//!   like the first, and selects the first stop: every occurrence of it, so linked stops are several selections and
//!   typing edits them all. Tab and Shift+Tab move between stops ([`crate::actions::NextSnippetStop`] in the
//!   `in_snippet` key context); Tab on the last stop, Enter or Escape ends the session, and so does moving the caret
//!   out of the stop. The stops are anchors, so they follow the edits made inside them.

use std::collections::BTreeMap;
use std::ops::Range;

use gpui::Context;
use text::Anchor;

use crate::view::{Decoration, DecorationStyle, EditorView};

/// One tab stop of a parsed snippet: its number, where its occurrences are in [`ParsedSnippet::text`], and its
/// choices (`${1|a,b|}`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabStop {
    pub index: u32,
    pub ranges: Vec<Range<usize>>,
    pub choices: Vec<String>,
}

/// A snippet as text and tab stops, in the order Tab visits them: `$1`, `$2`, ... and `$0` (the end, added at the end
/// of the text when the snippet has none) last.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParsedSnippet {
    pub text: String,
    pub stops: Vec<TabStop>,
}

impl ParsedSnippet {
    /// The final position (`$0`).
    pub fn end(&self) -> usize {
        self.stops
            .last()
            .and_then(|s| s.ranges.first())
            .map_or(self.text.len(), |r| r.start)
    }

    /// True when Tab has somewhere to go: a stop other than `$0`.
    pub fn has_stops(&self) -> bool {
        self.stops.len() > 1
    }

    /// The text with `indent` after every line break, the stops moved with it (an inserted snippet's lines line up
    /// with its first one).
    pub fn indented(&self, indent: &str) -> ParsedSnippet {
        if indent.is_empty() || !self.text.contains('\n') {
            return self.clone();
        }
        let shift =
            |offset: usize| offset + indent.len() * self.text[..offset].matches('\n').count();
        let mut text = String::with_capacity(self.text.len() + indent.len() * 8);
        for (i, line) in self.text.split('\n').enumerate() {
            if i > 0 {
                text.push('\n');
                text.push_str(indent);
            }
            text.push_str(line);
        }
        ParsedSnippet {
            text,
            stops: self
                .stops
                .iter()
                .map(|s| TabStop {
                    index: s.index,
                    ranges: s
                        .ranges
                        .iter()
                        .map(|r| shift(r.start)..shift(r.end))
                        .collect(),
                    choices: s.choices.clone(),
                })
                .collect(),
        }
    }
}

/// True when `text` uses snippet syntax that [`parse`] would change (a `$` or a `\` escape).
pub fn is_snippet_syntax(text: &str) -> bool {
    text.contains('$') || text.contains('\\')
}

struct Parser<'a> {
    chars: Vec<char>,
    i: usize,
    out: String,
    stops: BTreeMap<u32, (Vec<Range<usize>>, Vec<String>)>,
    variables: &'a dyn Fn(&str) -> Option<String>,
    /// Text of each placeholder, for a bare `$N` that repeats it.
    mirror: &'a BTreeMap<u32, String>,
}

impl Parser<'_> {
    fn peek(&self, ahead: usize) -> Option<char> {
        self.chars.get(self.i + ahead).copied()
    }

    fn name_at(&self, from: usize) -> (String, usize) {
        let mut j = from;
        let mut name = String::new();
        while let Some(&c) = self.chars.get(j) {
            if c.is_alphanumeric() || c == '_' {
                name.push(c);
                j += 1;
            } else {
                break;
            }
        }
        (name, j)
    }

    fn add_stop(&mut self, index: u32, range: Range<usize>, choices: Vec<String>) {
        let entry = self.stops.entry(index).or_default();
        entry.0.push(range);
        if entry.1.is_empty() {
            entry.1 = choices;
        }
    }

    /// Skip a `${...}` body after its `:` up to and including the matching `}`.
    fn skip_body(&mut self) {
        let mut depth = 1;
        while let Some(c) = self.peek(0) {
            match c {
                '\\' => self.i += 2,
                '$' if self.peek(1) == Some('{') => {
                    depth += 1;
                    self.i += 2;
                }
                '}' => {
                    depth -= 1;
                    self.i += 1;
                    if depth == 0 {
                        return;
                    }
                }
                _ => self.i += 1,
            }
        }
    }

    /// Parse until the end, or (`nested`) up to and including the `}` closing a placeholder.
    fn run(&mut self, nested: bool) {
        while let Some(c) = self.peek(0) {
            match c {
                '\\' if matches!(self.peek(1), Some('$' | '}' | '\\' | ',' | '|')) => {
                    self.out.push(self.peek(1).expect("checked"));
                    self.i += 2;
                }
                '}' if nested => {
                    self.i += 1;
                    return;
                }
                '$' if self.peek(1).is_some_and(|d| d.is_ascii_digit()) => {
                    let (digits, end) = self.name_at(self.i + 1);
                    match digits.parse::<u32>() {
                        Ok(index) if digits.chars().all(|d| d.is_ascii_digit()) => {
                            let start = self.out.len();
                            if let Some(text) = self.mirror.get(&index) {
                                self.out.push_str(text);
                            }
                            let range = start..self.out.len();
                            self.add_stop(index, range, Vec::new());
                            self.i = end;
                        }
                        _ => {
                            self.out.push(c);
                            self.i += 1;
                        }
                    }
                }
                '$' if self.peek(1) == Some('{') => {
                    let (name, end) = self.name_at(self.i + 2);
                    let numeric = !name.is_empty() && name.chars().all(|d| d.is_ascii_digit());
                    match (self.chars.get(end), numeric) {
                        (Some('}'), true) => {
                            let index = name.parse().unwrap_or(0);
                            let start = self.out.len();
                            if let Some(text) = self.mirror.get(&index) {
                                self.out.push_str(text);
                            }
                            let range = start..self.out.len();
                            self.add_stop(index, range, Vec::new());
                            self.i = end + 1;
                        }
                        (Some(':'), true) => {
                            let index = name.parse().unwrap_or(0);
                            let start = self.out.len();
                            self.i = end + 1;
                            self.run(true);
                            let range = start..self.out.len();
                            self.add_stop(index, range, Vec::new());
                        }
                        (Some('|'), true) => {
                            let index = name.parse().unwrap_or(0);
                            let mut choices = Vec::new();
                            let mut current = String::new();
                            let mut j = end + 1;
                            while let Some(&ch) = self.chars.get(j) {
                                match ch {
                                    '\\' if j + 1 < self.chars.len() => {
                                        current.push(self.chars[j + 1]);
                                        j += 2;
                                        continue;
                                    }
                                    ',' => choices.push(std::mem::take(&mut current)),
                                    '|' if self.chars.get(j + 1) == Some(&'}') => {
                                        choices.push(std::mem::take(&mut current));
                                        j += 2;
                                        break;
                                    }
                                    _ => current.push(ch),
                                }
                                j += 1;
                            }
                            let start = self.out.len();
                            self.out
                                .push_str(choices.first().map(String::as_str).unwrap_or_default());
                            let range = start..self.out.len();
                            self.add_stop(index, range, choices);
                            self.i = j;
                        }
                        (Some('}'), false) if !name.is_empty() => {
                            if let Some(v) = (self.variables)(&name) {
                                self.out.push_str(&v);
                            }
                            self.i = end + 1;
                        }
                        (Some(':'), false) if !name.is_empty() => {
                            self.i = end + 1;
                            match (self.variables)(&name) {
                                Some(v) => {
                                    self.out.push_str(&v);
                                    self.skip_body();
                                }
                                None => self.run(true),
                            }
                        }
                        _ => {
                            self.out.push(c);
                            self.i += 1;
                        }
                    }
                }
                '$' if self.peek(1).is_some_and(|d| d.is_alphabetic() || d == '_') => {
                    let (name, end) = self.name_at(self.i + 1);
                    if let Some(v) = (self.variables)(&name) {
                        self.out.push_str(&v);
                    }
                    self.i = end;
                }
                _ => {
                    self.out.push(c);
                    self.i += 1;
                }
            }
        }
    }
}

fn parse_once(
    snippet: &str,
    variables: &dyn Fn(&str) -> Option<String>,
    mirror: &BTreeMap<u32, String>,
) -> (String, BTreeMap<u32, (Vec<Range<usize>>, Vec<String>)>) {
    let mut p = Parser {
        chars: snippet.chars().collect(),
        i: 0,
        out: String::with_capacity(snippet.len()),
        stops: BTreeMap::new(),
        variables,
        mirror,
    };
    p.run(false);
    (p.out, p.stops)
}

/// Parse LSP snippet syntax (see the module docs). `variables` resolves `$name`; `None` means unknown.
pub fn parse(snippet: &str, variables: &dyn Fn(&str) -> Option<String>) -> ParsedSnippet {
    let empty = BTreeMap::new();
    let (mut text, mut stops) = parse_once(snippet, variables, &empty);
    // A bare `$N` repeating `${N:text}` shows the same text: parse again with the placeholders' texts.
    let mirror: BTreeMap<u32, String> = stops
        .iter()
        .filter_map(|(i, (ranges, _))| {
            let first = ranges.iter().find(|r| !r.is_empty())?;
            ranges
                .iter()
                .any(|r| r.is_empty())
                .then(|| (*i, text[first.clone()].to_owned()))
        })
        .collect();
    if !mirror.is_empty() {
        (text, stops) = parse_once(snippet, variables, &mirror);
    }
    let end = stops.remove(&0);
    let mut ordered: Vec<TabStop> = stops
        .into_iter()
        .map(|(index, (ranges, choices))| TabStop {
            index,
            ranges,
            choices,
        })
        .collect();
    ordered.push(match end {
        Some((ranges, choices)) => TabStop {
            index: 0,
            ranges,
            choices,
        },
        None => TabStop {
            index: 0,
            ranges: vec![text.len()..text.len()],
            choices: Vec::new(),
        },
    });
    ParsedSnippet {
        text,
        stops: ordered,
    }
}

/// The active session in a view: each stop's occurrences as anchors, and the stop selected.
pub(crate) struct SnippetSession {
    stops: Vec<Vec<Range<Anchor>>>,
    active: usize,
}

/// The decoration layer that marks the session's fields.
pub const SNIPPET_LAYER: &str = "snippet";

/// The session as shown, for commands and tests.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnippetSnapshot {
    /// The stop selected (0-based in Tab order) and how many there are (`$0` included).
    pub active: usize,
    pub stops: usize,
    /// The selected stop's occurrences (byte ranges).
    pub ranges: Vec<Range<usize>>,
}

impl EditorView {
    /// Replace `range` with `snippet` (LSP snippet syntax) as one undo step and start a session on its stops. `extra`
    /// adds variables (`TM_FILENAME`); `TM_SELECTED_TEXT` and `SELECTION` are the replaced text. Returns the inserted
    /// text.
    pub fn insert_snippet(
        &mut self,
        range: Range<usize>,
        snippet: &str,
        extra: &[(&str, String)],
        cx: &mut Context<Self>,
    ) -> String {
        let buffer = self.editor.buffer();
        let range = range.start.min(buffer.len())..range.end.min(buffer.len());
        let selected = buffer.text_for_range(range.clone());
        let point = buffer.offset_to_point(range.start);
        let line = buffer.line(point.row);
        let indent: String = line
            .chars()
            .take_while(|c| *c == ' ' || *c == '\t')
            .collect();
        let word = {
            let w = self.editor.word_at(range.start);
            buffer.text_for_range(w)
        };
        let line_text = line.clone();
        let variables = |name: &str| -> Option<String> {
            match name {
                "TM_SELECTED_TEXT" | "SELECTION" => Some(selected.clone()),
                "TM_CURRENT_LINE" => Some(line_text.clone()),
                "TM_CURRENT_WORD" => Some(word.clone()),
                "TM_LINE_INDEX" => Some(point.row.to_string()),
                "TM_LINE_NUMBER" => Some((point.row + 1).to_string()),
                "CLIPBOARD" => Some(String::new()),
                other => extra
                    .iter()
                    .find(|(k, _)| *k == other)
                    .map(|(_, v)| v.clone()),
            }
        };
        let parsed = parse(snippet, &variables).indented(&indent);
        self.snippet_session_end(cx);
        self.editor
            .apply_edits(vec![(range.clone(), parsed.text.clone())]);
        let base = range.start;
        let buffer = self.editor.buffer();
        let stops: Vec<Vec<Range<Anchor>>> = parsed
            .stops
            .iter()
            .map(|s| {
                s.ranges
                    .iter()
                    .map(|r| {
                        buffer.anchor_before(base + r.start)..buffer.anchor_after(base + r.end)
                    })
                    .collect()
            })
            .collect();
        if parsed.has_stops() {
            self.snippet = Some(SnippetSession { stops, active: 0 });
            self.select_snippet_stop(cx);
        } else {
            self.editor.set_caret(base + parsed.end());
            self.snippet = None;
        }
        self.changed(cx);
        self.close_completion(cx);
        parsed.text
    }

    /// The active session, `None` when there is none.
    pub fn snippet_session(&self) -> Option<SnippetSnapshot> {
        let s = self.snippet.as_ref()?;
        let buffer = self.editor.buffer();
        Some(SnippetSnapshot {
            active: s.active,
            stops: s.stops.len(),
            ranges: s.stops[s.active]
                .iter()
                .map(|r| buffer.offset_for_anchor(&r.start)..buffer.offset_for_anchor(&r.end))
                .collect(),
        })
    }

    fn select_snippet_stop(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.snippet else {
            return;
        };
        let buffer = self.editor.buffer();
        let ranges: Vec<crate::SelectionRange> = s.stops[s.active]
            .iter()
            .map(|r| {
                let (start, end) = (
                    buffer.offset_for_anchor(&r.start),
                    buffer.offset_for_anchor(&r.end),
                );
                crate::SelectionRange {
                    tail: start,
                    head: end.max(start),
                }
            })
            .collect();
        let last = s.active + 1 == s.stops.len();
        let decorations: Vec<Decoration> = s
            .stops
            .iter()
            .take(s.stops.len() - 1)
            .flatten()
            .filter(|r| {
                let b = self.editor.buffer();
                b.offset_for_anchor(&r.start) < b.offset_for_anchor(&r.end)
            })
            .map(|r| Decoration {
                range: r.clone(),
                style: DecorationStyle::Background(self.style().find_match),
            })
            .collect();
        self.editor.set_selections(ranges, 0);
        if last {
            self.snippet = None;
            self.clear_decorations(SNIPPET_LAYER, cx);
        } else {
            self.set_decorations(SNIPPET_LAYER, decorations, cx);
        }
        cx.notify();
    }

    /// Tab (`delta` 1) or Shift+Tab (-1) in a session: select the next or previous stop; Tab past the last field
    /// ends the session at `$0`. False when no session is active.
    pub fn move_snippet_stop(&mut self, delta: isize, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.snippet.as_mut() else {
            return false;
        };
        let last = s.stops.len() as isize - 1;
        s.active = (s.active as isize + delta).clamp(0, last) as usize;
        self.select_snippet_stop(cx);
        self.close_completion(cx);
        true
    }

    /// End the session; `to_end`: put the caret at `$0` (Enter). False when no session was active.
    pub fn end_snippet_session(&mut self, to_end: bool, cx: &mut Context<Self>) -> bool {
        let Some(s) = self.snippet.as_mut() else {
            return false;
        };
        if to_end {
            s.active = s.stops.len() - 1;
            self.select_snippet_stop(cx);
            let head = self.editor.primary_selection().head;
            self.editor.set_caret(head);
        }
        self.snippet_session_end(cx);
        true
    }

    fn snippet_session_end(&mut self, cx: &mut Context<Self>) {
        if self.snippet.take().is_some() {
            self.clear_decorations(SNIPPET_LAYER, cx);
        }
    }

    /// After an edit or a caret move: the session ends when a caret left the selected stop.
    pub(crate) fn snippet_follow_caret(&mut self, cx: &mut Context<Self>) {
        let Some(s) = &self.snippet else {
            return;
        };
        let buffer = self.editor.buffer();
        let ranges: Vec<Range<usize>> = s.stops[s.active]
            .iter()
            .map(|r| buffer.offset_for_anchor(&r.start)..buffer.offset_for_anchor(&r.end))
            .collect();
        let inside = self.editor.selections().iter().all(|sel| {
            ranges
                .iter()
                .any(|r| r.start <= sel.start() && sel.end() <= r.end)
        });
        if !inside {
            self.snippet_session_end(cx);
        }
    }

    /// Set what Tab expands when the word before the caret is a snippet's shortcut (Tab-Tab): `expander` maps a word
    /// (and the text before it) to the snippet's LSP text. `None` turns it off.
    pub fn set_snippet_expander(&mut self, expander: Option<SnippetExpander>) {
        self.snippet_expander = expander;
    }

    /// Tab with no session and no list: expand the snippet named by the word before the caret. False when there is
    /// none (Tab goes on to indent).
    pub fn expand_snippet_shortcut(&mut self, cx: &mut Context<Self>) -> bool {
        if self.is_read_only() || self.snippet.is_some() {
            return false;
        }
        let Some(expander) = self.snippet_expander.clone() else {
            return false;
        };
        let selections = self.editor.selections();
        let [sel] = selections.as_slice() else {
            return false;
        };
        if !sel.is_empty() {
            return false;
        }
        let caret = sel.head;
        let buffer = self.editor.buffer();
        let point = buffer.offset_to_point(caret);
        let line_start = buffer.point_to_offset(text::Point::new(point.row, 0));
        let before = buffer.text_for_range(line_start..caret);
        let start = line_start + crate::intellisense::word_start_in(&before, before.len());
        if start == caret {
            return false;
        }
        let word = buffer.text_for_range(start..caret);
        let context_start =
            buffer.point_to_offset(text::Point::new(point.row.saturating_sub(400), 0));
        let before = buffer.text_for_range(context_start..start);
        let Some(body) = expander(&word, &before) else {
            return false;
        };
        self.insert_snippet(start..caret, &body, &[], cx);
        true
    }
}

/// Maps a shortcut to its snippet's LSP text (Tab-Tab), given the text before it (up to 400 lines, for snippets that
/// name the enclosing type).
pub type SnippetExpander = std::rc::Rc<dyn Fn(&str, &str) -> Option<String>>;

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &str) -> Option<String> {
        None
    }

    #[test]
    fn stops_placeholders_and_the_end() {
        let p = parse(
            "for (${1:int} i = 0; i < ${2:length}; i++)\n{\n\t$0\n}",
            &none,
        );
        assert_eq!(p.text, "for (int i = 0; i < length; i++)\n{\n\t\n}");
        assert_eq!(p.stops.len(), 3);
        assert_eq!(p.stops[0].index, 1);
        assert_eq!(&p.text[p.stops[0].ranges[0].clone()], "int");
        assert_eq!(&p.text[p.stops[1].ranges[0].clone()], "length");
        assert_eq!(p.stops[2].index, 0);
        assert_eq!(p.end(), p.text.len() - 2);
        assert!(p.has_stops());
    }

    #[test]
    fn no_end_means_the_end_of_the_text() {
        let p = parse("Foo($1)", &none);
        assert_eq!(p.text, "Foo()");
        assert_eq!(p.stops[0].ranges, vec![4..4]);
        assert_eq!(p.end(), 5);
        let plain = parse("Console", &none);
        assert!(!plain.has_stops());
        assert_eq!(plain.end(), 7);
    }

    #[test]
    fn linked_stops_mirror_their_placeholder() {
        let p = parse("${1:x} = $1 + ${1}", &none);
        assert_eq!(p.text, "x = x + x");
        assert_eq!(p.stops[0].ranges, vec![0..1, 4..5, 8..9]);
    }

    #[test]
    fn nested_placeholders_and_choices() {
        let p = parse("${1:new ${2:List}()}; ${3|public,private|} $0", &none);
        assert_eq!(p.text, "new List(); public ");
        let one = &p.stops[0];
        assert_eq!(&p.text[one.ranges[0].clone()], "new List()");
        assert_eq!(&p.text[p.stops[1].ranges[0].clone()], "List");
        assert_eq!(p.stops[2].choices, ["public", "private"]);
        assert_eq!(&p.text[p.stops[2].ranges[0].clone()], "public");
    }

    #[test]
    fn variables_escapes_and_defaults() {
        let vars = |n: &str| (n == "TM_SELECTED_TEXT").then(|| "a();".to_owned());
        let p = parse("if (${1:true})\n{\n    $TM_SELECTED_TEXT$0\n}", &vars);
        assert_eq!(p.text, "if (true)\n{\n    a();\n}");
        let p = parse(
            "cost \\$5 ${UNKNOWN:fallback} ${TM_SELECTED_TEXT:x} \\}",
            &vars,
        );
        assert_eq!(p.text, "cost $5 fallback a(); }");
        let p = parse("$UNKNOWN${1:a}", &none);
        assert_eq!(p.text, "a");
        assert_eq!(p.stops[0].ranges, vec![0..1]);
        // A lone `$` and a `}` outside a placeholder are text.
        assert_eq!(parse("a $ b }", &none).text, "a $ b }");
    }

    #[test]
    fn indentation_follows_the_first_line() {
        let p = parse("if ($1)\n{\n    $0\n}", &none).indented("        ");
        assert_eq!(p.text, "if ()\n        {\n            \n        }");
        assert_eq!(p.stops[0].ranges, vec![4..4]);
        assert_eq!(p.end(), "if ()\n        {\n            ".len());
    }
}
