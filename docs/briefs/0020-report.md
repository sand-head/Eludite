# Brief 0020 report: Integration pass after build and debug

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0020-integration-pass`, rebased onto `origin/main` at `e37776f` (main moved once during the work: the
fake debug adapter's crash-during-stall fix). Date: 2026-10-02.
Brief: [0020-integration-pass.md](0020-integration-pass.md).

## 1. Summary

- **The daily loop now behaves like Visual Studio's.** F5 and Ctrl+F5 build the startup project first through
  `eludite.build.project` and launch only when that build succeeds; a failed build leaves the Error List in front and
  says why in the status bar. The debuggee's output is the Output window's **Debug** source; the Debug Console
  window is gone, with a layout migration (version 2 to 3). Settings live in a layered, live-reloaded store
  (`settings.json` per user, `.eludite/settings.json` per solution) with `eludite.settings.get` and `set`, and
  **Tools > Options** is generated from the settings schema. The host answers **`eludite/build/status`** and the shell
  replays a running build after the host restarts. The Workspace window has its first **context menu** on projects:
  Build, Rebuild, Clean, Set as Startup Project (bold, kept per solution) and Open Containing Folder.
- **The manual run works** on `dotnet/Eludite.slnx` with the real `eludite-host`, Roslyn and netcoredbg 3.2.0-1092,
  driven with real XTest input in a nested KWin (section 7): Tools > Options opened from the menu, a check box
  toggled twice (each a `settings.json` write applied live); an `x` typed at the end of `HostRpcTarget.cs`, Ctrl+S,
  F5: "Build failed: 1 error, 0 warnings" and "Not started: the build failed (1 error, 0 warnings)", the Error List in
  front with CS0116 (Build + IntelliSense), nothing launched; Backspace, Ctrl+S, F5: built and launched,
  `Debugging: Eludite.Host (running)`, the Output window's Debug source with the program's first lines; Shift+F5 in
  45 ms. `dotnet/` was restored and is clean.
- **Budgets** (release build; nested KWin; 1-minute load average 7.3 and 8.4 before the two drives, from the release
  and .NET builds just before them; nothing else ran):

  | Budget | Result |
  |---|---|
  | F5 to launch adds no more than the build time plus 50 ms | Real host, real `dotnet build` of Eludite.Host (incremental): F5 to the launch thread started **1128.75 ms**, of which the build (request to finished) **1126.74 ms**: F5 added **2.00 ms** (first drive: 1090.74 / 1087.69 / **3.04 ms**). Headless, fake host: added 1.02 to 1.04 ms (3 runs). Pass |
  | Settings reload applies within 200 ms of the file change | User file replaced from outside the IDE, write to the shell's "settings applied" trace: **1, 81, 1, 80, 99, 98, 96, 92, 91, 91 ms** (max 99 ms; the poll is 100 ms; first drive max 99 ms). The shell's own part, change seen to applied: **0.05 to 0.29 ms**. Headless (20 ms poll): p50 22.4 to 22.8 ms, max 23.9 ms. Pass |

- **Tests:** `cargo test --workspace` **440 passed**, 0 failed, 1 ignored (a doc example); 421 on `main` before the
  brief, so 19 new (several existing tests grew new assertions too). `dotnet test dotnet/Eludite.slnx` **153 total,
  148 passed, 5 skipped** (150/145/5 on `main`; the 5 skips need the legacy corpus or a generated solution).
  `dotnet build` 0 warnings. fmt and clippy (`-D warnings`) clean.
- **No new dependency.** The settings store polls with `std` (no file-watcher crate); the dialog and the context menu
  are GPUI and `eludite-ui`.

## 2. What was built, by commit

Each commit passes `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and
`cargo test --workspace`; the host commits also `dotnet build` (0 warnings) and `dotnet test`.

1. `dd01c60` **Schemas.** `host/build-status.json`; `settings.json` (the settings file schema); command schemas
   `settings-get`, `settings-set`, `workspace-set-startup-project`, `workspace-open-containing-folder`,
   `tools-options` (input and output); `debug` added to the Output sources; `building` added to the debug modes and
   `build` to `eludite.debug.start`; `startup` on `eludite.workspace.tree`'s projects; host-rpc.md's "Status" paragraph.
   The protocol crate's method list gained `eludite/build/status` in the same commit, because its schema tests require
   every host schema file to name a known method (no other code).
