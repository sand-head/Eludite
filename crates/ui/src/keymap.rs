//! The keymap: a table from keystrokes to command-bus commands.
//!
//! Every binding produces one GPUI action, [`RunCommand`], carrying the command
//! id and its JSON input. The shell handles `RunCommand` by invoking the
//! command bus, so keys and menus reach the same commands agents do (PLAN.md
//! 5.1). Visual Studio's table is [`vs_keymap`] (brief 0008); brief 0078 adds the Rider (IntelliJ) and VS Code
//! presets as tables of bindings that replace Visual Studio's ([`preset_keymap`]), the person's `keybindings.json`
//! ([`parse_user_keybindings`]) on top of a preset ([`active_keymap`]), and their registration with GPUI
//! ([`bind_active_keymap`]).

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
        // File.OpenFolder (brief 0019); Ctrl+Shift+O opens a workspace too, since a workspace is a folder (PLAN.md 14,
        // decision 12).
        command(
            "ctrl-shift-o",
            "Ctrl+Shift+O",
            "eludite.workspace.open_folder",
        ),
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
        // Brief 0078: View.CommandPalette (Ctrl+Shift+P), Window.QuickLaunch / Feature Search (Ctrl+Q),
        // View.FullScreen (Shift+Alt+Enter) and Window.ApplyWindowLayout1 to 9 (Ctrl+Alt+1 to 9).
        command("ctrl-shift-p", "Ctrl+Shift+P", PALETTE),
        KeyBindingSpec {
            keystrokes: "ctrl-q",
            display: "Ctrl+Q",
            command: PALETTE,
            args: json!({ "mode": "features" }),
        },
        command("shift-alt-enter", "Shift+Alt+Enter", FULL_SCREEN),
        layout_key("ctrl-alt-1", "Ctrl+Alt+1", 1),
        layout_key("ctrl-alt-2", "Ctrl+Alt+2", 2),
        layout_key("ctrl-alt-3", "Ctrl+Alt+3", 3),
        layout_key("ctrl-alt-4", "Ctrl+Alt+4", 4),
        layout_key("ctrl-alt-5", "Ctrl+Alt+5", 5),
        layout_key("ctrl-alt-6", "Ctrl+Alt+6", 6),
        layout_key("ctrl-alt-7", "Ctrl+Alt+7", 7),
        layout_key("ctrl-alt-8", "Ctrl+Alt+8", 8),
        layout_key("ctrl-alt-9", "Ctrl+Alt+9", 9),
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

// ---- Brief 0078: presets, user bindings and the active keymap. ----------------------------------------------------

/// The command palette's command (`{"mode": "features"}` is Feature Search, Ctrl+Q).
pub const PALETTE: &str = "eludite.view.command_palette";
/// View > Full Screen.
pub const FULL_SCREEN: &str = "eludite.view.full_screen";
/// Window > Apply Window Layout (`{"slot": n}`).
pub const APPLY_LAYOUT: &str = "eludite.view.apply_window_layout";

fn layout_key(keystrokes: &'static str, display: &'static str, slot: u32) -> KeyBindingSpec {
    KeyBindingSpec {
        keystrokes,
        display,
        command: APPLY_LAYOUT,
        args: json!({ "slot": slot }),
    }
}

/// The keyboard mapping scheme (the setting `keyboard.preset`, Tools > Options > Environment > Keyboard).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum KeymapPreset {
    /// Visual Studio's default scheme: [`vs_keymap`].
    #[default]
    VisualStudio,
    /// Rider's IntelliJ scheme (Rider's own default on Windows is its Visual Studio scheme, which is ours).
    Rider,
    /// Visual Studio Code's default bindings.
    VsCode,
}

impl KeymapPreset {
    pub const ALL: [KeymapPreset; 3] = [Self::VisualStudio, Self::Rider, Self::VsCode];

    /// The setting's value (`visualStudio`, `rider`, `vsCode`); an unknown value is Visual Studio's.
    pub fn from_setting(value: &str) -> Self {
        match value {
            "rider" => Self::Rider,
            "vsCode" => Self::VsCode,
            _ => Self::VisualStudio,
        }
    }

    pub fn setting(self) -> &'static str {
        match self {
            Self::VisualStudio => "visualStudio",
            Self::Rider => "rider",
            Self::VsCode => "vsCode",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::VisualStudio => "Visual Studio",
            Self::Rider => "Rider (IntelliJ)",
            Self::VsCode => "Visual Studio Code",
        }
    }
}

