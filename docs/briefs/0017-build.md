# Brief 0017: Build with Output and Error List

Status: done on Linux (Windows and macOS not run); [report](0017-report.md)
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 2, 3), 3 (D4), 4.4, 5.1, 8, 9
Related ADRs: ADR-0002, ADR-0004
Depends on: briefs 0007 (bridge), 0012 (open solution, Error List), 0016 (sizes this brief as 0017a)

## Goal

Ctrl+Shift+B builds the open solution through `eludite-host`, the Output window shows the build log live as it streams, the Error List shows build diagnostics (distinct from live Roslyn diagnostics, deduplicated against them where they overlap) with click-through, the status bar shows progress and the result, builds are cancelable, and an agent can build and read the result through the bus. Legacy projects build with the MSBuild the host located (Mono, Build Tools or SDK), and a Windows-only target failing on Mono produces the Output message brief 0003 listed instead of a stack trace.

## Files in scope

- `protocol/schemas/` first and alone: `eludite/build/start` (solution or project, configuration, platform, target: build, rebuild, clean), `eludite/build/cancel`, `eludite/build/progress` and `eludite/build/output` notifications (chunked, ordered), `eludite/build/finished` (summary, per-project results, diagnostics with file, line, column, code, message, project), plus command schemas `eludite.build.solution`, `eludite.build.project`, `eludite.build.cancel`, `eludite.build.rebuild`, `eludite.build.clean`, `eludite.output.show` and `eludite.output.clear`
- `protocol/rust/**`, `crates/lsp/**` for the typed messages and notifications
- `dotnet/src/Eludite.Host/**` and tests: a build service that runs MSBuild out of process (`dotnet build` for SDK solutions; the located MSBuild for legacy, with the brief 0003 environment for user-space Mono), streams its console output, writes a binary log (`-bl`) and parses it with `Microsoft.Build.Logging.StructuredLogger` (MIT) or the MSBuild binlog reader for structured diagnostics and timings; one build at a time per host, cancelable (kill the process tree cleanly)
- `crates/eludite/**`: the Output window as a real text view (ANSI colors stripped or rendered, 100k lines without jank, auto-scroll with a pause when the user scrolls up, Clear, a source dropdown with Build and Host entries), Error List integration, status bar build slot, menu items under Build, Ctrl+Shift+B and F6, build-on-save off by default behind a setting
- `crates/commands/src/**`, `crates/ui/**`, `crates/docking/**` as needed for the Output window
- `docs/briefs/0017-report.md` (new)

## Contract

- Output is streamed in order over the bridge in chunks (document the chunk size and the backpressure story); the UI thread never waits; 100k lines render under budget.
- Build diagnostics replace the previous build's diagnostics in the Error List but never the live Roslyn ones; where a build diagnostic has the same file, position and code as a live one it is shown once, flagged as both.
- Cancel kills the MSBuild process tree and reports cancelled within 2 s.
- The host refuses a second concurrent build with a clear error; the UI disables the menu items while building.
- Windows-only targets failing under Mono surface the brief 0003 messages in Output and as one diagnostic per project.
- Agents: `eludite.build.solution` returns the finished summary (blocking on the bus from a non-UI thread, cancelable), and `diagnostics.list` includes build diagnostics with a `source` field (`build` or `live`).
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Host tests: build the SDK fixture and a deliberately failing one, parse the binlog, cancel mid-build, refuse a concurrent build; legacy corpus build on Mono skipped cleanly without the corpus.
- Headless tests against the fake host: streamed output renders and auto-scrolls, pause on scroll-up, Error List rows and click-through, dedup against live diagnostics, status bar states, cancel, menu items disabled while building, an agent building from the bus.
- Manual, recorded with two screenshots: build `dotnet/Eludite.slnx` from Eludite (Output streaming, status bar, Error List empty), then introduce an error and rebuild (Error List row from the build, click-through). Revert afterwards.

## Budget

- Ctrl+Shift+B to first Output line under 100 ms (PLAN.md section 9).
- 100k Output lines appended over 10 s with frame cost under 8 ms p99.
- Build finish to Error List rows under 200 ms after the host's finished notification.

## Exit criterion

1. The manual flow works with screenshots and the numbers.
2. All tests green; workspace fmt, clippy, tests green; `dotnet build` zero warnings and `dotnet test` green.
3. The report sizes brief 0018 (run and debug with netcoredbg) and lists protocol gaps.

## Out of scope

- Running or debugging the built program (brief 0018).
- Build configurations UI beyond Debug and Release and the platform dropdown values the solution file lists.
- Incremental design-time builds, parallel multi-solution builds, publish, NuGet restore UI (restore runs as part of the build).
- Windows and macOS runs.
