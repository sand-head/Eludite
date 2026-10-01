# Brief 0018: Run and debug with netcoredbg

Status: done on Linux (Windows and macOS not run); [report](0018-report.md)
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 2, 3), 3 (D3, D7), 4.5, 5.1, 5.5, 8, 9
Related ADRs: ADR-0003, ADR-0007
Depends on: briefs 0008 (docking), 0012 (open solution), 0016 (sizes this as 0017b); runs in parallel with brief 0017 (build) and must not touch its files

## Goal

F5 runs the startup project under the debugger, Ctrl+F5 without it, with the Visual Studio debugging windows: breakpoints in the margin (F9), Locals, Watch, Call Stack, Threads, Breakpoints and Exception Settings, step over, into and out (F10, F11, Shift+F11), Continue (F5), Stop (Shift+F5), Run to Cursor, data tips on hover, and the Debug toolbar state in the status bar. The debugger is netcoredbg (MIT) over DAP through `crates/dap`, reached through the transport abstraction from D7 so a remote adapter is the same code path. Every debugger action is a command so an agent can drive a session (PLAN.md 5.5), and the state machine is safe for two drivers.

## Files in scope

- `protocol/schemas/` first and alone: command schemas for `eludite.debug.start` (project, with or without debugging, launch profile), `eludite.debug.stop`, `eludite.debug.continue`, `eludite.debug.step_over`, `eludite.debug.step_into`, `eludite.debug.step_out`, `eludite.debug.run_to_cursor`, `eludite.debug.toggle_breakpoint`, `eludite.debug.evaluate`, `eludite.debug.state` (query), plus a `debug-state` output schema the debugger windows and agents read
- `crates/dap/**`: the DAP client proper (initialize, launch and attach, breakpoints, stopped and continued events, stack traces, scopes and variables, evaluate, disconnect), the stdio transport now and the TCP transport typed and tested against a fake, adapter discovery for netcoredbg (bundled path beside the executable, `ELUDITE_NETCOREDBG`, PATH), launch configuration from `launchSettings.json` and the project's output path
- `crates/eludite/**`: the debugger windows, margin breakpoints, current-line highlight, data tips, the Debug menu and keys, status bar slot, the two-driver state machine; do not touch the Output window or build files owned by brief 0017 (if the debuggee's console output needs a window, use a dedicated "Debug" source added later by an integration commit; for now show it in a Debug Console tool window of your own)
- `crates/editor/**` only for the breakpoint margin, current-line decoration and data-tip hook
- `crates/commands/src/**`, `crates/ui/**`, `crates/docking/**` as needed
- `docs/briefs/0018-report.md` (new)

For the manual run, build the target with `dotnet build` from a terminal, since the in-IDE build is brief 0017's and runs in parallel.

## Contract

- netcoredbg is located, not vendored into git; the report documents how to obtain it (release binary or build from source) and `tools/netcoredbg/` holds a fetch script pinned to a version with its SPDX id.
- Launch: for the startup project (first executable project in the solution, settable later), run `dotnet` on the built DLL under netcoredbg with the `launchSettings.json` profile's environment and arguments; Ctrl+F5 runs without the adapter in a terminal-like Debug Console.
- Breakpoints persist per solution; conditional and hit-count breakpoints through the Breakpoints window; exception settings for first-chance CLR exceptions.
- Windows update on `stopped`; Locals and Watch expand lazily through `variables`; Call Stack click selects a frame and updates Locals; data tips evaluate on hover with the stopped frame.
- Two drivers: user and agent commands go through one queue with a session generation; a command issued against a stale state (for example step while running) is refused with a clear error, never queued blindly; the agent sees the same `eludite.debug.state` the windows render.
- Remote-ready: the DAP client takes a transport; the TCP transport is tested against a fake adapter even though no remote run happens here.
- The UI thread never waits on the adapter.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/dap` tests against a scripted fake adapter: handshake, launch, breakpoint set and hit, stack and variables, evaluate, step, continue, stop, adapter crash, TCP transport round trip.
- Headless shell tests: F9 toggles and persists, F5 launches and stops at the breakpoint with windows populated, F10 and F11 move the current line, Watch evaluates, Stop tears down, stale commands refused, an agent driving a session from the bus and reading the state.
- Manual, recorded with three screenshots: debug `eludite-host` itself from `dotnet/Eludite.slnx` (built with `dotnet build`), breakpoint in `HostRpcTarget.Ping`, trigger it by sending a ping from a second terminal, inspect Locals and the Call Stack, step, continue, stop.

## Budget

- F5 to the first `stopped` at a breakpoint in `eludite-host` under 3 s warm (report cold too).
- Step over round trip (F10 to the windows updated) under 150 ms p95.
- Locals window with 200 variables renders under 50 ms; frame cost under 8 ms p99 while stepping.

## Exit criterion

1. The manual flow works with screenshots and numbers.
2. All tests green; workspace fmt, clippy, tests green.
3. The report lists DAP features netcoredbg lacks or misreports, the two-driver rules, and sizes brief 0019 (rust-analyzer and cargo through the generic paths, so Eludite can build Eludite).

## Out of scope

- Building inside the IDE (brief 0017), Edit and Continue, Hot Reload, attach to process, remote attach, .NET Framework (`eludite-dbg-netfx`), Memory and Disassembly windows, mixed mode.
- Windows and macOS runs.
