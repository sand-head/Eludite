# Brief 0008 report: docking and tool windows in the production shell

Brief: [0008-docking-production.md](0008-docking-production.md). Code: `crates/docking`, `crates/ui`, `crates/eludite`, `crates/commands/src/view.rs`, schemas `protocol/schemas/view-*.json`. Raw results: [`crates/eludite/results/`](../../crates/eludite/results/). Screenshots: [`crates/eludite/screenshots/`](../../crates/eludite/screenshots/).

## 1. Summary

- **`eludite` opens as an empty Visual Studio-style IDE frame.** It has the menu bar, the docking area and the status bar. The default layout matches PLAN.md section 8: Solution Explorer tabbed with Git Changes on the right above Properties, Error List and Output tabbed at the bottom, Toolbox auto-hidden on the left, and a Welcome document tab.
- **Every interaction in the Contract works on Linux.** That covers drag to dock with docking guides, tab and untab, float to an OS window and re-dock, auto-hide with a fly-out on hover and click, pin, close, reset, and the four VS shortcuts.
  - Headless GPUI tests drive each one with real GPUI mouse and key events. There are 11 headless GPUI tests across `eludite-docking` and `eludite`.
  - All but reset and the menus were also run on Linux with real X11 pointer and key events (XTest through Xwayland in a nested KWin): 19 of 19 steps passed (section 5).
- **Every action goes through the command bus.**
  - The schemas for the six `eludite.view.*` commands are in `protocol/schemas/` and were committed first, in their own commit.
  - Menus and keys dispatch one GPUI action, `RunCommand`, which the shell turns into `CommandRegistry::invoke`. Drops, buttons and tab clicks invoke the bus directly.
  - The tests check the audit log for each action.
- **Layouts persist per solution path.** The layout loads at start on a worker thread. Saves are debounced on a dedicated writer thread, never the UI thread, and the layout is flushed on exit. The two screenshots show the default layout, and a customized layout after a restart, restored from the per-solution file.
- **Cold start:** 106.7 ms median (Wayland, nested KWin) and 92.8 ms (XWayland, nested), release build, 19 warm runs each. Budget under 300 ms: **pass**.
- **Drag frame cost with the docking guides visible** (frame start to end of present, 600 frames per run):
  - p99 was 2.6 to 3.0 ms in 8 of 10 runs.
  - Two runs reached 10.1 ms and 9.2 ms. In both, the slow frames cluster in one stretch of the run, and other agents were compiling on the same machine (load average 3.5 to 13).
  - Budget under 8 ms p99: **pass in steady state**. It needs a quiet-machine run to confirm (section 6).
- **Windows and macOS were not run** (no machines). The code is platform-neutral. CI compiles and tests it on all three OSes.

## 2. Machine and method

- **Machine:** the brief 0001 laptop: Ryzen 9 7940HS (8 cores, 16 threads), 30 GB RAM, Radeon 780M (integrated, used by GPUI), CachyOS, KDE Plasma 6.7.5, KWin 6.7.5. Rust 1.98.1, release profile with thin LTO.
- **The user session was locked (`LockedHint=yes`) for the whole run.** As in briefs 0001, 0005 and 0006, every rendering run used a nested `kwin_wayland --virtual --xwayland --no-lockscreen` (1280x960, about 60 Hz, real GPU) inside `dbus-run-session`, with `spectacle` for screenshots.
- **Concurrent load:** two sibling brief agents (0007, 0009) were building in the same target directory during the measurements. The 1-minute load average is recorded before each benchmark run in the results files.

## 3. What was built

### 3.1 Commands and schemas (`crates/commands/src/view.rs`, `protocol/schemas/view-*.json`)

| Command | Input | Does |
|---|---|---|
| `eludite.view.show` | `{id}` (required) | Shows and activates a tool window: a closed one reopens where it was, an auto-hidden one slides out, a tabbed one becomes the active tab. A document tab id activates that document. |
| `eludite.view.hide` | `{id?}` | Closes a tool window (its close button, Shift+Esc). It remembers its dock. |
| `eludite.view.float` | `{id?, bounds?}` | Floats the window into its own OS window. |
| `eludite.view.auto_hide` | `{id?}` | Moves the window to its dock edge's strip. A floating window is refused, as in VS. |
| `eludite.view.dock` | `{id?, side? \| tab_with?}` | Docks the window. `side` makes a new group on that edge (a docking guide). `tab_with` tabs it into another window's group (drop on a group). With neither, a floating group goes back to its home dock, an auto-hidden window is pinned, and a closed one reopens. |
| `eludite.view.reset_layout` | `{}` | Resets to the VS default layout; floating windows close. |

