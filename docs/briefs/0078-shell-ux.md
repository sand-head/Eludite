# Brief 0078: The shell's everyday chrome

Status: in progress
Phase: 2 (PLAN.md section 10, "out of the prototype stage"; section 8)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5), 5.1, 8 (Chrome, Themes, Keymaps), 14 (decision 14: nine menus, Visual Studio's names)
Related ADRs: ADR-0010 (Eludite draws the title bar), ADR-0014 (tests fail on behavior only)
Depends on: brief 0008 (menus, keymap, command bus), brief 0020 (settings, Tools > Options), brief 0044 (title bar), brief 0055 (`build.json`, the update notice)

Written from the owner's request of 2026-10-11 ("on par with Visual Studio/Rider for .NET work ... out of a prototype
stage"), which is this brief's acceptance, and an audit of `main` at `f312628`.

## Goal

The chrome a Visual Studio or Rider user reaches for in the first hour works:

1. **Command palette and Feature Search.** Ctrl+Shift+P (View > Command Palette) opens an overlay picker over the
   command registry: user-invocable commands only, fuzzy matched on the title and category, each row with its title,
   category and current shortcut, recently used first; a command whose only required input is one string asks for it
   in the same box; commands listed by the menus with arguments (View > Workspace is `eludite.view.show` with
   `{"id": "workspace"}`) are listed per variant. Ctrl+Q (Visual Studio 2022's Feature Search, also a search box in
   the title bar) lists the same commands plus every menu item (with its path, `Debug > Windows > Locals`) and every
   Options page. Commands nothing registers are not listed.
2. **Menu bar mechanics.** Alt+letter opens a menu by its access key (underlined while Alt is down or the bar has the
   keyboard); tapping Alt gives the bar the keyboard; Left, Right, Up, Down, Home, End, Enter, Escape and the item
   access keys move through the menus; labels with ` > ` (`Windows > Locals`, `Other Windows > Web Browser`) become
   real cascading submenus, opened with Right or Enter or a hover, closed with Left or Escape. The data shape of
   `vs_menus` does not change, so other briefs' one-line `item(...)` and `show(...)` additions merge as they are.
   F10 stays Debug.StepOver as in Visual Studio's default scheme (Visual Studio gives the bar the keyboard on Alt).
3. **Keymap presets, user bindings, the keymap editor.** The setting `keyboard.preset` (Tools > Options >
   Environment > Keyboard) is `visualStudio` (default), `rider` (Rider's IntelliJ scheme) or `vsCode`, switched live;
   `<config>/eludite/keybindings.json` (an array of `{"key", "command", "args"?, "when"?}`; `"command": "-id"` or
   `null` removes a binding) overrides the preset and is reloaded live; the Keyboard page of Options lists every
   command with its bindings, searched by name or keystroke, with Assign (a keystroke box, conflicts shown with the
   commands they would take over) and Remove, scope Global or Text Editor. Help > Keyboard Shortcuts opens a reference
   generated from the active keymap.
4. **Themes.** The setting `workbench.theme` (`dark`, `light`, `classic` or a user theme's id; Options >
   Environment > General, and Tools > Theme) switches live; a JSON theme format (`docs/user/themes.md`,
   `protocol/schemas/theme.json`) covers every token of `crates/ui/src/theme.rs`, `extends` a built-in theme, and is
   loaded from `<config>/eludite/themes/*.json`, reloaded live, a broken file reported in Output (and in
   `eludite.theme.list`'s `errors`). The theme is one GPUI global (`ActiveTheme`); `--theme` still picks the
   starting theme over the setting.
5. **UI font and scale.** Settings `workbench.uiFontFamily`, `workbench.uiFontSize` and `workbench.uiScale`
   (Options > Environment > Fonts and Colors) applied live to the shell's chrome (menu, title bar, status bar, docking).
6. **Window bounds restore and full screen.** The main window's bounds, maximized and full-screen state and display
   are saved on change (debounced) and on exit to `<config>/eludite/window.json` and restored at the next start,
   clamped to the displays present. View > Full Screen (Shift+Alt+Enter, `eludite.view.full_screen`) toggles full
   screen; the menu bar stays drawn while full screen so there is always a way out.
7. **Crash reports and logs.** A panic hook writes `<state>/eludite/logs/crash-<time>.txt` (message, location,
   backtrace, version, channel and build from `build.json`, OS, the last 20 commands of the audit log) and the shell
   mirrors its stderr lines to a rotating `<state>/eludite/logs/eludite.log` (1 MB, three generations). The next
   start says "Eludite closed unexpectedly" in the notifications with Open Report. Help > Open Logs Folder and Show
   Log; Help > About Eludite opens a dialog (version, channel, build, commit, eludite-host, Roslyn, netcoredbg and
   .NET SDK versions when known, OS, Copy Info, the license and the third-party notices); Help > Report a Problem
   opens the issue tracker in the system browser with the version information prefilled (no telemetry; nothing is
   sent by Eludite).
8. **Notifications.** A bell in the status bar with the unread count opens the Notifications pane (Visual Studio's
   notification hub): items with a severity, a source, a time, actions and a dismiss button; Dismiss All. A small API
   (`notifications::Notifications::push`) other modules call; three sources wired: the update notice (brief 0055),
   a build that finishes while the window is not active, and the crash notice.
9. **Files dropped on the window** open as documents; a dropped folder, `.sln`, `.slnx` or project file asks "Open
   as workspace?" and then runs `eludite.workspace.open_folder` or `eludite.solution.open`.
10. **Window layouts.** View > Window Layouts > Save Window Layout..., Apply Window Layout (Ctrl+Alt+1 to 9, in save
    order) and Manage Window Layouts... over `crates/docking/src/persist.rs`'s named layouts; applying keeps the
    open documents.

## Files in scope

- `protocol/schemas/` first and alone (new files only): `view-command-palette`, `commands-list`, `keymap-get`,
  `keymap-set`, `keymap-remove`, `help-keyboard-shortcuts`, `theme-list`, `theme-set`, `theme.json` (the file
  format), `view-full-screen`, `help-about.output.json`, `help-logs`, `help-report-problem`, `notifications-list`,
  `notifications-show`, `notifications-dismiss`, `view-window-layout` (each `.input.json` and `.output.json`), and
  `keybindings.json` (the file format); additive edits to `settings.json` (the keys above).
- `crates/commands/src/ux.rs` (new): the command ids, parsing, specs and `UxTarget`; one `pub mod ux;` line in
  `lib.rs`.
- `crates/ui/src/menu.rs` (mechanics: keyboard, access keys, submenus), `keymap.rs` (presets, user overrides, the
  active keymap), `theme.rs` (the global, the file format, loading), `title_bar.rs` (the search box), `status.rs`
  (the bell slot, additive), `lib.rs` (exports).
- New shell modules in `crates/eludite/src/shell/`: `ux.rs` (wiring, the jobs, the settings), `command_palette.rs`,
  `keymap_editor.rs`, `notifications.rs`, `about.rs`, `layouts.rs` and their `*_tests.rs`; `crates/eludite/src/
  crash.rs`, `window_state.rs`; marked blocks in `main.rs`, `app.rs`, `shell.rs` (mod lines, the `Services` field,
  the overlay children, the drop handler) and `shell/settings.rs`, `shell/update.rs`, `shell/build.rs` (one call each
  to feed notifications).
- `docs/user/themes.md`, `docs/briefs/0078-shell-ux.md`, `docs/briefs/0078-report.md`.

## Contract

- Every action is a command with a stable id and checked-in schemas: `eludite.view.command_palette`
  (`{"query"?, "mode"?: "commands" | "features"}`), `eludite.commands.list` (read: every user-invocable command with
  its title, category, shortcut and arguments, filtered by `query`), `eludite.keymap.get` (read), `eludite.keymap.set`
  and `eludite.keymap.remove` (write `keybindings.json`), `eludite.help.keyboard_shortcuts`, `eludite.theme.list`
  (read), `eludite.theme.set`, `eludite.view.full_screen`, `eludite.help.about` (replaced by the shell with a superset
  of the built-in output), `eludite.help.open_logs_folder`, `eludite.help.show_log`, `eludite.help.report_problem`,
  `eludite.notifications.list` (read), `eludite.notifications.show`, `eludite.notifications.dismiss`,
  `eludite.view.save_window_layout`, `eludite.view.apply_window_layout`, `eludite.view.manage_window_layouts`.
- The keymap's data shape (`KeyBindingSpec`, `vs_keymap()`) and the menus' (`MenuEntry`, `vs_menus()`) stay as they
  are; presets are tables beside `vs_keymap`; the active keymap is `keymap::active_keymap(preset, user)`.
- The UI thread never blocks: theme and keybinding files are read by a polling thread, crash and log files written
  off the UI thread (the panic hook writes synchronously, as the process is dying).
- No network call; Report a Problem only opens a URL in the system browser when the person asks.

## Proving test

Headless (`cargo test -p eludite-ui`, `-p eludite-commands`, `-p eludite`):

- palette: filtering and ranking (recent use first, then score), the shortcut column, a command run from it, a
  one-string command asking for its input, Feature Search listing menu items and Options pages;
- menu: Alt+F opens File, Down/Up move, Right opens a submenu (Debug > Windows), Enter runs its item, Escape closes,
  the access keys are unique per menu;
- keymap: switching `keyboard.preset` changes a binding live; a user binding overrides the preset and a removal
  removes; the editor shows a conflict;
- theme: switching recolors the shell; a user theme file is listed and applied; a broken file is reported;
- window bounds: saved and restored through the store, clamped to a display;
- crash: the report writer writes every section; the log file rotates;
- notifications: push, the bell count, dismiss, the update and build sources;
- drop: a dropped file opens as a document.

## Budget

- The palette opens within one frame and filters 2,000 commands in under 5 ms (`assert_budget`).
- A theme switch re-renders within 2 frames.
- Crash hook and log setup add under 5 ms at startup (`assert_budget` on the setup call).

## Exit criterion

`cargo fmt --check`, clippy with `-D warnings` and the tests of every touched crate green; the report records the
numbers, what was not built, and the integration lines for `CLAUDE.md`, `README.md` and the briefs index.

## Out of scope

- The editor font and per-editor zoom (brief 0067); Open File, Save All, Exit, Cut/Copy/Paste, Go To All, View Help,
  File > Recent (briefs 0068 to 0072, 0080): this brief registers none of them.
- The automatic Design/Debug layout switch (brief 0077).
- Migrating every view that keeps its own copy of the theme to the global (each is a one-line change in a file
  another brief owns; listed in the report). This brief switches the shell's own chrome live.
- Syntax highlighting colors in theme files (the editor keeps Visual Studio's Dark or Light by the theme's
  background).
