//! Keystrokes to the bytes a terminal sends, as xterm sends them (`TERM=xterm-256color`): control characters for
//! Ctrl and a letter, ESC before a character for Alt, CSI sequences for the arrows, Home, End, Insert, Delete, Page
//! Up, Page Down and the function keys, with xterm's modifier parameter (`ESC [1;5A` is Ctrl+Up), and SS3 forms for
//! the arrows, Home and End while the application asked for cursor keys mode.

use gpui::Keystroke;

/// The bytes for `ks`, or `None` for a keystroke the terminal does not send (a lone modifier).
pub fn to_bytes(ks: &Keystroke, app_cursor: bool) -> Option<Vec<u8>> {
    let m = &ks.modifiers;
    let key = ks.key.as_str();
    // xterm's modifier parameter: 1 + Shift + 2 Alt + 4 Ctrl.
    let param = 1 + u8::from(m.shift) + 2 * u8::from(m.alt) + 4 * u8::from(m.control);
    let csi = |final_byte: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{final_byte}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_byte}").into_bytes()
        } else {
            format!("\x1b[{final_byte}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[{n};{param}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let ss3 = |final_byte: char| -> Vec<u8> {
        if param > 1 {
            format!("\x1b[1;{param}{final_byte}").into_bytes()
        } else {
            format!("\x1bO{final_byte}").into_bytes()
        }
    };
    let alt = |mut b: Vec<u8>| {
        if m.alt {
            b.insert(0, 0x1b);
        }
        b
    };
    let bytes = match key {
        "up" => csi('A'),
        "down" => csi('B'),
        "right" => csi('C'),
        "left" => csi('D'),
        "home" => csi('H'),
        "end" => csi('F'),
        "insert" => tilde(2),
        "delete" => tilde(3),
        "pageup" => tilde(5),
        "pagedown" => tilde(6),
        "f1" => ss3('P'),
        "f2" => ss3('Q'),
        "f3" => ss3('R'),
        "f4" => ss3('S'),
        "f5" => tilde(15),
        "f6" => tilde(17),
        "f7" => tilde(18),
        "f8" => tilde(19),
        "f9" => tilde(20),
        "f10" => tilde(21),
        "f11" => tilde(23),
        "f12" => tilde(24),
        "enter" => alt(b"\r".to_vec()),
        "tab" if m.shift => b"\x1b[Z".to_vec(),
        "tab" => alt(b"\t".to_vec()),
        "escape" => alt(b"\x1b".to_vec()),
        "backspace" if m.control => alt(vec![0x08]),
        "backspace" => alt(vec![0x7f]),
        "space" if m.control => alt(vec![0]),
        "space" => alt(b" ".to_vec()),
        "shift" | "control" | "alt" | "platform" | "function" | "capslock" => return None,
        _ => {
            let ch = if m.control && !m.platform {
                control_char(key, m.shift)
            } else {
                None
            };
            match ch {
                Some(c) => alt(vec![c]),
                None => {
                    if m.platform || (m.control && key.chars().count() == 1) {
                        // Ctrl with a key that has no control character, or the Super key: nothing.
                        return None;
                    }
                    let text = ks.key_char.clone().or_else(|| {
                        (key.chars().count() == 1).then(|| {
                            if m.shift {
                                key.to_uppercase()
                            } else {
                                key.to_owned()
                            }
                        })
                    })?;
                    alt(text.into_bytes())
                }
            }
        }
    };
    Some(bytes)
}

/// Ctrl and `key`: the control character, as a terminal sends it.
fn control_char(key: &str, shift: bool) -> Option<u8> {
    let mut chars = key.chars();
    let c = chars.next()?;
    if chars.next().is_some() {
        return None;
    }
    Some(match c.to_ascii_lowercase() {
        c @ 'a'..='z' => c as u8 - b'a' + 1,
        '@' | '2' => 0,
        '[' | '3' => 0x1b,
        '\\' | '4' => 0x1c,
        ']' | '5' => 0x1d,
        '^' | '6' => 0x1e,
        '_' | '7' => 0x1f,
        '-' if shift => 0x1f,
        '/' => 0x1f,
        '8' | '?' => 0x7f,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bytes(s: &str) -> Option<Vec<u8>> {
        to_bytes(&Keystroke::parse(s).unwrap(), false)
    }

    #[test]
    fn control_characters_and_alt() {
        assert_eq!(bytes("ctrl-c"), Some(vec![3]));
        assert_eq!(bytes("ctrl-d"), Some(vec![4]));
        assert_eq!(bytes("ctrl-l"), Some(vec![12]));
        assert_eq!(bytes("ctrl-["), Some(vec![0x1b]));
        assert_eq!(bytes("ctrl-space"), Some(vec![0]));
        assert_eq!(bytes("alt-b"), Some(b"\x1bb".to_vec()));
        assert_eq!(bytes("ctrl-alt-c"), Some(vec![0x1b, 3]));
        assert_eq!(bytes("enter"), Some(b"\r".to_vec()));
        assert_eq!(bytes("backspace"), Some(vec![0x7f]));
        assert_eq!(bytes("shift-tab"), Some(b"\x1b[Z".to_vec()));
        assert_eq!(bytes("shift"), None);
    }

    #[test]
    fn cursor_and_function_keys_follow_xterm() {
        assert_eq!(bytes("up"), Some(b"\x1b[A".to_vec()));
        assert_eq!(
            to_bytes(&Keystroke::parse("up").unwrap(), true),
            Some(b"\x1bOA".to_vec())
        );
        assert_eq!(bytes("ctrl-left"), Some(b"\x1b[1;5D".to_vec()));
        assert_eq!(bytes("shift-end"), Some(b"\x1b[1;2F".to_vec()));
        assert_eq!(bytes("pageup"), Some(b"\x1b[5~".to_vec()));
        assert_eq!(bytes("delete"), Some(b"\x1b[3~".to_vec()));
        assert_eq!(bytes("f1"), Some(b"\x1bOP".to_vec()));
        assert_eq!(bytes("f5"), Some(b"\x1b[15~".to_vec()));
        assert_eq!(bytes("ctrl-f12"), Some(b"\x1b[24;5~".to_vec()));
    }

    #[test]
    fn characters_use_the_typed_text() {
        let mut ks = Keystroke::parse("a").unwrap();
        assert_eq!(to_bytes(&ks, false), Some(b"a".to_vec()));
        ks.key_char = Some("\u{e9}".into());
        assert_eq!(to_bytes(&ks, false), Some("\u{e9}".as_bytes().to_vec()));
        assert_eq!(bytes("shift-a"), Some(b"A".to_vec()));
    }
}