/// A preset's own bindings: each takes its command (with those arguments) from Visual Studio's table, and any Visual
/// Studio binding on the same keys.
fn preset_overrides(preset: KeymapPreset) -> Vec<KeyBindingSpec> {
    let show = |keystrokes, display, id: &str| KeyBindingSpec {
        keystrokes,
        display,
        command: SHOW,
        args: json!({ "id": id }),
    };
    let with = |keystrokes, display, cmd, args: Value| KeyBindingSpec {
        keystrokes,
        display,
        command: cmd,
        args,
    };
    match preset {
        KeymapPreset::VisualStudio => Vec::new(),
        // Rider's IntelliJ scheme on Windows and Linux.
        KeymapPreset::Rider => vec![
            show("alt-1", "Alt+1", "workspace"),
            show("alt-6", "Alt+6", "error_list"),
            show("alt-f12", "Alt+F12", "terminal"),
            show("alt-0", "Alt+0", "git_changes"),
            show("alt-9", "Alt+9", "git_repository"),
            show("alt-8", "Alt+8", "tests"),
            command("ctrl-shift-a", "Ctrl+Shift+A", PALETTE),
            with(
                "shift shift",
                "Shift, Shift",
                PALETTE,
                json!({ "mode": "features" }),
            ),
            command("ctrl-b", "Ctrl+B", "eludite.editor.go_to_definition"),
            command("alt-f7", "Alt+F7", "eludite.editor.find_references"),
            command("shift-f6", "Shift+F6", "eludite.editor.rename"),
            command("ctrl-alt-l", "Ctrl+Alt+L", "eludite.editor.format_document"),
            command("ctrl-p", "Ctrl+P", "eludite.editor.signature_help"),
            command("ctrl-q", "Ctrl+Q", "eludite.editor.hover"),
            command("ctrl-shift-z", "Ctrl+Shift+Z", "eludite.editor.redo"),
            command("ctrl-alt-left", "Ctrl+Alt+Left", "eludite.navigation.back"),
            command(
                "ctrl-alt-right",
                "Ctrl+Alt+Right",
                "eludite.navigation.forward",
            ),
            command("ctrl-shift-r", "Ctrl+Shift+R", "eludite.search.replace"),
            command("ctrl-f9", "Ctrl+F9", "eludite.build.solution"),
            command("shift-f9", "Shift+F9", "eludite.debug.start"),
            with(
                "shift-f10",
                "Shift+F10",
                "eludite.debug.start",
                json!({ "debug": false }),
            ),
            command("ctrl-f2", "Ctrl+F2", "eludite.debug.stop"),
            command("ctrl-f8", "Ctrl+F8", "eludite.debug.toggle_breakpoint"),
            command("f8", "F8", "eludite.debug.step_over"),
            command("f7", "F7", "eludite.debug.step_into"),
            command("shift-f8", "Shift+F8", "eludite.debug.step_out"),
            command("alt-f9", "Alt+F9", "eludite.debug.run_to_cursor"),
            command("f9", "F9", "eludite.debug.continue"),
            command("ctrl-alt-s", "Ctrl+Alt+S", "eludite.tools.options"),
            command("ctrl-k", "Ctrl+K", "eludite.git.commit"),
        ],
        // Visual Studio Code's defaults on Windows and Linux.
        KeymapPreset::VsCode => vec![
            command("f1", "F1", PALETTE),
            command("ctrl-shift-p", "Ctrl+Shift+P", PALETTE),
            command("ctrl-p", "Ctrl+P", "eludite.edit.go_to_all"),
            show("ctrl-shift-e", "Ctrl+Shift+E", "workspace"),
            show("ctrl-shift-m", "Ctrl+Shift+M", "error_list"),
            show("ctrl-shift-u", "Ctrl+Shift+U", "output"),
            show("ctrl-shift-g", "Ctrl+Shift+G", "git_changes"),
            command(
                "shift-alt-f",
                "Shift+Alt+F",
                "eludite.editor.format_document",
            ),
            command("ctrl-shift-z", "Ctrl+Shift+Z", "eludite.editor.redo"),
            command("alt-left", "Alt+Left", "eludite.navigation.back"),
            command("alt-right", "Alt+Right", "eludite.navigation.forward"),
            command(
                "ctrl-k ctrl-s",
                "Ctrl+K, Ctrl+S",
                "eludite.help.keyboard_shortcuts",
            ),
            command("ctrl-,", "Ctrl+,", "eludite.tools.options"),
            command("f11", "F11", FULL_SCREEN),
            command(
                "ctrl-shift-f11",
                "Ctrl+Shift+F11",
                "eludite.debug.step_into",
            ),
        ],
    }
}

