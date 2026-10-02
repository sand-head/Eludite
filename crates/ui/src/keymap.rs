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
        show("ctrl-alt-l", "Ctrl+Alt+L", "workspace"),
        show("ctrl-alt-x", "Ctrl+Alt+X", "toolbox"),
        show("ctrl-alt-o", "Ctrl+Alt+O", "output"),
        show("ctrl-\\ ctrl-e", "Ctrl+\\, Ctrl+E", "error_list"),
        show("f4", "F4", "properties"),
        show("ctrl-0 ctrl-g", "Ctrl+0, Ctrl+G", "git_changes"),
        // The Agents window (brief 0016), on Visual Studio's chat window chord (View.GitHub.Copilot.Chat).
        show("ctrl-\\ ctrl-c", "Ctrl+\\, Ctrl+C", "agents"),
        KeyBindingSpec {
            keystrokes: "shift-escape",
            display: "Shift+Esc",
            command: "eludite.view.hide",
            args: json!({}),
        },
        command("ctrl-shift-o", "Ctrl+Shift+O", "eludite.solution.open"),
        // File.OpenFolder (brief 0019).
        command(
            "ctrl-shift-alt-o",
            "Ctrl+Shift+Alt+O",
            "eludite.workspace.open_folder",
        ),
        command("ctrl-s", "Ctrl+S", "eludite.editor.save"),
        command("ctrl-z", "Ctrl+Z", "eludite.editor.undo"),
        command("ctrl-y", "Ctrl+Y", "eludite.editor.redo"),
        command("ctrl-f", "Ctrl+F", "eludite.editor.find"),
        // Edit > IntelliSense (brief 0013).
        command("ctrl-space", "Ctrl+Space", "eludite.editor.complete"),
        command(
            "ctrl-shift-space",
            "Ctrl+Shift+Space",
            "eludite.editor.signature_help",
        ),
        command("ctrl-k ctrl-i", "Ctrl+K, Ctrl+I", "eludite.editor.hover"),
        // Navigation (brief 0014): Edit.GoToDefinition, Edit.FindAllReferences, View.NavigateBackward and
        // View.NavigateForward.
        command("f12", "F12", "eludite.editor.go_to_definition"),
        command("shift-f12", "Shift+F12", "eludite.editor.find_references"),
        command("ctrl--", "Ctrl+-", "eludite.navigation.back"),
        command("ctrl-shift--", "Ctrl+Shift+-", "eludite.navigation.forward"),
        // The same keys where the layout reports Shift+- as `_`.
        command("ctrl-_", "Ctrl+Shift+-", "eludite.navigation.forward"),
        // Refactoring (brief 0015): Refactor.Rename (Ctrl+R, Ctrl+R; F2 too) and View.QuickActions (Ctrl+.; Alt+Enter
        // too).
        command("ctrl-r ctrl-r", "Ctrl+R, Ctrl+R", "eludite.editor.rename"),
        command("f2", "F2", "eludite.editor.rename"),
        command("ctrl-.", "Ctrl+.", "eludite.editor.code_actions"),
        command("alt-enter", "Alt+Enter", "eludite.editor.code_actions"),
        // Build (brief 0017): Build.BuildSolution (Ctrl+Shift+B; F6 in the C# profile) and Build.BuildSelection
        // (Shift+F6).
        command("ctrl-shift-b", "Ctrl+Shift+B", "eludite.build.solution"),
        command("f6", "F6", "eludite.build.solution"),
        command("shift-f6", "Shift+F6", "eludite.build.project"),
        // Debug (brief 0018): Debug.Start (F5, Continue while in break mode), Debug.StartWithoutDebugging,
        // Debug.StopDebugging, Debug.ToggleBreakpoint, Debug.StepOver, Debug.StepInto, Debug.StepOut and
        // Debug.RunToCursor.
        command("f5", "F5", "eludite.debug.start"),
        KeyBindingSpec {
            keystrokes: "ctrl-f5",
            display: "Ctrl+F5",
            command: "eludite.debug.start",
            args: json!({ "debug": false }),
        },
        command("shift-f5", "Shift+F5", "eludite.debug.stop"),
        command("f9", "F9", "eludite.debug.toggle_breakpoint"),
        command("f10", "F10", "eludite.debug.step_over"),
        command("f11", "F11", "eludite.debug.step_into"),
        command("shift-f11", "Shift+F11", "eludite.debug.step_out"),
        command("ctrl-f10", "Ctrl+F10", "eludite.debug.run_to_cursor"),
    ]
}

/// The editor actions that are commands (briefs 0012 and 0013): the shell binds these keys in the editor's own key
/// context too, so they reach the command bus instead of the editor's built-in actions.
pub const EDITOR_COMMAND_KEYS: [&str; 7] = [
    "ctrl-z",
    "ctrl-y",
    "ctrl-shift-z",
    "ctrl-f",
    "ctrl-space",
    "ctrl-shift-space",
    "ctrl-k ctrl-i",
];

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
        assert_eq!(find("workspace"), Some("Ctrl+Alt+L"));
        assert_eq!(find("toolbox"), Some("Ctrl+Alt+X"));
        assert_eq!(find("output"), Some("Ctrl+Alt+O"));
        assert_eq!(find("error_list"), Some("Ctrl+\\, Ctrl+E"));
        assert_eq!(find("agents"), Some("Ctrl+\\, Ctrl+C"));
        assert_eq!(find("nope"), None);
        assert_eq!(
            shortcut_for(&k, "eludite.editor.save", &json!({})),
            Some("Ctrl+S")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.solution.open", &json!({})),
            Some("Ctrl+Shift+O")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.complete", &json!({})),
            Some("Ctrl+Space")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.signature_help", &json!({})),
            Some("Ctrl+Shift+Space")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.hover", &json!({})),
            Some("Ctrl+K, Ctrl+I")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.go_to_definition", &json!({})),
            Some("F12")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.find_references", &json!({})),
            Some("Shift+F12")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.navigation.back", &json!({})),
            Some("Ctrl+-")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.navigation.forward", &json!({})),
            Some("Ctrl+Shift+-")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.rename", &json!({})),
            Some("Ctrl+R, Ctrl+R")
        );
        assert!(
            k.iter()
                .any(|b| b.keystrokes == "f2" && b.command == "eludite.editor.rename")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.editor.code_actions", &json!({})),
            Some("Ctrl+.")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.build.solution", &json!({})),
            Some("Ctrl+Shift+B")
        );
        assert!(
            k.iter()
                .any(|b| b.keystrokes == "f6" && b.command == "eludite.build.solution")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.build.project", &json!({})),
            Some("Shift+F6")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.debug.start", &json!({})),
            Some("F5")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.debug.start", &json!({"debug": false})),
            Some("Ctrl+F5")
        );
        for (cmd, key) in [
            ("eludite.debug.stop", "Shift+F5"),
            ("eludite.debug.toggle_breakpoint", "F9"),
            ("eludite.debug.step_over", "F10"),
            ("eludite.debug.step_into", "F11"),
            ("eludite.debug.step_out", "Shift+F11"),
            ("eludite.debug.run_to_cursor", "Ctrl+F10"),
        ] {
            assert_eq!(shortcut_for(&k, cmd, &json!({})), Some(key), "{cmd}");
        }
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