2. `09843e5` **Host:** `eludite/build/status` (section 3), with three tests.
3. `c022e09` **Protocol, client, shell:** the typed request, the fake host's status and restart behavior, the session
   asking after a restart, the shell's replay (section 3).
4. `1b6dfab` **Settings store** and `eludite.settings.get`/`set`; build on save, the tool paths and the agents registry
   moved into it (section 4).
5. `9e7b00f` **Tools > Options** generated from the schema (section 5).
6. `ac55617` **F5 and Ctrl+F5 build first** (section 6).
7. `aaaf2c5` **The Debug output source**, the Debug Console retired, layout version 3 (section 6).
8. `c4304f7` **Workspace context menu** and the startup project (section 8).
9. `ac46ece` Headless tests for the Debug source and a saved version 2 layout loading.
10. `db931cb` The manual-run driver (`tools/integration-linux.sh`, `tools/integration.py`), and bounds probes for the menu
    bar and the Options dialog (`--bounds-out`) so real input can click them.
11. `e90df75` The run's screenshots and `results/linux-integration.json`.
12. This report and the brief's Status line.

## 3. `eludite/build/status` and the replay

- **Host** (`Build/BuildService.cs`, `Build/OutputHistory.cs`): every output chunk is recorded before it is sent;
  `eludite/build/status` answers `{ running, last? }`: the running build with its start result's members,
  `elapsedMs`, the last progress, and `output: { firstSeq, nextSeq, text, truncated }` (the last **8 MiB** of whole
  chunks); `last` is the last finished build's result and summary without its diagnostics. It never fails and is
  allowed before `eludite/host/initialize`.
- **Shell:** when the host exits mid-build the build is kept (status "recovering the build…") until the restarted host
  answers. A build that still runs is **replayed**: the Build pane is refilled with `output.text`, then only chunks with
  `seq >= nextSeq` are applied, so a chunk sent before the reply is neither lost nor shown twice. A build that did not
  survive ends as "canceled: eludite-host restarted and the build ended", or with the host's `last` result if it
  finished meanwhile; a waiting agent's `eludite.build.*` call gets that result.
