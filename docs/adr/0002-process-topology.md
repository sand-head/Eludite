# ADR-0002: Out-of-process topology

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 2 (principles 1 and 2), section 3 (D2)

## Context

The heavy .NET machinery (Roslyn, MSBuild, NuGet, Edit and Continue) is written in C# and expects a .NET runtime. The shell is Rust (ADR-0001). Debuggers, test runners and agents are separate programs anyway. The plan's first principle is that the UI thread never waits, and its second is that a hung analyzer must not freeze the editor and a crashed debugger must lose the session, not the IDE.

Roslyn's own language server is built for VS Code and changes with it (PLAN.md section 13, risk 3). The C# Dev Kit, which Microsoft ships alongside it, has proprietary pieces we cannot use.

## Decision

Run each of these in its own process, and talk to them over a protocol:

- `niello` (Rust, GPUI): window, docking, editor core, command bus, settings, tree-sitter highlighting, search, terminal, git via libgit2, ACP client, MCP server, WASM extension host.
- `niello-host` (.NET): one long-lived process per solution. It embeds the Roslyn language server (`Microsoft.CodeAnalysis.LanguageServer`, MIT, from dotnet/roslyn), the MSBuild-based project system, the NuGet client libraries and the Edit and Continue service.
- Debug adapters, test hosts and agents: separate processes per ADR-0003 and ADR-0007.

Transport and message rules:
- JSON-RPC 2.0 over stdio with Content-Length framing for control. A pipe or shared-memory side channel carries bulk data (semantic tokens for a 20k-line file, a full-solution symbol index, test output).
- Every message is cancelable and carries a solution generation number. The shell drops stale results instead of rendering them.
- Build `niello-host` from source at a pinned dotnet/roslyn commit. Do not consume the Azure-feed binaries.
- Host stdout carries protocol messages only. Logs go to stderr or a file.
- Every boundary has a schema in `protocol/` before code on either side (CLAUDE.md invariant 4).

## Alternatives considered

- In-process Roslyn via an embedded runtime: lowest latency, but a synchronous call or a crash takes the UI with it. This is the Visual Studio failure mode.
- One host process per project: isolates more, but a solution-wide symbol index and cross-project analysis then need a coordinator. One process per solution matches how Roslyn's workspace works.
- Consume the prebuilt Roslyn language server binaries from the Azure feed: less build work, but we lose control of the version and risk coupling to proprietary Dev Kit pieces.
- Native interop (P/Invoke or C ABI) between Rust and .NET: avoids a process but reintroduces shared fate and GC interactions.

## Consequences

Positive:
- A hung analyzer or runaway MSBuild evaluation cannot freeze keystrokes.
- A crashed debugger or host is restartable without losing editor state.
- Shell and hosts can be developed, tested and replaced independently. Each agent works on one side of a schema.
- Replaceable hosts: an alternative host that speaks the protocol can be used with the shell.

Negative:
- Serialization cost on every request, and a second runtime (.NET) to ship, locate and version.
- Cold start must hide host startup: the window shows tree-sitter highlighting while the host loads, and semantic features stream in afterward.
- Memory is spread across processes. The shell budget (400 MB) excludes the host, so host memory is reported separately in the status bar.
- Pinned Roslyn commits mean we own upgrades and must track upstream churn.

## Revisit when

- Measured host round-trip latency blocks the completion budget (50 ms p95) after tuning the side channel.
- Process startup of `niello-host` prevents meeting the 1 s solution-to-editable budget.
- Roslyn ships a supported embedding API that makes in-process hosting safe.
- Brief 0002 finds the pinned-source build of the Roslyn language server impractical to maintain.