/// The table of a preset: Visual Studio's with the preset's own bindings in place of Visual Studio's for the same
/// commands (with the same arguments) or the same keys.
pub fn preset_keymap(preset: KeymapPreset) -> Vec<KeyBindingSpec> {
    let overrides = preset_overrides(preset);
    let mut out: Vec<KeyBindingSpec> = vs_keymap()
        .into_iter()
        .filter(|vs| {
            !overrides.iter().any(|o| {
                o.keystrokes == vs.keystrokes
                    || (o.command == vs.command
                        && o.args == vs.args
                        // A preset may keep a second key for the same command (VS Code's Ctrl+Shift+P and F1).
                        && !overrides.iter().any(|p| p.keystrokes == vs.keystrokes))
            })
        })
        .collect();
    out.extend(overrides);
    out
}

/// Where a binding applies: everywhere in the main window, or only while a text editor has the focus.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum BindingScope {
    #[default]
    Global,
    TextEditor,
}

impl BindingScope {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Global => "global",
            Self::TextEditor => "editor",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "global" | "Global" => Some(Self::Global),
            "editor" | "Text Editor" | "textEditor" => Some(Self::TextEditor),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Global => "Global",
            Self::TextEditor => "Text Editor",
        }
    }
}

/// Where a binding of the active keymap comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BindingSource {
    Preset,
    User,
}

/// One binding of the active keymap.
#[derive(Debug, Clone, PartialEq)]
pub struct ActiveBinding {
    pub spec: KeyBindingSpec,
    pub scope: BindingScope,
    pub source: BindingSource,
}

/// One entry of the person's `keybindings.json`: `command` `None` (or `-id`) removes the bindings of `key` (of that
/// command, when it names one).
#[derive(Debug, Clone, PartialEq)]
pub struct UserBinding {
    /// GPUI keystrokes (`ctrl-k ctrl-c`); the file may say `Ctrl+K, Ctrl+C`.
    pub key: String,
    pub command: Option<String>,
    /// For a removal: only this command's binding of `key`.
    pub removes: Option<String>,
    pub args: Value,
    pub scope: BindingScope,
}

/// The person's bindings file, `keybindings.json` in Eludite's config directory.
pub const USER_KEYBINDINGS_FILE: &str = "keybindings.json";

/// Leak `s` once: bindings carry `&'static str` (the table's shape); a reload of the same file leaks nothing new.
pub fn intern(s: &str) -> &'static str {
    use std::collections::HashSet;
    use std::sync::{Mutex, OnceLock};
    static POOL: OnceLock<Mutex<HashSet<&'static str>>> = OnceLock::new();
    let mut pool = POOL
        .get_or_init(Default::default)
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    if let Some(found) = pool.get(s) {
        return found;
    }
    let leaked: &'static str = Box::leak(s.to_owned().into_boxed_str());
    pool.insert(leaked);
    leaked
}

/// `Ctrl+K, Ctrl+C` or `ctrl-k ctrl-c` as GPUI keystrokes (`ctrl-k ctrl-c`), checked; `None` when a chord does not
/// parse.
pub fn normalize_keystrokes(text: &str) -> Option<String> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    let chords: Vec<String> = if text.contains('+') || text.contains(',') {
        text.split(',')
            .map(|chord| {
                let parts: Vec<&str> = chord.trim().split('+').map(str::trim).collect();
                // `Ctrl++` (the plus key) leaves an empty last part.
                let mut keys: Vec<String> = Vec::new();
                for (i, p) in parts.iter().enumerate() {
                    if p.is_empty() {
                        if i == parts.len() - 1 {
                            keys.push("+".into());
                        }
                        continue;
                    }
                    keys.push(match p.to_ascii_lowercase().as_str() {
                        "ctrl" | "control" => "ctrl".into(),
                        "shift" => "shift".into(),
                        "alt" | "option" => "alt".into(),
                        "cmd" | "win" | "super" | "meta" => "cmd".into(),
                        "esc" => "escape".into(),
                        "del" => "delete".into(),
                        "ins" => "insert".into(),
                        "break" => "pause".into(),
                        "pgup" | "page up" => "pageup".into(),
                        "pgdn" | "page down" => "pagedown".into(),
                        other => other.into(),
                    });
                }
                keys.join("-")
            })
            .collect()
    } else {
        text.split_whitespace().map(str::to_owned).collect()
    };
    for chord in &chords {
        let ks = gpui::Keystroke::parse(chord).ok()?;
        if !known_key(&ks.key) {
            return None;
        }
    }
    Some(chords.join(" "))
}