- **Today's real path:** the host is the shell's child over stdio, so a host restart is a new process and its builds
  are gone: the shell reports the build ended. The replay serves a host the shell reattaches to (D7's remote hosts) and
  is tested against the fake host, whose build can survive a reconnection (`set_build_survives_restart`).

## 4. Settings

**Files and paths.** One JSON object of dotted keys per file, merged user then solution (solution wins), over the
schema's defaults; an environment variable named by the schema overrides both while set.

| File | Linux | Windows | macOS |
|---|---|---|---|
| User | `$XDG_CONFIG_HOME/eludite/settings.json` (default `~/.config/eludite/settings.json`) | `%APPDATA%\eludite\settings.json` | `~/Library/Application Support/eludite/settings.json` |
| Solution | `.eludite/settings.json` in the solution's folder, or in the open folder | same | same |

`ELUDITE_CONFIG_DIR` replaces the `eludite` folder (as for layouts). Documented in `protocol/schemas/settings.json`,
`crates/eludite/src/settings.rs` and `eludite --help`. Windows and macOS paths come from `dirs::config_dir()` and were
not run.

**The settings** (`protocol/schemas/settings.json`; each has a default, a description, an Options section and label):

| Key | Default | Was | Override |
|---|---|---|---|
| `keyboard.preset` | `visualStudio` (the only preset) | fixed in code | |
| `build.beforeRun` | `true` | new | |
| `build.onSave` | `false` | `ELUDITE_BUILD_ON_SAVE=1` | `ELUDITE_BUILD_ON_SAVE` |
| `build.showOutputOnStart` | `true` | always | |
| `build.showErrorListOnFailure` | `true` | always | |
| `build.cargoPath` | `""` (PATH) | `ELUDITE_CARGO` | `ELUDITE_CARGO` |
| `debugger.netcoredbgPath` | `""` (search) | `ELUDITE_NETCOREDBG` | `ELUDITE_NETCOREDBG` |
| `languageServers.rustAnalyzerPath` | `""` (search) | `ELUDITE_RUST_ANALYZER` | `ELUDITE_RUST_ANALYZER` |
| `agents.default` | `""` | `agents.json` `default` | |
| `agents.claudeCodeAdapterPath` | `""` (search) | `ELUDITE_CLAUDE_ACP` | `ELUDITE_CLAUDE_ACP` |
| `agents.custom` | `[]` | `agents.json` `agents` | (`agents.json` is read when no settings file has the key) |

**Store** (`crates/eludite/src/settings.rs`): an `eludite-settings` thread reads the files (never the UI thread),
stats them every 100 ms and signals the shell when one changed; `eludite.settings.set` changes the in-memory layer at
once, signals, and hands the file (other keys kept, sorted) to the same thread to write. A file that does not parse is
ignored with its error reported by `eludite.settings.get`; unknown keys and values of the wrong type are ignored and
reported. **Applying** (`shell/settings.rs`): build on save and the two "show" switches, the cargo path, the netcoredbg
path (searched where `ELUDITE_NETCOREDBG` was), the rust-analyzer path (tried first unless the variable is set; the
next server started), and the agents registry (searched again off the UI thread when its settings change; a newly
chosen default is selected). **Commands:** `eludite.settings.get` (read class) and `eludite.settings.set` (edit
class, both agent-visible) run on the calling thread against the store.

## 5. The Options dialog

Generated from the schema: the page tree is the schema's `x-eludite-sections` (Environment ▸ Keyboard, Projects and
Solutions ▸ Build and Run, Debugging ▸ General, Text Editor ▸ Rust, Agents); each setting gets the editor its type
asks for: a check box, a button per enum value, a text box (click, type, Enter; Escape cancels), a text box with
Browse... (the system file dialog) for paths, and the count of `agents.custom` entries (edited in the file). Each
row shows the description and, when an environment variable or the solution's file provides the value, says so.
Every change runs `eludite.settings.set` through the bus (audited as any command) in the user file and applies at once;
a file edited elsewhere shows in the open dialog. `eludite.tools.options` (Tools > Options..., not agent-visible) opens
it, optionally on a page. Screenshot: `crates/eludite/screenshots/linux-integration-options.png`.

## 6. F5 gating and the Debug source

- **How F5 gates on the build** (`shell/debug.rs`): `eludite.debug.start` (F5; Ctrl+F5 with `debug: false`) with the
  setting `build.beforeRun` on (or `build: true` in the call) and a solution open puts the session in mode
  **`building`** (a new generation), resolves the project to start off the UI thread (the hint, else Set as Startup
  Project's, else the first executable project), and runs **`eludite.build.project`** for it through the bus, as Build
  > Build Project does: the build streams into the Output window's Build source with the usual status bar and Error
  List. The build's ticket is remembered; when that build's `eludite/build/finished` arrives: **succeeded** → the launch
  starts in the same session (mode `launching`, the Building line kept in the Debug source); **failed** → the session
  ends with "Not started: the build failed (N errors, M warnings)" in the status bar and `message`, and the Error List
  comes forward (whatever `build.showErrorListOnFailure` says); **refused, canceled or lost with the host** → ended with
  the reason. A second F5 meanwhile is refused (not queued); Shift+F5 cancels the build and the start. MSBuild's
  incremental build makes an up-to-date project cost a check, so F5 always builds rather than the shell guessing
  staleness. An agent's `eludite.debug.start` waits up to `wait_ms` (≤ 30 s) and may answer with mode `building`.
- **Debug source:** the adapter's `output` events, the program's stdout and stderr under Ctrl+F5, and the debugger's own
  lines go to the Output window's **Debug** source (cleared when a session starts, selected by F5 and Ctrl+F5; Ctrl+F5
  also brings Output forward). `eludite.debug.state`'s `console` keeps the last lines for agents, and
  `eludite.output.show { source: "debug" }` reads it.
