//! Brace and quote completion (brief 0068): typing an opener inserts its closer, typing the closer over one the editor
//! inserted skips it, Backspace between an empty pair the editor inserted deletes both, and an opener typed over a
//! selection surrounds it. Which characters pair, and where strings and comments are, come from the language's
//! [`crate::syntax::editing::EditingConfig`].

use crate::editor::{Editor, SelectionRange};
use crate::syntax::editing::{Lexical, scan};

fn is_word(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// What typing one character does at one selection.
enum Typed {
    Pair(char, char),
    Surround(char, char),
    Skip,
    Plain,
}

impl Editor {
    /// Typing `text` at every selection, as the keyboard does: brace completion (when on), the box selection's virtual
    /// space, then smart indent of a closer or a block end. Programmatic insertion is [`Editor::insert`].
    pub fn type_text(&mut self, text: &str) {
        self.forget_autoclosed();
        self.ops.ring_paste = None;
        // Typing in a box pads its short rows first, in the same undo step.
        let before = self.buffer.last_transaction();
        if self.ops.boxed.is_some() {
            self.pad_box_rows();
        }
        let padded = self
            .buffer
            .last_transaction()
            .filter(|p| Some(*p) != before);
        let mut chars = text.chars();
        let single = match (chars.next(), chars.next()) {
            (Some(c), None) => Some(c),
            _ => None,
        };
        let paired = single.is_some_and(|c| self.ops.indent.auto_close && self.type_with_pairs(c));
        if !paired {
            self.insert(text);
        }
        if let (Some(pad), Some(typed)) = (padded, self.buffer.last_transaction())
            && pad != typed
        {
            self.merge_transactions(typed, pad);
            let after = self.selections.clone();
            if let Some(h) = self.history.get_mut(&pad) {
                h.1 = after;
            }
        }
        self.reindent_after_typing(text);
    }

    fn classify(&self, s: SelectionRange, c: char) -> Typed {
        let config = self.ops.language;
        let closer_of = config
            .brackets
            .iter()
            .find(|(o, _)| *o == c)
            .map(|(_, k)| *k)
            .or_else(|| config.quotes.contains(&c).then_some(c));
        let snapshot = self.buffer.snapshot();
        if !s.is_empty() {
            return match closer_of {
                Some(k) => Typed::Surround(c, k),
                None => Typed::Plain,
            };
        }
        let next = snapshot.chars_at(s.head).next();
        let is_closer = config.brackets.iter().any(|(_, k)| *k == c) || config.quotes.contains(&c);
        if is_closer && next == Some(c) && self.autoclosed_at(s.head) {
            return Typed::Skip;
        }
        let Some(k) = closer_of else {
            return Typed::Plain;
        };
        let p = self.buffer.offset_to_point(s.head);
        let line = self.buffer.line(p.row);
        if scan(config, &line, p.column as usize) != Lexical::Code {
            return Typed::Plain;
        }
        if next.is_some_and(is_word) {
            return Typed::Plain;
        }
        if k == c {
            // A quote: not after a word character (`don't`), nor right before another quote.
            let prev = snapshot.reversed_chars_at(s.head).next();
            if prev.is_some_and(is_word) || next == Some(c) {
                return Typed::Plain;
            }
        }
        Typed::Pair(c, k)
    }

    /// Type `c` with pairs: false (nothing done) when no selection pairs, skips or surrounds.
    fn type_with_pairs(&mut self, c: char) -> bool {
        let kinds: Vec<(SelectionRange, Typed)> = self
            .selections()
            .into_iter()
            .map(|s| (s, self.classify(s, c)))
            .collect();
        if kinds.iter().all(|(_, t)| matches!(t, Typed::Plain)) {
            return false;
        }
        let len = c.len_utf8();
        let paired = kinds.iter().any(|(_, t)| matches!(t, Typed::Pair(..)));
        self.edit_each_select(|this, s| {
            let kind = this.classify(s, c);
            Some(match kind {
                Typed::Pair(o, k) => (s.range(), format!("{o}{k}"), len..len),
                Typed::Surround(o, k) => {
                    let inner = this.buffer.text_for_range(s.range());
                    let n = inner.len();
                    let sel = if s.reversed() {
                        len + n..len
                    } else {
                        len..len + n
                    };
                    (s.range(), format!("{o}{inner}{k}"), sel)
                }
                Typed::Skip => (s.head..s.head, String::new(), len..len),
                Typed::Plain => (s.range(), c.to_string(), len..len),
            })
        });
        if paired {
            // Remember the closers just inserted: each caret sits right before one.
            let snapshot = self.buffer.snapshot().clone();
            for s in self.selections() {
                let next = snapshot.chars_at(s.head).next();
                let prev = snapshot.reversed_chars_at(s.head).next();
                let config = self.ops.language;
                let pair = match (prev, next) {
                    (Some(o), Some(k)) => {
                        config.brackets.contains(&(o, k)) || (o == k && config.quotes.contains(&o))
                    }
                    _ => false,
                };
                if s.is_empty() && pair && !self.autoclosed_at(s.head) {
                    let anchor = self.buffer.anchor_after(s.head);
                    self.ops.autoclosed.push(anchor);
                }
            }
        }
        true
    }

    /// An auto-inserted closer is at `offset`.
    pub(crate) fn autoclosed_at(&self, offset: usize) -> bool {
        self.ops
            .autoclosed
            .iter()
            .any(|a| self.buffer.offset_for_anchor(a) == offset)
    }

    /// Backspace between an empty pair the editor inserted: the range of both characters.
    pub(crate) fn autoclosed_pair_at(&self, offset: usize) -> Option<std::ops::Range<usize>> {
        if !self.autoclosed_at(offset) {
            return None;
        }
        let snapshot = self.buffer.snapshot();
        let prev = snapshot.reversed_chars_at(offset).next()?;
        let next = snapshot.chars_at(offset).next()?;
        let config = self.ops.language;
        let pair = config.brackets.contains(&(prev, next))
            || (prev == next && config.quotes.contains(&prev));
        pair.then(|| offset - prev.len_utf8()..offset + next.len_utf8())
    }

    /// Drop the remembered closers that no caret's line holds any more.
    fn forget_autoclosed(&mut self) {
        if self.ops.autoclosed.is_empty() {
            return;
        }
        let rows: Vec<u32> = self
            .selections()
            .iter()
            .map(|s| self.row_of(s.head))
            .collect();
        let buffer = &self.buffer;
        self.ops
            .autoclosed
            .retain(|a| rows.contains(&buffer.offset_to_point(buffer.offset_for_anchor(a)).row));
    }
}

#[cfg(test)]
mod tests {
    use crate::indent::IndentSettings;
    use crate::indent::tests::{lang, marked};

    #[test]
    fn openers_pair_and_closers_skip() {
        let mut e = lang("x = |;", "csharp");
        e.type_text("(");
        assert_eq!(marked(&e), "x = (|);");
        e.type_text("[");
        assert_eq!(marked(&e), "x = ([|]);");
        e.type_text("1");
        e.type_text("]");
        assert_eq!(marked(&e), "x = ([1]|);");
        e.type_text(")");
        assert_eq!(marked(&e), "x = ([1])|;");
        // A closer the editor did not insert is typed.
        let mut e = lang("f(|)", "csharp");
        e.type_text(")");
        assert_eq!(marked(&e), "f()|)");
    }

    #[test]
    fn quotes_pair_in_code_only() {
        let mut e = lang("var s = |;", "csharp");
        e.type_text("\"");
        assert_eq!(marked(&e), "var s = \"|\";");
        e.type_text("a");
        e.type_text("\"");
        assert_eq!(marked(&e), "var s = \"a\"|;");
        // Inside a string or a comment, no pair.
        let mut e = lang("var s = \"a|b\";", "csharp");
        e.type_text("(");
        assert_eq!(marked(&e), "var s = \"a(|b\";");
        let mut e = lang("var s = @\"C:\\|\";", "csharp");
        e.type_text("{");
        assert_eq!(marked(&e), "var s = @\"C:\\{|\";");
        let mut e = lang("// note |", "csharp");
        e.type_text("(");
        assert_eq!(marked(&e), "// note (|");
        let mut e = lang("var r = \"\"\"raw |\"\"\";", "csharp");
        e.type_text("[");
        assert_eq!(marked(&e), "var r = \"\"\"raw [|\"\"\";");
        // Not before a word, nor a quote after a word.
        let mut e = lang("|x", "csharp");
        e.type_text("(");
        assert_eq!(marked(&e), "(|x");
        let mut e = lang("don|", "");
        e.type_text("'");
        assert_eq!(marked(&e), "don'|");
        // Visual Basic's ' is a comment; Rust's is a lifetime.
        let mut e = lang("Dim x = 1 |", "vb");
        e.type_text("'");
        assert_eq!(marked(&e), "Dim x = 1 '|");
        let mut e = lang("fn f<|>", "rust");
        e.type_text("'");
        assert_eq!(marked(&e), "fn f<'|>");
        let mut e = lang("const s = |", "typescript");
        e.type_text("`");
        assert_eq!(marked(&e), "const s = `|`");
    }

    #[test]
    fn backspace_deletes_an_empty_pair_the_editor_inserted() {
        let mut e = lang("f|", "csharp");
        e.type_text("(");
        e.backspace();
        assert_eq!(marked(&e), "f|");
        let mut e = lang("f(|)", "csharp");
        e.backspace();
        assert_eq!(marked(&e), "f|)");
    }

    #[test]
    fn an_opener_surrounds_the_selection() {
        let mut e = lang("x = [a + b];", "csharp");
        e.type_text("(");
        assert_eq!(marked(&e), "x = ([a + b]);");
        e.type_text("\"");
        assert_eq!(marked(&e), "x = (\"[a + b]\");");
    }

    #[test]
    fn every_caret_and_the_setting() {
        let mut e = lang("a|\nb|", "csharp");
        e.type_text("{");
        assert_eq!(marked(&e), "a{|}\nb{|}");
        e.type_text("}");
        assert_eq!(marked(&e), "a{}|\nb{}|");
        let mut e = lang("a|", "csharp");
        e.set_indent_settings(IndentSettings {
            auto_close: false,
            ..IndentSettings::default()
        });
        e.type_text("(");
        assert_eq!(marked(&e), "a(|");
    }

    #[test]
    fn enter_after_an_auto_inserted_brace() {
        let mut e = lang("class C |", "csharp");
        e.type_text("{");
        e.newline();
        assert_eq!(marked(&e), "class C {\n    |\n}");
    }
}
