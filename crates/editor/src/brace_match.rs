//! Brace matching (brief 0067): the bracket at the caret and its match get Visual Studio's brace highlight; Go To
//! Brace (Ctrl+]) moves the caret to the match and Extend to Brace (Ctrl+Shift+]) selects through it.
//!
//! The bracket after the caret is tried first, then the one before it, as Visual Studio does. The scan skips brackets
//! inside strings and comments (by the current highlights) and gives up [`SCAN_LIMIT`] bytes away.

use std::ops::Range;

use gpui::Context;

use crate::SelectionRange;
use crate::syntax::HighlightKind;
use crate::view::EditorView;

/// How far from the caret the scan looks for the match.
pub const SCAN_LIMIT: usize = 100_000;

fn pair(c: char) -> Option<(char, char, bool)> {
    Some(match c {
        '(' => ('(', ')', true),
        '[' => ('[', ']', true),
        '{' => ('{', '}', true),
        ')' => ('(', ')', false),
        ']' => ('[', ']', false),
        '}' => ('{', '}', false),
        _ => return None,
    })
}

/// The matching pair for the bracket starting at byte `at` of `text` (`code(offset)` says whether a byte is code, not a
/// string or comment): the open bracket's offset and the close bracket's.
pub fn match_at(text: &str, at: usize, code: impl Fn(usize) -> bool) -> Option<(usize, usize)> {
    let c = text.get(at..)?.chars().next()?;
    let (open, close, forward) = pair(c)?;
    if !code(at) {
        return None;
    }
    let mut depth = 0usize;
    if forward {
        let end = (at + SCAN_LIMIT).min(text.len());
        for (i, ch) in text[at..end].char_indices() {
            let o = at + i;
            if ch == open && code(o) {
                depth += 1;
            } else if ch == close && code(o) {
                depth -= 1;
                if depth == 0 {
                    return Some((at, o));
                }
            }
        }
    } else {
        let start = at.saturating_sub(SCAN_LIMIT);
        let start = crate::display::floor_char_boundary(text, start);
        for (i, ch) in text[start..=at].char_indices().rev() {
            let o = start + i;
            if ch == close && code(o) {
                depth += 1;
            } else if ch == open && code(o) {
                depth -= 1;
                if depth == 0 {
                    return Some((o, at));
                }
            }
        }
    }
    None
}

/// The pair at a caret at `caret`: the bracket after it, else the one before it.
pub fn match_near(
    text: &str,
    caret: usize,
    code: impl Fn(usize) -> bool,
) -> Option<(usize, usize)> {
    match_at(text, caret, &code).or_else(|| {
        let before = text[..caret.min(text.len())].char_indices().next_back()?.0;
        match_at(text, before, &code)
    })
}

impl EditorView {
    /// Whether `offset` is code by the current highlights (not inside a string or comment).
    fn is_code(&self, offset: usize) -> bool {
        let point = self.editor.buffer().offset_to_point(offset);
        !matches!(
            self.highlights().kind_at(point),
            Some(HighlightKind::String | HighlightKind::Comment | HighlightKind::DocComment)
        )
    }

    /// The bracket pair at the primary caret, as byte ranges of the two brackets (one byte each).
    pub fn brace_pair(&self) -> Option<(Range<usize>, Range<usize>)> {
        if !self.display.brace_matching {
            return None;
        }
        let sel = self.editor.primary_selection();
        if !sel.is_empty() {
            return None;
        }
        let buffer = self.editor.buffer();
        // A window around the caret is enough for the scan limit.
        let start = buffer.clip_offset(sel.head.saturating_sub(SCAN_LIMIT), text::Bias::Left);
        let end = buffer.clip_offset((sel.head + SCAN_LIMIT).min(buffer.len()), text::Bias::Right);
        let text = buffer.text_for_range(start..end);
        let (o, c) = match_near(&text, sel.head - start, |i| self.is_code(start + i))?;
        Some((start + o..start + o + 1, start + c..start + c + 1))
    }

    /// Go To Brace (Ctrl+]): the caret to the matching bracket (after a close bracket, as Visual Studio puts it).
    pub fn go_to_brace(&mut self, extend: bool, cx: &mut Context<Self>) -> bool {
        let saved = self.display.brace_matching;
        self.display.brace_matching = true;
        let pair = self.brace_pair();
        self.display.brace_matching = saved;
        let Some((open, close)) = pair else {
            return false;
        };
        let head = self.editor.primary_selection().head;
        // At the open bracket (before or after it) go after the close one; at the close one go before the open one.
        let at_open = head == open.start || head == open.end;
        let target = if at_open { close.end } else { open.start };
        if extend {
            let (tail, head) = if at_open {
                (open.start, close.end)
            } else {
                (close.end, open.start)
            };
            self.editor
                .set_selections(vec![SelectionRange { tail, head }], 0);
        } else {
            self.editor.set_caret(target);
        }
        self.reveal_hidden_carets(cx);
        self.request_autoscroll(cx);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pairs_nest_and_skip_what_is_not_code() {
        let text = "f(a[1], {b: (2)})";
        let all = |_: usize| true;
        assert_eq!(match_at(text, 1, all), Some((1, 16)));
        assert_eq!(match_at(text, 16, all), Some((1, 16)));
        assert_eq!(match_at(text, 3, all), Some((3, 5)));
        assert_eq!(match_at(text, 0, all), None);
        // After the caret first, then before it.
        assert_eq!(match_near(text, 8, all), Some((8, 15)));
        assert_eq!(match_near(text, 17, all), Some((1, 16)));
        // A bracket in a "string" is skipped.
        let text = "(\")\")";
        assert_eq!(match_at(text, 0, |i| !(1..=3).contains(&i)), Some((0, 4)));
        // Unbalanced: none.
        assert_eq!(match_at("((", 0, all), None);
    }
}
