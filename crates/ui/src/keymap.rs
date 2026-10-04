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
        // View.GitRepositoryWindow (brief 0040).
        show("ctrl-0 ctrl-r", "Ctrl+0, Ctrl+R", "git_repository"),
        // The Agents window (brief 0016), on Visual Studio's chat window chord (View.GitHub.Copilot.Chat).
        show("ctrl-\\ ctrl-c", "Ctrl+\\, Ctrl+C", "agents"),
        // View.Terminal and View.NewTerminal (brief 0041): Ctrl+` shows the Terminal window (with a terminal), Ctrl+Shift+`
        // opens a new one; `ctrl-~` is the same keys where the layout reports Shift+` as `~`.
        show("ctrl-`", "Ctrl+`", "terminal"),
        command("ctrl-shift-`", "Ctrl+Shift+`", "eludite.terminal.open"),
        command("ctrl-~", "Ctrl+Shift+`", "eludite.terminal.open"),
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
        // Edit.FindInFiles and Edit.ReplaceInFiles (brief 0042): the dialog, with the editor's selection as the query;
        // Edit.GoToNextLocation and Edit.GoToPrevLocation (F8, Shift+F8) step through the active Find Results window.
        command("ctrl-shift-f", "Ctrl+Shift+F", "eludite.search.find"),
        command("ctrl-shift-h", "Ctrl+Shift+H", "eludite.search.replace"),
        KeyBindingSpec {
            keystrokes: "f8",
            display: "F8",
            command: "eludite.search.results",
            args: json!({ "navigate": "next" }),
        },
        KeyBindingSpec {
            keystrokes: "shift-f8",
            display: "Shift+F8",
            command: "eludite.search.results",
            args: json!({ "navigate": "previous" }),
        },
        // Edit > IntelliSense (brief 0013).
        command("ctrl-space", "Ctrl+Space", "eludite.editor.complete"),
        command(
            "ctrl-shift-space",
            "Ctrl+Shift+Space",
            "eludite.editor.signature_help",
        ),
        command("ctrl-k ctrl-i", "Ctrl+K, Ctrl+I", "eludite.editor.hover"),
        // Edit.FormatDocument (brief 0050).
        command(
            "ctrl-k ctrl-d",
            "Ctrl+K, Ctrl+D",
            "eludite.editor.format_document",
        ),
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
        // Debug.BreakAll (brief 0025): Ctrl+Alt+Break, which a Linux keyboard sends as Pause.
        command("ctrl-alt-pause", "Ctrl+Alt+Break", "eludite.debug.pause"),
        // Debug.SetNextStatement and Debug.Breakpoints (brief 0026).
        command(
            "ctrl-shift-f10",
            "Ctrl+Shift+F10",
            "eludite.debug.set_next_statement",
        ),
        show("ctrl-alt-b", "Ctrl+Alt+B", "breakpoints"),
        // Debug.Restart and Debug.AttachToProcess (brief 0027): Ctrl+Shift+F5 and Ctrl+Alt+P (the dialog).
        command("ctrl-shift-f5", "Ctrl+Shift+F5", "eludite.debug.restart"),
        command("ctrl-alt-p", "Ctrl+Alt+P", "eludite.debug.attach"),
        // Test (brief 0035): TestExplorer.ShowTestExplorer (Ctrl+E, T), TestExplorer.RunAllTests (Ctrl+R, A),
        // TestExplorer.DebugAllTests (Ctrl+R, Ctrl+A), TestExplorer.RunAllTestsInContext (Ctrl+R, T),
        // TestExplorer.DebugAllTestsInContext (Ctrl+R, Ctrl+T) and TestExplorer.RepeatLastRun (Ctrl+R, L).
        command("ctrl-e t", "Ctrl+E, T", "eludite.test.explorer"),
        command("ctrl-r a", "Ctrl+R, A", "eludite.test.run"),
        command("ctrl-r ctrl-a", "Ctrl+R, Ctrl+A", "eludite.test.debug"),
        KeyBindingSpec {
            keystrokes: "ctrl-r t",
            display: "Ctrl+R, T",
            command: "eludite.test.run",
            args: json!({ "selection": true }),
        },
        KeyBindingSpec {
            keystrokes: "ctrl-r ctrl-t",
            display: "Ctrl+R, Ctrl+T",
            command: "eludite.test.debug",
            args: json!({ "selection": true }),
        },
        KeyBindingSpec {
            keystrokes: "ctrl-r l",
            display: "Ctrl+R, L",
            command: "eludite.test.run",
            args: json!({ "repeat_last": true }),
        },
    ]
}

/// The editor actions that are commands (briefs 0012 and 0013): the shell binds these keys in the editor's own key
/// context too, so they reach the command bus instead of the editor's built-in actions.
pub const EDITOR_COMMAND_KEYS: [&str; 8] = [
    "ctrl-z",
    "ctrl-y",
    "ctrl-shift-z",
    "ctrl-f",
    "ctrl-space",
    "ctrl-shift-space",
    "ctrl-k ctrl-i",
    "ctrl-k ctrl-d",
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
    fn find_in_files_keys() {
        let k = vs_keymap();
        assert_eq!(
            shortcut_for(&k, "eludite.search.find", &json!({})),
            Some("Ctrl+Shift+F")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.search.replace", &json!({})),
            Some("Ctrl+Shift+H")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.search.results", &json!({"navigate": "next"})),
            Some("F8")
        );
        assert_eq!(
            shortcut_for(
                &k,
                "eludite.search.results",
                &json!({"navigate": "previous"})
            ),
            Some("Shift+F8")
        );
    }

    #[test]
    fn test_explorer_keys() {
        let k = vs_keymap();
        assert_eq!(
            shortcut_for(&k, "eludite.test.explorer", &json!({})),
            Some("Ctrl+E, T")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.test.run", &json!({})),
            Some("Ctrl+R, A")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.test.debug", &json!({})),
            Some("Ctrl+R, Ctrl+A")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.test.run", &json!({"selection": true})),
            Some("Ctrl+R, T")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.test.debug", &json!({"selection": true})),
            Some("Ctrl+R, Ctrl+T")
        );
        assert_eq!(
            shortcut_for(&k, "eludite.test.run", &json!({"repeat_last": true})),
            Some("Ctrl+R, L")
        );
    }

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
            ("eludite.debug.pause", "Ctrl+Alt+Break"),
            ("eludite.debug.set_next_statement", "Ctrl+Shift+F10"),
            ("eludite.debug.restart", "Ctrl+Shift+F5"),
            ("eludite.debug.attach", "Ctrl+Alt+P"),
        ] {
            assert_eq!(shortcut_for(&k, cmd, &json!({})), Some(key), "{cmd}");
        }
        assert_eq!(find("terminal"), Some("Ctrl+`"));
        assert_eq!(
            shortcut_for(&k, "eludite.terminal.open", &json!({})),
            Some("Ctrl+Shift+`")
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
