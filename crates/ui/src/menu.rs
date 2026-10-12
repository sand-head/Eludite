//! The menu bar: Visual Studio's menus, each item bound to a command id.
//!
//! Items whose command is registered on the bus are enabled and dispatch
//! [`RunCommand`] (the same action key bindings produce). Items for commands
//! that do not exist yet are drawn disabled and do nothing.
//!
//! Brief 0078: the keyboard and submenus. A label with ` > ` is an item of a cascading submenu ([`menu_rows`]), so
//! the data stays one flat list per menu. Access keys follow Windows' rule ([`access_keys`]) and are underlined while
//! Alt is held or the bar has the keyboard; [`MenuBar::key`] takes the window's keystrokes before the key bindings
//! (Alt tap, Alt+letter, the arrows, Home, End, Enter, Space, Escape, the items' access keys).

use std::rc::Rc;

use gpui::{
    AnyElement, Context, FontWeight, HighlightStyle, InteractiveElement, IntoElement, Keystroke,
    MouseButton, ParentElement, Render, StatefulInteractiveElement, Styled, StyledText,
    UnderlineStyle, Window, anchored, deferred, div, point, px,
};
use serde_json::{Value, json};

use crate::keymap::{KeyBindingSpec, RunCommand, shortcut_for};
use crate::theme::Theme;

#[derive(Debug, Clone, PartialEq)]
pub enum MenuEntry {
    Item {
        label: &'static str,
        command: &'static str,
        args: Value,
    },
    /// A check item: its command takes `{"enabled": bool}`; [`MenuBar::set_checked`] says whether it is on.
    Check {
        label: &'static str,
        command: &'static str,
    },
    /// A check item bound to a boolean setting (brief 0037): a click runs `eludite.settings.set` with the key and the
    /// other value; [`MenuBar::set_checked`] is asked with the key.
    SettingCheck {
        label: &'static str,
        key: &'static str,
    },
    Separator,
}

impl MenuEntry {
    /// The label, command and arguments a click dispatches (a check item's from its state); `None` for a separator.
    fn action(&self, checked: bool) -> Option<(&'static str, &'static str, Value)> {
        match self {
            MenuEntry::Item {
                label,
                command,
                args,
            } => Some((label, command, args.clone())),
            MenuEntry::Check { label, command } => {
                Some((label, command, json!({ "enabled": !checked })))
            }
            MenuEntry::SettingCheck { label, key } => Some((
                label,
                SETTINGS_SET,
                json!({ "key": key, "value": !checked }),
            )),
            MenuEntry::Separator => None,
        }
    }

    /// What [`MenuBar::set_checked`] is asked about a check item: its command, or its setting's key.
    fn check_key(&self) -> Option<&'static str> {
        match self {
            MenuEntry::Check { command, .. } => Some(command),
            MenuEntry::SettingCheck { key, .. } => Some(key),
            _ => None,
        }
    }
}

/// The command a setting's check item runs.
pub const SETTINGS_SET: &str = "eludite.settings.set";

#[derive(Debug, Clone, PartialEq)]
pub struct Menu {
    pub title: &'static str,
    pub entries: Vec<MenuEntry>,
}

fn item(label: &'static str, command: &'static str) -> MenuEntry {
    MenuEntry::Item {
        label,
        command,
        args: json!({}),
    }
}

fn show(label: &'static str, id: &str) -> MenuEntry {
    MenuEntry::Item {
        label,
        command: "eludite.view.show",
        args: json!({ "id": id }),
    }
}

/// Tools > Theme's item for a theme (brief 0078).
fn theme_item(label: &'static str, name: &str) -> MenuEntry {
    MenuEntry::Item {
        label,
        command: "eludite.theme.set",
        args: json!({ "name": name }),
    }
}

/// The Workspace window's context menu items on a file in a Git repository (brief 0040), in Visual Studio's order:
/// (selector suffix, label, command). Each runs its command with the file (`path`, or `paths` for the index
/// commands).
pub const WORKSPACE_GIT_ITEMS: [(&str, &str, &str); 5] = [
    ("compare", "Compare with Unmodified...", "eludite.git.diff"),
    ("undo", "Undo Changes...", "eludite.git.discard"),
    ("stage", "Stage", "eludite.git.stage"),
    ("unstage", "Unstage", "eludite.git.unstage"),
    ("blame", "Blame (Annotate)", "eludite.git.blame"),
];

/// The Workspace window's context menu item on a project or folder that opens a terminal there (brief 0041):
/// (selector suffix, label, command). It runs the command with the item's path as `cwd` (a file's folder).
pub const WORKSPACE_TERMINAL_ITEM: (&str, &str, &str) =
    ("terminal", "Open in Terminal", "eludite.terminal.open");

/// The menu titles, in order (PLAN.md 8). Visual Studio's names and order, trimmed to the menus a workspace of any
/// language needs: no Project, Analyze, Extensions or Window menu (PLAN.md 14, decision 14).
pub const MENU_TITLES: [&str; 9] = [
    "File", "Edit", "View", "Git", "Build", "Debug", "Test", "Tools", "Help",
];

