//! Comment Selection, Uncomment Selection, Toggle Line Comment and Toggle Block Comment (brief 0068), with the
//! language's tokens ([`crate::syntax::editing::EditingConfig`]): line comments at the lowest indentation of the
//! touched lines, block comments around a selection within a line or for languages without line comments. Razor
//! comments its markup with `@* *@` and its code (inside `@code { }`, `@functions { }`, `@{ }`) with `//`.

use std::ops::Range;

use crate::editor::{Editor, SelectionRange};

/// The tokens that apply at one place.
#[derive(Clone, Copy, Debug)]
struct Tokens {
    line: Option<&'static str>,
    block: Option<(&'static str, &'static str)>,
}

impl Editor {
    fn tokens_at(&self, row: u32) -> Tokens {
        let config = self.ops.language;
        if let Some(markup) = config.markup_comment
            && !self.razor_code_at(row)
        {
            return Tokens {
                line: None,
                block: Some(markup),
            };
        }
        Tokens {
            line: config.line_comment,
            block: config.block_comment,
        }
    }

    /// Razor: row `row` is inside a `@code { }`, `@functions { }` or `@{ }` block (by braces, looking back at most 1,000
    /// rows).
    fn razor_code_at(&self, row: u32) -> bool {
        let mut depth: i32 = 0;
        for r in (row.saturating_sub(1000)..row).rev() {
            let line = self.buffer.line(r);
            let opens = line.matches('{').count() as i32;
            let closes = line.matches('}').count() as i32;
            depth += opens - closes;
            if depth > 0 {
                if line.contains("@code") || line.contains("@functions") || line.contains("@{") {
                    return true;
                }
                // Inside another block; keep looking for the one that holds it.
                depth = 0;
            }
        }
        false
    }

    /// Edit > Advanced > Comment Selection (Ctrl+K, Ctrl+C). One undo step; false when nothing changed.
    pub fn comment_selection(&mut self) -> bool {
        let sels = self.selections();
        let primary_row = self.row_of(self.primary_selection().start());
        let tokens = self.tokens_at(primary_row);
        // A selection within one line, not the whole line, takes a block comment where there is one.
        let partial = sels.iter().all(|s| {
            let (a, b) = self.touched_rows(*s);
            let row_text = self.buffer.line(a);
            let content_start = self.line_start(a) + (row_text.len() - row_text.trim_start().len());
            let content_end = self.line_start(a) + row_text.trim_end().len();
            a == b && !s.is_empty() && (s.start() > content_start || s.end() < content_end)
        });
        if let Some((open, close)) = tokens.block
            && (partial || tokens.line.is_none())
        {
            let ranges: Vec<Range<usize>> = if partial {
                sels.iter().map(|s| s.range()).collect()
            } else {
                self.row_blocks()
                    .into_iter()
                    .filter_map(|(a, b)| self.content_range(a, b))
                    .collect()
            };
            let mut edits = Vec::new();
            for r in ranges {
                edits.push((r.start..r.start, open.to_owned()));
                edits.push((r.end..r.end, close.to_owned()));
            }
            edits.sort_by_key(|(r, _)| r.start);
            let shifted = crate::indent::shift_inside(&sels, &edits);
            let primary = self.primary;
            return self.step(edits, Some((shifted, primary)));
        }
        let Some(token) = tokens.line else {
            return false;
        };
        let mut edits = Vec::new();
        for (a, b) in self.row_blocks() {
            edits.extend(self.line_comment_insertions(a, b, token));
        }
        self.step(edits, None)
    }

    /// The text of rows `a..=b` without the first row's indentation and the last row's trailing white space.
    fn content_range(&self, a: u32, b: u32) -> Option<Range<usize>> {
        let first = self.buffer.line(a);
        let last = self.buffer.line(b);
        let start = self.line_start(a) + (first.len() - first.trim_start().len());
        let end = self.line_start(b) + last.trim_end().len();
        (start < end).then_some(start..end)
    }

    /// Edit > Advanced > Uncomment Selection (Ctrl+K, Ctrl+U): the block comment around or at the selection, else one
    /// line comment token (and the space after it) from each touched line. One undo step.
    pub fn uncomment_selection(&mut self) -> bool {
        let primary_row = self.row_of(self.primary_selection().start());
        let tokens = self.tokens_at(primary_row);
        if let Some((open, close)) = tokens.block {
            let mut edits = Vec::new();
            for s in self.selections() {
                if let Some((o, c)) = self.block_comment_around(s, open, close) {
                    edits.push((o, String::new()));
                    edits.push((c, String::new()));
                }
            }
            if !edits.is_empty() {
                edits.sort_by_key(|(r, _)| r.start);
                edits.dedup_by_key(|(r, _)| r.start);
                return self.step(edits, None);
            }
        }
        let Some(token) = tokens.line else {
            return false;
        };
        let edits = self.line_comment_removals(token);
        self.step(edits, None)
    }

