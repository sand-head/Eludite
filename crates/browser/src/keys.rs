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

// ---- the Web Browser window's keys (brief 0032) ----

/// A key the Web Browser window forwards to the embedded engine (`tab/input` key events): GPUI's name for it, the
/// Windows virtual key code CEF takes on every platform, the X11 key code of the US layout CEF reads the DOM `code`
/// from on Linux, and the DOM `key` and `code` the page sees (unshifted).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ShellKey {
    pub gpui: &'static str,
    pub windows: i64,
    pub x11: i64,
    pub dom_key: &'static str,
    pub dom_code: &'static str,
}

const fn k(
    gpui: &'static str,
    windows: i64,
    x11: i64,
    dom_key: &'static str,
    dom_code: &'static str,
) -> ShellKey {
    ShellKey {
        gpui,
        windows,
        x11,
        dom_key,
        dom_code,
    }
}

/// Every key the window sends, by GPUI key name (letters and digits on the US layout's positions).
pub const SHELL_KEYS: &[ShellKey] = &[
    k("backspace", 8, 22, "Backspace", "Backspace"),
    k("tab", 9, 23, "Tab", "Tab"),
    k("enter", 13, 36, "Enter", "Enter"),
    k("escape", 27, 9, "Escape", "Escape"),
    k("space", 32, 65, " ", "Space"),
    k("pageup", 33, 112, "PageUp", "PageUp"),
    k("pagedown", 34, 117, "PageDown", "PageDown"),
    k("end", 35, 115, "End", "End"),
    k("home", 36, 110, "Home", "Home"),
    k("left", 37, 113, "ArrowLeft", "ArrowLeft"),
    k("up", 38, 111, "ArrowUp", "ArrowUp"),
    k("right", 39, 114, "ArrowRight", "ArrowRight"),
    k("down", 40, 116, "ArrowDown", "ArrowDown"),
    k("insert", 45, 118, "Insert", "Insert"),
    k("delete", 46, 119, "Delete", "Delete"),
    k("menu", 93, 135, "ContextMenu", "ContextMenu"),
    k("0", 48, 19, "0", "Digit0"),
    k("1", 49, 10, "1", "Digit1"),
    k("2", 50, 11, "2", "Digit2"),
    k("3", 51, 12, "3", "Digit3"),
    k("4", 52, 13, "4", "Digit4"),
    k("5", 53, 14, "5", "Digit5"),
    k("6", 54, 15, "6", "Digit6"),
    k("7", 55, 16, "7", "Digit7"),
    k("8", 56, 17, "8", "Digit8"),
    k("9", 57, 18, "9", "Digit9"),
    k("a", 65, 38, "a", "KeyA"),
    k("b", 66, 56, "b", "KeyB"),
    k("c", 67, 54, "c", "KeyC"),
    k("d", 68, 40, "d", "KeyD"),
    k("e", 69, 26, "e", "KeyE"),
    k("f", 70, 41, "f", "KeyF"),
    k("g", 71, 42, "g", "KeyG"),
    k("h", 72, 43, "h", "KeyH"),
    k("i", 73, 31, "i", "KeyI"),
    k("j", 74, 44, "j", "KeyJ"),
    k("k", 75, 45, "k", "KeyK"),
    k("l", 76, 46, "l", "KeyL"),
    k("m", 77, 58, "m", "KeyM"),
    k("n", 78, 57, "n", "KeyN"),
    k("o", 79, 32, "o", "KeyO"),
    k("p", 80, 33, "p", "KeyP"),
    k("q", 81, 24, "q", "KeyQ"),
    k("r", 82, 27, "r", "KeyR"),
    k("s", 83, 39, "s", "KeyS"),
    k("t", 84, 28, "t", "KeyT"),
    k("u", 85, 30, "u", "KeyU"),
    k("v", 86, 55, "v", "KeyV"),
    k("w", 87, 25, "w", "KeyW"),
    k("x", 88, 53, "x", "KeyX"),
    k("y", 89, 29, "y", "KeyY"),
    k("z", 90, 52, "z", "KeyZ"),
    k("f1", 112, 67, "F1", "F1"),
    k("f2", 113, 68, "F2", "F2"),
    k("f3", 114, 69, "F3", "F3"),
    k("f4", 115, 70, "F4", "F4"),
    k("f5", 116, 71, "F5", "F5"),
    k("f6", 117, 72, "F6", "F6"),
    k("f7", 118, 73, "F7", "F7"),
    k("f8", 119, 74, "F8", "F8"),
    k("f9", 120, 75, "F9", "F9"),
    k("f10", 121, 76, "F10", "F10"),
    k("f11", 122, 95, "F11", "F11"),
    k("f12", 123, 96, "F12", "F12"),
    k(";", 186, 47, ";", "Semicolon"),
    k("=", 187, 21, "=", "Equal"),
    k(",", 188, 59, ",", "Comma"),
    k("-", 189, 20, "-", "Minus"),
    k(".", 190, 60, ".", "Period"),
    k("/", 191, 61, "/", "Slash"),
    k("`", 192, 49, "`", "Backquote"),
    k("[", 219, 34, "[", "BracketLeft"),
    k("\\", 220, 51, "\\", "Backslash"),
    k("]", 221, 35, "]", "BracketRight"),
    k("'", 222, 48, "'", "Quote"),
];

/// The window's key for a GPUI key name (`a`, `A`, `enter`, `f5`).
pub fn shell_key(gpui: &str) -> Option<&'static ShellKey> {
    let lower = gpui.to_ascii_lowercase();
    SHELL_KEYS.iter().find(|k| k.gpui == lower)
}

#[cfg(test)]
mod shell_key_tests {
    use super::*;

    #[test]
    fn the_shell_key_table_is_complete_and_unambiguous() {
        let mut names: Vec<_> = SHELL_KEYS.iter().map(|k| k.gpui).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), SHELL_KEYS.len(), "one row per key");
        let mut x11: Vec<_> = SHELL_KEYS.iter().map(|k| k.x11).collect();
        x11.sort_unstable();
        x11.dedup();
        assert_eq!(x11.len(), SHELL_KEYS.len(), "one X11 key code per key");
        for c in ('a'..='z').chain('0'..='9') {
            assert!(shell_key(&c.to_string()).is_some(), "{c}");
        }
        assert_eq!(shell_key("A").unwrap().windows, 65);
        assert_eq!(shell_key("enter").unwrap().x11, 36);
        assert_eq!(shell_key("f12").unwrap().windows, 123);
        // The same codes the agent's key table (input) uses.
        for name in [
            "Enter",
            "Tab",
            "Escape",
            "Backspace",
            "ArrowLeft",
            "Home",
            "F5",
        ] {
            let d = key_def(name, false);
            assert!(
                SHELL_KEYS
                    .iter()
                    .any(|k| k.windows == d.key_code && k.dom_code == d.code),
                "{name}"
            );
        }
        assert!(shell_key("fly").is_none());
    }
}