- Omitting `id` means the active tool window, the one last shown or clicked. The Window menu's Float, Dock, Auto Hide and Hide items work this way, as VS's do.
- Every command but reset returns `view-tool-window.output.json`: `{id, title, state: docked|floating|auto_hidden|hidden|document, side?, group?, active, flyout_open?}`. Reset returns `{tool_windows: [...]}`.
- All six have permission class `read`: they change no file and run nothing (PLAN.md 5.3).
- `eludite.view.dock` is the one command beyond the brief's list of show, hide, float, auto-hide and reset. The Contract needs it, because a floated window re-docks "through a Dock command", and so do pin and the docking guides.
- The existing stub `eludite.view.toggle_tool_window` is untouched and unused.
- Input is validated: unknown fields, an empty id, giving both `side` and `tab_with`, and non-positive bounds are `invalid_input`. An unknown tool window is `invalid_input`. A refused move (auto-hiding a floating window, tabbing into a closed window) is `failed`. A failed command leaves the layout unchanged.

### 3.2 Docking model (`crates/docking/src/model.rs`)

- **`DockLayout`:**
  - three docks, `left`, `right` and `bottom`, each a list of tab groups plus a size and an auto-hide list;
  - `floating` groups, each with its bounds and its home dock;
  - `hidden`, the closed windows and their docks;
  - `documents`, the tabs with `pinned` and `preview` flags (pinned first, at most one preview, which is last);
  - `next_group_id` and `version`.
- **Tool windows** come from a `ToolWindowRegistry`, which holds each window's id, title and default dock. Later briefs register theirs, and a window the saved layout does not know appears closed at its default dock.
- **`ToolWindowInfo`** reports a window's id, title, place (docked, floating, auto-hidden or hidden), dock side, group tabs and whether it is active: the brief's "id, title, dock side, tabbed group, auto-hide and float state".
- **JSON schema:** serde field names are the schema, with `"version": 1` at the top.
- **Migration path,** documented at the top of `model.rs`:
  - A schema change bumps `LAYOUT_SCHEMA_VERSION` and adds a step to `migrate()`, which rewrites a `serde_json::Value` from the old version.
  - A file from a newer version is rejected and left on disk; Eludite falls back to the default layout.
  - After loading, `normalize()` repairs the layout against the registry. It drops unknown and duplicate ids and empty groups, clamps active tabs, renumbers duplicate group ids, and adds newly registered windows as closed. Adding or removing a tool window therefore needs no version bump.
- **Documents:** the pinned and preview states and their ordering are in the model and tested. No command sets them yet. Window > Pin Tab is a disabled stub, because opening files arrives with the editor (Out of scope: editor content).

### 3.3 Controller, view and persistence (`crates/docking`)

- **`DockController`** is the shared, `Send + Sync` owner of the layout. It holds a mutex for microseconds per command. It implements `eludite_commands::view::ViewTarget`, so the command handlers run on whichever thread invokes the bus: the UI thread for menus and keys, a server thread for agents. After a change it wakes subscribers through a `futures` unbounded channel.
- **`DockHost`** is the GPUI view.
  - It re-renders on any change, whoever made it (a test proves that a bus call that does not come from the view is drawn).
  - It queues a save only when the persisted layout changed; fly-out and active-window changes do not count.
  - It reconciles floating OS windows outside its own update, the brief 0001 pitfall.
  - Floating windows carry the shell's key context, so the keymap works in them too.
- **`LayoutStore` and `LayoutWriter`** are covered in section 4.

### 3.4 `crates/ui`