    /// `token` and a space at the lowest indentation of rows `a..=b`, on the rows that are not blank.
    fn line_comment_insertions(&self, a: u32, b: u32, token: &str) -> Vec<(Range<usize>, String)> {
        let rows: Vec<(u32, usize)> = (a..=b)
            .filter_map(|r| {
                let line = self.buffer.line(r);
                let content = line.trim_start_matches([' ', '\t']);
                (!content.trim_end().is_empty()).then(|| (r, line.len() - content.len()))
            })
            .collect();
        let Some(indent) = rows.iter().map(|(_, i)| *i).min() else {
            return Vec::new();
        };
        let with = format!("{token} ");
        rows.into_iter()
            .map(|(r, _)| {
                let at = self.line_start(r) + indent;
                (at..at, with.clone())
            })
            .collect()
    }

    fn line_comment_removals(&self, token: &str) -> Vec<(Range<usize>, String)> {
        let mut edits = Vec::new();
        for r in self.touched_row_list() {
            let line = self.buffer.line(r);
            let ws = line.len() - line.trim_start().len();
            if line[ws..].starts_with(token) {
                let mut len = token.len();
                if line[ws + len..].starts_with(' ') {
                    len += 1;
                }
                let at = self.line_start(r) + ws;
                edits.push((at..at + len, String::new()));
            }
        }
        edits
    }

    /// The ranges of the `open` and `close` tokens of a block comment that the selection holds exactly (white space
    /// aside), that wraps the touched lines' content, or that encloses the selection on its line.
    fn block_comment_around(
        &self,
        s: SelectionRange,
        open: &str,
        close: &str,
    ) -> Option<(Range<usize>, Range<usize>)> {
        let (a, b) = self.touched_rows(s);
        let candidates = [(!s.is_empty()).then(|| s.range()), self.content_range(a, b)];
        for r in candidates.into_iter().flatten() {
            let text = self.buffer.text_for_range(r.clone());
            let lead = text.len() - text.trim_start().len();
            let trimmed = text.trim();
            if trimmed.len() >= open.len() + close.len()
                && trimmed.starts_with(open)
                && trimmed.ends_with(close)
            {
                let o = r.start + lead;
                let c = o + trimmed.len() - close.len();
                return Some((o..o + open.len(), c..c + close.len()));
            }
        }
        // The caret inside a comment on its line.
        let line = self.buffer.line(a);
        let start = self.line_start(a);
        let col = s.start() - start;
        let o = line[..col.min(line.len())].rfind(open)?;
        let c = line[o + open.len()..].find(close)? + o + open.len();
        (c + close.len() >= s.end() - start).then(|| {
            (
                start + o..start + o + open.len(),
                start + c..start + c + close.len(),
            )
        })
    }

    /// Edit > Advanced > Toggle Line Comment (Ctrl+/): uncomment when every touched non-blank line is commented,
    /// else comment. Languages without line comments toggle a block comment.
    pub fn toggle_line_comment(&mut self) -> bool {
        let primary_row = self.row_of(self.primary_selection().start());
        let tokens = self.tokens_at(primary_row);
        let Some(token) = tokens.line else {
            return self.toggle_block_comment();
        };
        let all_commented = self
            .touched_row_list()
            .into_iter()
            .map(|r| self.buffer.line(r))
            .filter(|l| !l.trim().is_empty())
            .all(|l| l.trim_start().starts_with(token));
        if all_commented {
            let edits = self.line_comment_removals(token);
            self.step(edits, None)
        } else {
            let mut edits = Vec::new();
            for (a, b) in self.row_blocks() {
                edits.extend(self.line_comment_insertions(a, b, token));
            }
            self.step(edits, None)
        }
    }