- **Layout migration** (`crates/docking/src/model.rs`): `LAYOUT_SCHEMA_VERSION` 2 → 3; the step retires `debug_console`:
  where Output is not placed, Output takes the Debug Console's place (and leaves the closed list); otherwise the Debug
  Console is removed from tab lists, auto-hide strips, floating groups and the closed list, keeping each group's active
  tab on a neighbor. Tests: a checked-in version 2 layout (Debug Console active in a second bottom group) migrated
  both ways, auto-hidden, and a saved version 2 file loading through `LayoutStore`. The window id is gone from the
  registry and the Debug > Windows menu (which now offers Output).

## 7. Manual run (Linux) and screenshots

`PYTHONDONTWRITEBYTECODE=1 ELUDITE_NETCOREDBG=… crates/eludite/tools/integration-linux.sh OUT_DIR`: nested
`kwin_wayland --virtual` 1280×960, X11 backend on its Xwayland, real XTest input, release build, the host run from a
copy (F5 builds Eludite.Host and rewrites its `bin/`), `ELUDITE_TRACE_LSP=1`, a fresh `ELUDITE_CONFIG_DIR`. Results:
[`crates/eludite/results/linux-integration.json`](../../crates/eludite/results/linux-integration.json) (both drives).

- Solution loaded (8 projects, 2.5 s). Tools menu clicked, then Options...; the Build and Run page;
  [`linux-integration-options.png`](../../crates/eludite/screenshots/linux-integration-options.png). "Build the
  project after saving one of its files" clicked on and off: `settings.json` became `{"build.onSave": true}` then
  `{"build.onSave": false}`, each applied (the trace 0.05 to 0.29 ms after the change was seen). OK closed it.
- The user file rewritten ten times from the driver: write to applied 1 to 99 ms (section 1).
- `x` at the end of `HostRpcTarget.cs`, Ctrl+S, F5: "debug start: building the startup project first", the build of
  `Eludite.Host.csproj` requested, "Build failed: 1 error, 0 warnings", "Not started: the build failed (1 error, 0
  warnings)" 881 ms after F5, no adapter started;
  [`linux-integration-f5-build-failed.png`](../../crates/eludite/screenshots/linux-integration-f5-build-failed.png)
  (Error List in front: CS0116 at 163:1 from Build + IntelliSense; both status bar slots).