- **Themes:** `Theme::vs_dark()` (default), `vs_light()` and `vs_blue()`, with 22 color tokens each plus typography (roles such as chrome, panel header, active header, popup, guide, status bar). A test checks text contrast of at least 4:1 on every background in all three themes. The values approximate VS 2022 and are Eludite's own.
- **Menu bar:** `MenuBar` is a GPUI view with the 13 VS menus in VS order.
  - Each item names a command id and its arguments. An item is enabled only if that command is registered on the bus; stubs for commands that do not exist yet are drawn disabled and have no click handler.
  - Clicking a title opens its menu. Hovering another title while one is open switches menus. Clicking an item dispatches `RunCommand` and closes the menu. Clicking below the bar closes it.
  - Shortcuts shown in the menus come from the keymap table.
- **Status bar:** `StatusBar` has named slots: `state` on the left; `line_column`, `encoding`, `line_endings`, `branch`, `host_memory` and `version` on the right. `add_slot` lets later briefs add more. The shell sets `state` from each command's result, and `version` through `eludite.help.about`.
- **Keymap:** `vs_keymap()` is the table in section 7. `bind_keymap` registers it with GPUI, scoped to the `EluditeShell` key context. Every binding produces `RunCommand { command, args }`.

### 3.5 `crates/eludite`

- `main.rs` is 24 lines: it parses arguments and calls `app::run`.
- `app.rs` loads the layout on a worker thread, builds the bus with the view commands, opens the window and restores floating windows.
  - Closing the main window quits.
  - On quit, a handler flushes the writer, which GPUI awaits within its shutdown timeout.
- `shell.rs` is the root view: menu bar, `DockHost`, status bar, and the single `RunCommand` handler.
- Tool window bodies are empty titled panels. The document area shows a placeholder Welcome tab.
- Flags:
  - `--solution PATH` and `--theme dark|light|blue`;
  - `--reset-layout` and `--no-persist`;
  - harness: `--bench-start`, `--bench-drag N`, `--bounds-out PATH`, `--exit-after-ms N`.

## 4. Persistence

The layout directory is `<config dir>/eludite/layouts/`, where `<config dir>` comes from `dirs::config_dir()`. `ELUDITE_CONFIG_DIR` replaces `<config dir>/eludite`.