/// The menus. Command ids that are not registered yet are stubs: the bar shows them disabled until a later brief
/// registers the command. No item names a solution or a project (PLAN.md 14, decision 12): a workspace is a folder,
/// and the .NET solution and projects in it only organize the Workspace window.
pub fn vs_menus() -> Vec<Menu> {
    use MenuEntry::Separator;
    let menu = |title, entries| Menu { title, entries };
    vec![
        menu(
            "File",
            vec![
                // A workspace is any folder: a Cargo workspace, a .NET solution, an npm package, any mix, or none.
                item("Open Workspace...", "eludite.workspace.open_folder"),
                item("Open File...", "eludite.file.open_file_dialog"),
                Separator,
                item("Save", "eludite.editor.save"),
                item("Save All", "eludite.file.save_all"),
                Separator,
                item("Close Workspace", "eludite.workspace.close"),
                item("Exit", "eludite.file.exit"),
            ],
        ),
        menu(
            "Edit",
            vec![
                item("Undo", "eludite.editor.undo"),
                item("Redo", "eludite.editor.redo"),
                Separator,
                item("Cut", "eludite.edit.cut"),
                item("Copy", "eludite.edit.copy"),
                item("Paste", "eludite.edit.paste"),
                Separator,
                // Visual Studio's Edit > Find and Replace: Quick Find (the editor's find bar), Find in Files and
                // Replace in Files (brief 0042).
                item("Find", "eludite.editor.find"),
                item("Find in Files", "eludite.search.find"),
                item("Replace in Files", "eludite.search.replace"),
                Separator,
                item("Go To All", "eludite.edit.go_to_all"),
                item("Go To Definition", "eludite.editor.go_to_definition"),
                item("Find All References", "eludite.editor.find_references"),
                item("Navigate Backward", "eludite.navigation.back"),
                item("Navigate Forward", "eludite.navigation.forward"),
                Separator,
                // Visual Studio's Edit > Refactor > Rename, the Quick Actions and Refactorings, and Edit > Advanced >
                // Format Document (brief 0050).
                item("Rename...", "eludite.editor.rename"),
                item(
                    "Quick Actions and Refactorings...",
                    "eludite.editor.code_actions",
                ),
                item("Format Document", "eludite.editor.format_document"),
                Separator,
                // Visual Studio's Edit > IntelliSense items.
                item("Complete Word", "eludite.editor.complete"),
                item("Parameter Info", "eludite.editor.signature_help"),
                item("Quick Info", "eludite.editor.hover"),
            ],
        ),
        menu(
            "View",
            vec![
                item("Command Palette", "eludite.view.command_palette"),
                // Brief 0078: Visual Studio's Feature Search (Ctrl+Q).
                MenuEntry::Item {
                    label: "Feature Search",
                    command: "eludite.view.command_palette",
                    args: json!({ "mode": "features" }),
                },
                Separator,
                show("Workspace", "workspace"),
                show("Agents", "agents"),
                show("Error List", "error_list"),
                show("Output", "output"),
                // View > Terminal, Ctrl+` (brief 0041).
                show("Terminal", "terminal"),
                // Visual Studio's View > Other Windows: the Web Browser document tab (brief 0032) and Find Results 1
                // and 2 (brief 0042).
                show("Other Windows > Web Browser", "web_browser"),
                show("Other Windows > Find Results 1", "find_results_1"),
                show("Other Windows > Find Results 2", "find_results_2"),
                Separator,
                // Visual Studio's Window menu, folded in: the active tool window's state and the layout.
                item("Float", "eludite.view.float"),
                item("Dock", "eludite.view.dock"),
                item("Auto Hide", "eludite.view.auto_hide"),
                item("Hide", "eludite.view.hide"),
                item("Reset Window Layout", "eludite.view.reset_layout"),
                // Brief 0078: Visual Studio's Window > Save, Apply and Manage Window Layouts, the notifications, Full
                // Screen.
                item(
                    "Window Layouts > Save Window Layout...",
                    "eludite.view.save_window_layout",
                ),
                item(
                    "Window Layouts > Apply Window Layout...",
                    "eludite.view.apply_window_layout",
                ),
                item(
                    "Window Layouts > Manage Window Layouts...",
                    "eludite.view.manage_window_layouts",
                ),
                Separator,
                item("Notifications", "eludite.notifications.show"),
                item("Full Screen", "eludite.view.full_screen"),
            ],
        ),
        // Visual Studio's Git menu (brief 0040): Commit or Stash... shows Git Changes, Manage Branches the Git
        // Repository window, New Branch... (checkout without a name) its New Branch box, Open in File Explorer (without
        // a path) the repository's folder.
        menu(
            "Git",
            vec![
                show("Commit or Stash...", "git_changes"),
                show("Manage Branches", "git_repository"),
                item("New Branch...", "eludite.git.checkout"),
                Separator,
                item("Fetch", "eludite.git.fetch"),
                item("Pull", "eludite.git.pull"),
                item("Push", "eludite.git.push"),
                item("Sync", "eludite.git.sync"),
                Separator,
                // The forge of the repository's remote (brief 0046): its windows, the form, and the sign-in dialog for
                // its host.
                show("Pull Requests", "pull_requests"),
                show("Issues", "issues"),
                item("Create Pull Request", "eludite.forge.pull_create"),
                MenuEntry::Item {
                    label: "Sign in to Forge...",
                    command: "eludite.forge.auth",
                    args: json!({ "action": "sign_in" }),
                },
                Separator,
                item(
                    "Open in File Explorer",
                    "eludite.workspace.open_containing_folder",
                ),
            ],
        ),
        // The workspace's build systems (brief 0019): Ctrl+Shift+B builds the one that owns the active document.
        menu(
            "Build",
            vec![
                item("Build", "eludite.build.solution"),
                item("Rebuild", "eludite.build.rebuild"),
                item("Clean", "eludite.build.clean"),
                Separator,
                item("Cancel", "eludite.build.cancel"),
            ],
        ),
        menu(
            "Debug",
            vec![
                item("Start Debugging", "eludite.debug.start"),
                MenuEntry::Item {
                    label: "Start Without Debugging",
                    command: "eludite.debug.start",
                    args: json!({ "debug": false }),
                },
                // A web page in the system browser instead of the Web Browser window (brief 0037).
                MenuEntry::Item {
                    label: "Start in External Browser",
                    command: "eludite.debug.start",
                    args: json!({ "browser": "external" }),
                },
                // Where F5 opens a web page: the setting browser.useBuiltIn (brief 0037).
                MenuEntry::SettingCheck {
                    label: "Open in Web Browser Window",
                    key: "browser.useBuiltIn",
                },
                item("Stop Debugging", "eludite.debug.stop"),
                // Restart (brief 0027): Ctrl+Shift+F5; not for an attached session.
                item("Restart", "eludite.debug.restart"),
                item("Continue", "eludite.debug.continue"),
                // Break All (brief 0025): only a running debuggee; the command refuses it otherwise.
                item("Break All", "eludite.debug.pause"),
                Separator,
                item("Attach to Process...", "eludite.debug.attach"),
                // A page of the Web Browser window, with vscode-js-debug (brief 0038): the same dialog, its tabs only.
                MenuEntry::Item {
                    label: "Attach to Browser Tab...",
                    command: "eludite.debug.attach",
                    args: json!({ "adapter": "javascript" }),
                },
                // Who may drive the session (brief 0027): the person's switch for agents.
                MenuEntry::Check {
                    label: "Allow Agents to Drive",
                    command: "eludite.debug.allow_agents",
                },
                Separator,
                item("Step Into", "eludite.debug.step_into"),
                item("Step Over", "eludite.debug.step_over"),
                item("Step Out", "eludite.debug.step_out"),
                item("Run To Cursor", "eludite.debug.run_to_cursor"),
                // Set Next Statement (brief 0026): only where the debug adapter has gotoTargets; the command refuses it
                // otherwise, naming the adapter.
                item("Set Next Statement", "eludite.debug.set_next_statement"),
                Separator,
                item("Toggle Breakpoint", "eludite.debug.toggle_breakpoint"),
                // New Breakpoint > Function Breakpoint (brief 0026): the Breakpoints window, whose name box sets one.
                show("New Breakpoint > Function Breakpoint...", "breakpoints"),
                MenuEntry::Item {
                    label: "Delete All Breakpoints",
                    command: "eludite.debug.toggle_breakpoint",
                    args: json!({ "action": "delete_all" }),
                },
                Separator,
                show("Windows > Breakpoints", "breakpoints"),
                show("Windows > Exception Settings", "exception_settings"),
                show("Windows > Watch 1", "watch"),
                show("Windows > Locals", "locals"),
                show("Windows > Call Stack", "call_stack"),
                show("Windows > Threads", "threads"),
            ],
        ),
        menu(
            "Test",
            vec![
                // Brief 0035: the Test Explorer's commands (Run All Tests and Debug All Tests are eludite.test.run and
                // eludite.test.debug without a selection).
                item("Run All Tests", "eludite.test.run"),
                item("Debug All Tests", "eludite.test.debug"),
                MenuEntry::Item {
                    label: "Run Failed Tests",
                    command: "eludite.test.run",
                    args: json!({ "failed_only": true }),
                },
                MenuEntry::Item {
                    label: "Repeat Last Run",
                    command: "eludite.test.run",
                    args: json!({ "repeat_last": true }),
                },
                Separator,
                item("Test Explorer", "eludite.test.explorer"),
            ],
        ),
        menu(
            "Tools",
            vec![
                // The workspace's NuGet packages (brief 0048): the window over every package reference in the
                // workspace, and the Options page of the sources.
                MenuEntry::Item {
                    label: "NuGet Packages...",
                    command: "eludite.nuget.manage",
                    args: json!({ "solution": true }),
                },
                MenuEntry::Item {
                    label: "NuGet Package Sources...",
                    command: "eludite.tools.options",
                    args: json!({ "section": "NuGet Package Manager > Package Sources" }),
                },
                Separator,
                // Brief 0078: the color theme (`workbench.theme`); user themes are in the palette and on the Options
                // page.
                theme_item("Theme > Eludite Dark", "dark"),
                theme_item("Theme > Eludite Light", "light"),
                theme_item("Theme > Classic", "classic"),
                Separator,
                item("Options...", "eludite.tools.options"),
            ],
        ),
        menu(
            "Help",
            vec![
                item("View Help", "eludite.help.view"),
                // Brief 0078: the keyboard reference, the logs and the issue tracker.
                item("Keyboard Shortcuts", "eludite.help.keyboard_shortcuts"),
                Separator,
                item("Show Log", "eludite.help.show_log"),
                item("Open Logs Folder", "eludite.help.open_logs_folder"),
                item("Report a Problem...", "eludite.help.report_problem"),
                Separator,
                // Brief 0055: the self-updater's check; the status bar's update slot carries the download and the
                // restart.
                item("Check for Updates...", "eludite.update.check"),
                Separator,
                item("About Eludite", "eludite.help.about"),
            ],
        ),
    ]
}

