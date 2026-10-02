# Brief 0007: Production LSP bridge between the shell and eludite-host

Status: done on Linux; Windows not run. Report: [0007-report.md](0007-report.md)
Phase: 1
Plan reference: PLAN.md sections 2 (principles 2, 3, 6), 3 (D2, D3), 4.3, 9, 11
Related ADRs: ADR-0002, ADR-0003
Depends on: brief 0002 (report), brief 0003 (report)

## Goal

Turn the spike-grade LSP proxy in `eludite-host` into the production bridge the shell will use for every language that lives in the host, with its schema in `protocol/` first. After this brief, a Rust client can start the host, open a solution, receive diagnostics, and request completion, hover, definitions and references through typed messages, with cancellation and solution generations enforced on both sides, and the finding from brief 0002 (Roslyn computes full semantics only when asked) handled so completion on referenced-project types is never empty.

## Files in scope

- `protocol/schemas/**` and `protocol/schemas/host-rpc.md` (the contract, written first, in its own commit)
- `protocol/rust/**` (crate `eludite-protocol`: typed messages for everything in the schema)
- `dotnet/src/Eludite.Host/**`, `dotnet/tests/Eludite.Host.Tests/**`, `dotnet/Directory.Packages.props`
- `crates/lsp/**` (the Rust client of the bridge: process supervision, framing, request/response, cancellation, generation tracking)
- `bench/roslyn-200/**` (only to keep the bench driver compiling against renamed methods)
- `docs/briefs/0007-report.md` (new)

Do not touch `crates/eludite`, `crates/editor`, `crates/docking`, `crates/ui`, `docs/adr/**`, `vendor/**`.

## Contract

- Resolve the gaps listed in `docs/briefs/0002-report.md` and `docs/briefs/0003-report.md` (protocol sections). In particular: the host's own `initialize`, `shutdown` and `exit` are renamed to `eludite/host/initialize`, `eludite/host/shutdown` and `eludite/host/exit` so plain LSP names are reserved for the forwarded language server; LSP forwarding is documented method by method; every forwarded request carries `eluditeGeneration`; the host answers stale generations with the documented error code and never forwards them.
- Solution lifecycle methods: `eludite/solution/open` (path to `.sln`, `.slnx`, or a project file; returns the new generation), `eludite/solution/close`, and `eludite/solution/status` notifications (loading, loaded with counts, failed with a diagnostic). Legacy projects go through the brief 0003 evaluator automatically; the status reports which MSBuild was used (Mono, Build Tools, SDK) and any corrections applied.
- Semantics warming: the host pulls diagnostics for every open document on open and on each change (debounced, documented interval), so Roslyn's frozen-partial completion is populated. Document this in `HOST.md` with the brief 0002 reference.
- Supported forwarded LSP requests for this brief: `textDocument/didOpen`, `didChange`, `didClose`, `publishDiagnostics` or pull diagnostics (whichever the pinned Roslyn supports, documented), `completion`, `completionItem/resolve`, `hover`, `definition`, `references`, `documentSymbol`, `workspace/symbol`, `$/cancelRequest`. Others pass through untyped and are listed as "forwarded, untyped".
- stdout is protocol only. Logs to stderr or a file under a documented path. No modal behavior; failures are diagnostics or status notifications.
- Cancellation: a canceled request returns within 50 ms with no result notification (brief 0002 measured 0.5 ms; keep that).
- The existing 14 host tests from the scaffold are updated for the renames, not deleted.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers, no co-author or sign-off lines.

## Proving test

- `cargo test -p eludite-lsp -p eludite-protocol`: typed round trips for every schema; a client test against a fake host (scripted stdio process) covering generation rejection, cancellation and status notifications.
- `dotnet test dotnet/Eludite.slnx`: host unit tests for renames, generation checks, warming debounce; the existing Roslyn integration test extended to assert that completion on a type from a referenced project returns members after the warming pull (brief 0002's failure case), skipping cleanly when the Roslyn build is absent.
- `bench/roslyn-200/run.sh` still runs; report T2 and T3 again so the renames and warming did not regress them (3 cold, 3 warm runs are enough).

## Budget

- Completion p95 under 50 ms from the host after warm-up (PLAN.md section 9); brief 0002 measured 7 ms, do not regress past 10 ms.
- Warming must not block completion: measure completion latency during a warming pull and report it.

## Exit criterion

1. `protocol/schemas/host-rpc.md` documents every method the host accepts, with a schema file per Eludite-specific message and a reference to the LSP spec for forwarded ones; the schema commit precedes the code.
2. `cargo test` and `dotnet test` green on Linux; CI green on the three Rust and two .NET jobs after merge (report what CI says if it runs before you finish).
3. The referenced-project completion test passes.
4. T2 and T3 numbers reported with the brief 0002 numbers beside them.
5. The report lists what remains untyped and sizes the next bridge brief.

## Out of scope

- Any UI. No editor, no Solution Explorer.
- Edit and Continue, Hot Reload, NuGet, builds, tests, debugging.
- The bulk-data side channel. Use stdio; note where it hurts.
- Windows runs. Report "not run on this machine".
- Changing the Roslyn pin.
