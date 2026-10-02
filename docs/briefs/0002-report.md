# Brief 0002 report: `eludite-host` with Roslyn, time-to-IntelliSense

Status: Linux done; Windows not run on this machine (none available). Spike code, not production.
Branch: `brief/0002-eludite-host-roslyn`. Date: 2026-10-01.

## Summary

- The Roslyn language server builds from source at the pinned commit with one command
  (`tools/roslyn-pin/build.sh`) in **117 s** wall time on Linux, with 0 warnings. That time
  includes downloading Roslyn's own SDK.
- eludite-host **spawns** the server as a child process and proxies LSP over its existing stdio
  connection. Loading the server in-process was tried and fails at this commit (see "Embed vs
  spawn").
- The 200-project solution loads in about 9.2 to 9.6 s. Completion in the depth-6 project
  succeeds at **T2 ≈ 9.6 s** (median). Completion latency is **T3 p95 = 7.0 ms** (warm median),
  well under 50 ms. In a more realistic typing loop (a `didChange` before every completion),
  p95 is 36.5 ms.
- Cancellation: the response to a canceled request arrives in under 3 ms (worst case 6.4 ms over
  3,000 trials), and no result is ever delivered for a canceled request.
- **D2 stands.** The follow-up is a medium brief (see the end).
- Big finding: Roslyn LSP completion runs on *frozen-partial semantics*, and the server does not
  compile dependencies in the background on its own. If the client never sends a request that
  needs full semantics (a `textDocument/diagnostic` pull or a hover), member completion on types
  from referenced projects stays **empty indefinitely** for that document version. The VS Code
  client hides this because it pulls diagnostics on every change. Eludite's shell must do the
  same, or the host must warm compilations itself.

## Machine

| | |
|---|---|
| CPU | AMD Ryzen 9 7940HS (8 cores, 16 threads) |
| RAM | 30 GiB |
| Disk | NVMe, WD_BLACK SN850X 2 TB, btrfs |
| OS | CachyOS (Arch-based), kernel 7.2.4-1-cachyos |
| .NET | SDK 10.0.302, runtime 10.0.10 (Roslyn's build used its own SDK 11.0.100-rc.1.26425.128) |
| Roslyn | `7c238e7cb19650384de7dcb93a93d4e38a5f7ed9`, dotnet/roslyn `main` tip, committed 2026-10-01T20:42:35Z |

Windows: **not run on this machine.** No Windows numbers exist. `build.ps1` and `run.ps1` are
written but untested.

## What was built

| Path | What |
|---|---|
| `tools/roslyn-pin/COMMIT` | Pinned commit (the `main` tip on 2026-10-01; no release tag was tried because `main` built cleanly on the first attempt) |
| `tools/roslyn-pin/build.sh`, `build.ps1` | Clone or fetch into `$ROSLYN_SRC_DIR` (default `~/.cache/eludite/roslyn`), check out the pin, build the language server. The ps1 is untested. |
| `dotnet/src/Eludite.Host/Lsp/` | `ILanguageServerLauncher`, `RoslynProcessLauncher` (spawn plus locate), `LspProxy` (forwarding, upstream handshake, cancellation, solution generation) |
| `dotnet/src/Eludite.Host/{Program.cs, Rpc/*}` | `--roslyn-ls <path>` and `--no-roslyn` flags; `initialize` starts the server in the background; the proxy is attached to the same connection |
| `dotnet/tests/Eludite.Host.Tests/LspProxyTests.cs` | 9 tests against an in-memory fake LSP server |
| `dotnet/tests/Eludite.Host.Tests/RoslynIntegrationTests.cs` | Proving test against the real host, the real Roslyn server and the generated solution. Skips with a message when either is absent. |
| `bench/roslyn-200/generate.py` | Deterministic generator for the 200-project solution |
| `bench/roslyn-200/run.sh`, `run.ps1`, `driver/` | Bench driver (C#, so `run.ps1` can reuse it); `run.ps1` is untested |

Test counts: `dotnet test dotnet/Eludite.slnx` gives 55 tests, all passing. The 24 host tests are
the 14 existing ones, 9 proxy tests and 1 integration test. On a fresh clone the integration test
is skipped.

## Exit criteria

### 1. Builds from source at the pinned commit with a single command (Linux and Windows)

**Linux: pass.** `tools/roslyn-pin/build.sh` runs:

```
./build.sh --restore --build --configuration Release \
  --solution src/LanguageServer/Microsoft.CodeAnalysis.LanguageServer/Microsoft.CodeAnalysis.LanguageServer.csproj \
  --nodeReuse false
```

- Wall time: 117 s, including the download of Roslyn's pinned SDK into `.dotnet/` and the restore.
- Result: 0 warnings, 0 errors.
- Output: `$ROSLYN_SRC_DIR/artifacts/bin/Microsoft.CodeAnalysis.LanguageServer/Release/net10.0/Microsoft.CodeAnalysis.LanguageServer.dll`
  (119 MB, 98 assemblies, plus `BuildHost-netcore/` and `BuildHost-net472/`).

**Windows: not run on this machine.** `build.ps1` calls `Build.cmd` with the same arguments and
is untested.

### 2. Handshake, `eludite/ping`, `eludite/host/info`

**Pass.**
- The 14 existing host tests still pass unchanged. That includes the one asserting
  `capabilities` is `{}`, and the one asserting that `eludite/nope` returns MethodNotFound.
- The integration test runs `initialize`, `eludite/ping`, `eludite/host/info`, `shutdown` and
  `exit` against the real process with Roslyn attached, and checks for exit code 0.
- Host stdout stayed protocol-only: the bench client's strict framing parser read every byte of
  20 runs without error. Roslyn's stdout is a private pipe to the host, and its stderr is copied
  to the host's stderr with a `[roslyn-ls]` prefix.

### 3. The 200-project solution loads and T2 succeeds (Linux and Windows)

**Linux: pass.** Windows: **not run on this machine.**

What the generator produces:
- 160 class libraries in 7 layers (L0 to L6). Every project in layer k references one project in
  layer k-1 plus up to two lower ones, so layer 6 sits 6 reference levels deep.
- 40 xunit.v3 test projects (20 percent).
- 50 source files per project, 10,001 `.cs` files in total, all `net10.0` and SDK-style.
- About 30 percent of the libraries use `Microsoft.Extensions.DependencyInjection.Abstractions`
  10.0.10 and about 15 percent use `Humanizer.Core` 2.14.1. Tests use `xunit.v3` 4.0.1.
- Restore uses a generated `nuget.config` whose only source is `~/.nuget/packages`, so the run is
  offline. The solution also builds clean (`dotnet build`, 0 warnings).
- The probe is `Bench.L6.P00/Probe.cs`: completion after `widget.`, where `widget` is a
  `Bench.L5.P00.Widget00`. The expected item is `Compute`.

### 4. T0 to T3 and peak memory, with medians and ranges

10 cold and 10 warm runs. The host process tree is killed after every run.

- **Cold:** before each run, Roslyn's MEF composition cache (`<ls dir>/cache`) and
  `/tmp/roslyn-canonical-misc` are deleted, and `dotnet build-server shutdown` is run. The OS page
  cache was **not** dropped because that needs root. So "cold" here means cold Roslyn caches with
  a warm page cache. Only the first cold run of the session shows true cold-disk effects.
- **Warm:** consecutive runs with the caches in place.

Raw data is in `bench/roslyn-200/results/20261001-175022/` (gitignored); the summary below comes
from it.

| Metric | Cold median (min to max) | Warm median (min to max) |
|---|---|---|
| T0: process start to `initialize` response | 103.8 ms (101.4 to 327.3) | 104.3 ms (102.6 to 106.5) |
| T1: `initialize` to first non-empty `documentSymbol` | 1,218 ms (1,208 to 4,172) | 1,012 ms (1,003 to 1,212) |
| T2: `initialize` to completion containing `Compute`, depth-6 project | 9,623 ms (9,405 to 15,505) | 9,643 ms (9,296 to 9,918) |
| (Tload: `initialize` to `workspace/projectInitializationComplete`) | 9,631 ms (9,133 to 14,986) | 9,211 ms (8,752 to 9,659) |
| T3: completion latency over 1,000 requests, p50 | 4.7 ms (4.0 to 5.3) | 4.6 ms (4.3 to 5.4) |
| T3 p95 | **7.1 ms** (6.6 to 8.3) | **7.0 ms** (6.4 to 7.7) |
| T3 p99 / max | 8.5 / 35.6 ms | 8.1 / 38.0 ms |
| T3-typing (`didChange`, then completion) p50 | 12.7 ms (8.4 to 14.1) | 12.8 ms (10.5 to 14.3) |
| T3-typing p95 | 34.9 ms (15.3 to 39.4) | 36.5 ms (14.5 to 39.6) |
| T3-typing p99 | 48.2 ms (39.4 to 52.8) | 49.1 ms (36.8 to 65.5) |
| Peak RSS of eludite-host (VmHWM) | 72.0 MB (71.6 to 74.0) | 71.9 MB (71.5 to 72.3) |
| Peak RSS of the Roslyn LS child (VmHWM) | 1,582 MB (1,116 to 1,853) | 1,417 MB (1,079 to 1,888) |
| Peak RSS of the whole host tree (host, LS and BuildHost processes, sampled every 50 ms) | 2,575 MB (2,537 to 2,616) | 2,544 MB (2,508 to 2,662) |

Notes on these numbers:
- **First run of the session.** The outliers (T0 327 ms, T1 4.2 s, T2 15.5 s) all come from the
  first cold run, which is the only one where the page cache had not yet seen the server and the
  BuildHost. Every later "cold" run is within a few percent of warm. Clearing Roslyn's caches
  costs about 0.2 s, almost all of it in T1, where MEF composition is rebuilt.
- **T2 is load-bound.** T2 is roughly Tload plus 0.4 s. Load time is dominated by the design-time
  builds of 200 projects, run by Roslyn's out-of-process BuildHost. Those builds also account for
  about 1 GB of the tree peak.
- **T1 comes from Roslyn's misc-files workspace.** It needs only syntax, so it is served about
  1 s after `initialize`, long before the solution has loaded. That second is the server's own
  startup.
- **Server GC.** The Roslyn server runs with Server GC, which its runtimeconfig enables. That
  inflates its RSS on a 16-thread machine. Workstation GC is worth testing in the follow-up.
- **T3 as the brief defines it.** The same request is repeated on an unchanged document.
  T3-typing adds a new document version before each request, which is closer to real typing; its
  p95 is still under 50 ms, and its p99 sits right at the line.
- **Proxy overhead.** I compared 1,000 completions driven by the same Python client straight at
  the Roslyn server against the same requests through eludite-host, alternating twice. Results:
  direct p50 4.4 / 4.9 ms and p95 6.2 / 8.1 ms; through the host p50 4.6 / 4.8 ms and
  p95 6.6 / 7.8 ms. The extra hop is within run-to-run noise (under 0.5 ms).

**Is T3 p95 under 50 ms? Yes.** It is 7.0 ms warm and 7.1 ms cold. Even the typing variant
(36.5 ms) is under the budget.

**What T2 would need for the 1 s budget.** The budget in PLAN.md section 9 is "solution to
editable text with syntax highlighting in under 1 s, semantic features stream in after."

- **Editable, highlighted text does not depend on the host at all.** The shell shows the file
  with tree-sitter highlighting.
- **The host's first syntax feature is fast enough.** It is ready at about T0 + T1, roughly
  1.1 s from process start. Getting that under 1 s means trimming about 150 ms of server startup,
  or starting the host before the window is up.
- **Semantic completion arriving in under 1 s would require T2 ≤ about 1 s.** T2 is about Tload
  plus 0.4 s, so the solution would have to load in about 0.5 s instead of 9.2 s: a 15 to 20x
  cut. Roslyn's own loading cannot do that, because it re-runs a design-time build for every
  project on every start.
- **A realistic target is T2 ≤ 3 s for 200 projects (Tload ≤ 2.5 s).** Two changes get there:
  - Persist design-time build results, keyed by a hash of each project file and its imports, and
    replay them on start. Visual Studio's project system caches the same way.
  - Load the open document's project closure first. Today Roslyn has to load all 200 projects
    before `projectInitializationComplete`.

  Until then, semantic features honestly "stream in" at about 10 s on this machine, with
  tree-sitter as the fallback.

### 5. Cancellation: a canceled completion returns within 50 ms and produces no result

**Pass.** Per run, 50 trials each of: completion, `workspace/symbol`, and `textDocument/diagnostic`
after a fresh `didChange` (a request with real work in flight). Each request was sent and
immediately followed by `$/cancelRequest`. Across 20 runs:

| Request | Canceled | Cancel to response, p50 | Worst case |
|---|---|---|---|
| Completion | 985 of 1,000 (15 had already completed before the cancel arrived) | 0.5 ms | 6.4 ms |
| `workspace/symbol` | 1,000 of 1,000 | under 1 ms | 1.4 ms |
| Diagnostic pull | 1,000 of 1,000 | 0.5 ms | 1.3 ms |
| **Late results for canceled ids** | **0** | | |

How it works:
- StreamJsonRpc cancels the forwarded handler's token when `$/cancelRequest` arrives.
- The handler awaits the upstream call with `.WaitAsync(token)`, so it fails at once. The shell
  gets error -32800 (RequestCancelled), never a result.
- StreamJsonRpc separately sends `$/cancelRequest` to Roslyn. Roslyn's own reply to that cancel
  is discarded by the host.
- `LspProxyTests.CanceledRequest_ReturnsRequestCancelledWithin50msAndCancelsUpstream` checks both
  halves: under 50 ms to the shell, and the upstream token canceled. The integration test checks
  under 50 ms against the real server.

**Solution generation.**
- `LspProxy.Generation` counts completed solution loads. It goes up by one on each
  `workspace/projectInitializationComplete`, and its value is 0 until the first load finishes.
- A forwarded request may carry `params.eluditeGeneration`. The host strips that field before
  forwarding.
- If the value does not match the current generation, the request fails at once with -32801
  (ContentModified) and is never forwarded.
- If the generation moves while the request is in flight, the upstream call is canceled and the
  shell gets -32801.

So a stale result is never delivered. The client also drops any response whose generation it
knows to be stale. Both paths are unit-tested
(`StaleGeneration_IsRejectedWithContentModified`,
`GenerationChangeWhileInFlight_CancelsUpstreamAndReturnsContentModified`). The bench client does
not send generations, so it is unaffected. This check is deliberately simple. Real generation
bumps (project file edits, reloads) are follow-up work.

### 6. Roslyn LSP extensions used, and what was awkward

**Extensions used.** None of the `roslyn/`-prefixed methods were needed. The non-standard pieces
used are:

- `solution/open` (client to server notification, `{ solution: <uri> }`): opens the solution
  explicitly. This is what the VS Code C# extension and roslyn.nvim send.
- `workspace/projectInitializationComplete` (server to client notification, **no params**):
  readiness signal after a solution or project load.
- `workspace/configuration` section `projects.dotnet_enable_file_based_programs`, which the host
  answers with `false`. Every other section gets `null`, meaning Roslyn's default.

Present at this commit but not used: `project/open`, `roslyn/resolveContext@2`,
`roslyn/updateLogLevel`, `workspace/_roslyn_restore`, `workspace/_roslyn_restorableProjects`,
`workspace/_roslyn_refreshSourceGenerators`.

**Awkward to build or embed:**

1. **Roslyn `main` needs a .NET 11 RC SDK** (`11.0.100-rc.1.26425.128`), even though the language
   server targets `net10.0`. The eng scripts download that SDK into `.dotnet/`, which works but
   means a pinned bump can silently change the toolchain. The output runs on the system 10.0.10
   runtime (`rollForward: Major`).
2. **Scoped builds work, but pull in Razor.** Scoping with `--solution <csproj>` builds cleanly,
   but it still builds the bundled Razor cohost extension
   (`Microsoft.VisualStudioCode.RazorExtension`). The server registers Razor capabilities at
   startup whether or not Razor is used.
3. **Frozen-partial semantics (the most important finding).**
   - LSP completion calls `WithFrozenPartialSemantics`, and the server has no background compiler.
     A document version whose first completion runs before its dependency compilations exist gets
     no members from referenced projects. It stays that way until some request computes full
     semantics for that document's project.
   - Example: on Probe.cs, `widget.` and `global::Bench.L5.P00.` stayed empty for as long as I polled (up to 8 s after
     the load finished) with completion-only traffic, while `"x".` (BCL members) worked.
   - What fixes it: a `textDocument/diagnostic` pull or a hover on the current document version.
     After one such pull, later versions keep working.
   - So the shell must pull diagnostics on open and on change, as VS Code does. Alternatively, the
     host could proactively compile the open document's project closure after
     `projectInitializationComplete`.
   - The bench and the integration test model this explicitly.
4. **File-based programs capture early-opened files.**
   - With file-based programs on (the default), a `.cs` file opened before the solution finishes
     loading is captured by a "canonical misc" project (`/tmp/roslyn-canonical-misc/...`), which
     runs `dotnet restore` on the fly. The file never moves to its real project.
   - The host turns the feature off through `workspace/configuration`. Afterwards the file
     correctly moves from the misc-files workspace to `Bench.L6.P00` once the solution loads.
5. **`$/progress` collides with StreamJsonRpc.** StreamJsonRpc has built-in `$/progress` handling
   that expects numeric tokens; LSP uses string GUIDs. Every Roslyn progress notification logs a
   deserialization error. Relaying still works, but tracing is off by default for that reason
   (`ELUDITE_LSP_TRACE=1|warn` turns it on).
6. **No-params notifications don't bind.** `workspace/projectInitializationComplete` has no
   `params` member, so it does not bind to a single-object handler. The host registers a
   parameterless overload as well. This was found against the real server; the fake server in
   the unit tests sends `params: []` and passed anyway.
7. **Loading in-process fails** (next section).

### Embed vs spawn

**Decision: spawn**, with the server as a child of eludite-host and LSP proxied over the host's
stdio on the same connection.

- The child runs `dotnet <dll> --stdio --clientProcessId <host pid> --telemetryLevel off`.
- `--clientProcessId` makes the server exit when the host dies. The bench additionally kills the
  whole tree.

**In-process was tried** (a throwaway harness, not committed). It loaded the server's assembly and
invoked its entry point with `--pipe` against an in-process `NamedPipeServerStream`. It failed
three ways:

1. **Separate load context:** the custom `AssemblyLoadContext` with an
   `AssemblyDependencyResolver` failed MEF composition ("Expected 1 export ...
   ExtensionAssemblyManagerMefProvider ... found 0"). The server discovers its MEF parts from
   `AppContext.BaseDirectory`, which in-process is the *host's* directory.
2. **Rebased directory:** overriding `APP_CONTEXT_BASE_DIRECTORY` (process-wide) got past that,
   but the server's `CustomExportAssemblyLoader` then could not find its own assembly "in any
   extension context". It expects to live in the default load context.
3. **Default load context:** loading into the default context with a resolver failed on a
   StreamJsonRpc version conflict. The server ships 2.26.114; the host pins 2.25.29.

Even with all of that fixed, the server's `Program.cs` is top-level statements with no public
API, and it changes process-wide state:
- `StandardHandleInheritance.SetStandardHandlesInheritable(false)`
- `Console.SetOut` in stdio mode
- Windows error mode
- a Server GC setting that only its own runtimeconfig applies

Trade-off:
- **Spawn costs** one more process and one serialization hop. The measured hop is under 0.5 ms
  and within noise. Spawn also means two runtimes' worth of base memory: the host's own RSS is
  about 72 MB.
- **Spawn buys** dependency isolation (the host keeps its own StreamJsonRpc and MSBuild versions),
  crash isolation, and an upgrade path that is just "replace the dll".

Embedding should be revisited only if Roslyn ships a supported hosting API (ADR-0002, "Revisit
when").

### 7. Recommendation on D2, and the follow-up brief

**D2 stands.** The one-process-per-solution .NET host works with the language server built from
source at a pinned commit. The pin built in 2 minutes with one command. The shell-to-host latency
budget is met with a large margin: about 7 ms p95, against 50 ms. Two adjustments to how D2 is
realized:

- The host **supervises** the Roslyn server as a child process rather than embedding it in the
  same process. ADR-0002's wording ("embeds") should say "hosts or supervises".
- Time-to-semantic is load-bound (about 9.6 s for 200 projects), not latency-bound. Caching
  project load results is the main lever for the 1 s experience.

**Follow-up brief: "eludite-host LSP bridge, production cut" (medium, about one agent-week).**
- Specify LSP forwarding, generation and readiness in `protocol/schemas/host-rpc.md` first (the
  gaps below), plus Rust bindings.
- Move the forwarding table, the configuration answers and the server-to-client requests to a
  real implementation:
  - relay the `workspace/*/refresh` requests to the shell;
  - feed settings from the shell instead of hard-coded answers.
- Add host-side compilation warming after load (or a documented diagnostic-pull contract) so
  completion never gets stuck on frozen semantics.
- Add generation bumps on project and solution reloads, with tests.
- Add crash and restart supervision of the child, including a test that kills it mid-request.
- Run on Windows: `build.ps1`, `run.ps1`, and the integration test in the .NET CI job.
- Make the bench part of CI on the reference machine, with T3 p95 and T2 as tracked metrics.

**A separate, larger item (its own brief, medium to large): project-load caching to bring T2
toward 2 to 3 s.** It may need changes upstream in Roslyn's `LanguageServerProjectSystem`, or a
eludite-owned project system feeding Roslyn's `workspace/_roslyn_*` project APIs.

## Protocol gaps (not edited; `protocol/**` is out of scope)

1. **`host-rpc.md` does not describe LSP forwarding at all.** The spike forwards plain LSP 3.17
   method names on the same connection (the list is in `LspProxy.ForwardedRequests` and
   `ForwardedNotifications`). The schema should list them, or say "any non-`eludite/` LSP method".
2. **Name collision on `initialize`, `shutdown` and `exit`.** The eludite handshake and LSP use the
   same method names, so the host owns the upstream LSP handshake. As a result, the shell cannot
   send LSP `ClientCapabilities` or see Roslyn's `ServerCapabilities`. Proposal: add
   `lspClientCapabilities` to `initialize` params and `capabilities.lsp` (Roslyn's server
   capabilities) to the result. `capabilities` stays `{}` today because the existing test pins it
   and the schema leaves its keys unspecified.
3. **Solution generation has no schema.** The spike uses an optional `params.eluditeGeneration`
   and error -32801. The schema needs:
   - where the generation travels (params or a JSON-RPC envelope extension);
   - how the shell learns the current value (for example a `eludite/solution/generation`
     notification);
   - whether responses echo it.
4. **Readiness.** The spike relays Roslyn's `workspace/projectInitializationComplete` verbatim.
   The protocol should either adopt it or define a `eludite/solution/loaded` notification
   carrying the generation.
5. **Server-to-client requests** (`workspace/configuration`, `client/registerCapability`,
   `window/workDoneProgress/create`, the `workspace/*/refresh` requests) are answered by the host.
   The schema should say which ones reach the shell, and where settings come from.
6. **Diagnostics-pull contract.** Because of frozen-partial semantics, either the shell must pull
   diagnostics on open and change, or the host must warm compilations. This needs to be written
   down.
7. **Bulk data on stdio.** Completion at statement level returns 64 to 123 KB of JSON (1,000 items,
   capped by Roslyn's `MaxCompletionListSize`). That was fine here. Semantic tokens for large files
   and the planned symbol index are where the side channel will matter; this spike did not
   measure them.

## Not covered

- Windows, for every criterion: not run on this machine.
- VB.NET feature coverage, which PLAN.md section 4.3 mentions for "Spike 2". It is not in this
  brief's exit criteria and was not tested.
- True cold-disk numbers, which need dropping the page cache (root). Only the first cold run
  approximates them.
- The side channel, source generators beyond what the solution uses, and everything else in the
  brief's "Out of scope".

## Reproduce

```
tools/roslyn-pin/build.sh                    # ~2 min; clone + build at tools/roslyn-pin/COMMIT
bench/roslyn-200/run.sh --prepare-only       # generate, restore (offline), build host + driver
dotnet test dotnet/Eludite.slnx               # includes the integration test once the above exist
bench/roslyn-200/run.sh                      # 10 cold + 10 warm; COLD=3 WARM=3 for a quick run
```
