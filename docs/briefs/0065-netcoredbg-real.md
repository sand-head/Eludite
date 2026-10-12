# Brief 0065: .NET (Core) 5+ debugging proven against the real netcoredbg, end to end

Status: in progress
Phase: 2 (PLAN.md section 10: alpha quality for the .NET workload; section 4.5, "netcoredbg for .NET (Core) 5+")
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 6, 8, 12), 4.5 (debugging), 5.1 and 5.2 (the agents' debugging commands), 13 (packaging)
Related ADRs: ADR-0002 (process topology: the adapter is its own process), ADR-0003 (protocols: DAP), ADR-0005 (licensing: netcoredbg is MIT and may be redistributed), ADR-0014 (tests fail on behavior only)
Depends on: brief 0018 (the DAP client and netcoredbg's launch), briefs 0025 to 0027 (inspection depth, run control, attach and policy), brief 0030 (the agent debugging proving scenario), brief 0033 (the DAP conformance corpus and its recorder), brief 0039 (the Linux package), brief 0055 (companions in the package).

## Goal

F5 on an SDK-style C# console app, Debug Test on a test and F5 on an ASP.NET Core app debug with netcoredbg 3.2.0-1092
as well as Visual Studio's basics, proven on a machine that runs the real adapter rather than from its documentation.
Every debug brief since 0018 says "netcoredbg not run" because earlier machines could not download it; this one runs
every netcoredbg-gated test, records netcoredbg's DAP conformance scenarios with the recorder so they replay on every
CI platform without the adapter, fixes what the real adapter shows the shell gets wrong, and measures the budgets
reports 0025 to 0027 left as "documented; not run here". The packaged build finds netcoredbg out of the box, and a
missing netcoredbg says once, in the status bar and the Output window, how to get it.

## Files in scope

- `crates/dap/**`: netcoredbg quirks in the client, the launch and attach plans, discovery (the fetch script's cache
  folder is searched after `PATH`), the recorder and replayer as far as netcoredbg's sessions need, `tests/netcoredbg.rs`.
- `crates/eludite/src/shell/debug.rs`, `crates/eludite/src/shell/debug/**`: fixes for netcoredbg only (no new
  windows); the netcoredbg conformance tests in `debug/conformance_tests.rs`.
- `crates/eludite/src/shell/session.rs`: launch-configuration fixes for netcoredbg only.
- `tools/netcoredbg/**`, `tools/dap-corpus/**`.
- `corpus/dap/netcoredbg/**` (recorder output only: `<scenario>.dap.json` and `<scenario>.golden.json`),
  `corpus/dap/README.md` (the netcoredbg column), `corpus/debug*` programs if a scenario needs one.
- `tools/package/companions.sh` and the netcoredbg lines of `tools/package/*.sh` and `windows.ps1`.
- `docs/agents/debugging.md` (netcoredbg notes), `.github/workflows/ci.yml` (the netcoredbg recording step: it
  becomes a check once the recordings are checked in).
- `docs/briefs/0065-netcoredbg-real.md` (this file), `docs/briefs/0065-report.md`.

Not in scope: new debugger windows (Parallel Stacks, Memory and the rest), Hot Reload and Edit and Continue, editor
rendering, `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, `docs/PLAN.md` (their lines go in the report's
Integration notes), the old reports 0025 to 0027 (their netcoredbg numbers go in this brief's report).

## Contract

- **Adapter.** netcoredbg 3.2.0-1092 as `tools/netcoredbg/fetch.sh` pins it, started `--interpreter=vscode` on stdio.
  Discovery order: beside `eludite` (`netcoredbg/netcoredbg[.exe]`, then `netcoredbg[.exe]`), `ELUDITE_NETCOREDBG`,
  `PATH`, then the fetch script's cache (`~/.cache/eludite/netcoredbg/3.2.0-1092/netcoredbg/`). A missing adapter is
  one `LaunchFailed` whose message names `tools/netcoredbg/fetch.sh`; the shell puts it in the status bar and the
  Debug Output once.
- **Launch.** `type: coreclr`, `request: launch`, the project's built `program` (`dotnet <app>.dll` for an
  apphost-less build), `cwd`, `args`, `env` from the launch profile, `justMyCode: true` (Visual Studio's default; a setting to turn it
  off is not part of this brief), `stopAtEntry: false`. Debug Test and ASP.NET Core launches use the same plan.
- **Behaviors proven against the real adapter** (by a gated test each, and by a recorded scenario where the shell's
  state is involved): breakpoints set before launch and in a second file bind (`breakpoint` events with `verified`)
  and hit; a condition; a hit count (the shell's emulation: netcoredbg does not advertise
  `supportsHitConditionalBreakpoints`); a tracepoint (the shell's emulation: no `supportsLogPoints`); a function
  breakpoint; step over, into and out; Run to Cursor; Set Next Statement refused with the adapter named
  (`gotoTargets` is not implemented by netcoredbg); Locals, Autos, Watch, Call Stack and Threads populated; a data
  tip (`evaluate` with context `hover`); an exception stop with its type and message (`exceptionInfo`) and Exception
  Settings by type (`filterOptions`); `evaluate` in `watch` and `repl` contexts; `setVariable`; attach by
  `processId` to a running `dotnet` process and detach leaving it running; Stop (`terminate`, then `disconnect`) and
  Restart (stop and start: no `supportsRestartRequest`); the agent commands `snapshot`, `stack`, `variables`,
  `wait`, `run_until`, `trace` and tracepoints.
- **Corpus.** `corpus/dap/netcoredbg/` holds `launch-break-step`, `tracepoints-and-exceptions` and
  `run-until-and-trace`, each `.dap.json` with its `.golden.json`, written only by `tools/dap-corpus/record.sh
  netcoredbg`. They replay through the headless shell on every platform with no adapter, and
  `record.sh --check netcoredbg` reproduces them on Linux (CI's step then fails on a difference instead of
  continuing).
- **Package.** `tools/package/companions.sh` copies the pinned netcoredbg folder (fetched by `tools/netcoredbg/fetch.sh`
  when absent and the network allows) to `netcoredbg/` beside `eludite`, with its `LICENSE` under
  `licenses/netcoredbg/`; `windows.ps1` does the same from the Windows layout when it is present.

## Proving test

On Linux with `ELUDITE_NETCOREDBG="$(tools/netcoredbg/fetch.sh)"`, the debugging corpus built
(`corpus/debugging/build.sh`) and `dotnet build dotnet/Eludite.slnx`:

```
ELUDITE_REQUIRE=netcoredbg cargo test -p eludite-dap --test netcoredbg -- --nocapture
ELUDITE_REQUIRE=netcoredbg cargo test -p eludite --bin eludite -- debug:: agents:: --test-threads=1
tools/dap-corpus/record.sh netcoredbg && tools/dap-corpus/record.sh --check netcoredbg
cargo test -p eludite --bin eludite -- conformance_tests::netcoredbg_
```

all pass, the last one with no netcoredbg installed (`ELUDITE_NETCOREDBG` unset).

## Budget

The budgets of briefs 0025 to 0027 measured against netcoredbg: `snapshot` < 150 ms p95 (20 calls), emulated
tracepoint overhead < 15 ms per hit mean, `set_variable` < 150 ms p95, attach to first stop reported. Replay of the
three recordings adds no more than a few seconds to the shell's test run.

## Exit criterion

Every netcoredbg-gated test passes here with `ELUDITE_REQUIRE=netcoredbg`; the three recordings are checked in and
replay; the report states the measured netcoredbg column of reports 0025 to 0027 and every discrepancy found and
fixed; `cargo fmt --check` and `cargo clippy -D warnings` are clean on the touched crates.

## Out of scope

New debugger windows, Hot Reload and Edit and Continue, `eludite-dbg-netfx`, recording an attach scenario for
netcoredbg (its attach is covered by the gated test; the conformance attach scenario's fixed port is Mono's), macOS
and Windows runs (CI's).