/// Whether a command id is registered (the item is enabled).
pub type IsEnabled = Rc<dyn Fn(&str) -> bool>;

/// Whether the check item of a command id (or of a setting's key) is on.
pub type IsChecked = Rc<dyn Fn(&str) -> bool>;

/// Whether an item with this command and these arguments is enabled, for items that share a command with another
/// (Start in External Browser is `eludite.debug.start` too; brief 0037). Asked after [`IsEnabled`].
pub type IsItemEnabled = Rc<dyn Fn(&str, &Value) -> bool>;

/// What a menu (or a submenu) shows, row by row (brief 0078). Entries whose label has ` > ` are grouped under a
/// submenu row named by the part before it, at the place of the first of them: `Windows > Locals` is Locals in the
/// Windows submenu. The data stays [`MenuEntry`]s, so a brief adds an item to a submenu with a plain `item(...)`.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuRow {
    /// `entries[ix]`, shown as `label`.
    Entry {
        ix: usize,
        label: &'static str,
    },
    /// A cascading submenu and its items: (index into the menu's entries, the label shown in the submenu).
    Submenu {
        label: &'static str,
        items: Vec<(usize, &'static str)>,
    },
    Separator,
}

impl MenuRow {
    pub fn label(&self) -> Option<&'static str> {
        match self {
            MenuRow::Entry { label, .. } | MenuRow::Submenu { label, .. } => Some(label),
            MenuRow::Separator => None,
        }
    }
}

/// The rows of `entries`.
pub fn menu_rows(entries: &[MenuEntry]) -> Vec<MenuRow> {
    let mut rows: Vec<MenuRow> = Vec::new();
    for (ix, e) in entries.iter().enumerate() {
        let Some(label) = e.label() else {
            rows.push(MenuRow::Separator);
            continue;
        };
        match label.split_once(" > ") {
            Some((parent, child)) => {
                let existing = rows.iter_mut().find_map(|r| match r {
                    MenuRow::Submenu { label, items } if *label == parent => Some(items),
                    _ => None,
                });
                match existing {
                    Some(items) => items.push((ix, child)),
                    None => rows.push(MenuRow::Submenu {
                        label: parent,
                        items: vec![(ix, child)],
                    }),
                }
            }
            None => rows.push(MenuRow::Entry { ix, label }),
        }
    }
    rows
}

impl MenuEntry {
    /// The label; `None` for a separator.
    pub fn label(&self) -> Option<&'static str> {
        match self {
            MenuEntry::Item { label, .. }
            | MenuEntry::Check { label, .. }
            | MenuEntry::SettingCheck { label, .. } => Some(label),
            MenuEntry::Separator => None,
        }
    }
}

/// Visual Studio's access keys of the menu titles where the first free letter is not it (Te&st, &Tools).
const TITLE_KEYS: [(&str, char); 2] = [("Test", 's'), ("Tools", 't')];

/// The access key of each label (Windows' rule, as Visual Studio assigns them): the first letter of a word that no
/// earlier label took, else any free letter or digit, else none. `preferred` names fixed keys. The result is the
/// byte offset of the underlined character and the key (lowercase).
pub fn access_keys(labels: &[&str], preferred: &[(&str, char)]) -> Vec<Option<(usize, char)>> {
    let mut used: Vec<char> = preferred
        .iter()
        .filter(|(l, _)| labels.contains(l))
        .map(|(_, c)| *c)
        .collect();
    labels
        .iter()
        .map(|label| {
            if let Some((_, c)) = preferred.iter().find(|(l, _)| l == label) {
                return label
                    .char_indices()
                    .find(|(_, ch)| ch.to_ascii_lowercase() == *c)
                    .map(|(i, _)| (i, *c));
            }
            let free =
                |ch: char| ch.is_ascii_alphanumeric() && !used.contains(&ch.to_ascii_lowercase());
            let word_start = label.char_indices().find(|(i, ch)| {
                free(*ch)
                    && (*i == 0
                        || label[..*i]
                            .chars()
                            .last()
                            .is_some_and(|p| !p.is_ascii_alphanumeric()))
            });
            let found = word_start.or_else(|| label.char_indices().find(|(_, ch)| free(*ch)));
            found.map(|(i, ch)| {
                let k = ch.to_ascii_lowercase();
                used.push(k);
                (i, k)
            })
        })
        .collect()
}

/// What a keystroke did to the menu bar.
#[derive(Debug, Clone, PartialEq)]
pub enum MenuKey {
    /// Not the menu bar's: dispatch it as usual.
    Ignored,
    /// The menu bar took it.
    Handled,
    /// An item was chosen: the menus closed; dispatch this.
    Run(RunCommand),
}

/// The keyboard's place in the menu bar (brief 0078).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Keyboard {
    /// The bar has the keyboard (Alt was tapped or an access key opened a menu): access keys are underlined.
    active: bool,
    /// The highlighted title while no menu is open.
    title: usize,
    /// The highlighted row of the open menu.
    row: Option<usize>,
    /// The open submenu (its row in the open menu) and its highlighted item.
    sub: Option<usize>,
    sub_row: Option<usize>,
}

/// The menu bar view: titles, and the open drop-down.
pub struct MenuBar {
    menus: Vec<Menu>,
    keymap: Vec<KeyBindingSpec>,
    theme: Theme,
    is_enabled: IsEnabled,
    is_checked: Option<IsChecked>,
    is_item_enabled: Option<IsItemEnabled>,
    open: Option<usize>,
    keyboard: Keyboard,
    /// Alt is held: access keys are underlined.
    alt_held: bool,
    /// Records where the titles (`menu-<title>`) and the open menu's items (`menu-item-<title>-<label>`) are drawn
    /// (the manual runs' real-input drivers).
    probe: Option<crate::BoundsMap>,
}

impl MenuBar {
    pub fn new(
        menus: Vec<Menu>,
        keymap: Vec<KeyBindingSpec>,
        theme: Theme,
        is_enabled: IsEnabled,
    ) -> Self {
        Self {
            menus,
            keymap,
            theme,
            is_enabled,
            is_checked: None,
            is_item_enabled: None,
            open: None,
            keyboard: Keyboard::default(),
            alt_held: false,
            probe: None,
        }
    }

    /// Where check items read their state from.
    pub fn set_checked(&mut self, is_checked: IsChecked) {
        self.is_checked = Some(is_checked);
    }

    fn checked(&self, command: &str) -> bool {
        self.is_checked.as_ref().is_some_and(|f| f(command))
    }

    /// Where items that share a command read whether they are enabled (brief 0037).
    pub fn set_item_enabled(&mut self, is_item_enabled: IsItemEnabled) {
        self.is_item_enabled = Some(is_item_enabled);
    }

    fn enabled(&self, command: &str, args: &Value) -> bool {
        (self.is_enabled)(command)
            && self
                .is_item_enabled
                .as_ref()
                .is_none_or(|f| f(command, args))
    }

    /// Whether `entry` is a check item that is on.
    fn entry_checked(&self, entry: &MenuEntry) -> bool {
        entry.check_key().is_some_and(|k| self.checked(k))
    }

    /// Whether the check item labelled `label` in menu `title` is on (`None`: no such check item).
    pub fn is_item_checked(&self, title: &str, label: &str) -> Option<bool> {
        self.menus
            .iter()
            .find(|m| m.title == title)?
            .entries
            .iter()
            .find_map(|e| match e {
                MenuEntry::Check { label: l, .. } | MenuEntry::SettingCheck { label: l, .. }
                    if *l == label =>
                {
                    Some(self.entry_checked(e))
                }
                _ => None,
            })
    }

    /// Record where the titles and the open menu's items are drawn (`eludite --bounds-out`).
    pub fn set_probe(&mut self, probe: Option<crate::BoundsMap>) {
        self.probe = probe;
    }

    pub fn open_menu(&self) -> Option<&str> {
        self.open.map(|ix| self.menus[ix].title)
    }