- Backspace, Ctrl+S, F5: build succeeded, launched 2.00 ms after the build's result, netcoredbg running the program
  1160 ms after F5;
  [`linux-integration-f5-launched.png`](../../crates/eludite/screenshots/linux-integration-f5-launched.png) (the
  Output window's Debug source: "Building the startup project before starting…", "Starting debugging the startup
  project…", the program's first lines; status `Build succeeded`, `Debugging: Eludite.Host (running)`). Shift+F5:
  ended in 45 ms.
- `git status -- dotnet/` empty afterwards (the script also runs `git checkout -- dotnet/`).
- The first drive found a dialog bug: the footer's long settings paths pushed OK out of the dialog, so the click missed
  it; the footer now wraps (fixed before the second drive, which is the one reported).

## 8. Workspace context menu and the startup project

Right-click on a .NET project or a Cargo package: Build, Rebuild, Clean (`eludite.build.project` with the project
file or `Cargo.toml` and the target), Set as Startup Project (`eludite.workspace.set_startup_project`; .NET projects
only, disabled on Cargo packages until Rust can be debugged), Open Containing Folder
(`eludite.workspace.open_containing_folder`: `xdg-open`, Explorer or Finder on a thread of its own; execute class, not
agent-visible). The startup project is kept per solution and per user, as Visual Studio keeps it in the `.suo`: in the
solution's file under `<config dir>/eludite/breakpoints/solutions/` beside its breakpoints (`startup_project`);
without one it is the first executable project, found off the UI thread when the tree arrives. Workspace draws it bold
and `eludite.workspace.tree` marks it `startup: true`; F5 starts it.

## 9. Headless tests (Proving test)

| Behavior | Test |
|---|---|
| F5 builds then launches (the startup project, through `eludite.build.project`; refused second F5; setting off; per-call `build`; Shift+F5 cancels) | `shell::debug::tests::f5_builds_the_startup_project_then_launches` |
| F5 on a failing build stops with the Error List forward (and Ctrl+F5 too) | `shell::debug::tests::f5_on_a_failing_build_stops_with_the_error_list_forward` |
| Debug source receives adapter output (partial lines, selected, cleared per session, `eludite.output.show`) | `shell::debug::tests::the_debug_source_receives_the_adapters_output`; also `f5_breaks_…` and `ctrl_f5_runs_…` |
| Settings load, merge and live-reload (user, solution wins, file edits, a bad file) | `shell::settings_tests::settings_load_merge_and_reload_live`; `settings::tests` (2, environment overrides) |
| Settings through the bus (agent thread, file written keeping keys, null removes, scopes, class) | `shell::settings_tests::agents_get_and_set_settings_through_the_bus` |
| Agents registry follows its settings | `shell::settings_tests::the_agents_registry_follows_its_settings` |
| The Options dialog edits a setting through the bus (every page and editor generated, check box, text, Browse..., enum, OK) | `shell::settings_tests::the_options_dialog_is_generated_from_the_schema_and_edits_through_the_bus` |
| The context menu sets the startup project and it persists (bold, `startup` in the tree, F5 runs it, Build/Rebuild/Clean, Open Containing Folder, an agent by name, restored after switching solutions) | `shell::debug::tests::the_context_menu_sets_the_startup_project_and_builds_and_it_persists` |
| Build status replay after a simulated host restart (replay, no duplicates, missed output, a build that died) | `shell::build_tests::build_status_replays_a_running_build_after_a_host_restart`; `crates/lsp/tests/in_process.rs::build_status_replays_the_running_build_across_a_restart` |
| Layout migration | `docking::model::tests::version_2_layouts_retire_the_debug_console`, `persist::tests::a_saved_version_2_layout_loads_without_the_debug_console` |
| Schemas, parsing, validation | `eludite-protocol` `build_status_conforms_to_its_schema`; `eludite-commands` `settings::tests` (3), `project::tests` (1), `build`/`debug` parsing updated |
| Host | `BuildServiceTests`: `Status_ReturnsTheRunningBuildWithItsOutputSoFar_ThenTheLastResult` (real MSBuild), `Status_IsAnsweredBeforeInitialize_WithNoBuild`, `OutputHistory_KeepsTheLastWholeChunks_WithinItsLimit` |

## 10. Gaps and findings

- **F5 does not save unsaved documents first** (Visual Studio's "Save all changes before build" is on by default). The
  manual run saved with Ctrl+S. Small follow-up: a `build.saveBeforeBuild` setting, on by default, saving through
  `eludite.editor.save`.
- **The replay's real use needs a reattachable host.** With the host as a stdio child, a restart always ends the build
  (reported correctly); the replay path is exercised against the fake host and the C# tests only (section 3).
- **The policy file was not moved.** The brief's contract names the agents registry entries; `.eludite/agents-policy.json`
  stays where brief 0016 put it (per solution, committable, written by Always Allow, with its own schema).
- **The Options dialog** applies each change at once (no Cancel batching as in Visual Studio), writes to the user file
  only (the solution scope is through `eludite.settings.set`), edits text with a minimal line editor (no caret movement
  or selection), and shows `agents.custom` as a count. Settings files are plain JSON (no comments). PLAN.md 4.12's
  middle "workspace" layer is the open folder's `.eludite/settings.json`; there is no third layer.
- **`keyboard.preset` has one value**, so there is nothing to switch live yet; it is in the store and the dialog.
- **The Debug Console's expression box is gone** with the window; expressions are evaluated in Watch 1 or with
  `eludite.debug.evaluate` (whose `repl` results go to the Debug source). Visual Studio's Immediate window (PLAN.md 4.5)
  is the proper successor: about 1 day on the existing evaluate path.
- **Context menu:** projects only (no file or folder menus), no keyboard access (Shift+F10 or the menu key), and Build
  items are not disabled while a build runs (the start is refused with a message, as from the Build menu).
- **The startup project is stored in the breakpoints store** (`breakpoints/solutions/…json`); the folder name is
  historical. Agents' `eludite.debug.start` may answer with mode `building` when the build outlasts `wait_ms`.
- **Not in my files, so not updated:** an ADR for the settings store (a structural decision under CLAUDE.md's
  definition of done; `docs/adr/` was not in scope), README.md and CLAUDE.md (the settings file, `integration-linux.sh`),
  and `docs/briefs/README.md`'s index row.
- **Scope notes:** the schema commit also carries the protocol crate's one-line method list entry (its tests require
  every host schema to name a known method); two command schemas beyond the brief's list (`eludite.tools.options`,
  `eludite.workspace.open_containing_folder`) exist because invariant 3 makes every user-visible action a command.