    /// Edit > Advanced > Toggle Block Comment (Ctrl+Shift+/): unwrap the block comment at the selection, or wrap the
    /// selection (a caret's line) in one.
    pub fn toggle_block_comment(&mut self) -> bool {
        let primary_row = self.row_of(self.primary_selection().start());
        let tokens = self.tokens_at(primary_row);
        let Some((open, close)) = tokens.block else {
            return false;
        };
        let sels = self.selections();
        let mut edits = Vec::new();
        let mut unwrap = false;
        for s in &sels {
            if let Some((o, c)) = self.block_comment_around(*s, open, close) {
                edits.push((o, String::new()));
                edits.push((c, String::new()));
                unwrap = true;
            }
        }
        if !unwrap {
            for s in &sels {
                let r = if s.is_empty() {
                    let row = self.row_of(s.head);
                    match self.content_range(row, row) {
                        Some(r) => r,
                        None => continue,
                    }
                } else {
                    s.range()
                };
                edits.push((r.start..r.start, open.to_owned()));
                edits.push((r.end..r.end, close.to_owned()));
            }
        }
        edits.sort_by_key(|(r, _)| r.start);
        edits.dedup_by_key(|(r, _)| r.start);
        let shifted = crate::indent::shift_inside(&sels, &edits);
        let primary = self.primary;
        self.step(edits, Some((shifted, primary)))
    }
}

#[cfg(test)]
mod tests {
    use crate::indent::tests::{lang, marked};

    #[test]
    fn line_comments_per_language() {
        let mut e = lang("    [a();\n      b();\n\n    c();]", "csharp");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "    // a();\n    //   b();\n\n    // c();");
        assert!(e.uncomment_selection());
        assert_eq!(e.text(), "    a();\n      b();\n\n    c();");
        for (id, token) in [
            ("vb", "'"),
            ("fsharp", "//"),
            ("rust", "//"),
            ("typescript", "//"),
        ] {
            let mut e = lang("x = 1|", id);
            assert!(e.comment_selection(), "{id}");
            assert_eq!(e.text(), format!("{token} x = 1"), "{id}");
            assert!(e.uncomment_selection(), "{id}");
            assert_eq!(e.text(), "x = 1", "{id}");
        }
        // JSON has no comments.
        let mut e = lang("{|}", "json");
        assert!(!e.comment_selection());
        let mut e = lang("{|}", "jsonc");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "// {}");
    }

    #[test]
    fn block_comments_for_partial_selections_and_block_only_languages() {
        let mut e = lang("f(a, [b], c);", "csharp");
        assert!(e.comment_selection());
        assert_eq!(marked(&e), "f(a, /*[b]*/, c);");
        assert!(e.uncomment_selection());
        assert_eq!(e.text(), "f(a, b, c);");
        let mut e = lang("a { color: red;| }", "css");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "/*a { color: red; }*/");
        assert!(e.uncomment_selection());
        assert_eq!(e.text(), "a { color: red; }");
        let mut e = lang("  [<p>x</p>\n<br>]", "html");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "  <!--<p>x</p>\n<br>-->");
        assert!(e.uncomment_selection());
        assert_eq!(e.text(), "  <p>x</p>\n<br>");
        let mut e = lang("let x = (1|)", "fsharp");
        assert!(e.toggle_block_comment());
        assert_eq!(e.text(), "(*let x = (1)*)");
        assert!(e.toggle_block_comment());
        assert_eq!(e.text(), "let x = (1)");
    }

    #[test]
    fn razor_comments_markup_and_code_apart() {
        let mut e = lang("<h1>Hi|</h1>\n@code {\n    int x;\n}", "razor");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "@*<h1>Hi</h1>*@\n@code {\n    int x;\n}");
        let mut e = lang("<h1>Hi</h1>\n@code {\n    int |x;\n}", "razor");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "<h1>Hi</h1>\n@code {\n    // int x;\n}");
    }

    #[test]
    fn toggles() {
        let mut e = lang("[a();\n// b();]", "csharp");
        assert!(e.toggle_line_comment());
        assert_eq!(e.text(), "// a();\n// // b();");
        assert!(e.toggle_line_comment());
        assert_eq!(e.text(), "a();\n// b();");
        let mut e = lang("x = [y];", "csharp");
        assert!(e.toggle_block_comment());
        assert_eq!(marked(&e), "x = /*[y]*/;");
        let mut e = lang("x = /*[y]*/;", "csharp");
        assert!(e.toggle_block_comment());
        assert_eq!(e.text(), "x = y;");
        // Every caret.
        let mut e = lang("a|\nb\nc|", "csharp");
        assert!(e.comment_selection());
        assert_eq!(e.text(), "// a\nb\n// c");
        assert!(e.undo());
        assert_eq!(e.text(), "a\nb\nc");
    }
}
