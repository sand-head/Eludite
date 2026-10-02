//! Keys for `eludite.browser.input` (brief 0024): what `Input.dispatchKeyEvent` needs for a key name or a character
//! (the DOM `key` and `code`, the Windows virtual key code Chrome maps to a keyboard event, and the text it
//! inserts), on a US layout, as Puppeteer's and Playwright's key tables give them.

use eludite_commands::browser::Modifiers;
use serde_json::{Value, json};

/// One key as CDP sends it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyDef {
    /// The DOM `key` (`Enter`, `a`, `A`).
    pub key: String,
    /// The DOM `code` (`Enter`, `KeyA`); empty when the US layout has no key for the character.
    pub code: String,
    /// The Windows virtual key code.
    pub key_code: i64,
    /// What the key inserts, if anything (`\r` for Enter).
    pub text: Option<String>,
}

fn def(key: &str, code: &str, key_code: i64, text: Option<&str>) -> KeyDef {
    KeyDef {
        key: key.to_owned(),
        code: code.to_owned(),
        key_code,
        text: text.map(str::to_owned),
    }
}

/// US-layout punctuation: (unshifted, shifted, code, key code).
const PUNCTUATION: [(char, char, &str, i64); 11] = [
    ('-', '_', "Minus", 189),
    ('=', '+', "Equal", 187),
    ('[', '{', "BracketLeft", 219),
    (']', '}', "BracketRight", 221),
    ('\\', '|', "Backslash", 220),
    (';', ':', "Semicolon", 186),
    ('\'', '"', "Quote", 222),
    (',', '<', "Comma", 188),
    ('.', '>', "Period", 190),
    ('/', '?', "Slash", 191),
    ('`', '~', "Backquote", 192),
];
const SHIFTED_DIGITS: &str = ")!@#$%^&*(";

/// The definition of a key name from `eludite_commands::browser::KEY_NAMES` or of one character. `shift` makes a
/// lowercase letter uppercase, as the keyboard would.
pub fn key_def(name: &str, shift: bool) -> KeyDef {
    match name {
        "Enter" => def("Enter", "Enter", 13, Some("\r")),
        "Tab" => def("Tab", "Tab", 9, None),
        "Escape" => def("Escape", "Escape", 27, None),
        "Backspace" => def("Backspace", "Backspace", 8, None),
        "Delete" => def("Delete", "Delete", 46, None),
        "Insert" => def("Insert", "Insert", 45, None),
        "ArrowUp" => def("ArrowUp", "ArrowUp", 38, None),
        "ArrowDown" => def("ArrowDown", "ArrowDown", 40, None),
        "ArrowLeft" => def("ArrowLeft", "ArrowLeft", 37, None),
        "ArrowRight" => def("ArrowRight", "ArrowRight", 39, None),
        "Home" => def("Home", "Home", 36, None),
        "End" => def("End", "End", 35, None),
        "PageUp" => def("PageUp", "PageUp", 33, None),
        "PageDown" => def("PageDown", "PageDown", 34, None),
        "Space" => def(" ", "Space", 32, Some(" ")),
        "ContextMenu" => def("ContextMenu", "ContextMenu", 93, None),
        f if f.len() >= 2
            && f.starts_with('F')
            && f[1..].parse::<i64>().is_ok_and(|n| (1..=12).contains(&n)) =>
        {
            let n: i64 = f[1..].parse().unwrap_or(1);
            def(f, f, 111 + n, None)
        }
        other => char_def(other.chars().next().unwrap_or(' '), shift),
    }
}

