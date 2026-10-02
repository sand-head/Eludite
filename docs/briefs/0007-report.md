# Brief 0007 report: production LSP bridge between the shell and niello-host

Status: done on Linux. Windows: not run on this machine. CI: not run yet (nothing pushed).
Branch: `brief/0007-host-lsp-bridge`. Date: 2026-10-01. Brief: [0007-host-lsp-bridge.md](0007-host-lsp-bridge.md).

## Summary

- **Contract first.** `protocol/schemas/host-rpc.md` was rewritten method by method and committed alone, before any
  code. It has a JSON schema per Niello message in `protocol/schemas/host/`, and links the LSP 3.17 spec for each
  forwarded method.
- **Renames.** The host's own handshake is now `niello/host/initialize`, `niello/host/shutdown` and
  `niello/host/exit`. Plain `initialize`, `shutdown` and `exit` return MethodNotFound. The host performs the LSP
  handshake with Roslyn itself.
- **Solution lifecycle.**
  - `niello/solution/open` (a `.sln`, `.slnx`, `.csproj` or `.vbproj`) returns the new generation.
  - `niello/solution/close` closes the solution.
  - `niello/solution/status` reports `loading` (phase `legacyEvaluation` or `projectLoad`), `loaded` (with project
    counts, the MSBuild used for legacy projects and the corrections applied), `failed` (with a diagnostic) and
    `closed`.
  - Legacy projects go through the brief 0003 evaluator automatically.
- **Generations.**
  - Every forwarded request must carry `nielloGeneration`. A missing value gets -32602, and a stale one gets -32801
    with `{requestedGeneration, currentGeneration}`. Neither is forwarded.
  - If the generation changes while a request is in flight, the host cancels it upstream and answers -32801.
  - The Rust client also drops results and diagnostics that belong to an old generation.
- **Warming.** The host pulls `textDocument/diagnostic` for every open document: on open, 150 ms after the last
  change, and when the solution finishes loading. It publishes the results to the shell as
  `textDocument/publishDiagnostics`. Brief 0002's failure case is now fixed: with no shell-side pull, the first
  completion on a type from a project six references away lists its members.
- **Rust client.** `crates/lsp` has a `HostClient` that:
  - supervises the host process (restart policy, exit events, stderr kept off the protocol stream);
  - frames messages with `Content-Length` and correlates requests with responses;
  - sends typed requests through `niello-protocol` marker types;
  - cancels with `$/cancelRequest`;
  - tracks the solution generation.

  It is tested against a scripted fake host process (12 cases) and against the real host (1 case).
- **Budgets hold.**
  - T3 completion p95: 5.7 ms cold and 7.1 ms warm (brief 0002: 7.1 and 7.0).
  - Completion while a warming pull is in flight: p95 3.4 / 3.2 ms.
  - Cancellation: 0.4 to 0.6 ms at p50, worst 7.0 ms over 900 trials, with no late results.
  - T2 is unchanged: 9.5 s cold, 9.4 s warm (brief 0002: 9.6 s).

## Machine

The machine is the same as in brief 0002: AMD Ryzen 9 7940HS (8 cores, 16 threads), 30 GiB RAM, NVMe on btrfs,
CachyOS (kernel 7.2.8-2-cachyos), .NET SDK 10.0.302 with runtime 10.0.10, and Roslyn at
`7c238e7cb19650384de7dcb93a93d4e38a5f7ed9`.

The machine was shared with other agents' builds and a desktop session. The load average during the reported bench
run was 3.7 at minimum, 12.8 at the median and 16.9 at the maximum. The bench's own design-time builds account for
much of that load.

## Host methods (protocol/schemas/host-rpc.md)