/// A key GPUI names: one character, a function key, or one of the named keys.
fn known_key(key: &str) -> bool {
    const NAMED: [&str; 26] = [
        "enter",
        "escape",
        "tab",
        "space",
        "backspace",
        "delete",
        "insert",
        "home",
        "end",
        "pageup",
        "pagedown",
        "up",
        "down",
        "left",
        "right",
        "pause",
        "shift",
        "alt",
        "ctrl",
        "cmd",
        "fn",
        "menu",
        "printscreen",
        "scrolllock",
        "capslock",
        "numlock",
    ];
    key.chars().count() == 1
        || NAMED.contains(&key)
        || key
            .strip_prefix('f')
            .and_then(|n| n.parse::<u8>().ok())
            .is_some_and(|n| (1..=35).contains(&n))
}

/// Visual Studio's way to show GPUI keystrokes: `ctrl-k ctrl-c` is `Ctrl+K, Ctrl+C`.
pub fn display_keystrokes(keystrokes: &str) -> String {
    keystrokes
        .split_whitespace()
        .map(|chord| {
            let mut parts: Vec<String> = Vec::new();
            // A trailing `-` key (`ctrl--`) is the minus key.
            let (mods, key) = match chord.rsplit_once('-') {
                Some((m, "")) => (m.trim_end_matches('-'), "-"),
                Some((m, k)) => (m, k),
                None => ("", chord),
            };
            for m in mods.split('-').filter(|m| !m.is_empty()) {
                parts.push(
                    match m {
                        "ctrl" => "Ctrl",
                        "shift" => "Shift",
                        "alt" => "Alt",
                        "cmd" | "super" | "win" => "Win",
                        "fn" => "Fn",
                        other => other,
                    }
                    .to_owned(),
                );
            }
            let key = match key {
                "escape" => "Esc".to_owned(),
                "pause" => "Break".to_owned(),
                "pageup" => "PgUp".to_owned(),
                "pagedown" => "PgDn".to_owned(),
                "shift" | "alt" | "ctrl" => {
                    let mut c = key.chars();
                    c.next()
                        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                        .unwrap_or_default()
                }
                k if k.len() == 1 => k.to_ascii_uppercase(),
                k => {
                    let mut c = k.chars();
                    c.next()
                        .map(|f| f.to_ascii_uppercase().to_string() + c.as_str())
                        .unwrap_or_default()
                }
            };
            parts.push(key);
            parts.join("+")
        })
        .collect::<Vec<_>>()
        .join(", ")
}