- **A flaky test** (`crates/dap/tests/client.rs::adapter_crash_fails_pending_requests_and_reports_closed`) failed once
  in a full run before the rebase; `origin/main`'s `e37776f` fixes it, and the branch is rebased onto it.
- **Windows and macOS not run:** settings paths, `explorer`/`open` for Open Containing Folder, and the dialog's
  Browse... are untested there.

## 11. Sizing the Phase 1 close-out

What PLAN.md's Phase 1 still lists after this brief, and the platform passes:

| Item | What it takes | Size |
|---|---|---|
| **Test Explorer via MTP** | Schemas for `eludite/test/*` (discover, run, debug, results and output streaming, cancel); in the host, Microsoft.Testing.Platform's server mode (JSON-RPC to the test app, as `dotnet test` runs it), with VSTest only where a project has not migrated (PLAN.md 4.6) on the existing `Eludite.TestBridge` stub; in the shell, the Test Explorer window (hierarchy, filter, run and debug at any node, live results, per-test output), failures as Error List rows, Debug Test through the existing DAP path (netcoredbg attached to the test host), `eludite.test.*` commands for agents | about **2.5 agent-weeks** (host 1, window and commands 1, debugging a test and the agent path 0.5) |
| **Git beyond status** | `crates/git` has status and branch only. The Git Changes window (stage, unstage, commit with message, the review diff view reused for file diffs), branches (switch, create), fetch/pull/push (libgit2 with credential helpers, or the user's `git` for network operations), gutter change markers, `eludite.git.*` commands (commit and push in the dangerous class) | about **2 agent-weeks** |
| **Integrated terminal** | `crates/terminal` has only a launch configuration. A PTY (`portable-pty`, MIT), a VT engine (`alacritty_terminal`, Apache-2.0, or `vte`, MIT/Apache-2.0; Zed's terminal crates are excluded by invariant 7), a GPUI grid view with selection and scrollback, the Terminal tool window (View > Terminal), `eludite.terminal.*` commands with output readable by agents | about **2 agent-weeks** |
| **Windows pass** | Briefs 0007 to 0020 on a Windows machine (`windows-checklist.md`): Build Tools MSBuild and `taskkill /T /F`, `netcoredbg.exe`, rust-analyzer's `fetch.ps1`, settings and layout paths under `%APPDATA%`, Explorer for Open Containing Folder, the CI Windows jobs; then fixes | about **1 week** with a machine (runs 3 days, fixes 2), plus **3 to 5 days** for macOS |
| **Rust debugging via CodeLLDB** | The DAP client and windows are adapter-neutral (brief 0018). An adapter registry (CodeLLDB discovery and a pinned fetch script, MIT; `lldb-dap` as an alternative), a launch configuration from the package's binary target (cargo's JSON artifact message gives the executable), Set as Startup Project on Cargo packages, F5's build gate through the Cargo build path, and LLDB's variable formatting for Rust types in Locals and Watch | about **1 agent-week** |

Small follow-ups found here: save before build (0.5 day), the Immediate window (1 day), file and folder context menus
with keyboard access (1 day), an ADR for settings (0.5 day), README/CLAUDE.md updates (0.5 day). With the items above,
the Phase 1 close-out is about **9 to 10 agent-weeks** of work that splits into five independent briefs (they share
only the command bus and the docking registry), so with parallel agents it is two to three calendar weeks of review.

## 12. How to reproduce

```
cargo test --workspace
dotnet build dotnet/Eludite.slnx && dotnet test dotnet/Eludite.slnx
cargo build --release -p eludite
PYTHONDONTWRITEBYTECODE=1 ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) \
  crates/eludite/tools/integration-linux.sh OUT_DIR >OUT_DIR.log 2>&1
cargo test -p eludite -- --nocapture settings_load_merge f5_builds_the_startup   # the headless timings
```

The script needs `dotnet/` clean in git and restores it at the end. Results go to `OUT_DIR/drive.json`,
`OUT_DIR/loadavg.txt` and the PNGs.
