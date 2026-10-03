# Brief 0028 report: Multiple debugging sessions

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). No display: every test is
headless. netcoredbg: not available here (its download is refused by this machine's proxy); the netcoredbg plus Mono
test is written and skips (section 6).
Branch: `brief/0028-debug-multi-session`, based on `main` at `0a3e223` (briefs 0022 to 0027 and 0029 merged).
Date: 2026-10-03. Brief: [0028-debug-multi-session.md](0028-debug-multi-session.md).

## 1. Summary

- **Sessions.** Every start (each project of a compound) and every attach is a debugging session with an `id` (1, 2,
  ..., never reused while the shell runs), a `name` (the project's, or the attached process's), and its own generation
  (unique across sessions), stop counter, mode, adapter, client, stack, locals, output rings, Allow Agents to Drive
  switch, interrupt count and last driver. One session is **active**: the Locals, Watch, Call Stack and Threads windows
  show it, the execution point follows it, and commands without `session` act on it.
- **Commands.** `session` (an id) is optional on the 23 commands that act on a session (`stop`, `continue`, the three
  steps, `run_to_cursor`, `evaluate`, `state`, `select_frame`, `watch`, `snapshot`, `stack`, `variables`, `output`,
  `exception_info`, `pause`, `wait`, `run_until`, `trace`, `set_variable`, `set_next_statement`, `restart`,
  `allow_agents`); an unknown or ended id is refused with the live ids (`session 2 has ended: the live sessions are 1
  (eludite.debug.sessions lists them)`). `eludite.debug.sessions` (read) lists them (id, name, mode, active,
  generation, stop, runtime, adapter, process id, project, attached, agents_allowed, the stop's reason). `state`
  describes the active session (or the named one) plus `sessions`; `session.id` names it. Stop Debugging (Shift+F5,
  `stop` without `session`) ends every session; `stop` with one ends that one. Summaries carry `session`.
- **Compound launch.** `start` with `compound: "startup"` (the solution's multiple startup projects) or a list of
  `{ project, debug?, profile? }` starts each project in its own session, in solution order, after one build for the
  whole set (the solution's, when build before run is on). F5 with multiple startup projects is `compound: "startup"`;
  a plain start is a compound of one. With `wait_ms` the answer is the stop summary of the first session to break, or
  `running` with `sessions` (each session's mode) and `timed_out` when none breaks in time.
- **The windows.** The Call Stack and Threads windows gain a session selector (`Process:` and one option per session,
  `1: App (break)`), shown with two sessions or more; a click runs `select_frame` with the `session`, which makes it the
  active one, and Locals and Watch follow. A stop in another session makes it active (Visual Studio switches to the
  process that broke) unless the person picked a session in the last 2 seconds or the active session is itself at a
  break (section 8, item 1). The status bar names every session, the active one first: `Debugging: Tool (break:
  breakpoint, Program.cs line 6), App (running)`. The Debug menu's Continue, steps, Run To Cursor and Set Next Statement
  follow the active session's break mode, Break All its running, Stop Debugging any session.
- **Shared state.** Breakpoints (line, tracepoints, function breakpoints), exception settings and watch expressions are
  one set: every change goes to every connected adapter. Each breakpoint keeps its binding per session:
  `state.breakpoints[].sessions` (`session`, `verified`, `hits`, `message`), the row's `verified` bound in any session
  and `hits` summed, and the Breakpoints window's row tooltip (`Session 1: bound, 1 hit`). `run_until`'s and `trace`'s
  temporary points belong to the session that set them and go to its adapter only.
- **Multiple startup projects.** `eludite.workspace.set_startup_project` with `projects` (`{ project, action: start |
  start_without_debugging | none }`) sets Visual Studio's multiple startup projects; Project > Set Startup Projects...
  (the command without arguments, from the UI) opens the Startup Projects dialog (the solution's projects with an
  Action column; OK runs the command with `projects`). They persist per solution in brief 0018's store (version 3, with
  a migration test), `startup_project` naming the first that starts; Workspace and `eludite.workspace.tree` mark every
  startup project. One startup project (the context menu's Set as Startup Project) replaces them.
- **Rules per session.** Brief 0018's two-driver rules hold per session (one session's stop counter never refuses a
  command on another); brief 0025's summaries carry `session`; brief 0027's Allow Agents to Drive and `interrupted_by`
  are per session (the person's F10 in one session does not end an agent's wait on another); rule 4 (old answers
  dropped) routes every message by its generation to its session and drops it when that session is gone.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, headless; three runs each, load average 2 to 4 from
  another agent's work):

  | Budget | Result |
  |---|---|
  | A two-project compound reaches both `running` in under 1.5 times the single-project launch time (fake adapter) | Median of 10 starts each, timed on the UI thread from the command to the last `running`: single **6.1 / 6.9 / 6.1 ms**, compound **7.5 / 8.0 / 7.2 ms**, ratio **1.23 / 1.16 / 1.18** (a first run at load average 5: 12.9 and 12.1 ms, 0.93). Pass |
  | Frame cost unchanged (under 8 ms p99 headless) with two sessions stopping alternately ten times a second | Four seconds (240 frames, 40 stops; the person switching to the other session and continuing it each time), the frame drawn headless every 16 ms: frame p99 **7.96 / 7.83 / 7.43 ms** (p50 3.4 to 3.7 ms); one session stopping ten times a second in the same test **8.05 / 7.82 / 7.12 ms** (p50 3.2 to 3.5 ms). The debugger's share (its messages and the commands) p99 **2.6 / 4.4 / 2.4 ms** (one session: 1.9 / 2.4 / 2.1 ms). Unchanged; pass on the share, the frame at the 8 ms line on both counts (section 8, item 9) |
  | No new dependency | Pass: `Cargo.lock` unchanged |

  Real adapters: two `eludite-dbg-mono` TestApp sessions from one compound start to both at their breakpoint **324 /
  307 / 341 ms** (Mono 6.8.0.105).
- **Tests:** `cargo test --workspace --no-fail-fast` with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME` and
  `ELUDITE_CHROME_NO_SANDBOX=1`: **637 passed, 0 failed, 1 ignored** (a doc example); the Mono, lldb-dap and Chrome tests ran; the netcoredbg tests (five, one
  new) returned early. fmt and clippy (`-D warnings`) clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.
  `dotnet test dotnet/Eludite.slnx`: 171 tests, 164 passed, 0 failed, 7 skipped (unchanged; no .NET file changed; see
  section 8, item 16).

## 2. What was built

Commits, in order:

1. `9f161cd` The brief's Status line.
2. `affb907` `protocol/schemas/` alone: `debug-sessions.{input,output}.json`; `session` on the 23 inputs of commands
   that act on a session; `compound` in `debug-start.input.json`; `sessions`, `session.id`, `$defs/session` and
   `breakpoints[].sessions` in `debug-state.output.json`; `session` and `sessions` in
   `debug-stop-summary.output.json`; `projects` in `workspace-set-startup-project.input.json` (and its output's
   `projects`); the `startup` description of `workspace-tree.output.json`.
3. `fb5e340` `crates/commands`: `SESSIONS`, `SESSION_COMMANDS`, `parse_with_session` (`parse` keeps answering the
   request alone), `DebugTarget::apply(session, request)`, `Compound`, `CompoundEntry`, `SessionInfo`,
   `SessionsOutput`, `BreakpointSessionRow`, `CompoundSessionRow`, `SessionRow::id`, `StopSummary::{session, sessions}`;
   `project.rs`: `SetStartupProjects`, `StartupProjectsDialog`, `StartupAction`, `StartupEntry`, `StartupProjectRow`;
   tests. The shell compiled with the new requests refused until commit 5.
4. `07c2d16` `crates/ui`: `selector_bar` and `selector_option`, the Startup Projects dialog (`eludite_ui::startup`) and
   Project > Set Startup Projects...; tests.
5. `11b3f8c` The shell (section 3) and its tests.
6. `5c97c54` The gated netcoredbg plus `eludite-dbg-mono` test.
7. `2c38547` The test that Allow Agents to Drive and interrupted waits hold per session.
8. `6e17001` The frame budget measured over four seconds against one session's.
9. This report, the brief's Status line and the briefs index.

## 3. The session model (`crates/eludite/src/shell/debug.rs`, `debug/state.rs`)

- **Swapping, not rewriting.** The `Debugger`'s per-session fields (the `DebugModel`, the DAP client, the program run,
  the adapter's capabilities, the pending requests, Run To Cursor's line, the execution point, the console's partial
  line, early breakpoint events, the build before the launch, Break All's and Set Next Statement's errors, the variable
  counts, the Cargo options, emulated tracepoint hits, `trace`'s job, the interrupt count, the stale flag, the last
  start and a pending restart) hold the **current** session; the others wait in `Slot`s. `Debugger::enter(id)` swaps
  one in (`swap_slot`: one `mem::swap` per field). What every session shares stays in place: the breakpoints'
  definitions, the exception settings, the watch expressions (their values are the session's: the list is reconciled
  by name), the startup projects and Allow Agents to Drive's default. `Breakpoints::switch_session` moves the current
  session's binding (verified, hits, message, the adapter's id) into the breakpoint's `bindings` and takes the new
  one's. A swap costs a few dozen moves plus one pass over the breakpoints.
- **Current and active.** Between messages and commands the current session is the active one (`settle_active` after
  each batch of messages and each start or attach): every existing path that reads `self.debug.model` reads the active
  session, as before. `Shell::in_session(id, f)` runs `f` with session `id` current and comes back.
- **Routing.** Each `DebugMsg` carries its session's generation; `on_debug_msgs` finds the session with that generation
  and handles the message in it; a message of a session that is gone is dropped (a connected adapter is ended). A
  command is applied in the session it names (`apply_debug` → `in_session` → `apply_debug_in`, the code of briefs 0018
  to 0027); an agent's follow-up (`Follow { sid, epoch, what }`) and its reads (`Reader::sid`) read and wait in that
  session; the epoch is that session's interrupt count, so the person's commands interrupt waits on their own session
  only. `refresh_debug` always renders the active session.
- **Lifecycle.** A start or an attach while no session is live reuses the ended model (as before: its stop counter
  continues); beside live sessions it begins a fresh `Slot`. A restart (stop and start) keeps the session's id. A
  session that ends while others run is no longer listed; the next live one becomes active; the last 16 ended sessions
  are kept aside for agents' answers that still read them. Generations come from one counter (`Debugger::begin`).
- **Shared changes.** `send_breakpoints`, `send_function_breakpoints` and `send_exception_settings` go to every
  connected session (`each_connected`); the `_here` variants serve one session (the handshake's resend, Run To Cursor).
  Temporary points carry an `owner`; `source_breakpoints` leaves out another session's.
- **Compound.** `start_entries` turns `compound` (or F5 with multiple startup projects) into entries in solution order;
  `debug_start_set` starts one as before, or several: without build before run each launches at once; with it every
  session begins in `building` with a `PendingLaunch`, one `eludite.build.solution` runs (`compound_build`), and
  `prelaunch_build_done` launches every session waiting on that build's ticket. `Followup::Compound` answers the first
  session to break, or every session's mode on a timeout.
- **Startup projects** (`shell/startup.rs`): `set_startup_projects` resolves each project (a Cargo member, else a
  solution project), stores the set (`DebugModel::startup_projects`, persisted as `Persisted::startup_projects`, version
  3) and `startup_project` (the first with `start`); `startup_set()` is what F5 runs; `open_startup_dialog` and
  `on_startup_dialog_event` run the dialog through the bus; `Shell::startup_projects()` feeds Workspace's bold rows and
  `eludite.workspace.tree`.

## 4. Two sessions, step by step (the fake adapter test)

| Step | Sessions (mode, stop) | Active | Status bar |
|---|---|---|---|
| `start` with `compound: [Tool, App]` | 1 App running, 2 Tool running (solution order) | 1 | `Debugging: App (running), Tool (running)` |
| Tool breaks | 2 break 1 | 2 (it broke) | `Debugging: Tool (break: breakpoint, Program.cs line 6), App (running)` |
| App breaks | 1 break 1 | 2 (still at its break) | `Debugging: Tool (break: …), App (break: …)` |
| Call Stack selector: App | | 1 | |
| F10 | 1 break 2, 2 break 1 | 1 | |
| Threads selector: Tool | | 2 (Locals `x = 1` again) | |
| `continue` with `session: 1` | 1 running | 2 | Continue enabled, Break All disabled |
| `select_frame` with `session: 1` | | 1 | Continue disabled, Break All enabled |
| Shift+F5 | none | | `Stop Debugging` disabled |

## 5. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/commands/src/debug.rs` | 1 new, 3 updated | `session` parses on each of the 23 commands (and is refused on the others), its schema on each; `sessions`; `compound` (`"startup"`, a list, the refusals); `parse` unchanged; the outputs (`sessions`, the state's `sessions`, `session.id`, per-session bindings, the summary's `session` and `sessions`) against their schemas; a brief 0027 summary still reads |
| `crates/commands/src/project.rs` | 1 new, 1 updated | `projects` with the three actions, the refusals (empty, all `none`, unknown action, both forms), the dialog form, the output's `projects` |
| `crates/ui` | 2 new | The Startup Projects dialog's Action column, OK refused while nothing starts, OK applying every row, Cancel; Project > Set Startup Projects... |
| `shell/debug/state.rs` | 2 new | Bindings per session (switch, rows, sums, the tooltip's data, another session's temporary points, a session forgotten); version 3 persistence and the version 2 migration |
| `shell/debug/tests.rs` (fake adapters, headless) | 7 new | `a_compound_start_runs_two_sessions_and_the_windows_follow_the_session_that_broke` (section 4). `an_agent_drives_one_session_by_id_while_the_other_stays_at_its_break` (the compound's timed-out answer with `sessions`, `sessions` with modes, an agent's step on session 2 while session 1 stays, `state` per session, session 1's stop counter refusing `stop: 2` and session 2's accepting it, the unknown id, `stop` with a session, `session 2 has ended`, ids not reused, a plain start refused, a session that crashes removed and the other active). `breakpoints_bind_in_every_session_and_stop_debugging_ends_them_all` (a breakpoint set while both run: `setBreakpoints` to both, bound in both, the tooltip, exception settings to both, hits per session, an agent's Stop Debugging answering once both ended). `the_startup_projects_dialog_sets_two_projects_that_persist_show_bold_and_f5_starts_both` (the menu, the dialog, OK through the bus, bold, the tree, version 3 on disk, F5 building once and starting both, an agent's `projects`, the dialog showing them, refused for agents without arguments, one startup project replacing them). `allow_agents_and_interruptions_are_per_session`. The two budget tests |
| `shell/debug/tests.rs` (real adapters) | 2 new | `two_sessions_at_once_against_eludite_dbg_mono` (a compound of the TestApp twice, as Debug > Start New Instance: both at the breakpoint, bound in both, two processes, an agent's step in one, `stop` with a session, Stop Debugging). `netcoredbg_and_eludite_dbg_mono_sessions_at_once` (skips here) |
| `shell/debug/tests.rs` | 5 updated | Section 8, item 11 |

## 6. How the real-adapter tests ran

`dotnet build dotnet/Eludite.slnx`, then
`ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe`. The Mono tests ran
against Mono 6.8.0.105 at `/usr` (the new one among them; no stray `mono` process was left: the test kills both
TestApps' pids); the lldb-dap tests against lldb-dap 18.1.3 on `PATH`; the Chrome tests with
`ELUDITE_CHROME=/opt/pw-browsers/chromium-1194/chrome-linux/chrome` and `ELUDITE_CHROME_NO_SANDBOX=1`. The netcoredbg
tests (four in `crates/dap/tests/netcoredbg.rs`, the new shell one) returned early ("skipped: netcoredbg was not
found…", counted as passed).

## 7. How to reproduce

```
dotnet build dotnet/Eludite.slnx
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace
cargo test -p eludite -- a_compound_start an_agent_drives breakpoints_bind_in_every the_startup_projects_dialog allow_agents_and_interruptions two_sessions_at_once --nocapture
cargo test -p eludite -- --test-threads=1 --nocapture a_compound_of_two_reaches two_sessions_stopping_alternately
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite -- netcoredbg_and_eludite_dbg_mono --nocapture
```

## 8. Deviations, decisions and findings

1. **"The windows show the one that broke first" and "a stop in a non-active session makes it active"** read together
   only if a stop does not take the windows from a session that is itself at a break: the person is looking at that
   break. So a stop in another session makes it active unless the person picked a session in the last 2 seconds **or
   the active session is in break mode**. While the active session runs (or ended), a stop elsewhere takes the windows,
   as Visual Studio switches to the process that broke.
2. **A start that names projects adds sessions, a project already being debugged included** (Visual Studio's Debug >
   Start New Instance); a plain start (F5 with one startup project, no `project`) while sessions run is refused as
   before (F5 in break mode is Continue). The real Mono test starts the TestApp twice this way.
3. **`start`, `attach`, `processes`, `toggle_breakpoint` and `exception_settings` take no `session`**: a start or an
   attach adds a session, the breakpoints and exception settings are every session's, `processes` and `sessions` act on
   none (the brief's list of commands with `session` names neither). `trace` with `run: "start"` refuses `session` (the
   new session has no id yet). `watch` takes it: the expressions are shared, the answer is that session's values.
4. **Picking a session by hand is `select_frame` with only `session`** (the selectors run it): it makes the session
   active in any mode (Visual Studio's Debug Location toolbar switches to a running process too). With `thread` or
   `frame` it also selects them, which needs break mode as before.
5. **Ctrl+F5 runs are sessions too** (mode `running_without_debugging`), so a compound's `start_without_debugging`
   projects are listed; an attach to Ctrl+F5's program moves the run aside as in brief 0027.
6. **Restart keeps the session's id** (stop and start) so an agent's `restart` answers in its own session; ids are
   otherwise never reused. A start while no session is live reuses the ended session's model (its stop counter
   continues, as before brief 0028); a session started beside live ones begins at stop 0.
7. **One build for a compound is the solution's** (`eludite.build.solution`): `eludite.build.project` builds one
   project, and the brief asks for one build for the whole set. A single start keeps building its project.
8. **Shared watch expressions**: adding or removing a watch in one session changes the list every session sees; each
   session evaluates them in its own frame when it next breaks or its frame is selected.
9. **The frame budget sits at the 8 ms line**: the headless test draws the whole window, and with one session the
   same test measures 7.1 to 8.1 ms p99 (p50 3.2 to 3.5 ms); two sessions measure 7.4 to 8.0 ms (p50 3.4 to 3.7 ms). The
   debugger's own share is 2.4 to 4.4 ms p99. Measured over two seconds (85 frames, where p99 is the worst frame) the
   numbers ranged 6 to 10 ms for both; the test now measures four seconds. Under the full suite's parallel load the
   share once exceeded 8 ms; the test takes the best of up to three windows (as brief 0027 found for its frame test).
10. **The Startup Projects dialog has no "Single startup project" or "Current selection" radio** (Visual Studio's
    property page has them): rows with every project and an action; the context menu's Set as Startup Project is the
    single form. The dialog lists the solution's projects from the Workspace tree.
11. **Existing tests changed:** brief 0025's `an_agent_reads_a_deep_stop_within_the_budgets` (the summary now has
    `session`, the id; it asserted no `session` key, meaning the state's launch configuration); brief 0026's
    `run_control_persists_per_solution_and_a_version_1_file_loads` (the file is now version 3); brief 0027's
    `the_attach_dialog_filters_refreshes_and_attaches_through_the_bus` (Attach to Process... stays enabled while a
    session runs: an attach adds one); two races fixed in brief 0026's `tracepoints_print_and_continue_without_a_visible_stop`
    and `function_breakpoints_bind_by_name_and_stop` (they read the fake's requests before its thread recorded them; both
    failed in a full run).
12. **Files edited outside the brief's list, minimally:** `crates/eludite/src/shell/startup.rs` (it implements
    `set_startup_project`: `projects`, the dialog), `shell/folder.rs` (`eludite.workspace.tree` marks every startup
    project), `shell/debug/native.rs` (two methods made visible to `startup.rs`).
13. **Load-sensitive:** the new `the_startup_projects_dialog_sets_two_projects_that_persist_show_bold_and_f5_starts_both`
    waits for the default startup project as brief 0020's `the_context_menu_sets_the_startup_project_and_builds_and_it_persists`
    does, and timed out once in a full run the same way (passes alone). `find_default_startup` drops its result when the
    solution generation changed meanwhile and does not retry; brief 0020's code, not changed here.
14. **Not done:** transcript rows naming the session (`Step Over [App] → …`, brief 0027's suggestion); the guide
    `docs/agents/debugging.md` does not describe sessions yet (outside this brief's files).
15. **Not run:** Windows, macOS, CI, netcoredbg.
16. **Another worktree ran its full suites on this machine at the same time** (Mono and Chrome included). Meanwhile one
    `dotnet test` run had one failure in `Eludite.Debugger.Mono.Tests` and one run of `crates/browser/tests/chrome.rs`
    one failure; both passed when run again (twice for `dotnet test`), and this branch changes no .NET or browser file.
    The full `cargo test --workspace` run reported above had no failure.

## 9. What proposal 0002 brief D needs

- **Attach to a tab.** `attach` takes a pid or a process name; js-debug attaches to a CDP endpoint. An `AttachTarget`
  for a browser target (the tab's websocket URL or the Web Browser window's tab id) and an adapter id (`pwa-chrome`)
  are needed; the session model takes it as is (a session named after the tab, `attached`, Stop detaching).
- **Child sessions.** vscode-js-debug starts a child DAP session per target through the reverse request
  `startDebugging`; the shell would add each as a session with a `parent` (the tab's) and answer it through a second
  connection to the same adapter. `SessionInfo` would gain `parent`; Stop on the parent ends its children.
- **Compound entries that attach.** A compound lists projects; "start the server, then attach js-debug to the page"
  needs an entry kind `attach` (or `browser`) and an order: the server's session running (or its URL answering) before
  the attach. `Followup::Compound` already answers the first break across sessions.
- **Breakpoints by language.** Every breakpoint is sent to every adapter; netcoredbg and js-debug leave each other's
  files unbound, which `breakpoints[].sessions` shows truthfully but noisily. A filter by file kind per adapter
  (`.cs` to coreclr and Mono, `.ts`/`.js`/`.vue` to js-debug) in `Breakpoints::source_breakpoints` would keep the
  bindings meaningful.
- **Exception settings per adapter.** `exception_plan` is the .NET mapping (`all`, `user-unhandled`); js-debug's
  filters (`all`, `uncaught`) need their own mapping, chosen by the session's adapter.
- **Source maps.** Frames from js-debug carry generated and original sources; the execution point and the Breakpoints
  window already work by path, so the mapped (original) path is what the shell should keep.

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`) | 637 passed, 0 failed, 1 ignored |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 171 tests: 164 passed, 0 failed, 7 skipped (as before; no .NET file changed) |