    pub fn set_open(&mut self, title: Option<&str>, cx: &mut Context<Self>) {
        self.open = title.and_then(|t| self.menus.iter().position(|m| m.title == t));
        self.keyboard.row = None;
        self.keyboard.sub = None;
        self.keyboard.sub_row = None;
        cx.notify();
    }

    pub fn set_theme(&mut self, theme: Theme, cx: &mut Context<Self>) {
        self.theme = theme;
        cx.notify();
    }

    /// The table the shortcut column reads (brief 0078: the active keymap, switched live).
    pub fn set_keymap(&mut self, keymap: Vec<KeyBindingSpec>, cx: &mut Context<Self>) {
        self.keymap = keymap;
        cx.notify();
    }

    /// The menus (Feature Search lists their items).
    pub fn menus(&self) -> &[Menu] {
        &self.menus
    }

    /// Whether the item labelled `label` in menu `title` is enabled.
    pub fn is_item_enabled(&self, title: &str, label: &str) -> Option<bool> {
        self.menus
            .iter()
            .find(|m| m.title == title)?
            .entries
            .iter()
            .find_map(|e| match e.action(self.entry_checked(e)) {
                Some((l, command, args)) if l == label => Some(self.enabled(command, &args)),
                _ => None,
            })
    }

    /// The labels of `title`'s enabled items.
    pub fn enabled_labels(&self, title: &str) -> Vec<&'static str> {
        self.menus
            .iter()
            .find(|m| m.title == title)
            .map(|m| {
                m.entries
                    .iter()
                    .filter_map(|e| match e.action(self.entry_checked(e)) {
                        Some((label, command, args)) if self.enabled(command, &args) => Some(label),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default()
    }

    // ---- Brief 0078: the keyboard. ----------------------------------------------------------------------------------

    /// Whether the bar has the keyboard or a menu is open.
    pub fn is_active(&self) -> bool {
        self.keyboard.active || self.open.is_some()
    }

    /// Whether access keys are underlined now.
    pub fn shows_access_keys(&self) -> bool {
        self.keyboard.active || self.alt_held
    }

    /// Alt went down or up (the shell forwards the window's modifier changes).
    pub fn set_alt_held(&mut self, held: bool, cx: &mut Context<Self>) {
        if self.alt_held != held {
            self.alt_held = held;
            cx.notify();
        }
    }

    /// The highlighted title while no menu is open, the open menu's highlighted row label, and the open submenu's
    /// highlighted item label: what the keyboard is on.
    pub fn keyboard_focus(
        &self,
    ) -> (
        Option<&'static str>,
        Option<&'static str>,
        Option<&'static str>,
    ) {
        let title = (self.keyboard.active && self.open.is_none())
            .then(|| self.menus.get(self.keyboard.title).map(|m| m.title))
            .flatten();
        let Some(open) = self.open else {
            return (title, None, None);
        };
        let rows = menu_rows(&self.menus[open].entries);
        let row = self
            .keyboard
            .row
            .and_then(|r| rows.get(r))
            .and_then(MenuRow::label);
        let sub = match (self.keyboard.sub, self.keyboard.sub_row) {
            (Some(s), Some(r)) => match rows.get(s) {
                Some(MenuRow::Submenu { items, .. }) => items.get(r).map(|(_, l)| *l),
                _ => None,
            },
            _ => None,
        };
        (title, row, sub)
    }

    /// The open submenu's label.
    pub fn open_submenu(&self) -> Option<&'static str> {
        let open = self.open?;
        menu_rows(&self.menus[open].entries)
            .get(self.keyboard.sub?)
            .and_then(MenuRow::label)
    }

    fn close_all(&mut self) {
        self.open = None;
        self.keyboard = Keyboard::default();
    }

    fn title_keys(&self) -> Vec<Option<(usize, char)>> {
        let titles: Vec<&str> = self.menus.iter().map(|m| m.title).collect();
        access_keys(&titles, &TITLE_KEYS)
    }

    fn rows_of(&self, menu: usize) -> Vec<MenuRow> {
        menu_rows(&self.menus[menu].entries)
    }

    /// Whether a row can take the highlight: an item (enabled or not, as Windows does) or a submenu.
    fn selectable(row: &MenuRow) -> bool {
        !matches!(row, MenuRow::Separator)
    }

    fn step(
        len: usize,
        from: Option<usize>,
        forward: bool,
        ok: impl Fn(usize) -> bool,
    ) -> Option<usize> {
        if len == 0 {
            return None;
        }
        let mut ix = match (from, forward) {
            (None, true) => len - 1,
            (None, false) => 0,
            (Some(i), _) => i,
        };
        for _ in 0..len {
            ix = if forward {
                (ix + 1) % len
            } else {
                (ix + len - 1) % len
            };
            if ok(ix) {
                return Some(ix);
            }
        }
        None
    }

    fn open_at(&mut self, menu: usize) {
        self.open = Some(menu);
        self.keyboard.active = true;
        self.keyboard.title = menu;
        self.keyboard.sub = None;
        self.keyboard.sub_row = None;
        let rows = self.rows_of(menu);
        self.keyboard.row = Self::step(rows.len(), None, true, |i| Self::selectable(&rows[i]));
    }

    /// The entry `ix` of the open menu as a command to run, or `None` when it is disabled.
    fn choose(&mut self, menu: usize, ix: usize) -> MenuKey {
        let entry = &self.menus[menu].entries[ix];
        let Some((_, command, args)) = entry.action(self.entry_checked(entry)) else {
            return MenuKey::Handled;
        };
        if !self.enabled(command, &args) {
            return MenuKey::Handled;
        }
        self.close_all();
        MenuKey::Run(RunCommand::new(command, args))
    }

    fn open_sub(&mut self, row: usize, items: usize) {
        self.keyboard.sub = Some(row);
        self.keyboard.sub_row = (items > 0).then_some(0);
    }

    /// Handle a keystroke the window is about to dispatch (the shell calls this from a keystroke interceptor, before
    /// the key bindings): the Alt tap (`alt`, GPUI's keystroke for a lone modifier press) gives the bar the keyboard
    /// or takes it back; Alt+letter opens a menu by its access key; while the bar has the keyboard or a menu is open,
    /// the arrows, Home, End, Enter, Space, Escape and the items' access keys move through the menus. Keys with Ctrl
    /// close the menus and go on to their bindings.
    pub fn key(&mut self, keystroke: &Keystroke) -> MenuKey {
        let m = keystroke.modifiers;
        let key = keystroke.key.as_str();
        let plain = !m.control && !m.alt && !m.platform && !m.function;
        let alt_only = m.alt && !m.control && !m.platform && !m.function && !m.shift;
        if key == "alt" && !m.modified() {
            if self.is_active() {
                self.close_all();
            } else {
                self.keyboard = Keyboard {
                    active: true,
                    ..Keyboard::default()
                };
            }
            return MenuKey::Handled;
        }
        let letter = (key.chars().count() == 1)
            .then(|| key.chars().next())
            .flatten()
            .filter(char::is_ascii_alphanumeric)
            .map(|c| c.to_ascii_lowercase());
        // Alt+letter: the menu with that access key, whatever is open.
        if alt_only && let Some(c) = letter {
            if let Some(ix) = self
                .title_keys()
                .iter()
                .position(|k| k.is_some_and(|(_, k)| k == c))
            {
                self.open_at(ix);
                return MenuKey::Handled;
            }
            return if self.is_active() {
                MenuKey::Handled
            } else {
                MenuKey::Ignored
            };
        }
        if !self.is_active() {
            return MenuKey::Ignored;
        }
        if !plain && !alt_only {
            // Ctrl+anything: the menus close and the key does what it does.
            self.close_all();
            return MenuKey::Ignored;
        }
        let n = self.menus.len();
        let Some(open) = self.open else {
            // The bar has the keyboard, no menu is open: the titles.
            let t = self.keyboard.title;
            match key {
                "left" => self.keyboard.title = (t + n - 1) % n,
                "right" => self.keyboard.title = (t + 1) % n,
                "down" | "enter" | "space" | "up" => self.open_at(t),
                "escape" => self.close_all(),
                "home" => self.keyboard.title = 0,
                "end" => self.keyboard.title = n - 1,
                _ => match letter.and_then(|c| {
                    self.title_keys()
                        .iter()
                        .position(|k| k.is_some_and(|(_, k)| k == c))
                }) {
                    Some(ix) => self.open_at(ix),
                    None => {
                        self.close_all();
                        return MenuKey::Ignored;
                    }
                },
            }
            return MenuKey::Handled;
        };
        let rows = self.rows_of(open);
        // A submenu is open: its items.
        if let Some(s) = self.keyboard.sub
            && let Some(MenuRow::Submenu { items, .. }) = rows.get(s)
        {
            let items = items.clone();
            match key {
                "down" => {
                    self.keyboard.sub_row =
                        Self::step(items.len(), self.keyboard.sub_row, true, |_| true)
                }
                "up" => {
                    self.keyboard.sub_row =
                        Self::step(items.len(), self.keyboard.sub_row, false, |_| true)
                }
                "home" => self.keyboard.sub_row = (!items.is_empty()).then_some(0),
                "end" => self.keyboard.sub_row = items.len().checked_sub(1),
                "left" | "escape" => {
                    self.keyboard.sub = None;
                    self.keyboard.sub_row = None;
                }
                "right" => {
                    self.keyboard.sub = None;
                    self.open_at((open + 1) % n);
                }
                "enter" | "space" => {
                    if let Some(r) = self.keyboard.sub_row {
                        return self.choose(open, items[r].0);
                    }
                }
                _ => {
                    if let Some(c) = letter {
                        let labels: Vec<&str> = items.iter().map(|(_, l)| *l).collect();
                        if let Some(r) = access_keys(&labels, &[])
                            .iter()
                            .position(|k| k.is_some_and(|(_, k)| k == c))
                        {
                            return self.choose(open, items[r].0);
                        }
                    }
                }
            }
            return MenuKey::Handled;
        }
        match key {
            "down" => {
                self.keyboard.row = Self::step(rows.len(), self.keyboard.row, true, |i| {
                    Self::selectable(&rows[i])
                })
            }
            "up" => {
                self.keyboard.row = Self::step(rows.len(), self.keyboard.row, false, |i| {
                    Self::selectable(&rows[i])
                })
            }
            "home" => {
                self.keyboard.row =
                    Self::step(rows.len(), None, true, |i| Self::selectable(&rows[i]))
            }
            "end" => {
                self.keyboard.row =
                    Self::step(rows.len(), None, false, |i| Self::selectable(&rows[i]))
            }
            "left" => self.open_at((open + n - 1) % n),
            "right" => match self
                .keyboard
                .row
                .and_then(|r| rows.get(r).map(|row| (r, row)))
            {
                Some((r, MenuRow::Submenu { items, .. })) => self.open_sub(r, items.len()),
                _ => self.open_at((open + 1) % n),
            },
            "escape" => {
                // The menu closes; the bar keeps the keyboard on its title (a second Escape leaves it).
                self.open = None;
                self.keyboard.title = open;
                self.keyboard.row = None;
            }
            "enter" | "space" => match self
                .keyboard
                .row
                .and_then(|r| rows.get(r).map(|row| (r, row)))
            {
                Some((r, MenuRow::Submenu { items, .. })) => self.open_sub(r, items.len()),
                Some((_, MenuRow::Entry { ix, .. })) => return self.choose(open, *ix),
                _ => {}
            },
            _ => {
                if let Some(c) = letter {
                    let labels: Vec<&str> = rows.iter().map(|r| r.label().unwrap_or("")).collect();
                    if let Some(r) = access_keys(&labels, &[])
                        .iter()
                        .position(|k| k.is_some_and(|(_, k)| k == c))
                    {
                        self.keyboard.row = Some(r);
                        match &rows[r] {
                            MenuRow::Submenu { items, .. } => self.open_sub(r, items.len()),
                            MenuRow::Entry { ix, .. } => return self.choose(open, *ix),
                            MenuRow::Separator => {}
                        }
                    }
                }
            }
        }
        MenuKey::Handled
    }

    /// The menu bar's key handling with its effects: notify, and run the chosen item.
    pub fn handle_key(
        &mut self,
        keystroke: &Keystroke,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> bool {
        let before = (self.open, self.keyboard, self.alt_held);
        let outcome = self.key(keystroke);
        if before != (self.open, self.keyboard, self.alt_held) {
            cx.notify();
        }
        match outcome {
            MenuKey::Ignored => false,
            MenuKey::Handled => true,
            MenuKey::Run(action) => {
                window.dispatch_action(Box::new(action), cx);
                true
            }
        }
    }

    /// A label with its access key underlined while access keys show.
    fn label_element(&self, label: &'static str, key: Option<(usize, char)>) -> AnyElement {
        match key.filter(|_| self.shows_access_keys()) {
            Some((at, _)) => {
                let end = label[at..].chars().next().map_or(at, |c| at + c.len_utf8());
                StyledText::new(label)
                    .with_highlights([(
                        at..end,
                        HighlightStyle {
                            underline: Some(UnderlineStyle {
                                thickness: px(1.),
                                ..Default::default()
                            }),
                            ..Default::default()
                        },
                    )])
                    .into_any_element()
            }
            None => div().child(label).into_any_element(),
        }
    }

    fn render_item_row(
        &self,
        menu: usize,
        ix: usize,
        label: &'static str,
        key: Option<(usize, char)>,
        highlighted: bool,
        id: (&'static str, usize),
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let t = &self.theme;
        let ty = t.typography;
        let title = self.menus[menu].title;
        let entry = &self.menus[menu].entries[ix];
        let checked = self.entry_checked(entry);
        let (full, command, args) = entry.action(checked).expect("not a separator");
        let enabled = self.enabled(command, &args);
        let shortcut = shortcut_for(&self.keymap, command, &args).unwrap_or_default();
        let selector = format!("menu-item-{title}-{full}");
        let probed = crate::bounds_canvas(self.probe.as_ref(), selector.clone());
        let mut el = div()
            .id(id)
            .role(if entry.check_key().is_some() {
                gpui::Role::MenuItemCheckBox
            } else {
                gpui::Role::MenuItem
            })
            .aria_label(label)
            .relative()
            .children(probed)
            .debug_selector(move || selector.clone())
            .flex()
            .flex_row()
            .items_center()
            .h(px(24.))
            .pl_6()
            .pr_3()
            .gap_8()
            .children(checked.then(|| div().absolute().left(px(8.)).child("\u{2713}")))
            .child(div().flex_1().child(self.label_element(label, key)))
            .child(div().text_size(ty.small).child(shortcut));
        if highlighted {
            el = el.bg(t.menu_hover);
        }
        if enabled {
            let action = RunCommand::new(command, args);
            el = el
                .text_color(t.menu_text)
                .cursor_pointer()
                .hover(|s| s.bg(t.menu_hover))
                .on_click(cx.listener(move |this, _, window, cx| {
                    this.close_all();
                    cx.notify();
                    window.dispatch_action(Box::new(action.clone()), cx);
                }));
        } else {
            el = el.text_color(t.text_disabled);
        }
        el.into_any_element()
    }

    fn popup_frame(&self, id: &'static str) -> gpui::Stateful<gpui::Div> {
        let t = &self.theme;
        div()
            .id(id)
            .role(gpui::Role::Menu)
            .occlude()
            .flex()
            .flex_col()
            .min_w(px(240.))
            .py_1()
            .bg(t.popup_background)
            .border_1()
            .border_color(t.popup_border)
            .shadow_md()
            .text_size(t.typography.ui)
    }

    fn render_popup(&self, ix: usize, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let title = self.menus[ix].title;
        let rows = self.rows_of(ix);
        let labels: Vec<&str> = rows.iter().map(|r| r.label().unwrap_or("")).collect();
        let keys = access_keys(&labels, &[]);
        let mut top = px(4.);
        let mut elements = Vec::new();
        for (row, r) in rows.iter().enumerate() {
            let highlighted = self.keyboard.row == Some(row);
            match r {
                MenuRow::Separator => {
                    top += px(9.);
                    elements.push(
                        div()
                            .h(px(1.))
                            .my_1()
                            .mx_2()
                            .bg(t.popup_border)
                            .into_any_element(),
                    );
                }
                MenuRow::Entry { ix: e, label } => {
                    top += px(24.);
                    elements.push(self.render_item_row(
                        ix,
                        *e,
                        label,
                        keys[row],
                        highlighted,
                        ("menu-item", row),
                        cx,
                    ));
                }
                MenuRow::Submenu { label, items } => {
                    let row_top = top;
                    top += px(24.);
                    let open = self.keyboard.sub == Some(row);
                    let selector = format!("menu-submenu-{title}-{label}");
                    let probed = crate::bounds_canvas(self.probe.as_ref(), selector.clone());
                    let mut el = div()
                        .id(("menu-submenu", row))
                        .role(gpui::Role::MenuItem)
                        .aria_label(*label)
                        .relative()
                        .children(probed)
                        .debug_selector(move || selector.clone())
                        .flex()
                        .flex_row()
                        .items_center()
                        .h(px(24.))
                        .pl_6()
                        .pr_3()
                        .gap_8()
                        .text_color(t.menu_text)
                        .cursor_pointer()
                        .hover(|s| s.bg(t.menu_hover))
                        .child(div().flex_1().child(self.label_element(label, keys[row])))
                        .child(div().child("\u{25B8}"))
                        .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                            if *hovered && this.keyboard.sub != Some(row) {
                                this.keyboard.row = Some(row);
                                this.keyboard.sub = Some(row);
                                this.keyboard.sub_row = None;
                                cx.notify();
                            }
                        }))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.keyboard.row = Some(row);
                            this.keyboard.sub = Some(row);
                            cx.notify();
                        }));
                    if highlighted || open {
                        el = el.bg(t.menu_hover);
                    }
                    elements.push(el.into_any_element());
                    if open {
                        let sub_labels: Vec<&str> = items.iter().map(|(_, l)| *l).collect();
                        let sub_keys = access_keys(&sub_labels, &[]);
                        let sub_rows: Vec<AnyElement> = items
                            .iter()
                            .enumerate()
                            .map(|(i, (e, l))| {
                                self.render_item_row(
                                    ix,
                                    *e,
                                    l,
                                    sub_keys[i],
                                    self.keyboard.sub_row == Some(i),
                                    ("menu-subitem", i),
                                    cx,
                                )
                            })
                            .collect();
                        elements.push(
                            deferred(
                                div()
                                    .absolute()
                                    .top(row_top - px(4.))
                                    .left(px(238.))
                                    .child(self.popup_frame("menu-subpopup").children(sub_rows)),
                            )
                            .with_priority(3)
                            .into_any_element(),
                        );
                    }
                }
            }
        }
        self.popup_frame("menu-popup").relative().children(elements)
    }
}