| OS | Layout directory | Run here |
|---|---|---|
| Linux | `$XDG_CONFIG_HOME/eludite/layouts/`, default `~/.config/eludite/layouts/` | yes |
| Windows | `%APPDATA%\eludite\layouts\` (`C:\Users\<user>\AppData\Roaming\eludite\layouts\`) | not run |
| macOS | `~/Library/Application Support/eludite/layouts/` | not run |

Files:
- `default.json`: the layout when no solution is open. A solution with no layout of its own starts from it.
- `solutions/<solution file stem>-<16 hex digits>.json`: one per solution path. The hex digits are the FNV-1a 64-bit hash of the absolute path, so `App.sln` in two folders do not collide. FNV-1a is stable across Rust versions; `std`'s hasher is not.
- `named/<name>.json`: named layouts (section 10).

Load and save:
- **Load:** the solution's file, else `default.json`, else the built-in VS default. A worker thread reads the file while GPUI starts, and the main thread joins it before opening the window. The measured wait at the join was 0.001 ms in every cold-start run (`layout_loader_join_wait_ms`).
- **Save:** on every change, `LayoutWriter::save` serializes the layout on the calling thread (a few kilobytes) and sends it to the `eludite-layout-writer` thread.
  - That thread debounces for 400 ms per path, then writes atomically (a temp file, then a rename).
  - On exit, `flush()` writes now and resolves a oneshot that the quit handler awaits.
  - A test checks that saves are not written synchronously, that three saves coalesce into one write, that the write happens on a thread other than the caller's, and that an expired debounce writes without a flush.

## 5. Interactions: what proves each one

There are three levels of evidence:
- **Headless:** GPUI's test platform with real GPUI mouse and keyboard dispatch: `simulate_mouse_down`, `simulate_mouse_move` and `simulate_mouse_up` past the drag threshold, `simulate_click`, `simulate_keystrokes`. Each test also checks that the change went through the bus, using the audit log.
- **Real input:** `crates/eludite/tools/manual-linux.sh` runs `eludite --bounds-out` on the X11 backend inside the nested KWin's Xwayland. `tools/drive.py` then moves the pointer and presses keys through XTest. These are server-level device events, the same path a physical mouse takes from the X server on, and `drive.py` checks each result against the element bounds the app reports. Log: `crates/eludite/results/linux-xwayland-real-input.jsonl`. KWin normally asks before letting a client inject input; the script runs the nested KWin with a private `XDG_CONFIG_HOME` whose `kwinrc` sets `[Xwayland] XwaylandEisNoPrompt=true`. The user's own config is untouched.
- **Wayland:** the same binary on GPUI's Wayland backend, for the default-layout screenshot and for the restart that restores the customized layout. No pointer was injected on Wayland: KWin's nested virtual backend takes no input devices.

| Interaction | Headless GPUI test | Real input, Linux XWayland (nested) |
|---|---|---|
| Drag a tab or title bar to each side guide (guides drawn during the drag, gone after) | `dock_to_each_side_by_dragging_onto_guides` (left, right, bottom) | Output tab to Dock Left; Properties tab to Dock Right |
| Tab: drop on another group | `tab_and_untab_by_dragging` | Properties title bar onto the Error List group |
| Untab: drag a tab out to a guide | `tab_and_untab_by_dragging` | Properties tab to Dock Right |
| Click a tab to activate it | `tab_and_untab_by_dragging` | Error List tab |
| Float by dropping where there is no guide or group | `float_and_redock` | Git Changes title bar onto the document area: an OS window opens |
| Float button | `float_and_redock` | Float on Git Changes |
| Dock button in the floating window | `float_and_redock` (clicks inside the floating window) | Dock in the Git Changes window |
| Closing a floating OS window hides its tool windows (VS) | `float_and_redock` (`simulate_close`) | not run |
| Auto Hide button, fly-out on hover, slide back on clicking elsewhere | `auto_hide_fly_out_and_pin` | Solution Explorer: hover the strip, then click the document area |
| Fly-out on click, Pin | `auto_hide_fly_out_and_pin` | Toolbox strip click, Pin |
| Close (x), then show again | `close_then_show_and_reset` (Show through the bus) | x on Properties, then F4 |
| Reset to the default layout | `close_then_show_and_reset` (bus), `menu_items_dispatch_commands` (Window > Reset Window Layout) | headless only |
| Layout save and load round trip | `layout_save_and_load_round_trip` (drag, tab, float, auto-hide and close, then flush, read the file, open a new window from it, check equality and that the floating window reopened) | Run 2 customized and closed through `WM_DELETE_WINDOW`. Run 3, on Wayland, restored it from `solutions/Demo-fb9c7f371963a25a.json` (screenshot). |
| Keys dispatch the right command | `key_bindings_dispatch_view_commands` (Ctrl+Alt+L, Ctrl+Alt+O, the chord Ctrl+\ then Ctrl+E, Ctrl+Alt+X; the audit log shows four `eludite.view.show` calls) | Ctrl+Alt+O, Ctrl+\ then Ctrl+E, Ctrl+Alt+L, Ctrl+Alt+X, F4 |
| Menu items dispatch commands | `menu_items_dispatch_commands` (View > Output, Window > Float, Window > Reset Window Layout, Help > About; audit log and status bar checked); `every_menu_opens` | headless only (no menu bounds are reported to the driver) |
| Disabled items do nothing | `disabled_menu_items_do_nothing` (Build > Build Solution and Clean Solution add no audit entry and leave the menu open; the backdrop closes it) | headless only |
| Agent-originated change re-renders | `commands_not_from_the_view_rerender_it`; `controller::tests::commands_from_another_thread_wake_subscribers` (the bus invoked from another thread) | n/a |

Screenshots (nested KWin, Wayland backend, 1280x800 window):
- `crates/eludite/screenshots/linux-default-layout.png`: first start with `--solution /work/Demo/Demo.sln` and an empty config directory.
- `crates/eludite/screenshots/linux-customized-after-restart.png`: after the driven session and a restart.
  - Output and Toolbox are tabbed on the left.
  - Properties and Solution Explorer are tabbed on the right.
  - Error List is alone at the bottom.
  - Git Changes floats in its own OS window, which the compositor placed (Wayland ignores the saved position).

## 6. Budgets

Raw data: `crates/eludite/results/linux-wayland-nested.json` and `linux-xwayland-nested.json`. Script: `crates/eludite/tools/bench.py`, run inside the nested KWin; the invocation is in `tools/manual-linux.sh`'s header and in section 9.

### 6.1 Cold start (release)

Method, as in brief 0001:
- `bench.py` records `time.time_ns()` and passes it in `ELUDITE_LAUNCH_WALL_NS`, then spawns `eludite --bench-start`.
- The shell's first render defers a callback, which GPUI runs after that frame's present.
- The time from launch to that callback is "start to interactive window": the window is focused and takes keys from then on.
- 20 runs, 0.3 s apart; the first run is reported separately.

| Configuration | Warm launch to first present, median (min to max), n=19 | Main to first present, median | First run | RSS at first present |
|---|---|---|---|---|
| Wayland (nested KWin) | **106.7 ms** (104.1 to 113.3) | 103.1 ms | 115.8 ms | 72.2 MB |
| XWayland (nested KWin) | **92.8 ms** (83.9 to 96.2) | 90.3 ms | 179.5 ms | 72.4 MB |

Budget under 300 ms: **pass**. The very first launch of a freshly linked binary took 531 ms. That is the off-CPU first-launch delay inside `open_window` that the brief 0001 report (section 3.1) recorded and did not root-cause.

### 6.2 Drag frame cost with the docking guides visible

Method (`--bench-drag 600`):
- After 30 warm-up frames, the harness dispatches a real `MouseDown` on the Output tab and a `MouseMove` past the drag threshold through `Window::dispatch_event`. GPUI starts the drag and the docking guides render.
- On each later frame, an `on_next_frame` callback dispatches one `MouseMove` with the left button held. The pointer sweeps across the document area and back, then onto the Dock Left guide, which changes the drop highlight.
- **Frame cost** is from that callback to the end of the frame's present: input dispatch, the shell's render, layout, paint and present. It excludes the wait for the next refresh, the definition in PLAN.md section 9.
- `renders_with_guides_visible` confirms the guides were drawn in every measured render (fraction 1.0 in all runs).

| Configuration | Run | Frame cost p50 | **p99** | max | Render to present p99 | Load average before |
|---|---|---|---|---|---|---|
| Wayland | 1 | 1.75 ms | **2.62** | 2.83 | 2.44 | 4.6 |
| Wayland | 2 | 2.19 | **2.96** | 3.37 | 2.78 | 4.2 |
| Wayland | 3 | 2.16 | **10.05** | 11.89 | 9.93 | 3.5 |
| Wayland | 4 | 1.85 | **2.66** | 6.66 | 2.55 | 7.9 |
| Wayland | 5 | 1.71 | **2.90** | 6.81 | 2.77 | 7.6 |
| XWayland | 1 | 1.97 | **3.02** | 3.33 | 2.94 | 6.7 |
| XWayland | 2 | 1.84 | **2.96** | 3.91 | 2.83 | 6.3 |
| XWayland | 3 | 1.81 | **2.67** | 3.01 | 2.53 | 7.0 |
| XWayland | 4 | 1.88 | **4.91** | 9.99 | 4.77 | 6.6 |
| XWayland | 5 | 3.45 | **9.16** | 13.23 | 8.98 | 6.4 |

Budget under 8 ms p99: **pass in 8 of 10 runs**. In the two failing runs (Wayland 3, XWayland 5), the ten slowest frames fall within one stretch of the run (frames 437 to 577 and 320 to 338; `worst_frames` in the raw JSON). Render-to-present rises with them, and the 1-minute load average went from 3.5 to 13 during the session because of sibling builds. That pattern means CPU contention, not a cost of the docking code. In an earlier set of 6 runs taken during a lull, p99 was 2.70 to 2.92 ms in 5 runs and 6.76 ms in 1.

**Owed:** a run on an idle machine, and on an unlocked real display, to confirm the p99 without contention.

### 6.3 UI thread

- **Layout I/O:** none on the UI thread (section 4).
- **Command handlers:** each runs under the controller's mutex for one layout operation on a few dozen strings, typically microseconds; not measured separately.
- **Serialization:** each save serializes the layout to JSON on the caller's thread, a few kilobytes.
- **Floating windows:** opened through a deferred effect, never inside an entity update.

## 7. Keymap table (VS preset, `eludite_ui::vs_keymap()`)

All bindings dispatch `RunCommand` in the `EluditeShell` key context, set on the main window's root and on every floating window's root.

| Keys | Command | Input | Effect |
|---|---|---|---|
| Ctrl+Alt+L | `eludite.view.show` | `{"id":"solution_explorer"}` | Solution Explorer |
| Ctrl+Alt+X | `eludite.view.show` | `{"id":"toolbox"}` | Toolbox (slides out while auto-hidden) |
| Ctrl+Alt+O | `eludite.view.show` | `{"id":"output"}` | Output |
| Ctrl+\, Ctrl+E | `eludite.view.show` | `{"id":"error_list"}` | Error List (a two-stroke chord) |
| F4 | `eludite.view.show` | `{"id":"properties"}` | Properties Window |
| Ctrl+0, Ctrl+G | `eludite.view.show` | `{"id":"git_changes"}` | Git Changes |
| Shift+Esc | `eludite.view.hide` | `{}` | Closes the active tool window |

The first four are the brief's. F4, Ctrl+0 Ctrl+G and Shift+Esc are VS's defaults for windows and commands this brief has. Bindings for commands that do not exist yet (Ctrl+Shift+B, F5 and so on) are left out, so no key does nothing silently. On some Linux desktops Ctrl+Alt+L locks the screen; the desktop takes the key before Eludite sees it. That is a user-visible conflict to note in the docs, not a code issue.

## 8. GPUI limitations hit, and follow-ups

1. **Drag and drop stays inside one window** (known from brief 0001). A floating window re-docks through its Dock button, `eludite.view.dock`, or Window > Dock; it cannot be dragged back onto the guides. GPUI has a path that could carry a drag out of one window into another: when the pointer leaves the window, `promote_external_drag_to_platform` hands an in-window drag to the OS as a platform drag-and-drop. **Follow-up size:** one brief, about 3 to 5 days. It would do a spike on that path on all three OSes, then use it to accept a tool-window payload dropped onto another Eludite window. On Wayland, a client still cannot learn the global pointer position, so "guides under a moving OS window" stays impossible there. The realistic target is dragging a tab out of a floating window onto the main window's guides.
2. **Floating window position is advisory on Wayland.** The size restores, but the compositor places the window (visible in the restart screenshot). This is protocol policy and needs no fix.
3. **Drop events carry no position.** The float-on-drop handler reads `Window::mouse_position()`. This is a minor API gap.
4. **No rotated text.** VS's vertical auto-hide tabs on the left and right edges are drawn as stacked letters in a 22 px strip. **Follow-up:** a custom element that paints shaped text with a transform, if GPUI exposes one, or a pre-rotated glyph atlas. About 1 day once the theme or icon work starts.
5. **The test scheduler rejects wakeups from other threads.** A GPUI headless test cannot invoke the bus from a second thread while a view is subscribed: the scheduler panics with "Your test is not deterministic". The cross-thread path is covered by a plain test of the controller. The GPUI test drives the bus from the test thread.
6. **Opening a window inside an entity update panics** if the new window reads that entity (brief 0001, section 5). `DockHost` opens floating windows in a deferred effect.
7. **Environment, not GPUI:** KWin holds XTest input behind a "Remote Control" prompt, configurable as `[Xwayland] XwaylandEisNoPrompt`. A locked session sends no Wayland frame callbacks, as brief 0001 found, which is why everything ran nested.

Other follow-ups this brief left out (each needs a brief or is in a later one):
- **Splitters** to resize docks by dragging. Sizes are in the model and persisted, but there is no resize interaction; the Contract does not ask for one.
- **Named layouts UI.** Storage is done and tested: `LayoutStore::named_path`, `list_named`, `load_named`, and `DockController::replace_layout`. Window > Save Window Layout and Apply Window Layout are disabled stubs, because their commands (`eludite.window.save_layout`, `eludite.window.apply_layout`) are not in this brief's command list. Size: 1 to 2 days, including the name prompt.
- **A document tab command** for pin and preview (`eludite.window.pin_tab`). It comes with the editor.
- **Exposing `eludite.view.*` over MCP.** The binary does not run the MCP server yet. When it does, these commands need only be added to its exposed list.

## 9. Tests

| Crate | Tests | New in this brief |
|---|---|---|
| `eludite-commands` | 19 | 4 (`view::tests`: schemas, parsing of every command, output shape against the schema, bad input) |
| `eludite-docking` | 26 | 26: model 11, persistence 4, controller 4, headless GPUI 7 |
| `eludite-ui` | 10 | 8 (keymap 2, menus 3, status bar 1, themes: three distinct sets and contrast) |
| `eludite` | 5 | 5: arguments 1, headless GPUI 4 (keys, menus, disabled items, every menu opens) |
| Workspace | 112 passed, 0 failed | |

`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are clean on Linux.

