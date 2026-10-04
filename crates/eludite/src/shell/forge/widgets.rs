//! Small pieces the forge views share: typing into a text field, an age in words, the checks glyph.

use gpui::{KeyDownEvent, Rgba, rgb};

/// What a key did to a field.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Edit {
    Changed,
    Enter,
    Escape,
    Tab,
    Ignored,
}

/// Apply `event` to `text`: printable characters, Backspace, Enter (a new line in a `multiline` field with Shift,
/// else [`Edit::Enter`]), Escape and Tab.
pub fn edit(text: &mut String, event: &KeyDownEvent, multiline: bool) -> Edit {
    let k = &event.keystroke;
    if k.modifiers.control || k.modifiers.alt || k.modifiers.platform {
        return Edit::Ignored;
    }
    match k.key.as_str() {
        "escape" => Edit::Escape,
        "tab" => Edit::Tab,
        "enter" if multiline && k.modifiers.shift => {
            text.push('\n');
            Edit::Changed
        }
        "enter" => Edit::Enter,
        "backspace" => {
            text.pop();
            Edit::Changed
        }
        "space" => {
            text.push(' ');
            Edit::Changed
        }
        _ => {
            let typed = k.key_char.clone().or_else(|| {
                (k.key.chars().count() == 1).then(|| {
                    if k.modifiers.shift {
                        k.key.to_uppercase()
                    } else {
                        k.key.clone()
                    }
                })
            });
            match typed {
                Some(c) if !c.is_empty() && !c.chars().any(char::is_control) => {
                    text.push_str(&c);
                    Edit::Changed
                }
                _ => Edit::Ignored,
            }
        }
    }
}

/// "3 minutes ago".
pub fn age(seconds: u64) -> String {
    let (n, unit) = match seconds {
        0..=59 => return "just now".into(),
        60..=3599 => (seconds / 60, "minute"),
        3600..=86_399 => (seconds / 3600, "hour"),
        _ => (seconds / 86_400, "day"),
    };
    format!("{n} {unit}{} ago", if n == 1 { "" } else { "s" })
}

/// The glyph and color of a checks state (`success`, `failure`, `pending`).
pub fn checks_glyph(state: Option<&str>) -> (&'static str, Rgba) {
    match state {
        Some("success") => ("\u{2713}", rgb(0x4E_C9_4E)),
        Some("failure") => ("\u{2717}", rgb(0xF1_4C_4C)),
        Some("pending") => ("\u{25CF}", rgb(0xD7_BA_7D)),
        _ => ("", rgb(0x80_80_80)),
    }
}

/// The text of a check's conclusion.
pub fn conclusion_glyph(status: &str, conclusion: &str) -> (&'static str, Rgba) {
    if status != "completed" {
        return checks_glyph(Some("pending"));
    }
    match conclusion {
        "success" => checks_glyph(Some("success")),
        "skipped" | "neutral" => ("\u{2212}", rgb(0x80_80_80)),
        _ => checks_glyph(Some("failure")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_in_words() {
        assert_eq!(age(5), "just now");
        assert_eq!(age(60), "1 minute ago");
        assert_eq!(age(7200), "2 hours ago");
        assert_eq!(age(86_400 * 3), "3 days ago");
    }
}
