# Brief 0018 report: Run and debug with netcoredbg

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0018-debug`, rebased on `origin/main` at `b77ff34` (main moved during the work: brief 0017's build,
Output window and Error List rows, and brief 0019's brief). Date: 2026-10-02.
Brief: [0018-debug.md](0018-debug.md).

## 1. Summary

- **The manual flow works** with the real netcoredbg 3.2.0-1092 debugging the real `eludite-host` from
  `dotnet/Eludite.slnx` (built with `dotnet build`), driven with real XTest keys and clicks in a nested KWin
  (section 7): F9 on `Ping`'s first statement in `HostRpcTarget.cs`, F5; the breakpoint bound 35 ms after the
  program ran; a ping written into the debuggee's stdin from outside the IDE broke at line 69 with Locals
  (`this`, `timestamp = null`) and the Call Stack (`HostRpcTarget.Ping()` over `[Native Frames]` and StreamJsonRpc's
  dispatch) 29 ms later; F10 moved the execution point to line 70 with `timestamp` filled in (30 ms); a data tip on
  `timestamp` showed its value; the Breakpoints window showed the breakpoint with its hit count; F5 continued and the
  pong appeared in the Debug Console; Shift+F5 ended the session in 44 ms.
- **Budgets** (release build, nested KWin at 60 Hz, Wayland backend, `--bench-debug 3` with 20 pings per session and
  F10 twice per break, 3 runs; 1-minute load average 2.20 to 2.33 before each run):

  | Budget | Result |
  |---|---|
  | F5 to the first `stopped` at a breakpoint in `eludite-host` < 3 s warm (cold reported) | F5 to the break shown (locals in the windows): **warm 167 to 185 ms** (6 later sessions), **cold 221 ms** (first session after evicting netcoredbg, the debuggee's folder and the .NET runtime from the page cache), first session of the other two processes 183 and 204 ms; to the end of the present of the frame showing it 12 to 21 ms more. Pass |
  | Step over round trip (F10 to the windows updated) < 150 ms p95 | F10 to the stop's locals given to the windows **p95 11.5 to 11.7 ms** (p50 5.7 to 5.8, max 12.6; 360 steps); to the end of the present of the next frame p95 31 to 54 ms, p99 73 to 78 ms. Pass |
  | Locals with 200 variables renders < 50 ms | Rows given to the Locals window to the end of the present of the frame drawing them **p95 4.6 to 24.5 ms**, worst 38.1 ms (60 renders); that frame's own cost (render to present) p95 4.0 to 4.3 ms. Pass |
  | Frame cost < 8 ms p99 while stepping | Render to end of present for every frame between an F10 and its present: **p99 5.27 to 5.47 ms** (p50 3.5 ms, max 5.8 ms; 627 frames). Pass |

- **Tests:** `cargo test --workspace` 393 passed, 0 failed, 1 ignored (a doc example; 365 on main
  after brief 0017, so 28 new). fmt and clippy (`-D warnings`) clean; every commit
  passes them (re-checked commit by commit after the rebase). The real-adapter test
  (`crates/dap/tests/netcoredbg.rs`) passes with `ELUDITE_NETCOREDBG` set and skips with a message otherwise.
- **No new third-party dependency.** `eludite` now depends on `eludite-dap` (workspace crate) and `serde`
  (MIT OR Apache-2.0, already in the build); `eludite-dap` gained `tempfile` (MIT OR Apache-2.0, already a workspace
  dependency) as a dev-dependency. netcoredbg (MIT) is located at run time, never vendored and never a build
  dependency.

## 2. What was built

Commits, in order (each passes fmt, clippy and the workspace tests):

1. `0a3e33e` `protocol/schemas/` alone: an input schema for each of `eludite.debug.start`, `stop`, `continue`,
   `step_over`, `step_into`, `step_out`, `run_to_cursor`, `toggle_breakpoint`, `evaluate` and `state`, and for the
   windows' `select_frame`, `watch` and `exception_settings`; `debug-evaluate.output.json`; and the shared
   `debug-state.output.json` that the other commands answer with and the windows render.
2. `ea58a6f` `crates/dap`: the DAP client, stdio and TCP transports, the scripted fake adapter, netcoredbg discovery
   and the launch configuration (section 2.1).
3. `31bc8ea` The editor's breakpoint margin, execution point and data-tip hook; the debugger window ids; the Debug
   menu and keys.
4. `0a6e04f` The `eludite.debug.*` commands, their validation and the debugger state types in `eludite-commands`.
5. `94d65be` The shell: the two-driver state machine, sessions over `eludite-dap`, the debugger windows, margin
   breakpoints, the execution point, data tips, the status bar slot and the commands on the bus (section 2.2).
6. `2685678` The headless shell tests against the fake adapter.
7. `5814aea` The manual-run tooling: `--bench-debug`, the debug trace lines, `tools/debug-linux.sh` and `tools/debug.py`.
8. `ba9eebd` The Call Stack fix the manual run found (section 8, item 1), with its test.
9. `a81343f` The benchmark sets its breakpoint instead of toggling it; the drive captures the Breakpoints window and
   a data tip.
10. `1818b83` The run's results and screenshots.
11. `6c135ac` The confirmation run on the rebased tree, added to the results file.
12. This report and the brief's Status line.

The rebase onto `origin/main` had conflicts only where both briefs append to shared lists (the keymap and its test,
`eludite-commands`' module docs, the shell's fields, window bodies, status bar slots and command dispatch, and the
harness flags); each was resolved by keeping both. Every commit was then re-checked with `git rebase -x` (fmt, clippy,
tests: 377 to 393 tests passing along the series), after rebuilding `eludite-host` for brief 0017's real-host build
test. The manual drive and one benchmark run were repeated on the rebased tree with the same results (section 7).

### 2.1 `crates/dap`

- `DapClient` over a `Connection` (any reader and writer): `request` only queues (a writer thread sends), answers and
  events go to a sink on a reader thread, so no caller ever waits on the adapter. A closed connection fails every
  pending request and reports `Closed`.
- Transports (`AdapterTransport`, serializable, so a launch configuration can name one): a stdio child process, and
  TCP, tested against the fake adapter listening on loopback. SSH forwarding is a TCP endpoint and needs nothing new.
- `session::start`: DAP's handshake in order (`initialize`, `launch` or `attach`, `setBreakpoints` per file,
  `setExceptionBreakpoints`, `configurationDone`), with a timeout, accepting a `capabilities` event that arrives
  before `initialize`'s answer (netcoredbg sends one).
- Discovery (`discovery.rs`): beside the executable (`<exe dir>/netcoredbg/netcoredbg`, then `<exe dir>/netcoredbg`),
  `ELUDITE_NETCOREDBG`, then `PATH`; the error names every place searched and the fetch script.
- Launch configuration (`launch.rs`): the startup project (first executable project in the solution), its output DLL
  from the project's `TargetFramework`, `AssemblyName` and `OutputPath` (Debug), and the `launchSettings.json`
  profile's `commandLineArgs` (split as Visual Studio does), `environmentVariables` and `workingDirectory`;
  `dotnet <dll>` under netcoredbg, or directly for Ctrl+F5.
- `fake` feature: a scripted fake adapter (a program of steps over real files, threads, variables with children,
  output, exceptions, hangs and crashes), in-process, over stdio as a child process, or over TCP.

### 2.2 The shell (`crates/eludite`, `crates/editor`, `crates/ui`, `crates/docking`, `crates/commands`)

- `eludite.debug.*` on the bus (`crates/commands/src/debug.rs`, schemas in `protocol/schemas/debug-*.json`):
  `start` (project, `debug`, launch profile), `stop`, `continue`, `step_over`, `step_into`, `step_out`,
  `run_to_cursor`, `toggle_breakpoint` (toggle, set with condition, hit condition and enabled, delete, delete all),
  `evaluate`, `state`, plus `select_frame`, `watch` and `exception_settings` for the windows' own actions. Every
  window action is one of these commands, so agents and the UI use the same ones.
- The state machine (`shell/debug/state.rs`, no GPUI): modes design, launching, running, break, stopping; session
  generation and stop counter; breakpoints, stack, locals, watches, console; the debug-state output.
- Sessions (`shell/debug.rs`): launch on a `debug-launch` thread; adapter answers and events applied on the UI thread
  in batches, tagged with the session generation and the stop. Hit counts are counted by the shell (netcoredbg
  ignores `hitCondition`); Run To Cursor is a one-shot breakpoint; first-chance CLR exceptions through the `all`
  filter; breakpoints, exception settings and watches persist per solution under
  `<config dir>/eludite/breakpoints/solutions/`.
- Windows (`shell/debug/windows.rs`): Locals and Watch 1 (Name, Value, Type, lazy expansion through `variables`,
  the Watch box), Call Stack (click selects a frame, Locals and watches follow, the caller's arrow is green),
  Threads, Breakpoints (enable, condition, hit count, delete, delete all), Exception Settings (Common Language
  Runtime Exceptions: break when thrown, break when user-unhandled) and the Debug Console (program output and
  expressions evaluated in break mode). The first F5 lays them out as Visual Studio does: Locals and Watch beside
  the Error List, Call Stack, Breakpoints and Debug Console in a second group at the bottom.
- Editor: the breakpoint margin (click or F9; enabled, disabled, conditional, unbound glyphs), the execution point
  (yellow arrow and line for the current frame, green for a selected caller) and the data-tip hook (the member chain
  under the mouse, evaluated in the stopped frame with context `hover`).
- Debug menu and keys: F5 (Continue in break mode), Ctrl+F5, Shift+F5, F9, F10, F11, Shift+F11, Ctrl+F10; the status
  bar slot (`Debugging: Eludite.Host (break: breakpoint, HostRpcTarget.cs line 69)`).
- Harness: `eludite --bench-debug N`, `ELUDITE_TRACE_LSP=1` debug trace lines, `tools/debug-linux.sh` and
  `tools/debug.py`.

## 3. The two-driver rules

The user (keys, margin, windows) and agents (the same commands over MCP) drive one session
(`crates/eludite/src/shell/debug/state.rs` module docs):

1. **One queue.** Every command reaches the model on the UI thread, one at a time, through the command bus: the
   user's directly, an agent's through the shell's job queue. Nothing is applied concurrently.
2. **Refused, never queued.** Resuming commands (continue, the steps, Run To Cursor) and the commands that read a
   break (evaluate, select frame) need break mode. Applying one moves the model to `running` at once, before the
   adapter answers, so a second driver's step issued meanwhile is refused with a message naming the mode, the
   generation and the stop. Start is refused while a session runs.
3. **Stale stops are refused.** Each break increments `stop`. A command may quote the stop (and the session
   generation) it saw; if the debuggee has moved on since, it is refused as stale.
4. **Old answers are dropped.** Adapter answers carry the session generation and the stop they were asked in;
   answers for an older one are never shown (invariant 12).
5. **Both see the same state.** `eludite.debug.state` is built from the model the windows render, and records who
   resumed the debuggee last (`last_driver`, `stopped.driver`).
6. **Editing is always allowed.** Breakpoints, watches and exception settings change in any mode; a running session
   picks them up at once.

## 4. netcoredbg

- **Version:** 3.2.0-1092 (release of 2026-06-25; `netcoredbg --version`: "NET Core debugger 3.2.0-1 (9744e1f,
  Release)"). **License:** MIT (SPDX `MIT`), Samsung Electronics.
- **How it was obtained:** `tools/netcoredbg/fetch.sh` downloads the upstream release asset
  `https://github.com/Samsung/netcoredbg/releases/download/3.2.0-1092/netcoredbg-linux-amd64.tar.gz`, checks its
  SHA-256 (`080eb3b2…5afa`), unpacks it to `~/.cache/eludite/netcoredbg/3.2.0-1092/netcoredbg/` and prints the
  path. The script pins the Linux arm64, macOS arm64 and Windows x64 assets of the same release by checksum too
  (not run here). Building from source (upstream's CMake build, needing cmake, clang and the .NET SDK) is the
  fallback the script names; it was not needed (and this machine has no cmake).
- **How Eludite finds it:** beside the executable, `ELUDITE_NETCOREDBG`, `PATH`. Without it, F5 says where it
  looked and how to fetch it (headless test).

## 5. DAP features netcoredbg lacks or misreports

Probed against 3.2.0-1092 (its `initialize` answer is decoded in `crates/dap/src/types.rs`'s test):

| Feature | netcoredbg 3.2.0-1092 | What Eludite does |
|---|---|---|
| `hitCondition` on source breakpoints | Not advertised (`supportsHitConditionalBreakpoints` absent) and silently ignored: every hit breaks | The shell counts hits and resumes at once when the hit condition is not met; the stop is never shown. An adapter that advertises it gets the condition instead |
| `capabilities` event | Sent **before** the `initialize` response (the spec intends it for later changes), with the same body | `session::start` accepts either order |
| `breakpoint` events | Arrive (verified, with the bound line) before the `setBreakpoints` answer has given the ids | The shell keeps early events and applies them once the ids are known |
| Run To Cursor (`gotoTargets`, `goto`) | `gotoTargets` fails with `0x80004001` (E_NOTIMPL) | A one-shot breakpoint at the cursor line, removed at the next break |
| `stepInTargets` (Step Into Specific), `completions` (Watch and Immediate completion), `modules`, `loadedSources`, `readMemory`, `disassemble` | Each fails with `0x80004001` | Not used (Modules, Memory and Disassembly windows are out of scope) |
| `supportsExceptionOptions` | `false`, while `supportsExceptionFilterOptions` is `true` | Exception settings use the `all` and `user-unhandled` filters only, so per-exception-type settings (VS's full Exception Settings tree) are not possible with this adapter |
| `supportsLogPoints`, `supportsDataBreakpoints`, `supportsStepBack`, `supportsDelayedStackTraceLoading`, `supportsValueFormattingOptions`, `supportsEvaluateForHovers` | Not advertised | Tracepoints, data breakpoints and lazy stack loading are not offered; data tips use `evaluate` with context `hover`, which works although not advertised |
| Stack frames | A `[Native Frames]` pseudo-frame with line 0, no source and an empty `moduleId` | Shown as a row without a location, as Visual Studio shows external code |
| Evaluate errors | A failed evaluation returns the C# compiler's messages (`error CS1001: Identifier expected …` for a keyword) | Shown as the error (see finding 3 in section 8) |
| Debuggee stdin | Held by netcoredbg as a pipe that DAP cannot write to; `runInTerminal` is not requested | The Debug Console shows output only; the manual run writes the ping through `/proc/<pid>/fd/0` (Linux only) |

## 6. Tests

`cargo test --workspace`: 393 passed, 0 failed, 1 ignored. The behaviors with headless tests:

| Where | Tests | What they prove |
|---|---|---|
| `crates/dap/tests/client.rs` | 5 | Against the fake adapter in process: the full session (handshake, launch, breakpoint set and hit, stack, scopes and variables, evaluate, step, continue, stop); the **TCP transport round trip**; answers reach the sink without the caller waiting; an **adapter crash** fails pending requests and reports closed; first-chance exceptions and the ignored hit condition |
| `crates/dap/tests/stdio.rs` | 2 | The same over stdio with the fake as a child process, and its crash |
| `crates/dap/tests/netcoredbg.rs` | 1 | The real netcoredbg debugging the real `eludite-host`: launch, the breakpoint in `Ping` hit by a ping, stack, locals, evaluate, step, continue, stop (skips without netcoredbg) |
| `crates/dap/src` | 8 | Message shapes, transport serde, netcoredbg's bodies, discovery order, command lines, the missing-adapter error, project reading and the startup project, launch profile command lines |
| `crates/commands/src/debug.rs` | 3 | Every command parses and validates; hit conditions; outputs follow their schemas |
| `shell/debug/state.rs` | 4 | Two drivers refused, not queued; breakpoints toggle, persist, follow edits and bind; variables flatten by expansion; the state follows its schema and persists |
| `shell/debug/tests.rs` (headless shell, fake adapter) | 7 | **F9** and the margin toggle breakpoints that **persist per solution**; **F5** launches and **stops at the breakpoint with every window populated** (Locals, Watch, Call Stack, Threads, Breakpoints, Debug Console, status bar); the **Watch** box evaluates; **F10, F11, Shift+F11 move the current line**; Call Stack click selects a frame; lazy expansion; data tips; **Shift+F5 tears down**; **stale and concurrent commands refused**; conditions, hit counts, Run To Cursor and exception settings; an **adapter crash** ends the session and a **stalled adapter never blocks the UI**; **Ctrl+F5** runs without the debugger; **an agent drives a session from the bus and reads the same state** |
| `shell/debug/windows.rs` | 1 | Call Stack rows keep their height when the stack overflows the window (the bug the manual run found) |
| `crates/editor/tests/view.rs` | 1 | Breakpoint margin, execution point and data-tip expressions |
| `crates/ui` | 2 updated | The Debug keys and menu items |

## 7. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/debug-linux.sh OUT_DIR` (driver `tools/debug.py`, X11 backend in a nested
  Xwayland with real XTest input; benchmarks on the Wayland backend). For the run only, it writes
  `dotnet/src/Eludite.Host/Properties/launchSettings.json` with a profile (`--stdio --no-roslyn`,
  `ELUDITE_LEGACY=0`), so the launch exercises the profile, then deletes it and checks `dotnet/` is clean.
- **Raw results:** [`crates/eludite/results/linux-debug.json`](../../crates/eludite/results/linux-debug.json).
- **The run** (1-minute load 9.1 just after the release build; timings from the trace lines): F9, F5; the program
  running 87 ms after F5 and the breakpoint bound 35 ms later (the driver's figures include its 500 ms pause between
  F9 and F5); the ping written from the driver process (a second terminal, not the IDE); break at
  `HostRpcTarget.cs:69` 29 ms after the ping, Locals `this = {Eludite.Host.Rpc.HostRpcTarget}`, `timestamp = null`;
  F10 to line 70 in 30 ms with `timestamp = "2026-10-02T14:55:52.8904885Z"`; data tip; Breakpoints window; F5; the
  pong `{"jsonrpc":"2.0","id":1,"result":{"pong":true,"timestamp":…}}` in the Debug Console; Shift+F5 to the session
  ended in 44 ms.
- **Screenshots:**
  [`linux-debug-break.png`](../../crates/eludite/screenshots/linux-debug-break.png) (stopped at the breakpoint:
  execution point on line 69, Locals, Call Stack, status bar),
  [`linux-debug-step-datatip.png`](../../crates/eludite/screenshots/linux-debug-step-datatip.png) (after F10: line
  70, `timestamp` filled in Locals, the data tip `timestamp = "2026-10-02T14:55:52.8904885Z" (string)`),
  [`linux-debug-breakpoints.png`](../../crates/eludite/screenshots/linux-debug-breakpoints.png) (the Breakpoints
  window: the breakpoint, enabled, hit 1), and
  [`linux-debug-continue-console.png`](../../crates/eludite/screenshots/linux-debug-continue-console.png) (after F5:
  running, the pong in the Debug Console).
- **After the rebase** onto main with brief 0017 (load 2.0 to 2.4): the same drive (Ping's first statement is now line
  75) broke 26 ms after the ping, F10 in 55 ms, Shift+F5 in 46 ms; one benchmark run of 3 sessions: F5 to break shown
  183 to 198 ms, step to shown p95 11.7 ms, frame cost while stepping p99 5.7 ms, Locals 200 rows to present p95
  17.0 ms. Recorded under `after_rebase` in the results file.

## 8. Gaps and findings

1. **Fixed: Call Stack rows overlapped** when the stack was deeper than the window (the first manual run's
   screenshot): rows in a scrolling column shrank to fit. Rows are now `flex_none` in every debugger window, with a
   test.
2. **Fixed: the benchmark toggled its breakpoint**, which the previous run had persisted, so every other run had no
   breakpoint; it now sets it. A first benchmark round with that bug is not reported (its valid runs agree).
3. **Data tips show evaluation errors.** Hovering a keyword (`using`) showed netcoredbg's compiler errors as the tip;
   Visual Studio shows no tip when evaluation fails. Small follow-up in the shell (drop failed `hover` evaluations).
4. **Ctrl+F keeps the previous query** when nothing is selected, and typing appends to it (the editor's find bar,
   brief 0009's), so a second search for different text finds nothing; Visual Studio selects the query so typing
   replaces it. The drive moves the caret with the arrow keys instead. Outside this brief's files.
5. **F5 does not build first.** Visual Studio builds the startup project before launching; with brief 0017 on main,
   an integration commit should run `eludite.build.project` and launch on success. Until then F5 launches the last
   build, and when the DLL is missing the error says to build first (tested in `launch.rs`).
6. **The Debug Console is its own window.** With 0017's Output window on main, the program's output can move to a
   "Debug" source in Output by a later integration commit, as the brief foresaw.
7. **Cold is not fully cold:** the page cache of netcoredbg, the debuggee's folder and the .NET 10 runtime is
   evicted before the first benchmark run, but the IDE's own `eludite-host` runs on the same runtime and maps it
   again before the first F5, and a full eviction needs root. The cold first session (221 ms) is cold for netcoredbg
   and the debuggee's process only.
8. **What F5 to break measures:** the debuggee breaks only when pinged, so the harness writes the ping as soon as the
   program runs and the breakpoint is bound; the number is launch, handshake, binding, the host's startup to reading
   stdin, the ping, the stop and the stack and locals.
9. **The Breakpoints window's Name column** truncates `HostRpcTarget.cs, line 69` at the default width; the column is
   not resizable yet.
10. **Not updated** (outside this brief's files): `docs/briefs/README.md`'s index row. Windows and macOS: not run;
    discovery uses `netcoredbg.exe` on Windows and the fetch script pins every OS's asset, and the stdin ping in the
    benchmark is Linux-only (`/proc`).

## 9. Sizing brief 0019 (Rust through the generic paths)

What exists now: the editor features (completion, hover, signature help, definition, references, rename, code
actions) and their UI from briefs 0013 to 0015; the Workspace window; brief 0017's build pipeline (Output window,
Error List with build and live rows and dedup, status bar, cancel); and from this brief a DAP client that is already
adapter-neutral (adapter id, launch arguments as JSON, any transport). What is missing:

- **The generic LSP client** is the largest part. `crates/lsp` today is the `eludite-host` client: the shell reaches
  every language feature through `HostClient` (`eludite/*` plus forwarded LSP pinned to the solution generation).
  0019 needs a plain-LSP client to a server the shell launches (initialize, capabilities, document sync,
  `$/progress`, pull or push diagnostics, restart policy), and one "language server for these documents" trait both
  implement, so the 0013 to 0015 shell code routes by document instead of assuming the host. Expect most of the
  shell's language-feature code to be touched mechanically.
- **The Cargo model**: `cargo metadata` off the UI thread, members and targets, and the Workspace window's `kind`
  nodes with the .NET solution and the Cargo workspace as siblings under a folder root; File > Open Folder.
- **cargo build** through 0017's pipeline: `--message-format=json-diagnostic-rendered-ansi` parsed into build rows
  with spans, dedup against rust-analyzer's live rows, the active-system rule for Ctrl+Shift+B.
- **rust-analyzer discovery and fetch script** (MIT OR Apache-2.0; rustup's component exists on this machine), the
  status bar slot from `$/progress` and `experimental/serverStatus`.

Estimate: about two agent-weeks, 4,000 to 5,000 lines with tests (larger than this brief because of the shared
language-server abstraction across the existing features). The uncertain parts: refactoring the host-centric
feature code without regressing the C# budgets, and rust-analyzer's first diagnostics on this workspace (10 to 60 s
cold, as the brief expects). Debugging Rust stays out of 0019; when it comes, this brief's DAP client needs only an
adapter registry (CodeLLDB or lldb-dap discovery, a launch configuration from cargo's target) and the windows are
reused unchanged: about half an agent-week.

## 10. How to reproduce

```
cargo test --workspace
tools/netcoredbg/fetch.sh                                # prints the adapter's path
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite-dap --test netcoredbg
cargo build --release -p eludite
dotnet build dotnet/Eludite.slnx
crates/eludite/tools/debug-linux.sh OUT_DIR              # the manual run and RUNS=3 benchmark rounds
SKIP_DRIVE=1 crates/eludite/tools/debug-linux.sh OUT     # benchmarks only
SKIP_BENCH=1 crates/eludite/tools/debug-linux.sh OUT     # the manual run and screenshots only
```
