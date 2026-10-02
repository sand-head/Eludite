//! The keymap: a table from keystrokes to command-bus commands.
//!
//! Every binding produces one GPUI action, [`RunCommand`], carrying the command
//! id and its JSON input. The shell handles `RunCommand` by invoking the
//! command bus, so keys and menus reach the same commands agents do (PLAN.md
//! 5.1). Only the Visual Studio preset exists (brief 0008, out of scope: other
//! presets).

use gpui::{App, KeyBinding, SharedString};
use serde_json::{Value, json};

/// The one action every key binding and menu item dispatches.
#[derive(Debug, Clone, PartialEq, gpui::Action)]
#[action(namespace = eludite, no_json)]
pub struct RunCommand {
    pub command: SharedString,
    pub args: Value,
}

impl RunCommand {
    pub fn new(command: impl Into<SharedString>, args: Value) -> Self {
        Self {
            command: command.into(),
            args,
        }
    }
}

/// One row of the keymap table.
#[derive(Debug, Clone, PartialEq)]
pub struct KeyBindingSpec {
    /// GPUI keystroke syntax, chords separated by spaces (`ctrl-\ ctrl-e`).
    pub keystrokes: &'static str,
    /// As Visual Studio shows it (`Ctrl+\, Ctrl+E`).
    pub display: &'static str,
    pub command: &'static str,
    pub args: Value,
}

/// GPUI key context the bindings are scoped to; the shell's root element sets it.
pub const SHELL_CONTEXT: &str = "EluditeShell";

const SHOW: &str = "eludite.view.show";

/// Visual Studio's default bindings for the commands that exist.
pub fn vs_keymap() -> Vec<KeyBindingSpec> {
    let show = |keystrokes, display, id: &str| KeyBindingSpec {
        keystrokes,
        display,
        command: SHOW,
        args: json!({ "id": id }),
    };
    vec![
        show("ctrl-alt-l", "Ctrl+Alt+L", "solution_explorer"),
        show("ctrl-alt-x", "Ctrl+Alt+X", "toolbox"),
        show("ctrl-alt-o", "Ctrl+Alt+O", "output"),
        show("ctrl-\\ ctrl-e", "Ctrl+\\, Ctrl+E", "error_list"),
        show("f4", "F4", "properties"),
        show("ctrl-0 ctrl-g", "Ctrl+0, Ctrl+G", "git_changes"),
        KeyBindingSpec {
            keystrokes: "shift-escape",
            display: "Shift+Esc",
            command: "eludite.view.hide",
            args: json!({}),
        },
        command("ctrl-shift-o", "Ctrl+Shift+O", "eludite.solution.open"),
        command("ctrl-s", "Ctrl+S", "eludite.editor.save"),
        command("ctrl-z", "Ctrl+Z", "eludite.editor.undo"),
        command("ctrl-y", "Ctrl+Y", "eludite.editor.redo"),
        command("ctrl-f", "Ctrl+F", "eludite.editor.find"),
    ]
}

/// The editor actions that are commands (brief 0012): the shell binds these keys in the editor's own key context
/// too, so they reach the command bus instead of the editor's built-in actions.
pub const EDITOR_COMMAND_KEYS: [&str; 4] = ["ctrl-z", "ctrl-y", "ctrl-shift-z", "ctrl-f"];

fn command(
    keystrokes: &'static str,
    display: &'static str,
    command: &'static str,
) -> KeyBindingSpec {
    KeyBindingSpec {
        keystrokes,
        display,
        command,
        args: json!({}),
    }
}

/// Register `keymap` with GPUI, scoped to [`SHELL_CONTEXT`].
pub fn bind_keymap(cx: &mut App, keymap: &[KeyBindingSpec]) {
    cx.bind_keys(keymap.iter().map(|k| {
        KeyBinding::new(
            k.keystrokes,
            RunCommand::new(k.command, k.args.clone()),
            Some(SHELL_CONTEXT),
        )
    }));
}

/// The display text of the binding for `command` with exactly `args`.
pub fn shortcut_for(
    keymap: &[KeyBindingSpec],
    command: &str,
    args: &Value,
) -> Option<&'static str> {
    keymap
        .iter()
        .find(|k| k.command == command && &k.args == args)
        .map(|k| k.display)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vs_table() {
        let k = vs_keymap();
        let find = |id: &str| shortcut_for(&k, SHOW, &json!({ "id": id }));
        assert_eq!(find("solution_explorer"), Some("Ctrl+Alt+L"));
        assert_eq!(find("toolbox"), Some("Ctrl+Alt+X"));
        assert_eq!(find("output"), Some("Ctrl+Alt+O"));
        assert_eq!(find("error_list"), Some("Ctrl+\\, Ctrl+E"));
        assert_eq!(find("nope"), None);
        assert_eq!(
            shortcut_for(&k, "eludite.editor.save", &json!({})),
            Some("Ctrl+S")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.solution.open", &json!({})),
            Some("Ctrl+Shift+O")
        );
        let mut keys: Vec<_> = k.iter().map(|b| b.keystrokes).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), k.len(), "no duplicate keystrokes");
    }

    #[test]
    fn keystrokes_parse() {
        for b in vs_keymap() {
            for ks in b.keystrokes.split_whitespace() {
                assert!(gpui::Keystroke::parse(ks).is_ok(), "{ks}");
            }
        }
    }
}