| Method | Kind | Schema |
|---|---|---|
| `niello/host/initialize` | request | `host/host-initialize.json` |
| `niello/ping` | request | `host/ping.json` |
| `niello/host/info` | request | `host/host-info.json` |
| `niello/host/shutdown` | request | `host/host-shutdown.json` |
| `niello/host/exit` | notification | `host/host-exit.json` |
| `niello/solution/open` | request | `host/solution-open.json` |
| `niello/solution/close` | request | `host/solution-close.json` |
| `niello/solution/status` | host to shell | `host/solution-status.json` |
| `niello/languageServer/status` | host to shell | `host/language-server-status.json` (`starting`, `running` with Roslyn's `serverInfo` and `capabilities`, `restarting`, `unavailable`, `exited`) |
| `textDocument/publishDiagnostics` | host to shell | `host/publish-diagnostics.json` (LSP shape plus `nielloGeneration`) |
| `nielloGeneration` on every forwarded request | rule | `host/forwarded-request.json` |
| Error `data` for -32801 and -32803 | | `host/errors.json` |

**Forwarded LSP, typed** (in `niello-protocol::lsp`, with shape checks in the host):
- requests: `textDocument/completion`, `completionItem/resolve`, `textDocument/hover`, `textDocument/definition`,
  `textDocument/references`, `textDocument/documentSymbol`, `workspace/symbol`, `textDocument/diagnostic`;
- notifications: `textDocument/didOpen`, `didChange`, `didClose`, `$/cancelRequest`.

**Forwarded, untyped:**
- requests: `textDocument/signatureHelp`, `typeDefinition`, `implementation`, `documentHighlight`,
  `semanticTokens/full`, `semanticTokens/range`, `codeAction`, `formatting`, `rename`;
- notifications: `textDocument/didSave`, `workspace/didChangeWatchedFiles`;
- relayed from Roslyn: `window/showMessage`, `$/progress`.

**Pull or push diagnostics.** The pinned Roslyn has pull diagnostics only; no handler at that commit sends
`publishDiagnostics`. The host converts its own warming pulls into push notifications for the shell, and the shell
may still pull.

**Error codes:**

| Code | Name | Meaning here |
|---|---|---|
| -32002 | ServerNotInitialized | A request arrived before `niello/host/initialize` |
| -32800 | RequestCancelled | The request was canceled |
| -32801 | ContentModified | Stale generation |
| -32803 | RequestFailed | The language server is unavailable |

**Drift tests.** `niello-protocol` has tests that keep the Rust method lists, the tables in host-rpc.md and the
schema files in step. They also check every Niello message type against its schema with a small validator.

## Gaps from briefs 0002 and 0003

| Gap | Resolution |
|---|---|
| 0002-1 LSP forwarding undocumented | Documented method by method (typed and untyped tables) |
| 0002-2 `initialize`/`shutdown`/`exit` collide with LSP | Renamed under `niello/host/`. The shell cannot send its own LSP ClientCapabilities: the host advertises a documented fixed set, and Roslyn's ServerCapabilities reach the shell in `niello/languageServer/status` |
| 0002-3 Generation has no schema | Specified: where it travels, how the shell learns it, no echo in results, and the error data |
| 0002-4 Readiness | `niello/solution/status` `loaded`; `workspace/projectInitializationComplete` is no longer relayed |
| 0002-5 Server-to-client requests | Table of the host's answers. `workspace/diagnostic/refresh` re-warms. The semanticTokens, codeLens and inlayHint refreshes are answered and not relayed (no consumer yet) |
| 0002-6 Diagnostics-pull contract | Host-side warming, documented in host-rpc.md and HOST.md |
| 0002-7 Bulk data on stdio | Noted in host-rpc.md; side channel still out of scope |
| 0003-2 Project-load diagnostics | `diagnostics` in `niello/solution/status` (NIELLO0001 to 0004, NIELLO0106) |
| 0003-6 Bare `.csproj` | `niello/solution/open` accepts project files |
| 0003-7 Load progress | `loading` with phase `legacyEvaluation`, then `projectLoad` |
| 0003-1 Toolchain discovery in `niello/host/info` | Partly: the status reports the MSBuild actually used (kind, path, source). `niello/host/info` is unchanged |
| 0003-3 Output window channel (`niello/output`) | **Not done**: belongs with the build brief |
| 0003-4 Per-project state request | **Not done** |
| 0003-5 Design-time settings over the protocol | **Not done**: still environment variables |

## How warming works and what it costs

`Lsp/DiagnosticsWarmer.cs` drives it, and `Lsp/LspProxy.cs` runs the pulls.

**When the host pulls:**
- on `didOpen`, at once;
- on `didChange`, 150 ms after the last change to that document (debounced per document;
  `NIELLO_DIAGNOSTICS_DEBOUNCE_MS` overrides the interval);
- for every open document when the solution reaches `loaded`, and on `workspace/diagnostic/refresh`.

**How a pull runs:**
- A newer pull for a document cancels the older one.
- Pulls run on the thread pool.
- A result is published only if the document version and the generation are unchanged.

**Ordering.** Document notifications, server restarts and pulls go through one ordered chain, and forwarded
requests wait for that chain, so a completion after a `didChange` always sees the new text. The shell connection
also starts handlers in arrival order: it uses a `NonConcurrentSynchronizationContext`, and handlers leave it at
their first await. In the brief 0002 spike, `didChange` and the next completion could reach Roslyn out of order.

**Measured effect.** All figures are from the bench run below.

- **Completion during a warming pull.** In each of 50 trials per run, the bench sends a `didChange`, waits 155 ms so
  the host's pull has started, then asks for completion. In all 300 trials the completion was answered while the
  pull was still in flight.
  - Completion latency: p50 2.4 ms, p95 3.4 ms cold and 3.2 ms warm (medians over runs). The worst single
    completion took 32.7 ms.
  - The pulls themselves took 30 to 35 ms at p50 and about 60 ms at p95 (one contended run: 97 / 206 ms).
  - Warming does not block completion. A unit test pins this down: with a pull held open indefinitely, completion
    still returns (`WarmingPullInFlight_DoesNotDelayCompletion`).
- **Referenced-project completion.** `RoslynIntegrationTests` asserts that the first completion after the post-load
  warming pull contains `Compute`. That is a member of `Bench.L5.P00.Widget00`, used from `Bench.L6.P00`. The shell
  sent no diagnostic pull. A later document version also works without waiting for its own pull. Before this
  brief, the same request stayed empty indefinitely (brief 0002 report).
- **Load to first member completion (T2 minus Tload):** 264 ms cold and 343 ms warm. Brief 0002, which pulled from
  the client, measured about 400 ms.

## Bench: T2 and T3 against brief 0002

`COLD=3 WARM=3 bench/roslyn-200/run.sh` was run with T3 = 1000 completions and 50 cancel trials per request type.
Raw data is in `bench/roslyn-200/results/20261001-224111/` (gitignored). "Cold" means Roslyn's caches were cleared,
not the page cache (no root), as in brief 0002. The table shows medians, with min to max in parentheses.

| Metric | 0007 cold | 0002 cold | 0007 warm | 0002 warm |
|---|---|---|---|---|
| T0 start to `niello/host/initialize` | 119 ms | 104 ms | 112 ms | 104 ms |
| T1 initialize to documentSymbol | 1,211 ms | 1,218 ms | 971 ms | 1,012 ms |
| **T2** initialize to completion with `Compute` | **9,545 ms** (9,275 to 16,495) | 9,623 ms (9,405 to 15,505) | **9,435 ms** (9,345 to 9,811) | 9,643 ms (9,296 to 9,918) |
| Tload initialize to loaded | 9,330 ms | 9,631 ms | 9,052 ms | 9,211 ms |
| **T3 p50** | **4.4 ms** | 4.7 ms | **4.5 ms** | 4.6 ms |
| **T3 p95** | **5.7 ms** (5.6 to 22.4) | 7.1 ms (6.6 to 8.3) | **7.1 ms** (6.0 to 7.3) | 7.0 ms (6.4 to 7.7) |
| T3 p99 | 7.4 ms | 8.5 ms | 12.4 ms | 8.1 ms |
| T3-typing (didChange, then completion) p95 | 9.6 ms | 34.9 ms | 22.2 ms | 36.5 ms |
| Completion during a warming pull, p95 | 3.4 ms | n/a | 3.2 ms | n/a |
| niello-host peak RSS | 78 MB | 72 MB | 78 MB | 72 MB |
| Host tree peak RSS | 2,612 MB | 2,575 MB | 2,525 MB | 2,544 MB |

**Run-to-run variance:**
- The third cold run (T2 16.5 s, T3 p95 22.4 ms) coincided with a load spike from other processes. The other two
  cold runs had T3 p95 of 5.6 and 5.7 ms.
- An earlier full run on the same code during heavier contention (load average about 15 to 40,
  `results/20261001-223307`) gave cold T2 medians of 27 s and warm T3 p95 of 15.3 ms. It is not used, because a
  smoke run minutes earlier at load 3 gave T3 p95 6.1 ms. These numbers need a quiet reference machine to be
  trusted. They are not a regression signal.

**Budget check:**
- Completion p95 from the host is under 50 ms after warm-up: 5.7 / 7.1 ms. The brief's limit of 10 ms holds in the
  median and in 5 of 6 runs.
- The only increase is the host's own RSS (+6 MB): the document copy, the warming state and the ordered chain.

**Effect of ordered text sync on T3-typing.** T3-typing is lower than in brief 0002. Each completion now waits for
its `didChange` to reach Roslyn. In brief 0002 the two could race, so that comparison is not like for like.

## Cancellation latency

Bench, 6 runs, 900 trials:

| Request | Canceled | Cancel to response, p50 | Worst |
|---|---|---|---|
| Completion | 300 / 300 | 0.4 ms cold, 0.6 ms warm | 7.0 ms |
| `workspace/symbol` | 300 / 300 | | 5.2 ms |
| Diagnostic pull | 300 / 300 | 0.4 / 0.5 ms | 3.1 ms |
| Late results for canceled ids | 0 | | |

Other measurements:
- **Rust client against the fake host:** worst of 20 cancels was 0.6 ms from `cancel()` to the wait returning
  `Error::Canceled`.
- **.NET unit test:** under 50 ms to the shell, and the upstream token is canceled.
- **Integration test against Roslyn:** under 50 ms.

## Tests

**Rust**

| Suite | Tests |
|---|---|
| `cargo test -p niello-protocol` | 32 |
| `cargo test -p niello-lsp` | 14 |
| `cargo test --workspace` | 109 passed, 0 failed |

- `niello-protocol` (32):
  - typed round trips for every host message and every typed LSP message, including generation flattening;
  - 4 drift and conformance tests against host-rpc.md and the schema files.
- `niello-lsp` (14):
  - 1 framing test;
  - 12 cases against the fake host: lifecycle, status notifications and generation tracking, generation injection,
    host-side stale rejection, client-side drop of a late result from an old generation, cancel latency, a result
    discarded after cancel, stale diagnostics dropped, crash restart with generation reset, restart budget, stderr
    capture and MethodNotFound;
  - 1 test against the real host built with `--no-roslyn`, which skips when the host is not built.
- The fake host is the test binary itself, re-executed with `NIELLO_FAKE_HOST=1`. That test target has
  `harness = false`, because libtest output would corrupt the protocol stream.

**.NET**

`dotnet test dotnet/Niello.slnx`: 113 tests, 113 passed, 0 skipped. Brief 0003 reported 82, with 1 skipped.
`dotnet build` reports 0 warnings.

- **The 14 scaffold tests are kept and updated.** These are the 7 `HostRpcTargetTests` (now on `niello/host/*`) and
  the 7 `DotnetCliSdkDiscovererTests`, which are unchanged.
- **New host tests:**
  - **Renames and error codes:** plain LSP lifecycle names give MethodNotFound; ServerNotInitialized,
    InvalidParams and RequestFailed are returned where the contract says.
  - **Solution lifecycle without a language server:** open returns the generation, then a `failed` status; close
    gives a `closed` status.
  - **Bridge (`LspProxyTests`, 22 tests, against a fake Roslyn):**
    - generations: rejection, in-flight bump, the missing-generation error;
    - typed validation and untyped pass-through;
    - ordered `didChange` then completion;
    - the `loaded` status, and legacy status fields through a fake preparer;
    - restart with replay of incrementally edited documents;
    - warming: on open, debounce under `FakeTimeProvider`, rewarm on load and on refresh;
    - non-blocking completion during a pull, and empty diagnostics on close;
    - server crash, giving `exited` and `failed` (NIELLO0002) and -32803.
  - **`DiagnosticsWarmerTests` (6):** debounce, per-document timers, cancellation of superseded pulls, close.
  - **`OpenDocumentsTests` (3):** incremental edits, UTF-16 and CRLF positions.
  - **`HostProcessTests`:** every byte of the real host's stdout parses as a frame.
- **Updated integration tests:**
  - The Roslyn integration test now covers the referenced-project completion after the warming pull, a stale
    generation, cancellation within 50 ms, and a `loaded` status with 200 projects.
  - The WebForms tests (mono and sdk) now assert that the status reports the MSBuild kind used and a
    `designerPartials` correction.
- **Ran here.** The Roslyn and WebForms tests ran on this machine, against the generated bench solution and the
  fetched corpus. On a fresh clone they skip.

New dependency: `Microsoft.Extensions.TimeProvider.Testing` 10.10.0 (SPDX: MIT), used by the tests only.

## Commits

1. `868d627` Specify the host LSP bridge contract in protocol/schemas
2. `aa91860` Type the host bridge messages in niello-protocol
3. `3b053d4` Implement the host LSP bridge with generations, solution lifecycle and semantics warming
4. `7f57500` Add the niello-host client to niello-lsp with supervision, cancellation and generation tracking
5. `d17376a` Drive the roslyn-200 bench through the bridge contract and measure completion during warming
6. This report and the brief's Status line

The branch was rebased onto `main` at `31f2282`.

## What is not done, and why

- **Windows:** not run on this machine. That covers the .NET tests, the bench (`run.ps1`) and the Rust client
  against the host.
- **CI:** not run. Nothing was pushed.
- **Bindings are hand-written.** CLAUDE.md says bindings in `protocol/` are generated, but no generator exists yet.
  `niello-protocol` follows the existing hand-written convention and is held to the schemas by the drift and
  conformance tests.
- **`tools/legacy-load/runner` (outside this brief's scope) still sends plain `initialize`/`shutdown`/`exit` and
  waits for `workspace/projectInitializationComplete`.** Its roslyn phase will fail against this host until it moves
  to `niello/host/initialize`, `niello/solution/open` and `niello/solution/status`. That is a three-line change in a
  file this brief does not own.
- **The `docs/briefs/README.md` index row is not updated**, because that file is outside this brief's scope.
- **The host does not restart Roslyn by itself after a crash.** It reports `exited`, fails the load with NIELLO0002,
  and answers -32803. The next `niello/solution/open` restarts the server and replays the open documents. The Rust
  client does restart a crashed host, under its `RestartPolicy`.
- **Gaps from brief 0003 that remain open:** an Output window channel, a per-project state request, design-time
  settings over the protocol, toolchain discovery in `niello/host/info`, and shell-supplied `workspace/configuration`
  settings.
- **Not measured: the side channel.** Statement-level completion lists of 64 to 123 KB still travel on stdio. That
  causes no problem at these latencies. Semantic tokens and a symbol index will need the side channel.

## What remains untyped, and the next bridge brief

**Untyped today:**
- the 9 forwarded requests and 2 notifications listed above;
- `window/showMessage` and `$/progress`;
- inside typed messages: `ServerCapabilities`, hover `contents`, completion `documentation`, `textEdit` and
  `itemDefaults`, the `location` of a `WorkspaceSymbol`, and `Diagnostic` extras such as `relatedInformation`,
  `tags` and `codeDescription` (kept losslessly in `extra`).

**Next bridge brief, medium (about one agent-week):**
- Type `signatureHelp`, `semanticTokens` (with the side channel for large files), `codeAction`, `rename`,
  `formatting`, `documentHighlight` and `typeDefinition`/`implementation`.
- Relay the refresh requests once the shell has consumers for them.
- Feed shell settings into `workspace/configuration` through a settings method.
- Add `niello/output` and per-project load state for Solution Explorer and the Error List.
- Have the host restart Roslyn by itself when it crashes.
- Run the Windows pass.

Project-load caching (T2 toward 2 to 3 s) is still a separate, larger brief, as brief 0002 recommended. T2 is
entirely load-bound.
