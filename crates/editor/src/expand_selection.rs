//! Expand Selection and Contract Selection (brief 0068; Visual Studio's Shift+Alt+= and Shift+Alt+-).
//!
//! Each step grows every selection to the smallest larger range among: the word at a caret, then each enclosing
//! syntax node of the buffer's tree-sitter tree and, for a node delimited by brackets or quotes, its contents first,
//! then the whole document. Without a tree (no language, or none parsed yet) the steps are lexical: the word, a
//! string's contents and the string, a bracket pair's contents and the pair, the line, the document. Contract walks
//! back through the earlier steps; any other change of the selections forgets them.

use std::ops::Range;

use crate::editor::{Editor, SelectionRange};

const OPENERS: &[(&str, &str)] = &[
    ("(", ")"),
    ("[", "]"),
    ("{", "}"),
    ("<", ">"),
    ("\"", "\""),
    ("'", "'"),
    ("`", "`"),
    ("@\"", "\""),
    ("$\"", "\""),
    ("\"\"\"", "\"\"\""),
];

fn strictly_contains(outer: &Range<usize>, inner: &Range<usize>) -> bool {
    outer.start <= inner.start && outer.end >= inner.end && outer.len() > inner.len()
}

impl Editor {
    /// Grow every selection one step, by `tree` when it is the current text's syntax tree. False when nothing grew.
    pub fn expand_selection(&mut self, tree: Option<&tree_sitter::Tree>) -> bool {
        let current = self.selections();
        if current != self.ops.expand_last {
            self.ops.expand_stack.clear();
        }
        let text_len = self.buffer.len();
        let grown: Vec<SelectionRange> = current
            .iter()
            .map(|s| {
                let r = s.range();
                let next = match tree {
                    Some(tree) => self.syntax_step(tree, &r),
                    None => self.lexical_step(&r),
                }
                .unwrap_or(0..text_len);
                SelectionRange {
                    tail: next.start,
                    head: next.end,
                }
            })
            .collect();
        if grown == current {
            return false;
        }
        self.ops.expand_stack.push(current);
        let primary = self.primary;
        self.set_selections(grown, primary);
        self.ops.expand_last = self.selections();
        true
    }

    /// Go back one Expand Selection step. False when there is none (or the selections changed since).
    pub fn contract_selection(&mut self) -> bool {
        if self.selections() != self.ops.expand_last {
            self.ops.expand_stack.clear();
            return false;
        }
        let Some(previous) = self.ops.expand_stack.pop() else {
            return false;
        };
        let primary = self.primary.min(previous.len() - 1);
        self.set_selections(previous, primary);
        self.ops.expand_last = self.selections();
        true
    }

    fn word_step(&self, r: &Range<usize>) -> Option<Range<usize>> {
        if !r.is_empty() {
            return None;
        }
        let w = self.word_at(r.start);
        (!w.is_empty()).then_some(w)
    }

    fn syntax_step(&self, tree: &tree_sitter::Tree, r: &Range<usize>) -> Option<Range<usize>> {
        if let Some(w) = self.word_step(r) {
            return Some(w);
        }
        let root = tree.root_node();
        let mut node = root.descendant_for_byte_range(r.start, r.end)?;
        loop {
            let full = node.byte_range();
            if let Some(inner) = delimited_contents(&node)
                && strictly_contains(&inner, r)
                && !inner.is_empty()
            {
                return Some(inner);
            }
            let trimmed = trim_range(self, full.clone());
            if strictly_contains(&trimmed, r) {
                return Some(trimmed);
            }
            if strictly_contains(&full, r) {
                return Some(full);
            }
            node = node.parent()?;
        }
    }

    /// The lexical steps for buffers without a tree: brackets and quotes found by scanning outwards on the text
    /// around the selection (at most 64 KiB each way), then the line.
    fn lexical_step(&self, r: &Range<usize>) -> Option<Range<usize>> {
        if let Some(w) = self.word_step(r) {
            return Some(w);
        }
        const WINDOW: usize = 64 * 1024;
        let len = self.buffer.len();
        let from = self
            .buffer
            .clip_offset(r.start.saturating_sub(WINDOW), text::Bias::Left);
        let to = self
            .buffer
            .clip_offset((r.end + WINDOW).min(len), text::Bias::Right);
        let text = self.buffer.text_for_range(from..to);
        let (s, e) = (r.start - from, r.end - from);
        let mut best: Option<Range<usize>> = None;
        let mut consider = |c: Range<usize>| {
            let c = c.start + from..c.end + from;
            if strictly_contains(&c, r) && best.as_ref().is_none_or(|b| c.len() < b.len()) {
                best = Some(c);
            }
        };
        for (open, close) in [('(', ')'), ('[', ']'), ('{', '}')] {
            if let Some((o, c)) = enclosing_pair(&text, s, e, open, close) {
                consider(o + 1..c);
                consider(o..c + 1);
            }
        }
        // Quotes on the selection's line.
        let row = self.row_of(r.start);
        if row == self.row_of(r.end) {
            let line_start = self.line_start(row);
            let line = self.buffer.line(row);
            for q in ['"', '\'', '`'] {
                let (ls, le) = (r.start - line_start, r.end - line_start);
                let before = line[..ls].rfind(q);
                let after = line[le..].find(q).map(|i| i + le);
                if let (Some(b), Some(a)) = (before, after)
                    && line[..b].matches(q).count() % 2 == 0
                {
                    consider(line_start + b + 1 - from..line_start + a - from);
                    consider(line_start + b - from..line_start + a + 1 - from);
                }
            }
            let content_start = line_start + (line.len() - line.trim_start().len());
            let content_end = line_start + line.trim_end().len();
            consider(content_start - from..content_end - from);
        }
        best
    }
}