/// The key that types `c` on a US layout (or a key with no code for other characters).
pub fn char_def(c: char, shift: bool) -> KeyDef {
    let text = |c: char| Some(c.to_string());
    if c.is_ascii_alphabetic() {
        let c = if shift { c.to_ascii_uppercase() } else { c };
        let upper = c.to_ascii_uppercase();
        return KeyDef {
            key: c.to_string(),
            code: format!("Key{upper}"),
            key_code: upper as i64,
            text: text(c),
        };
    }
    if c.is_ascii_digit() {
        return KeyDef {
            key: c.to_string(),
            code: format!("Digit{c}"),
            key_code: c as i64,
            text: text(c),
        };
    }
    if let Some(i) = SHIFTED_DIGITS.find(c) {
        return KeyDef {
            key: c.to_string(),
            code: format!("Digit{i}"),
            key_code: 48 + i as i64,
            text: text(c),
        };
    }
    match c {
        ' ' => return def(" ", "Space", 32, Some(" ")),
        '\n' | '\r' => return def("Enter", "Enter", 13, Some("\r")),
        '\t' => return def("Tab", "Tab", 9, None),
        _ => {}
    }
    if let Some((_, _, code, kc)) = PUNCTUATION
        .iter()
        .find(|(plain, shifted, _, _)| *plain == c || *shifted == c)
    {
        return KeyDef {
            key: c.to_string(),
            code: (*code).to_owned(),
            key_code: *kc,
            text: text(c),
        };
    }
    KeyDef {
        key: c.to_string(),
        code: String::new(),
        key_code: 0,
        text: text(c),
    }
}

/// The modifier keys held, in the order they go down.
pub fn modifier_defs(m: Modifiers) -> Vec<(KeyDef, i64)> {
    let mut out = Vec::new();
    if m.control {
        out.push((def("Control", "ControlLeft", 17, None), 2));
    }
    if m.shift {
        out.push((def("Shift", "ShiftLeft", 16, None), 8));
    }
    if m.alt {
        out.push((def("Alt", "AltLeft", 18, None), 1));
    }
    if m.meta {
        out.push((def("Meta", "MetaLeft", 91, None), 4));
    }
    out
}

/// `Input.dispatchKeyEvent` parameters for pressing (`down`) or releasing `k` with modifier bits `modifiers`. A key
/// pressed with Control, Alt or Meta inserts no text.
pub fn key_event(k: &KeyDef, down: bool, modifiers: i64) -> Value {
    let text = k.text.as_deref().filter(|_| modifiers & 0b0111 == 0);
    let kind = match (down, text) {
        (false, _) => "keyUp",
        (true, Some(_)) => "keyDown",
        (true, None) => "rawKeyDown",
    };
    let mut e = json!({
        "type": kind,
        "key": k.key,
        "code": k.code,
        "windowsVirtualKeyCode": k.key_code,
        "nativeVirtualKeyCode": k.key_code,
        "modifiers": modifiers,
    });
    if down && let Some(t) = text {
        e["text"] = json!(t);
        e["unmodifiedText"] = json!(t);
    }
    e
}

/// The key events of pressing `name` with modifiers `m` held: the modifiers down, the key down and up, the
/// modifiers up.
pub fn press(name: &str, m: Modifiers) -> Vec<Value> {
    let k = key_def(name, m.shift);
    let mods = modifier_defs(m);
    let mut out = Vec::new();
    let mut bits = 0;
    for (d, bit) in &mods {
        bits |= bit;
        out.push(key_event(d, true, bits));
    }
    out.push(key_event(&k, true, bits));
    out.push(key_event(&k, false, bits));
    for (d, bit) in mods.iter().rev() {
        bits &= !bit;
        out.push(key_event(d, false, bits));
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_characters_and_modifiers() {
        assert_eq!(key_def("Enter", false).text.as_deref(), Some("\r"));
        assert_eq!(key_def("F5", false).key_code, 116);
        assert_eq!(key_def("Space", false).key, " ");
        let a = key_def("a", true);
        assert_eq!(
            (a.key.as_str(), a.code.as_str(), a.key_code),
            ("A", "KeyA", 65)
        );
        assert_eq!(char_def('!', false).code, "Digit1");
        assert_eq!(char_def('?', false).code, "Slash");
        assert_eq!(char_def('\u{e9}', false).key_code, 0);
        let events = press(
            "a",
            Modifiers {
                control: true,
                ..Default::default()
            },
        );
        let kinds: Vec<_> = events
            .iter()
            .map(|e| {
                (
                    e["type"].as_str().unwrap(),
                    e["key"].as_str().unwrap(),
                    e["modifiers"].as_i64().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            kinds,
            [
                ("rawKeyDown", "Control", 2),
                ("rawKeyDown", "a", 2),
                ("keyUp", "a", 2),
                ("keyUp", "Control", 0)
            ]
        );
        assert!(events[1].get("text").is_none(), "Control+a inserts nothing");
        let enter = press("Enter", Modifiers::default());
        assert_eq!(enter[0]["type"], "keyDown");
        assert_eq!(enter[0]["text"], "\r");
    }
}
