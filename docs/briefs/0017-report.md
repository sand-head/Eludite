# Brief 0017 report: Build with Output and Error List

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0017-build`, on `origin/main` at `6ac4171` (main did not move during the work). Date: 2026-10-02.
Brief: [0017-build.md](0017-build.md).

## 1. Summary

- **The manual flow works** on `dotnet/Eludite.slnx` with the real `eludite-host` and Roslyn, driven with real XTest
  keys and clicks in a nested KWin (section 6): Ctrl+Shift+B brought the Output window forward with the host's
  first line 58 ms after the key, streamed the `dotnet build` log, and the status bar went from "Building: 0 of 8
  projects" to "Build succeeded" (all 8 projects, 3.4 s, incremental); the Error List had no build rows. Then `x`
  typed at the end of `HostRpcTarget.cs`, Ctrl+S, Ctrl+Shift+B: "Build failed: 1 error, 0 warnings", the Error List
  came forward with the build's CS0116 row at 159:1, flagged **Build + IntelliSense** (the live diagnostic at the
  same file, position and code: shown once), and a double-click put the caret on line 159, column 1. `dotnet/` was
  restored with `git checkout -- dotnet/`.
- **Budgets** (release build, nested KWin at 60 Hz, Wayland backend; load average 1.0 to 2.0 during the runs, the
  1-minute figure logged before each run):

  | Budget | Result |
  |---|---|
  | Ctrl+Shift+B to first Output line < 100 ms | Key to the first line appended **p50 17.2 to 18.1 ms**, worst 49.6 ms; to the end of the present of the frame that shows it p50 25.0 to 32.1 ms, worst 69.2 ms (15 builds in 3 runs). Pass |
  | 100k Output lines over 10 s, frame cost < 8 ms p99 | **p99 3.70, 3.73 and 7.59 ms** in three runs (p50 2.4 to 2.6 ms, max 10.9 to 13.4 ms; 549 to 553 frames per run); appending a 160-line chunk p99 0.09 to 0.13 ms; RSS 105 MiB with 100k lines. Pass |
  | Build finish to Error List rows < 200 ms | The pump receiving `eludite/build/finished` to the rows set **p50 1.3 to 1.9 ms**, worst 2.4 ms; to the present showing them p50 9.0 to 11.6 ms, worst 16.4 ms (15 builds). Pass |

- **Tests:** `cargo test --workspace` 365 passed, 0 failed, 1 ignored (a doc example; 347 on main). `dotnet test
  dotnet/Eludite.slnx` 150 total, 145 passed, 5 skipped (each needs the legacy corpus or a generated solution not on
  this machine; one of them is the brief's legacy corpus build on Mono, which skips cleanly). `dotnet build` zero
  warnings. fmt and clippy (`-D warnings`) clean.
- **No new dependency.** The binary log is read with `Microsoft.Build` (MIT), already a host dependency (17.11.48,
  compile-time only, `ExcludeAssets="runtime"`; the SDK's copy is loaded at run time by `Microsoft.Build.Locator`,
  MIT). `Microsoft.Build.Logging.StructuredLogger` was not needed.

## 2. What was built

Commits, in order (each passes fmt, clippy and the workspace tests):

1. `d1c735d` `protocol/schemas/` alone: `host/build-start.json`, `build-cancel.json`, `build-output.json`,
   `build-progress.json`, `build-finished.json`, error -32010 in `host/errors.json`; the command schemas
   `build-solution`, `build-project`, `build-rebuild`, `build-clean`, `build-cancel` (input and output),
   `build-result.output.json`, `output-show`, `output-clear`; `diagnostics-list` gained `source` (input filter and
   output field); `host-rpc.md` "Build".
2. `f035460` The messages typed in `eludite-protocol` (`host.rs`) and `eludite-lsp` (client notifications), and
   scripted builds in the fake host.
3. `0367b52` The host's build service (section 4).
4. `8a46b09` Ctrl+Shift+B and F6 (Build Solution), Shift+F6 (Build Project); Build menu project items.
5. `8a36c1b` The shell: the Output window, build rows in the Error List, the status bar slot, the Debug/Release and
   platform dropdowns, the build and output commands on the bus.
6. `d173723` The headless tests (section 5).
7. `1d85ae6` A real build through the real host from the Rust client (`crates/lsp/tests/real_host.rs`).
8. `d8767fb` The manual-run driver (`tools/build-linux.sh`, `tools/build.py`) and the benchmarks `--bench-build N`
   and `--bench-output SECS`.
9. `5d84476` The four screenshots. 10. `d0bca5d` `results/linux-build.json`. 11. This report and the Status line.

## 3. Streaming and backpressure

- **Host.** `Build/OutputPipe.cs` turns MSBuild's stdout and stderr lines into ordered `eludite/build/output`
  chunks `{ buildId, seq, text }`: whole lines only, flushed at **16 KiB** or **16 ms** after the chunk's first line,
  whichever comes first. The host's own "Build started at ..." line and the command line are sent before MSBuild is
  spawned, which is why the first line arrives in about 17 ms whatever MSBuild's start-up costs.
- **Backpressure, host side.** One sender awaits each notification's write to stdout. While it waits (shell or pipe
  slow), lines queue and the next chunk takes everything queued up to 1 MiB: fewer, larger messages, nothing
  dropped. The queue holds 64k lines (about 8 MiB); when full, the reader stops reading MSBuild, so MSBuild blocks on
  its own console write rather than the host's memory growing. Node reuse is off (`-nr:false`) so that a cancel can
  kill every node, and the terminal logger is off (`-tl:off`).
- **Shell.** The client's reader thread turns notifications into `SessionEvent`s on an unbounded channel; the UI
  task drains every queued event in one update (one frame), so a burst costs one render. The UI thread never waits on
  the host: start and cancel requests go through the session worker, and their replies arrive as events.
- **Output window.** One `String` per source plus a `Vec<usize>` of line starts (100k lines: their bytes and one
  `usize` each); ANSI escapes and carriage returns are stripped on append; a `uniform_list` draws the visible lines
  only. The view follows output while scrolled to the end; scrolling up pauses it ("paused" label), scrolling back
  to the end resumes it. "Show output from" has Build and Host (the host's stderr, captured and still copied to the
  shell's stderr); Clear All.

## 4. Host changes (`dotnet/src/Eludite.Host`)

- `Build/BuildService.cs`: one build at a time (a second start is -32010 BuildInProgress with `data.buildId`);
  replies at once with the build id, toolchain, binlog path and command line; a new solution generation cancels the
  running build.
- `Build/BuildPlan.cs`: `dotnet build` (`-t:Rebuild`, `dotnet clean`) for SDK solutions; for legacy projects, Build
  Tools' `MSBuild.exe` on Windows or Mono's `MSBuild.dll` with the brief 0003 environment, else `dotnet build` with
  an `ELUDITE0111` warning. Every run: `-restore`, `-nologo -v:m -nr:false -clp:ForceNoAlign`, `-bl:<temp>/eludite-
  host/builds/build-<pid>-<id>.binlog` (the last 10 kept), the locator's MSBuild variables removed from the child's
  environment.
- `Build/BinlogReader.cs`: replays the binary log with `BinaryLogReplayEventSource` for diagnostics (file, line,
  column, end, code, message, project, target) and per-project results and times; a canceled build or unreadable
  log falls back to the console's canonical lines. A diagnostic repeated per target framework is listed once.
- `Build/ConsoleLines.cs`: progress (`Name -> output` completes a project; canonical errors and warnings counted
  once), at most every 100 ms.
- `Build/WindowsOnlyTargets.cs`: off Windows, raw errors from Windows-only targets become one brief 0003 diagnostic
  per project (`ELUDITE0101` to `0110`), written to the output too; a task's stack trace after MSB4018 becomes one
  "(stack trace omitted)" line.
- `Build/ProcessTree.cs`: cancel kills the tree (`Process.Kill(true)`; on Windows `taskkill /T /F` first,
  untested); `finished` with `canceled` follows within 2 s (tested).
- `HOST.md` and `host-rpc.md` document all of it.

## 5. Dedup against live diagnostics

The Error List is rebuilt from three lists: live Roslyn diagnostics, host diagnostics, and the last build's
diagnostics. A build diagnostic's key is (normalized path, line, column, code); a diagnostic without a file uses its
project file at 1:1. When a live row has the same key, that row's source becomes **Both** (shown "Build +
IntelliSense") and no second row is added; build diagnostics with equal keys are listed once. The next build
replaces the build list entirely; live rows are never replaced, and a row that was Both goes back to IntelliSense
when the next build no longer reports it. `diagnostics.list` returns `source` (`live`, `build` or `both`) and
filters on `source: "live"` or `"build"` (a Both row matches either).

## 6. Manual run (Linux) and screenshots

`crates/eludite/tools/build-linux.sh OUT_DIR` in a nested `kwin_wayland --virtual` (1280x960), release build, the
host's Debug build run from a copy (building `Eludite.slnx` rewrites the host's own `bin/`), `ELUDITE_TRACE_LSP=1`.
Load average before the run 12.2 (another agent was building in a sibling worktree); the benchmarks below ran later
at 1.0 to 2.0.

- Solution loaded (8 projects, 10.8 s under that load). Ctrl+Shift+B: first Output line 58 ms after the key (trace
  timestamps, including XTest delivery); finished "Succeeded, 0 errors, 0 warnings" 3.36 s after the key, Error List
  rows set 2.24 ms after the finished notification.
  [`linux-build-streaming.png`](../../crates/eludite/screenshots/linux-build-streaming.png) (Output streaming,
  status bar "Building: 0 of 8 projects"),
  [`linux-build-succeeded.png`](../../crates/eludite/screenshots/linux-build-succeeded.png) (VS summary lines,
  "Build succeeded"),
  [`linux-build-error-list-empty.png`](../../crates/eludite/screenshots/linux-build-error-list-empty.png) (0 errors,
  0 warnings; the only row is a live IntelliSense message, IDE0305, not from the build).
- `x` at the end of `HostRpcTarget.cs`, Ctrl+S, Ctrl+Shift+B: first line 18 ms; "Failed, 1 errors" 1.38 s after the
  key; rows set 0.16 ms after; the Error List came forward. Ctrl+Home moved the caret away; a double-click on row 0
  put it at 159:1. [`linux-build-error-click-through.png`](../../crates/eludite/screenshots/linux-build-error-click-through.png)
  (CS0116 "Build + IntelliSense"; the live-only IDE1007 for the same `x` below it; status "Build failed: 1 error, 0
  warnings").
- Afterwards `git diff -- dotnet/` was the one `+x` line; the script restored it with `git checkout -- dotnet/`.
- Not shown by the manual run: a long build streaming for minutes, and the Mono legacy path (its host test runs; the
  corpus build skips without the corpus).

## 7. Budgets and measurements

`results/linux-build.json` holds all six runs, the manual run's trace and the load averages.

- `--bench-build 5 --solution dotnet/Eludite.slnx` (real host, real `dotnet build`, incremental: 1.35 to 2.9 s per
  build): Ctrl+Shift+B through `Window::dispatch_keystroke`; first line = key to the first chunk appended to the
  Output window, and to the end of the present of the first frame rendered after it; rows = the pump receiving
  `eludite/build/finished` to the Error List rows set, and to the present showing them.
- `--bench-output 10`: 160-line chunks every 16 ms (the host's 16 ms flush with about 100-byte lines: 10,000 lines a
  second, 100,000 in 10 s) through the shell's own build-output handler, the window following; frame cost = render
  start to end of present for every frame (the brief 0009 method).
- Frame cost while a real build ran (not a budget): p50 2.1 ms, p99 10.1 to 13.8 ms over 76 to 82 frames per run,
  i.e. the worst one frame per run, with `dotnet build` saturating cores beside it. Recorded, not chased.

## 8. Headless tests

Against the fake host (`crates/eludite/src/shell/build_tests.rs`, `gpui::test`):

- `ctrl_shift_b_streams_output_follows_pauses_and_lists_errors`: Ctrl+Shift+B sends `eludite/build/start`; the
  Output window comes forward with the first line; 300 streamed lines with ANSI colors are stripped and followed;
  wheel up pauses (label shown, view stays as lines arrive), back to the end resumes; status bar states (started,
  "Building: 1 of 1 projects", "Build failed: 1 error, 1 warning"); menu items disabled while building and Cancel
  enabled; build rows in the Error List, which comes forward on errors; double-click to 3:17.
- `build_rows_are_deduplicated_against_live_ones_and_replaced_by_the_next_build`: Both rows, file-less diagnostics
  at the project file, `diagnostics.list` `source` values and filters, replacement by the next build.
- `cancel_from_the_menu_and_a_concurrent_start_is_refused`: Rebuild from the menu, a second start refused in the
  shell without reaching the host, Build > Cancel, "Rebuild All canceled".
- `an_agent_builds_from_another_thread_and_waits_for_the_result`: `eludite.build.solution` from a non-UI thread
  blocks until the finished summary and returns it; audited.
- `build_on_save_is_off_by_default_and_builds_the_project_when_on`; `the_configuration_dropdown_picks_release_and_the_host_log_has_its_own_source`.
- Unit tests: menu enablement, platforms from `.sln` / `.slnx`, a waiting agent gets its own build's result,
  command schemas and parsing (`crates/commands`), keymap and menu shortcuts (`crates/ui`), typed messages and a
  refused concurrent build over the fake host (`crates/lsp/tests/in_process.rs`), a real failing build through the
  real host (`crates/lsp/tests/real_host.rs`), output pane append and ANSI stripping.

Host tests (`BuildServiceTests.cs`, 13 methods, 5 theory cases): SDK fixture build with ordered output and binlog
projects; failing fixture with located errors and warnings from the binlog; cancel kills the tree and reports
canceled within 2 s; concurrent start refused with -32010; refusals before initialize, without a solution and for
a foreign project; project build; a legacy project with a Windows build event on Mono gets one `ELUDITE0108` (ran
here: Mono present); legacy corpus build on Mono (skipped: no corpus); `OutputPipe` ordering, coalescing, 16 ms
flush and chunk cap; console line parsing; Windows-only target classification.

## 9. Gaps and findings

- **Build on save is behind an environment variable** (`ELUDITE_BUILD_ON_SAVE=1`), not a settings file: the shell
  has no settings store yet. Off by default as the brief asks.
- **Canceling an agent's MCP call does not cancel the build**: the waiting `eludite.build.solution` returns when the
  build ends; an agent cancels with `eludite.build.cancel`. MCP `notifications/cancelled` is not wired to it.
- **Protocol gaps:** no `eludite/build/status` query, so a shell that reconnects mid-build cannot recover the build
  or its earlier output; progress is parsed from console lines (advisory), not structured per-project events from a
  live logger; configurations and platforms come from the shell parsing the solution file, not from the host
  (`eludite/solution/configurations` would let the host answer from MSBuild's solution parser, including project
  mappings); build diagnostics carry no help link or diagnostic category; there is no "build these projects" batch
  start (only solution or one project).
- **Windows:** `taskkill /T /F` and Build Tools `MSBuild.exe` are untested; macOS not run.
- **Tracked bytecode:** `crates/eludite/tools/__pycache__/*.pyc` are tracked in git, so every manual run dirties the
  tree; run the drivers with `PYTHONDONTWRITEBYTECODE=1` or untrack them (outside this brief's files).
- **Nested-session leftovers:** the nested KWin's dbus session leaves `xdg-desktop-portal-kde` and `ksecretd` holding
  the script's stdout open, so piping `build-linux.sh` into another command never sees EOF; redirect to a file.
- The real-build frame cost peaks (section 7) are one frame per build run; worth a look when Output is profiled
  against a long real build.

## 10. Sizing the follow-ups

- **Brief 0018 (run and debug with netcoredbg)** is already written and running in parallel. What it needs from this
  brief: F5 and Ctrl+F5 must build first (Visual Studio's "build before run"): call the shell's build start with the
  startup project and continue on `succeeded` only; about 0.5 day once 0018 lands, as an integration commit. The
  debuggee's console output should become a third Output source ("Debug"), which the Output window's per-source panes
  already support: about 0.5 day. 0018 itself: 4 to 6 days as briefed (DAP client, six debugger windows, the margin,
  the two-driver state machine).
- **Settings store** for build on save and "Show Output window when build starts" / "Always show Error List": 1 day
  as part of a general settings brief.
- **`eludite/build/status` and reconnect replay**: 1 day (host keeps the running build's last N chunks).
- **Configurations from the host** (`eludite/solution/configurations`): 1 day.
- **Windows run** of the build path (Build Tools MSBuild, `taskkill`): 0.5 day on a Windows machine.

## 11. How to reproduce

```
cargo build --release -p eludite
dotnet build dotnet/Eludite.slnx
PYTHONDONTWRITEBYTECODE=1 crates/eludite/tools/build-linux.sh OUT_DIR >OUT_DIR.log 2>&1   # RUNS=3; SKIP_DRIVE=1 or SKIP_BENCH=1
```

The script needs `dotnet/` clean in git and restores it with `git checkout -- dotnet/` at the end. Results go to
`OUT_DIR/drive.json`, `OUT_DIR/build-bench.jsonl`, `OUT_DIR/loadavg.txt` and the PNGs.
