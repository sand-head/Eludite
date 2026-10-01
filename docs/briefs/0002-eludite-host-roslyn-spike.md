# Brief 0002: `niello-host` with Roslyn, time-to-IntelliSense

Status: in progress (Linux done; Windows pending). Report: [0002-report.md](0002-report.md)
Plan reference: PLAN.md sections 3 (D2, D3), 4.3, 9, 10 (Phase 0 item 2), 13 (risk 3)
Related ADRs: ADR-0002, ADR-0003

## Goal

Prove that a `niello-host` .NET process can embed the Roslyn language server built from source at a pinned commit, load a 200-project SDK-style solution, and serve IntelliSense over the shell-to-host protocol. Measure time-to-IntelliSense and report whether it is compatible with the budgets in PLAN.md section 9.

## Files in scope

- `dotnet/Niello.Host/**` and its tests under `dotnet/Niello.Host.Tests/**` (or the test project name already in `dotnet/Niello.slnx`)
- `bench/roslyn-200/**` (new): generator script and the generated 200-project solution definition
- `tools/roslyn-pin/**` (new): the script that fetches and builds Roslyn at the pinned commit
- `docs/briefs/0002-report.md` (new)

Do not edit `protocol/**`. The spike implements the contract in `protocol/schemas/host-rpc.md`. If it is missing something, record it in the report.

## Contract

- Transport: JSON-RPC 2.0 over stdio, Content-Length framing. Methods: `initialize`, `niello/ping`, `niello/host/info`, `shutdown`, `exit`, plus LSP 3.17 requests forwarded to the embedded Roslyn language server. Details are in `protocol/schemas/host-rpc.md`.
- stdout carries protocol messages only. Logs go to stderr or a file.
- Roslyn: `Microsoft.CodeAnalysis.LanguageServer` from dotnet/roslyn (MIT), built from source at one pinned commit hash recorded in `tools/roslyn-pin/COMMIT`. Do not use the Azure feed binaries or any C# Dev Kit component.
- .NET SDK pinned by `global.json` (10.0.302). C# nullable enabled, warnings as errors, Central Package Management.
- Requests are cancelable and carry a solution generation number. A stale response is dropped by the client.
- The 200-project solution: generated, SDK-style, `net10.0`, a mix of libraries (about 80 percent) and test projects, project references forming layers at least 6 deep, about 50 source files per project, using NuGet packages from a local cache so the run is offline.

## Proving test

- `dotnet test dotnet/Niello.slnx` includes an integration test that starts `niello-host`, runs the `initialize` handshake, opens the generated solution, waits for readiness, then requests completion in a file near the bottom of the dependency graph.
- A bench program `bench/roslyn-200/run.sh` (and `run.ps1` for Windows) measures, over 10 cold runs and 10 warm runs, with the host process killed between runs:
  - T0: process start to `initialize` response.
  - T1: `initialize` to first successful `textDocument/documentSymbol`.
  - T2: `initialize` to first successful `textDocument/completion` returning at least one item in a project 6 layers deep.
  - T3: completion request latency p50 and p95 over 1000 requests after warm-up.
  - Peak host working set.
- Record the machine: CPU, core count, RAM, disk type, OS.

## Budget

- Completion after trigger under 50 ms p95 from the host (PLAN.md section 9).
- Host memory is reported, not capped, but record it.
- Host startup must not write to stdout anything that is not a protocol message.

## Exit criterion

1. The Roslyn language server builds from source at the pinned commit with a single documented command on Linux and Windows.
2. `niello-host` completes the handshake and answers `niello/ping` and `niello/host/info`, as specified.
3. The 200-project solution loads and the T2 completion succeeds, on Linux and Windows.
4. The report gives T0 through T3 and peak memory with medians and ranges. It states whether T3 p95 is under 50 ms, and what T2 would need to reach to meet the 1 s editable-text budget with semantic features streaming.
5. Cancellation works: a canceled completion request returns within 50 ms and produces no result notification.
6. The report lists the Roslyn LSP extensions (`roslyn/*`) the spike used and anything in the pinned commit that was awkward to build or embed.
7. A recommendation on whether D2 stands, and the size of the follow-up brief.

## Out of scope

- Legacy (non-SDK) projects. That is brief 0003.
- The Rust side of the protocol and any GPUI work.
- The solution-wide symbol index, MSBuild builds, NuGet UI, Edit and Continue, Razor.
- Test discovery, debugging, source generators beyond what the 200 projects naturally use.
- The side channel (pipe or shared memory) for bulk data. Use plain stdio and note where it hurts.
- Packaging, installers, auto-update.