/// For a node whose first and last children are matching delimiters, the range between them.
fn delimited_contents(node: &tree_sitter::Node) -> Option<Range<usize>> {
    let n = node.child_count();
    if n < 2 {
        return None;
    }
    let first = node.child(0)?;
    let last = node.child(n - 1)?;
    let pair = OPENERS
        .iter()
        .any(|(o, c)| first.kind() == *o && last.kind() == *c);
    // C#'s strings start with `"` (or `@"`, `$"`) tokens and end with `"`.
    let string = first.kind().ends_with('"') && last.kind() == "\"";
    (pair || string).then(|| first.end_byte()..last.start_byte())
}

/// A node's range without trailing white space and line breaks some grammars include.
fn trim_range(editor: &Editor, r: Range<usize>) -> Range<usize> {
    let text = editor.buffer.text_for_range(r.clone());
    let trimmed = text.trim_end_matches([' ', '\t', '\n']);
    r.start..r.start + trimmed.len()
}

/// The innermost `open`..`close` pair around `s..e` in `text`, skipping balanced pairs.
fn enclosing_pair(
    text: &str,
    s: usize,
    e: usize,
    open: char,
    close: char,
) -> Option<(usize, usize)> {
    let mut depth = 0usize;
    let mut o = None;
    for (i, c) in text[..s].char_indices().rev() {
        if c == close {
            depth += 1;
        } else if c == open {
            if depth == 0 {
                o = Some(i);
                break;
            }
            depth -= 1;
        }
    }
    let o = o?;
    // The selection itself must be balanced: start the close search at its end, counting what it opens.
    let mut depth =
        text[s..e].matches(open).count() as isize - text[s..e].matches(close).count() as isize;
    if depth < 0 {
        return None;
    }
    for (i, c) in text[e..].char_indices() {
        if c == open {
            depth += 1;
        } else if c == close {
            if depth == 0 {
                return Some((o, e + i));
            }
            depth -= 1;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use crate::indent::tests::{editor, marked};

    const FIXTURE: &str =
        "class C\n{\n    void M(int a)\n    {\n        var s = Call(a + 1, \"text\");\n    }\n}\n";

    fn csharp_tree(text: &str) -> tree_sitter::Tree {
        let mut p = tree_sitter::Parser::new();
        p.set_language(&tree_sitter_c_sharp::LANGUAGE.into())
            .unwrap();
        p.parse(text, None).unwrap()
    }

    fn selected(e: &crate::Editor) -> String {
        let s = e.primary_selection();
        e.buffer().text_for_range(s.range())
    }

    #[test]
    fn expand_by_syntax_on_a_csharp_fixture() {
        let at = FIXTURE.find("a + 1").unwrap();
        let mut e = editor(FIXTURE);
        e.set_caret(at);
        let tree = csharp_tree(FIXTURE);
        let mut steps = Vec::new();
        while e.expand_selection(Some(&tree)) {
            steps.push(selected(&e));
            assert!(steps.len() < 30, "expansion ends");
        }
        assert_eq!(steps[0], "a");
        assert_eq!(steps[1], "a + 1");
        assert!(steps.contains(&"a + 1, \"text\"".to_owned()), "{steps:#?}");
        assert!(
            steps.contains(&"Call(a + 1, \"text\")".to_owned()),
            "{steps:#?}"
        );
        assert!(
            steps.contains(&"var s = Call(a + 1, \"text\");".to_owned()),
            "{steps:#?}"
        );
        assert!(
            steps
                .iter()
                .any(|s| s.starts_with("void M(int a)") && s.ends_with('}')),
            "{steps:#?}"
        );
        assert_eq!(steps.last().unwrap(), FIXTURE);
        // Contract walks back to the caret.
        while e.contract_selection() {}
        assert_eq!(marked(&e).find('|'), Some(at));
    }

    #[test]
    fn string_contents_come_before_the_string() {
        let at = FIXTURE.find("text").unwrap();
        let mut e = editor(FIXTURE);
        e.set_caret(at + 1);
        let tree = csharp_tree(FIXTURE);
        e.expand_selection(Some(&tree));
        assert_eq!(selected(&e), "text");
        e.expand_selection(Some(&tree));
        assert_eq!(selected(&e), "\"text\"");
    }

    #[test]
    fn lexical_steps_without_a_tree() {
        let mut e = editor("f(a, g{b|c}, \"x y\")");
        e.expand_selection(None);
        assert_eq!(selected(&e), "bc");
        e.expand_selection(None);
        assert_eq!(selected(&e), "{bc}");
        e.expand_selection(None);
        assert_eq!(selected(&e), "a, g{bc}, \"x y\"");
        e.expand_selection(None);
        assert_eq!(selected(&e), "(a, g{bc}, \"x y\")");
        e.expand_selection(None);
        assert_eq!(selected(&e), "f(a, g{bc}, \"x y\")");
        assert!(!e.expand_selection(None));
        assert!(e.contract_selection());
        assert_eq!(selected(&e), "(a, g{bc}, \"x y\")");
        // Moving forgets the steps.
        e.move_left(false);
        assert!(!e.contract_selection());
    }

    #[test]
    fn every_caret_grows() {
        let mut e = editor("ab| cd|");
        e.expand_selection(None);
        assert_eq!(marked(&e), "[ab] [cd]");
    }
}