/// Parse `keybindings.json`: an array of `{"key", "command", "args"?, "when"?}` (`when`: `global`, the default, or
/// `editor`). Bad entries are reported by index and skipped; the others still apply.
pub fn parse_user_keybindings(text: &str) -> (Vec<UserBinding>, Vec<String>) {
    let mut errors = Vec::new();
    let value: Value = match serde_json::from_str(text) {
        Ok(v) => v,
        Err(e) => return (Vec::new(), vec![format!("not JSON: {e}")]),
    };
    let Some(items) = value.as_array() else {
        return (
            Vec::new(),
            vec!["the file must be a JSON array of bindings".into()],
        );
    };
    let mut out = Vec::new();
    for (i, item) in items.iter().enumerate() {
        let Some(raw) = item.get("key").and_then(Value::as_str) else {
            errors.push(format!("entry {i}: `key` must be a string"));
            continue;
        };
        let Some(key) = normalize_keystrokes(raw) else {
            errors.push(format!("entry {i}: `{raw}` is not a keystroke"));
            continue;
        };
        let scope = match item.get("when").and_then(Value::as_str) {
            None => BindingScope::Global,
            Some(w) => match BindingScope::parse(w) {
                Some(s) => s,
                None => {
                    errors.push(format!(
                        "entry {i}: `when` must be `global` or `editor`, not `{w}`"
                    ));
                    continue;
                }
            },
        };
        let args = item.get("args").cloned().unwrap_or_else(|| json!({}));
        if !args.is_object() {
            errors.push(format!("entry {i}: `args` must be an object"));
            continue;
        }
        let (command, removes) = match item.get("command") {
            None | Some(Value::Null) => (None, None),
            Some(Value::String(c)) if c.starts_with('-') => (None, Some(c[1..].to_owned())),
            Some(Value::String(c)) if !c.is_empty() => (Some(c.clone()), None),
            Some(_) => {
                errors.push(format!(
                    "entry {i}: `command` must be a command id, `-id` or null"
                ));
                continue;
            }
        };
        out.push(UserBinding {
            key,
            command,
            removes,
            args,
            scope,
        });
    }
    (out, errors)
}

/// The text of a `keybindings.json` holding `bindings`.
pub fn write_user_keybindings(bindings: &[UserBinding]) -> String {
    let items: Vec<Value> = bindings
        .iter()
        .map(|b| {
            let mut o = serde_json::Map::new();
            o.insert("key".into(), json!(b.key));
            o.insert(
                "command".into(),
                match (&b.command, &b.removes) {
                    (Some(c), _) => json!(c),
                    (None, Some(r)) => json!(format!("-{r}")),
                    (None, None) => Value::Null,
                },
            );
            if b.args.as_object().is_some_and(|a| !a.is_empty()) {
                o.insert("args".into(), b.args.clone());
            }
            if b.scope != BindingScope::Global {
                o.insert("when".into(), json!(b.scope.as_str()));
            }
            Value::Object(o)
        })
        .collect();
    let mut text = serde_json::to_string_pretty(&Value::Array(items)).unwrap_or_default();
    text.push('\n');
    text
}

/// The keymap in effect: the preset's table with the person's bindings applied in file order (a binding takes its
/// keys from whatever had them in its scope; a removal removes). The person's bindings come first.
pub fn active_keymap(preset: KeymapPreset, user: &[UserBinding]) -> Vec<ActiveBinding> {
    let mut out: Vec<ActiveBinding> = preset_keymap(preset)
        .into_iter()
        .map(|spec| ActiveBinding {
            spec,
            scope: BindingScope::Global,
            source: BindingSource::Preset,
        })
        .collect();
    for u in user {
        let key = intern(&u.key);
        match &u.command {
            Some(command) => {
                out.retain(|b| !(b.spec.keystrokes == key && b.scope == u.scope));
                out.push(ActiveBinding {
                    spec: KeyBindingSpec {
                        keystrokes: key,
                        display: intern(&display_keystrokes(key)),
                        command: intern(command),
                        args: u.args.clone(),
                    },
                    scope: u.scope,
                    source: BindingSource::User,
                });
            }
            None => out.retain(|b| {
                !(b.spec.keystrokes == key
                    && u.removes.as_deref().is_none_or(|c| c == b.spec.command))
            }),
        }
    }
    // The person's bindings first, so menus and the palette show the shortcut they chose.
    out.sort_by_key(|b| b.source != BindingSource::User);
    out
}

/// The bindings of the active keymap as the table's rows (the menu bar's and the palette's shortcut column).
pub fn specs_of(active: &[ActiveBinding]) -> Vec<KeyBindingSpec> {
    active.iter().map(|b| b.spec.clone()).collect()
}

/// Other bindings `keys` would take over in `scope`: the same keys, or a chord that starts with them or that they
/// start with (Visual Studio's "Shortcut currently used by").
pub fn conflicts<'a>(
    active: &'a [ActiveBinding],
    keys: &str,
    scope: BindingScope,
) -> Vec<&'a ActiveBinding> {
    let keys = keys.trim();
    active
        .iter()
        .filter(|b| {
            let k = b.spec.keystrokes;
            (b.scope == scope || b.scope == BindingScope::Global || scope == BindingScope::Global)
                && (k == keys
                    || k.starts_with(&format!("{keys} "))
                    || keys.starts_with(&format!("{k} ")))
        })
        .collect()
}