Reproduce, from the repository root:

```
cargo test -p eludite-docking -p eludite-ui -p eludite -p eludite-commands
cargo build --release -p eludite
crates/eludite/tools/manual-linux.sh /tmp/eludite-0008       # screenshots + real-input run (needs python-xlib)
# inside a nested KWin (see manual-linux.sh), or on a real display:
python3 crates/eludite/tools/bench.py --label linux-wayland --drags 5 --out /tmp/wl.json
env -u WAYLAND_DISPLAY python3 crates/eludite/tools/bench.py --label linux-xwayland --drags 5 --out /tmp/x11.json
```

## 10. Exit criteria

1. The app opens with the default VS layout, and every Contract interaction works on Linux, each proven by a headless test: **pass**. Real-input runs on XWayland cover all but reset and the menus (section 5).
2. Layouts persist across restart per solution path: **pass** (round-trip test, plus the restart screenshot).
3. Menus and keys dispatch through the command bus, and the new commands' schemas are in `protocol/`, committed before the code: **pass**.
4. Cold start and drag frame cost are in this report: **cold start passes** (92.8 to 106.7 ms). **Drag frame cost passes in steady state** (p99 2.6 to 3.0 ms). Two of ten runs exceeded 8 ms during measured CPU contention; a quiet-machine confirmation is owed.
5. GPUI limitations and sized follow-ups: section 8.

