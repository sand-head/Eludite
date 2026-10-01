# Windows run checklist

What Phase 0 still owes on Windows, in the order to run it on a Windows 11 machine with a real GPU. Each item names the brief, the command and the exit criterion to record in that brief's report under a "Windows" heading.

## Setup (once)

1. Install: Git, Rust via rustup (the repo's `rust-toolchain.toml` picks 1.98.1), .NET SDK 10.0.302 (or let `global.json` roll forward), Visual Studio Build Tools 2022 with the ".NET Framework build tools" and "Web development build tools" workloads, Node (only to compare against the npx adapter), Claude Code (native installer) logged in.
2. Clone `https://github.com/sand-head/eludite-ide` and run `cargo build --workspace` and `dotnet build dotnet/Eludite.slnx` from a PowerShell prompt. Record anything that fails to build; that is finding number one.

## Brief 0001, GPUI shell and docking

- `cd spikes/0001-gpui-shell; cargo run --release` must open the window; exercise drag to dock, tab, float, auto-hide, restore.
- `python tools/bench_all.py --label windows --refresh-hz <monitor Hz>` and `python tools/inject_keys.py` (check its Windows notes first). Record cold start, frame cost p99, keystroke to pixel p99, scroll frame intervals, RSS. GPU, driver and refresh rate go in the report.
- Exit: GO or NO-GO for Windows in `docs/briefs/0001-report.md` section 8.

## Brief 0002, eludite-host with Roslyn

- `tools/roslyn-pin/build.ps1` (untested) must build the language server; fix the script if it does not.
- `bench/roslyn-200/run.ps1` (untested) with 3 cold and 3 warm runs. Record T0 to T3 and peak memory.

## Brief 0003, legacy projects

- `tools/legacy-load/run.ps1` (untested) against the corpus with the Build Tools column; `vswhere` discovery must find MSBuild.
- Compare Compile items against `MSBuild.exe -getItem:Compile` for at least 3 projects.
- The WebForms code-behind completion test must pass.

## Brief 0004, ICorDebug proof (not started; Windows only)

- Read `docs/briefs/0004-icordebug-dap-spike.md` and launch it as an agent on the Windows machine, or in a worktree here with the Windows machine reachable for the runs.

## Briefs 0005 and 0006, agents

- `cargo build --release -p eludite-claude-acp` under `agents/claude-acp`; run the 0005 panel against it; confirm `claude.exe` discovery and that no Node is needed.

## Briefs 0007 to 0009, Phase 1

- CI already builds and tests these on `windows-latest`. Run the manual parts of each report's "Windows" section if the brief has one.