/// Register the active keymap with GPUI: every binding in [`SHELL_CONTEXT`]; the person's and a preset's own
/// bindings also in `editor_context`, so they win over the text editor's built-in keys there; Text Editor bindings
/// only there.
pub fn bind_active_keymap(cx: &mut App, active: &[ActiveBinding], editor_context: &str) {
    let vs: Vec<&'static str> = vs_keymap().iter().map(|k| k.keystrokes).collect();
    let mut bindings = Vec::new();
    for b in active {
        let action = || RunCommand::new(b.spec.command, b.spec.args.clone());
        if b.scope == BindingScope::Global {
            bindings.push(KeyBinding::new(
                b.spec.keystrokes,
                action(),
                Some(SHELL_CONTEXT),
            ));
        }
        if b.scope == BindingScope::TextEditor
            || b.source == BindingSource::User
            || !vs.contains(&b.spec.keystrokes)
        {
            bindings.push(KeyBinding::new(
                b.spec.keystrokes,
                action(),
                Some(editor_context),
            ));
        }
    }
    cx.bind_keys(bindings);
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
            shortcut_for(&k, "eludite.workspace.open_folder", &json!({})),
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

    /// Brief 0078: the presets parse, have no duplicate keys, and replace Visual Studio's bindings.
    #[test]
    fn presets_replace_visual_studio_bindings() {
        for preset in KeymapPreset::ALL {
            let k = preset_keymap(preset);
            for b in &k {
                for ks in b.keystrokes.split_whitespace() {
                    assert!(gpui::Keystroke::parse(ks).is_ok(), "{preset:?} {ks}");
                }
            }
            let mut keys: Vec<_> = k.iter().map(|b| b.keystrokes).collect();
            keys.sort_unstable();
            keys.dedup();
            assert_eq!(keys.len(), k.len(), "{preset:?}: no duplicate keystrokes");
            assert_eq!(KeymapPreset::from_setting(preset.setting()), preset);
        }
        let vs = preset_keymap(KeymapPreset::VisualStudio);
        assert_eq!(vs, vs_keymap());
        let rider = preset_keymap(KeymapPreset::Rider);
        assert_eq!(
            shortcut_for(&rider, "eludite.editor.go_to_definition", &json!({})),
            Some("Ctrl+B")
        );
        // F12 is no longer Go To Definition, Shift+F6 is Rename (not Build Selection), F8 is Step Over.
        assert!(!rider.iter().any(|b| b.keystrokes == "f12"));
        assert_eq!(
            shortcut_for(&rider, "eludite.editor.rename", &json!({})),
            Some("Shift+F6")
        );
        assert!(
            rider
                .iter()
                .any(|b| b.keystrokes == "f8" && b.command == "eludite.debug.step_over")
        );
        assert_eq!(
            shortcut_for(&rider, "eludite.build.project", &json!({})),
            None
        );
        // Bindings Rider does not name stay Visual Studio's.
        assert_eq!(
            shortcut_for(&rider, "eludite.editor.save", &json!({})),
            Some("Ctrl+S")
        );
        let code = preset_keymap(KeymapPreset::VsCode);
        assert_eq!(
            shortcut_for(&code, "eludite.editor.format_document", &json!({})),
            Some("Shift+Alt+F")
        );
        // Two keys for the palette: Ctrl+Shift+P and F1.
        assert!(
            code.iter()
                .any(|b| b.keystrokes == "f1" && b.command == PALETTE)
        );
        assert!(
            code.iter()
                .any(|b| b.keystrokes == "ctrl-shift-p" && b.command == PALETTE)
        );
        assert_eq!(
            KeymapPreset::from_setting("emacs"),
            KeymapPreset::VisualStudio
        );
    }

    #[test]
    fn the_palette_and_full_screen_keys() {
        let k = vs_keymap();
        assert_eq!(shortcut_for(&k, PALETTE, &json!({})), Some("Ctrl+Shift+P"));
        assert_eq!(
            shortcut_for(&k, PALETTE, &json!({"mode": "features"})),
            Some("Ctrl+Q")
        );
        assert_eq!(
            shortcut_for(&k, FULL_SCREEN, &json!({})),
            Some("Shift+Alt+Enter")
        );
        assert_eq!(
            shortcut_for(&k, APPLY_LAYOUT, &json!({"slot": 3})),
            Some("Ctrl+Alt+3")
        );
    }

    #[test]
    fn keystrokes_convert_both_ways() {
        assert_eq!(
            normalize_keystrokes("Ctrl+K, Ctrl+C").as_deref(),
            Some("ctrl-k ctrl-c")
        );
        assert_eq!(
            normalize_keystrokes("ctrl-shift-p").as_deref(),
            Some("ctrl-shift-p")
        );
        assert_eq!(
            normalize_keystrokes("Shift+Alt+Enter").as_deref(),
            Some("shift-alt-enter")
        );
        assert_eq!(normalize_keystrokes(""), None);
        assert_eq!(display_keystrokes("ctrl-k ctrl-c"), "Ctrl+K, Ctrl+C");
        assert_eq!(display_keystrokes("ctrl--"), "Ctrl+-");
        assert_eq!(display_keystrokes("shift-f12"), "Shift+F12");
        assert_eq!(display_keystrokes("ctrl-alt-pause"), "Ctrl+Alt+Break");
        // The display of every Visual Studio binding is what the table says, so the generated reference matches it.
        for b in vs_keymap() {
            if b.keystrokes.contains('`')
                || b.keystrokes.contains('~')
                || b.keystrokes.contains('_')
            {
                continue;
            }
            assert_eq!(
                display_keystrokes(b.keystrokes),
                b.display,
                "{}",
                b.keystrokes
            );
        }
    }

    /// The person's bindings override the preset, remove, and report bad entries.
    #[test]
    fn user_bindings_override_and_remove() {
        let (user, errors) = parse_user_keybindings(
            r#"[
                {"key": "Ctrl+K, Ctrl+O", "command": "eludite.view.show", "args": {"id": "output"}},
                {"key": "f5", "command": "eludite.build.solution"},
                {"key": "ctrl-s", "command": null},
                {"key": "ctrl-f", "command": "-eludite.editor.find"},
                {"key": "ctrl-e e", "command": "eludite.test.run", "when": "editor"},
                {"key": "not a key!!", "command": "x.y"},
                {"key": "ctrl-j", "command": "x.y", "when": "terminal"}
            ]"#,
        );
        assert_eq!(errors.len(), 2, "{errors:?}");
        let active = active_keymap(KeymapPreset::VisualStudio, &user);
        let specs = specs_of(&active);
        assert_eq!(
            shortcut_for(&specs, SHOW, &json!({"id": "output"})),
            Some("Ctrl+K, Ctrl+O")
        );
        // F5 builds now; Start Debugging lost F5.
        assert_eq!(
            shortcut_for(&specs, "eludite.debug.start", &json!({})),
            None
        );
        assert!(
            specs
                .iter()
                .any(|b| b.keystrokes == "f5" && b.command == "eludite.build.solution")
        );
        assert_eq!(
            shortcut_for(&specs, "eludite.editor.save", &json!({})),
            None
        );
        assert_eq!(
            shortcut_for(&specs, "eludite.editor.find", &json!({})),
            None
        );
        let editor = active
            .iter()
            .find(|b| b.spec.keystrokes == "ctrl-e e")
            .unwrap();
        assert_eq!(editor.scope, BindingScope::TextEditor);
        assert_eq!(editor.source, BindingSource::User);
        // Round trip.
        let text = write_user_keybindings(&user);
        assert_eq!(parse_user_keybindings(&text), (user, Vec::new()));
        assert!(!parse_user_keybindings("{").1.is_empty());
        assert!(!parse_user_keybindings("{}").1.is_empty());
    }

    #[test]
    fn conflicts_list_same_keys_and_chord_prefixes() {
        let active = active_keymap(KeymapPreset::VisualStudio, &[]);
        let c = conflicts(&active, "ctrl-r", BindingScope::Global);
        // Ctrl+R starts Rename's and the tests' chords.
        assert!(c.iter().any(|b| b.spec.command == "eludite.editor.rename"));
        assert!(c.iter().any(|b| b.spec.command == "eludite.test.run"));
        let c = conflicts(&active, "ctrl-s", BindingScope::TextEditor);
        assert_eq!(c.len(), 1);
        assert_eq!(c[0].spec.command, "eludite.editor.save");
        assert!(conflicts(&active, "ctrl-alt-shift-f12", BindingScope::Global).is_empty());
    }
}