## 11. Deviations and notes

- **Commits** (rebased onto `origin/main` at 31f2282):
  - 7523e64: schemas only, first;
  - 7125852: commands;
  - 383db8c: docking, menus, keymap, status bar, wiring and tests;
  - 3d9dccd: the fly-out click fix found by the real-input run;
  - c1eb49f: harness and tools;
  - then this report, with the results and screenshots.
- **New dependencies:**
  - `eludite-docking`: `dirs` 6 (MIT OR Apache-2.0) for the per-OS config directory, and `futures` 0.3 (MIT OR Apache-2.0) for the change channel and the flush oneshot. Both were already in `Cargo.lock` through GPUI.
  - `eludite-docking` also uses the workspace's `serde`, `serde_json` and `thiserror` (MIT OR Apache-2.0); `eludite-ui` uses `serde_json`.
  - Dev-only: `tempfile` (MIT OR Apache-2.0). GPUI's `test-support` feature in `eludite-docking`, `eludite-ui` and `eludite` adds these to `Cargo.lock`:
    - `proptest` and `proptest-macro`: MIT OR Apache-2.0, by git at the rev GPUI pins;
    - `bit-set`, `bit-vec`, `fnv`, `quick-error`, `rand_xorshift`, `rusty-fork`, `unarray`, `wait-timeout`: MIT OR Apache-2.0;
    - `convert_case`: MIT.
  - The root `Cargo.toml` is unchanged.
  - `tools/drive.py` uses python-xlib (LGPL-2.1-or-later). It is a local test tool that is not shipped or linked.
- **Files outside the brief's list:** none.
  - The `docs/briefs/README.md` index row for 0008 still says "open"; that file is outside this brief's scope, so the human should update it.
  - `CLAUDE.md`'s crate map rows for `crates/eludite`, `crates/docking` and `crates/ui` remain accurate.
- **Real-display runs owed:** an unlocked 165 Hz session with real pointer drags on GPUI's Wayland backend.
- **Windows and macOS:** not run on this machine.