impl Render for MenuBar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let t = self.theme;
        let ty = t.typography;
        let open = self.open;
        let title_keys = self.title_keys();
        let kb_title = (self.keyboard.active && open.is_none()).then_some(self.keyboard.title);
        let titles: Vec<_> = self
            .menus
            .iter()
            .enumerate()
            .map(|(ix, m)| {
                let title = m.title;
                let is_open = open == Some(ix);
                let mut el = div()
                    .id(("menu", ix))
                    .role(gpui::Role::MenuItem)
                    .aria_label(title)
                    .debug_selector(move || format!("menu-{title}"))
                    .relative()
                    .flex()
                    .items_center()
                    .h_full()
                    .px_2()
                    .cursor_pointer()
                    .hover(|s| s.bg(t.menu_hover))
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, _, _, cx| {
                            if this.open == Some(ix) {
                                this.close_all();
                            } else {
                                this.open = Some(ix);
                                this.keyboard.row = None;
                                this.keyboard.sub = None;
                                this.keyboard.sub_row = None;
                            }
                            cx.notify();
                        }),
                    )
                    .on_hover(cx.listener(move |this, hovered: &bool, _, cx| {
                        if *hovered && this.open.is_some() && this.open != Some(ix) {
                            this.open = Some(ix);
                            this.keyboard.row = None;
                            this.keyboard.sub = None;
                            this.keyboard.sub_row = None;
                            cx.notify();
                        }
                    }))
                    .children(crate::bounds_canvas(
                        self.probe.as_ref(),
                        format!("menu-{title}"),
                    ))
                    .child(self.label_element(title, title_keys[ix]));
                if kb_title == Some(ix) {
                    el = el.bg(t.menu_hover);
                }
                if is_open {
                    el = el.bg(t.menu_hover).child(
                        deferred(
                            div()
                                .absolute()
                                .top(ty.menu_bar_height)
                                .left_0()
                                .child(self.render_popup(ix, cx)),
                        )
                        .with_priority(2),
                    );
                }
                el
            })
            .collect();

        let mut bar = div()
            .id("menu-bar")
            .role(gpui::Role::MenuBar)
            .aria_label("Menu bar")
            .flex()
            .flex_row()
            .items_center()
            .flex_none()
            .h(ty.menu_bar_height)
            .px_1()
            .bg(t.menu_background)
            .text_size(ty.ui)
            .text_color(t.menu_text)
            .font_weight(FontWeight::NORMAL)
            .children(titles);
        if open.is_some() {
            // Clicking anywhere below the bar closes the menu.
            let size = window.viewport_size();
            let top = ty.menu_bar_height;
            bar = bar.child(
                deferred(
                    anchored().position(point(px(0.), top)).child(
                        div()
                            .id("menu-backdrop")
                            .debug_selector(|| "menu-backdrop".into())
                            .occlude()
                            .w(size.width)
                            .h(size.height - top)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.close_all();
                                    cx.notify();
                                }),
                            ),
                    ),
                )
                .with_priority(1),
            );
        }
        bar
    }
}

/// Convenience for building a [`MenuBar`] with an `is_enabled` closure.
pub fn menu_bar_with(
    theme: Theme,
    keymap: Vec<KeyBindingSpec>,
    is_enabled: impl Fn(&str) -> bool + 'static,
) -> MenuBar {
    MenuBar::new(vs_menus(), keymap, theme, Rc::new(is_enabled))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_in_order() {
        let titles: Vec<_> = vs_menus().iter().map(|m| m.title).collect();
        assert_eq!(titles, MENU_TITLES);
    }

    #[test]
    fn every_item_names_a_valid_command_id() {
        for m in vs_menus() {
            assert!(!m.entries.is_empty(), "{}", m.title);
            for e in &m.entries {
                if let MenuEntry::Item { command, args, .. } = e {
                    let segments: Vec<_> = command.split('.').collect();
                    assert!(segments.len() >= 2, "{command}");
                    assert!(
                        segments.iter().all(|s| s
                            .chars()
                            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')),
                        "{command}"
                    );
                    assert!(args.is_object(), "{command}");
                }
            }
        }
    }

    #[test]
    fn view_menu_shows_keymap_shortcuts() {
        let keymap = crate::keymap::vs_keymap();
        let view = vs_menus().into_iter().find(|m| m.title == "View").unwrap();
        let shortcuts: Vec<_> = view
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator | MenuEntry::Check { .. } | MenuEntry::SettingCheck { .. } => {
                    None
                }
            })
            .collect();
        assert!(shortcuts.contains(&("Workspace", Some("Ctrl+Alt+L"))));
        assert!(shortcuts.contains(&("Error List", Some("Ctrl+\\, Ctrl+E"))));
        // Brief 0078.
        assert!(shortcuts.contains(&("Command Palette", Some("Ctrl+Shift+P"))));
        assert!(shortcuts.contains(&("Feature Search", Some("Ctrl+Q"))));
        assert!(shortcuts.contains(&("Full Screen", Some("Shift+Alt+Enter"))));
        assert!(shortcuts.contains(&("Other Windows > Web Browser", None)));
        // Brief 0041.
        assert!(shortcuts.contains(&("Terminal", Some("Ctrl+`"))));
        // Brief 0042.
        assert!(shortcuts.contains(&("Other Windows > Find Results 1", None)));
        assert!(shortcuts.contains(&("Other Windows > Find Results 2", None)));
        // Brief 0046.
        let edit = vs_menus().into_iter().find(|m| m.title == "Edit").unwrap();
        let shortcuts: Vec<_> = edit
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator | MenuEntry::Check { .. } | MenuEntry::SettingCheck { .. } => {
                    None
                }
            })
            .collect();
        assert!(shortcuts.contains(&("Find", Some("Ctrl+F"))));
        assert!(shortcuts.contains(&("Find in Files", Some("Ctrl+Shift+F"))));
        assert!(shortcuts.contains(&("Replace in Files", Some("Ctrl+Shift+H"))));
        // Brief 0050.
        assert!(shortcuts.contains(&("Format Document", Some("Ctrl+K, Ctrl+D"))));
        let build = vs_menus().into_iter().find(|m| m.title == "Build").unwrap();
        let shortcuts: Vec<_> = build
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator | MenuEntry::Check { .. } | MenuEntry::SettingCheck { .. } => {
                    None
                }
            })
            .collect();
        assert_eq!(
            shortcuts,
            [
                ("Build", Some("Ctrl+Shift+B")),
                ("Rebuild", None),
                ("Clean", None),
                ("Cancel", None)
            ]
        );
    }

    #[test]
    fn the_debug_menu_has_run_control_items_with_their_keys() {
        let keymap = crate::keymap::vs_keymap();
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let items: Vec<_> = debug
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, *command, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator | MenuEntry::Check { .. } | MenuEntry::SettingCheck { .. } => {
                    None
                }
            })
            .collect();
        let at = |label: &str| items.iter().position(|i| i.0 == label).unwrap();
        assert_eq!(
            items[at("Set Next Statement")],
            (
                "Set Next Statement",
                "eludite.debug.set_next_statement",
                Some("Ctrl+Shift+F10")
            )
        );
        assert!(at("Run To Cursor") < at("Set Next Statement"));
        let f = &items[at("New Breakpoint > Function Breakpoint...")];
        assert_eq!(f.1, "eludite.view.show");
        assert!(at("Toggle Breakpoint") < at("New Breakpoint > Function Breakpoint..."));
        // Debug.Breakpoints: Ctrl+Alt+B shows the Breakpoints window.
        assert_eq!(items[at("Windows > Breakpoints")].2, Some("Ctrl+Alt+B"));
    }

    #[test]
    fn the_debug_menu_has_break_all_with_its_key() {
        let keymap = crate::keymap::vs_keymap();
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let items: Vec<_> = debug
            .entries
            .iter()
            .filter_map(|e| match e {
                MenuEntry::Item {
                    label,
                    command,
                    args,
                } => Some((*label, *command, shortcut_for(&keymap, command, args))),
                MenuEntry::Separator | MenuEntry::Check { .. } | MenuEntry::SettingCheck { .. } => {
                    None
                }
            })
            .collect();
        let at = |label: &str| items.iter().position(|i| i.0 == label).unwrap();
        assert_eq!(
            items[at("Break All")],
            ("Break All", "eludite.debug.pause", Some("Ctrl+Alt+Break"))
        );
        // Visual Studio's order: after Continue, before the steps.
        assert!(at("Continue") < at("Break All") && at("Break All") < at("Step Into"));
        assert!(
            keymap
                .iter()
                .any(|b| b.keystrokes == "ctrl-alt-pause" && b.command == "eludite.debug.pause")
        );
    }

    #[test]
    fn the_debug_menu_has_restart_attach_and_the_agents_check_item() {
        let keymap = crate::keymap::vs_keymap();
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let items: Vec<_> = debug
            .entries
            .iter()
            .filter_map(|e| {
                let (label, command, args) = e.action(true)?;
                Some((label, command, shortcut_for(&keymap, command, &args), args))
            })
            .collect();
        let at = |label: &str| items.iter().position(|i| i.0 == label).unwrap();
        let restart = &items[at("Restart")];
        assert_eq!(
            (restart.1, restart.2),
            ("eludite.debug.restart", Some("Ctrl+Shift+F5"))
        );
        assert!(at("Stop Debugging") < at("Restart") && at("Restart") < at("Continue"));
        let attach = &items[at("Attach to Process...")];
        assert_eq!(
            (attach.1, attach.2),
            ("eludite.debug.attach", Some("Ctrl+Alt+P"))
        );
        // The check item dispatches the other state.
        let allow = &items[at("Allow Agents to Drive")];
        assert_eq!(allow.1, "eludite.debug.allow_agents");
        assert_eq!(allow.3, json!({"enabled": false}));
        let off = MenuEntry::Check {
            label: "x",
            command: "c",
        }
        .action(false)
        .unwrap();
        assert_eq!(off.2, json!({"enabled": true}));
        assert!(MenuEntry::Separator.action(false).is_none());
    }

    /// Brief 0037: Debug > Start in External Browser starts with `browser: external` and no key of its own; Debug >
    /// Open in Web Browser Window is a check item on the setting browser.useBuiltIn, dispatching the other value.
    #[test]
    fn the_debug_menu_starts_in_the_external_browser_and_checks_the_web_browser_window() {
        let keymap = crate::keymap::vs_keymap();
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let labels: Vec<&str> = debug
            .entries
            .iter()
            .filter_map(|e| e.action(false).map(|a| a.0))
            .collect();
        let at = |label: &str| labels.iter().position(|l| *l == label).unwrap();
        assert!(at("Start Without Debugging") < at("Start in External Browser"));
        assert!(at("Start in External Browser") < at("Open in Web Browser Window"));
        assert!(at("Open in Web Browser Window") < at("Stop Debugging"));
        let external = debug.entries[at("Start in External Browser")]
            .action(false)
            .unwrap();
        assert_eq!(external.1, "eludite.debug.start");
        assert_eq!(external.2, json!({"browser": "external"}));
        assert_eq!(shortcut_for(&keymap, external.1, &external.2), None);
        let check = &debug.entries[at("Open in Web Browser Window")];
        assert_eq!(check.check_key(), Some("browser.useBuiltIn"));
        assert_eq!(
            check.action(true).unwrap(),
            (
                "Open in Web Browser Window",
                SETTINGS_SET,
                json!({"key": "browser.useBuiltIn", "value": false})
            )
        );
        assert_eq!(
            check.action(false).unwrap().2,
            json!({"key": "browser.useBuiltIn", "value": true})
        );
    }

    /// PLAN.md 14, decision 12: no menu item names a solution or a project, and there is no Project menu.
    #[test]
    fn no_menu_item_names_a_solution_or_a_project() {
        for m in vs_menus() {
            assert_ne!(m.title, "Project");
            for e in &m.entries {
                if let Some((label, _, _)) = e.action(false) {
                    for word in ["Solution", "Project"] {
                        assert!(!label.contains(word), "{} > {label}", m.title);
                    }
                }
            }
        }
    }

    /// Brief 0048: Tools > NuGet Packages... (the workspace's packages) and NuGet Package Sources... (the Options
    /// page).
    #[test]
    fn the_nuget_items_open_the_window_and_the_package_sources_page() {
        let tools = vs_menus().into_iter().find(|m| m.title == "Tools").unwrap();
        let find = |label: &str| {
            tools
                .entries
                .iter()
                .find_map(|e| match e {
                    MenuEntry::Item {
                        label: l,
                        command,
                        args,
                    } if *l == label => Some((*command, args.clone())),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no Tools > {label}"))
        };
        assert_eq!(
            find("NuGet Packages..."),
            ("eludite.nuget.manage", json!({"solution": true}))
        );
        assert_eq!(
            find("NuGet Package Sources..."),
            (
                "eludite.tools.options",
                json!({"section": "NuGet Package Manager > Package Sources"})
            )
        );
    }

    /// Brief 0038: Debug > Attach to Browser Tab... after Attach to Process..., the same command with
    /// `adapter: javascript` (the dialog's tabs) and no key of its own.
    #[test]
    fn the_debug_menu_attaches_to_a_browser_tab() {
        let keymap = crate::keymap::vs_keymap();
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let labels: Vec<&str> = debug
            .entries
            .iter()
            .filter_map(|e| e.action(false).map(|a| a.0))
            .collect();
        let at = |label: &str| labels.iter().position(|l| *l == label).unwrap();
        assert_eq!(
            at("Attach to Browser Tab..."),
            at("Attach to Process...") + 1
        );
        let item = debug
            .entries
            .iter()
            .filter_map(|e| e.action(false))
            .nth(at("Attach to Browser Tab..."))
            .unwrap();
        assert_eq!(item.1, "eludite.debug.attach");
        assert_eq!(item.2, json!({"adapter": "javascript"}));
        assert_eq!(shortcut_for(&keymap, item.1, &item.2), None);
    }

    fn ks(text: &str) -> Keystroke {
        Keystroke::parse(text).unwrap()
    }

    fn bar() -> MenuBar {
        MenuBar::new(
            vs_menus(),
            crate::keymap::vs_keymap(),
            Theme::default(),
            Rc::new(|_| true),
        )
    }

    /// Brief 0078: ` > ` labels become submenus at the place of their first item.
    #[test]
    fn labels_with_a_parent_become_submenus() {
        let debug = vs_menus().into_iter().find(|m| m.title == "Debug").unwrap();
        let rows = menu_rows(&debug.entries);
        let windows = rows
            .iter()
            .find_map(|r| match r {
                MenuRow::Submenu {
                    label: "Windows",
                    items,
                } => Some(items.clone()),
                _ => None,
            })
            .unwrap();
        let labels: Vec<_> = windows.iter().map(|(_, l)| *l).collect();
        assert_eq!(labels[0], "Breakpoints");
        assert!(labels.contains(&"Locals"));
        assert!(labels.contains(&"Call Stack"));
        // Each child points at its entry, whose full label is unchanged.
        for (ix, l) in &windows {
            assert_eq!(
                debug.entries[*ix].label().unwrap(),
                format!("Windows > {l}")
            );
        }
        let view = vs_menus().into_iter().find(|m| m.title == "View").unwrap();
        assert!(menu_rows(&view.entries).iter().any(|r| matches!(
            r,
            MenuRow::Submenu { label: "Other Windows", items } if items.len() >= 3
        )));
        // Rows never mix: no submenu row is also an entry.
        assert!(menu_rows(&view.entries).iter().all(|r| match r {
            MenuRow::Entry { label, .. } => !label.contains(" > "),
            _ => true,
        }));
    }

    #[test]
    fn access_keys_are_unique_and_visual_studios_for_the_titles() {
        let titles: Vec<&str> = MENU_TITLES.to_vec();
        let keys: Vec<char> = access_keys(&titles, &TITLE_KEYS)
            .into_iter()
            .map(|k| k.unwrap().1)
            .collect();
        assert_eq!(keys, ['f', 'e', 'v', 'g', 'b', 'd', 's', 't', 'h']);
        for m in vs_menus() {
            let rows = menu_rows(&m.entries);
            let labels: Vec<&str> = rows.iter().filter_map(MenuRow::label).collect();
            let mut k: Vec<char> = access_keys(&labels, &[])
                .into_iter()
                .flatten()
                .map(|k| k.1)
                .collect();
            let n = k.len();
            k.sort_unstable();
            k.dedup();
            assert_eq!(k.len(), n, "{}", m.title);
        }
        // A word's first letter first; the offset is the underlined character's.
        assert_eq!(
            access_keys(&["Save", "Save All"], &[]),
            [Some((0, 's')), Some((5, 'a'))]
        );
    }

    #[test]
    fn alt_letter_opens_a_menu_and_arrows_move_into_a_submenu() {
        let mut m = bar();
        assert_eq!(m.key(&ks("down")), MenuKey::Ignored, "inactive bar");
        assert_eq!(m.key(&ks("alt-d")), MenuKey::Handled);
        assert_eq!(m.open_menu(), Some("Debug"));
        assert!(m.shows_access_keys());
        assert_eq!(m.keyboard_focus().1, Some("Start Debugging"));
        m.key(&ks("down"));
        assert_eq!(m.keyboard_focus().1, Some("Start Without Debugging"));
        m.key(&ks("up"));
        m.key(&ks("up"));
        // Up from the first row wraps to the last: the Windows submenu.
        assert_eq!(m.keyboard_focus().1, Some("Windows"));
        assert_eq!(m.key(&ks("right")), MenuKey::Handled);
        assert_eq!(m.open_submenu(), Some("Windows"));
        assert_eq!(m.keyboard_focus().2, Some("Breakpoints"));
        m.key(&ks("down"));
        assert_eq!(m.keyboard_focus().2, Some("Exception Settings"));
        m.key(&ks("left"));
        assert_eq!(m.open_submenu(), None);
        m.key(&ks("enter"));
        assert_eq!(m.open_submenu(), Some("Windows"));
        m.key(&ks("end"));
        let last = m.keyboard_focus().2.unwrap();
        match m.key(&ks("enter")) {
            MenuKey::Run(action) => {
                assert_eq!(action.command.as_ref(), "eludite.view.show");
                assert_eq!(
                    action.args["id"],
                    json!(last.to_lowercase().replace(' ', "_"))
                );
            }
            other => panic!("{other:?}"),
        }
        assert!(!m.is_active());
        assert_eq!(m.open_menu(), None);
    }

    #[test]
    fn the_alt_tap_left_right_and_escape() {
        let mut m = bar();
        m.key(&ks("alt"));
        assert!(m.is_active());
        assert_eq!(m.keyboard_focus().0, Some("File"));
        m.key(&ks("right"));
        m.key(&ks("right"));
        assert_eq!(m.keyboard_focus().0, Some("View"));
        m.key(&ks("left"));
        m.key(&ks("left"));
        m.key(&ks("left"));
        assert_eq!(m.keyboard_focus().0, Some("Help"));
        m.key(&ks("down"));
        assert_eq!(m.open_menu(), Some("Help"));
        m.key(&ks("right"));
        assert_eq!(m.open_menu(), Some("File"), "Right wraps to the first menu");
        m.key(&ks("escape"));
        assert_eq!(m.open_menu(), None);
        assert_eq!(m.keyboard_focus().0, Some("File"));
        m.key(&ks("escape"));
        assert!(!m.is_active());
        // A letter opens the title with that access key while the bar has the keyboard; Ctrl keys close the menus.
        m.key(&ks("alt"));
        m.key(&ks("s"));
        assert_eq!(m.open_menu(), Some("Test"));
        assert_eq!(m.key(&ks("ctrl-s")), MenuKey::Ignored);
        assert!(!m.is_active());
        // A second Alt tap leaves the bar.
        m.key(&ks("alt"));
        m.key(&ks("alt"));
        assert!(!m.is_active());
        // An unknown Alt+letter goes on to the bindings.
        assert_eq!(m.key(&ks("alt-z")), MenuKey::Ignored);
    }

    #[test]
    fn an_item_access_key_runs_it_and_a_disabled_item_does_nothing() {
        let mut m = bar();
        m.key(&ks("alt-b"));
        assert_eq!(m.open_menu(), Some("Build"));
        // Build, Rebuild, Clean, Cancel: B, R, C, a.
        match m.key(&ks("r")) {
            MenuKey::Run(a) => assert_eq!(a.command.as_ref(), "eludite.build.rebuild"),
            other => panic!("{other:?}"),
        }
        let mut m = MenuBar::new(
            vs_menus(),
            crate::keymap::vs_keymap(),
            Theme::default(),
            Rc::new(|c| c != "eludite.build.rebuild"),
        );
        m.key(&ks("alt-b"));
        assert_eq!(m.key(&ks("r")), MenuKey::Handled);
        assert_eq!(m.open_menu(), Some("Build"));
    }
}
