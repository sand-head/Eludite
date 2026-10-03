# eludite-host JSON-RPC contract

Contract between the Eludite shell (client) and `eludite-host` (server), per PLAN.md D2 and ADR-0002/ADR-0003.
Every method the host accepts or sends is listed here. Eludite-specific messages have a JSON schema in
[`host/`](host/); forwarded LSP messages follow the
[LSP 3.17 specification](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/)
plus the generation rule below.

Rust mirror: `protocol/rust/src/host.rs` (Eludite messages) and `protocol/rust/src/lsp.rs` (the typed subset of LSP),
crate `eludite-protocol`.

## Transport

- JSON-RPC 2.0 over the host's stdin and stdout, framed with `Content-Length` headers exactly as LSP base protocol
  messages. Bodies are UTF-8 JSON. Field names are camelCase.
- The host's stdout carries protocol messages only. Host logs go to stderr; the Roslyn language server's own logs go
  to stderr (prefixed `[roslyn-ls]`) and to `<temp>/eludite-host/roslyn-logs/` (see `dotnet/src/Eludite.Host/HOST.md`).
- Nothing in the host is modal. Failures are error responses, `eludite/solution/status` or
  `eludite/languageServer/status` notifications, or log lines.
- There is no side channel yet. Large results (a statement-level completion list is 64 to 123 KB) travel on stdio.

## Lifecycle

1. The shell spawns the host and sends `eludite/host/initialize`. The host replies at once and starts its language
   server in the background (`eludite/languageServer/status` reports progress).
2. The shell sends `eludite/solution/open`. The reply carries the new solution generation; `eludite/solution/status`
   notifications report loading, loaded or failed for that generation.
3. The shell sends LSP traffic (below). Every forwarded request carries the generation it was issued under.
4. `eludite/host/shutdown`, then `eludite/host/exit`.

Plain LSP method names (`initialize`, `shutdown`, `exit`, `textDocument/*`, `workspace/*`, ...) are reserved for the
forwarded language server. The host performs the LSP `initialize`/`initialized` handshake with the language server
itself; the shell never sends LSP `initialize`, `initialized`, `shutdown` or `exit` (they return MethodNotFound).

## Solution generation

A generation is a non-negative integer that names one state of the loaded solution. A result computed under one
generation is never valid under another.

- It is `0` after `eludite/host/initialize`.
- It increases by one on every `eludite/solution/open` (the reply carries the new value) and on every
  `eludite/solution/close` of an open solution.
- A solution finishing its load does **not** change the generation: `eludite/solution/status` with state `loaded`
  carries the same generation the `open` returned.
- The shell learns the current value from the `open` and `close` replies and from `eludite/solution/status`.

Rules:
- **Every forwarded LSP request** (typed or untyped, below) **must** carry `eluditeGeneration` as a top-level member
  of its `params` object (schema: [`host/forwarded-request.json`](host/forwarded-request.json)). The host removes it
  before forwarding, so the language server sees plain LSP.
- Missing or non-integer `eluditeGeneration`: error `-32602` (InvalidParams); nothing is forwarded.
- A value that is not the current generation: error `-32801` (ContentModified) with
  `data: { "requestedGeneration": n, "currentGeneration": m }`; nothing is forwarded.
- If the generation changes while a request is in flight, the host cancels it upstream and answers `-32801`. A
  result computed under an old generation is never delivered.
- Responses do not echo the generation; the shell knows what it sent. A shell must still drop a result whose
  generation is no longer current when it arrives (CLAUDE.md invariant 12).
- Forwarded notifications (`didOpen`, `didChange`, ...) do not carry a generation and are never dropped: document
  text is independent of the solution state. A stray `eluditeGeneration` on a notification is removed.
- Host-to-shell notifications that depend on solution state (`eludite/solution/status`,
  `textDocument/publishDiagnostics`) carry the generation they were computed under.

## Cancellation

- The shell cancels a request with the LSP notification `$/cancelRequest` `{ "id": <request id> }`.
- The host answers the canceled request with error `-32800` (RequestCancelled) at once (budget 50 ms; measured
  well under 1 ms in brief 0002) and sends `$/cancelRequest` to the language server for the forwarded copy. The
  language server's reply to that copy is discarded.
- After the `-32800` error the host never sends a result for that id. If the result was already written when the
  cancel arrived, the shell receives that result instead of the error; the shell must discard results for ids it
  canceled.
- `$/cancelRequest` for an Eludite method (for example `eludite/host/info`) cancels it the same way.

## Error codes

| Code | Name | When |
|---|---|---|
| -32700, -32600, -32601, -32602, -32603 | JSON-RPC | As in JSON-RPC 2.0. -32601 for every method not listed here. |
| -32002 | ServerNotInitialized (LSP) | `eludite/solution/*` or a forwarded request before `eludite/host/initialize` |
| -32800 | RequestCancelled (LSP) | The request was canceled with `$/cancelRequest` |
| -32801 | ContentModified (LSP) | Stale `eluditeGeneration`, or the generation changed while the request was in flight |
| -32803 | RequestFailed (LSP) | The language server is unavailable (not configured, failed to start, or exited); `data.reason` is `"languageServerUnavailable"` |
| -32010 | BuildInProgress (Eludite) | `eludite/build/start` while a build runs; `data.buildId` is the running build |
| -32012 | TestRunInProgress (Eludite) | `eludite/test/run` naming a container that a run is running; `data` is `{ runId, container }` |

Error `data` shapes: [`host/errors.json`](host/errors.json).

## Methods the host accepts

### Eludite methods

| Method | Kind | Schema | Params | Result |
|---|---|---|---|---|
| `eludite/host/initialize` | request | [host-initialize.json](host/host-initialize.json) | `{ clientName, clientVersion }` | `{ hostName: "eludite-host", hostVersion, capabilities: { languageServer } }` |
| `eludite/ping` | request | [ping.json](host/ping.json) | none | `{ pong: true, timestamp }` |
| `eludite/host/info` | request | [host-info.json](host/host-info.json) | none | `{ dotnetSdks: [{ version, path }], runtime, os }` |
| `eludite/host/shutdown` | request | [host-shutdown.json](host/host-shutdown.json) | none | `null` |
| `eludite/host/exit` | notification | [host-exit.json](host/host-exit.json) | none | (none) |
| `eludite/solution/open` | request | [solution-open.json](host/solution-open.json) | `{ path }` | `{ generation }` |
| `eludite/solution/close` | request | [solution-close.json](host/solution-close.json) | none | `{ generation }` |
| `eludite/solution/tree` | request | [solution-tree.json](host/solution-tree.json) | none | `{ generation, path, projects: [{ name, path, kind, web, targetFrameworks, files: [{ path, itemType, dependentUpon?, link? }], error? }] }` |
| `eludite/build/start` | request | [build-start.json](host/build-start.json) | `{ target, system?, project?, configuration?, platform? }` | `{ buildId, generation, system?, path, target, configuration, platform, toolchain: { kind, path?, source? }, binlog, commandLine }` |
| `eludite/build/cancel` | request | [build-cancel.json](host/build-cancel.json) | `{ buildId? }` | `{ canceled, buildId? }` |
| `eludite/build/status` | request | [build-status.json](host/build-status.json) | none | `{ running: { buildId, generation, path, target, configuration, platform, toolchain, binlog, commandLine, elapsedMs, progress?, output: { firstSeq, nextSeq, text, truncated } } \| null, last?: { buildId, generation, target, path, result, elapsedMs, summary } }` |
| `eludite/test/discover` | request | [test-discover.json](host/test-discover.json) | `{ projects?, configuration?, runSettings?, vstestConsolePath? }` | `{ runId, generation, containers: [{ id, name, project, targetFramework, protocol, runtime?, program?, error? }] }` |
| `eludite/test/run` | request | [test-run.json](host/test-run.json) | `{ containers?: [{ id, tests? }], debug?, parallel?, configuration?, runSettings?, vstestConsolePath? }` | `{ runId, generation, containers, debug? }` |
| `eludite/test/cancel` | request | [test-cancel.json](host/test-cancel.json) | `{ runId? }` | `{ canceled, runId? }` |
| `eludite/test/attached` | request | [test-attached.json](host/test-attached.json) | `{ runId, processId, attached, message? }` | `{ accepted }` |
| `eludite/test/status` | request | [test-status.json](host/test-status.json) | none | `{ running: [{ runId, kind, generation, debug?, containers, elapsedMs, nextSeq, tests, results }], last?: { runId, kind, generation, state, summary, elapsedMs } }` |

#### `eludite/host/initialize`

The eludite handshake (renamed from `initialize` in brief 0007 so the LSP name stays with the language server). The
host answers immediately and, when a language server is configured, starts it in the background.
`capabilities.languageServer` is `true` when one is configured; whether it actually started is reported by
`eludite/languageServer/status`. Unknown params members are ignored. A second `eludite/host/initialize` returns the same
result and does nothing else.

#### `eludite/ping`

Liveness check. `timestamp` is ISO-8601 UTC from the host clock.

#### `eludite/host/info`

The .NET SDKs the host found (`dotnet --list-sdks`), the host's runtime description and the OS description.

#### `eludite/host/shutdown`

Stops the language server (LSP `shutdown`/`exit`, then kill after 5 s) when `eludite/host/exit` arrives. The host
stays alive until `eludite/host/exit`. Result `null`.

#### `eludite/host/exit`

Notification. The host process exits: code 0 if `eludite/host/shutdown` came first, else 1. A closed stdin is treated
as `exit` without `shutdown`.

#### `eludite/solution/open`

`path` is an absolute or host-relative path to a `.sln`, `.slnx` or project file (`.csproj`, `.vbproj`). The host:

1. Increments the generation and replies `{ generation }` at once. Requests in flight under the old generation
   fail with -32801.
2. If a solution was opened before in this host, restarts the language server (Roslyn cannot unload a solution) and
   replays `textDocument/didOpen` for every document the shell has open.
3. Sends `eludite/solution/status` `loading` (phase `legacyEvaluation` when the solution has legacy, non-SDK
   projects, then `projectLoad`).
4. Prepares legacy projects with the brief 0003 evaluator (Mono's MSBuild when located, Build Tools' MSBuild on
   Windows, else the .NET SDK's MSBuild in-process): designer partials, path-case fixups, COM references removed off
   Windows. Preparation problems are diagnostics, not failures.
5. Opens the solution in the language server (Roslyn's `solution/open`, or `project/open` for a project file).
6. Sends `eludite/solution/status` `loaded` when Roslyn reports `workspace/projectInitializationComplete`, with
   counts, the MSBuild used and the corrections applied; or `failed` with a diagnostic.

Errors: -32602 when `path` is missing, does not exist or has another extension; -32002 before
`eludite/host/initialize`. A missing language server is not an error response: the reply carries the generation and a
`failed` status follows.

#### `eludite/solution/close`

Closes the open solution: increments the generation, replies `{ generation }`, sends `eludite/solution/status`
`closed`, and restarts the language server without a solution (open documents are replayed as miscellaneous files).
When no solution is open it changes nothing and returns the current generation.

#### `eludite/solution/tree`

The projects of the open solution with their source files, for Workspace (brief 0012). The host computes it
from its own MSBuild evaluation, in the background and once per generation, so the shell can send the request right
after `eludite/solution/open` and get the answer when the evaluation finishes. It does not wait for the language
server's load.

- Projects are the `.csproj` files the solution lists, in solution order (a project file opened directly is a
  one-project tree). Each has `name` (the file name without extension), `path`, `kind` (`sdk` or `legacy`), `web`,
  `targetFrameworks` (short monikers: `net10.0`; `net48` for a legacy `TargetFrameworkVersion` of `v4.8`) and `files`.
- `files` are the `Compile` items, plus the `Content` items of web projects (`Microsoft.NET.Sdk.Web`, a WebForms or
  MVC project type GUID, or WebForms markup items), as absolute paths. `dependentUpon` is the `DependentUpon`
  metadata resolved to an absolute path; `link` is the `Link` metadata of a file outside the project directory.
  Generated files under `obj/` are not listed.
- Evaluation: the .NET SDK's MSBuild in-process (`Microsoft.Build.Locator`), evaluation only (no targets run), with
  missing imports ignored. Legacy projects get the brief 0003 design-time properties (`TargetFrameworkRootPath` from
  the reference-assembly packages); the designer partials and case fixups of the preparation are not part of the
  tree. A project that does not evaluate is listed with `error` and no files.
- `generation` is the generation the tree was computed under; `path` is `null` and `projects` empty when no
  solution is open.

Errors: -32002 before `eludite/host/initialize`; -32801 (ContentModified, with the usual `data`) when the
generation changes before the tree is ready; -32800 when canceled.

#### Build (brief 0017)

`eludite/build/start` builds the open solution, or one of its projects, **out of process** (CLAUDE.md invariant 2):

- **Toolchain.** `dotnet build` (`dotnet build -t:Rebuild` for `rebuild`, `dotnet clean` for `clean`) when every project
  of the solution is SDK-style. When the solution has legacy (non-SDK) projects, the MSBuild the host located for them
  (brief 0003): Mono's `MSBuild.dll` run with `mono` and the relocation environment of a user-space Mono
  (`MonoInstallation.EnvironmentFor`: `PATH`, `LD_LIBRARY_PATH`, `MONO_CFG_DIR`, `MONO_GAC_PREFIX`) plus
  `TargetFrameworkRootPath` from the reference-assembly packages; Build Tools' `MSBuild.exe` on Windows (located, never
  shipped: invariant 9). Without either, legacy solutions build with `dotnet build`, and the finished diagnostics say
  so (`ELUDITE0111`). Restore runs as part of the build (`-restore`). Every run gets `-nologo -v:m -nr:false
  -clp:ForceNoAlign` and `-tl:off` (the SDK's terminal logger is never used), `-bl:<file>` for the binary log, and
  `-p:Configuration=` / `-p:Platform=` when given. Node reuse is off so that cancel can kill every node.
- **Reply at once.** The reply carries the `buildId`, the generation, the toolchain, the binary log path and the
  command line. The build runs in the background; nothing else waits for it.
- **One build at a time per host.** A second `eludite/build/start` while one runs fails with -32010
  (BuildInProgress, `data: { buildId }`). Errors also: -32002 before `eludite/host/initialize`; -32602 when no
  solution is open, `project` is not a project of the open solution, or `target` is unknown.
- **Output: `eludite/build/output`**, chunked and ordered. The first chunk is the host's own start line
  (`Build started at <time>...` and the command line), sent before MSBuild is spawned, so the shell shows a line at
  once. Then MSBuild's stdout and stderr lines in arrival order. A chunk holds whole lines and is flushed when it
  reaches **16 KiB**, or **16 ms** after its first line, whichever comes first; `seq` counts chunks from 0.
  **Backpressure:** the host never drops lines. A single sender awaits each notification's write to its stdout; while
  the shell (or the pipe) is slow, lines read from MSBuild accumulate in the pending chunk, which is sent whole when
  the sender is free (one larger message instead of many). If 8 MiB accumulate unsent, the host stops reading
  MSBuild's output until the sender catches up, which pauses MSBuild on its own console write rather than growing
  the host's memory.
- **Progress: `eludite/build/progress`**, at most every 100 ms while it changes: projects completed of the total
  (a `Name -> output` line completes a project), and the errors and warnings counted from MSBuild's canonical
  `error` / `warning` lines. Advisory only.
- **Finished: `eludite/build/finished`**, exactly once per accepted start, after the last output chunk: `result`
  (`succeeded`, `failed`, `canceled`), the exit code, the time, per-project results with their time, and the
  diagnostics with file, line, column, code, message and project. They come from the binary log, read with MSBuild's
  own reader (`Microsoft.Build.Logging.BinaryLogReplayEventSource`, MIT, the SDK's MSBuild loaded by
  `Microsoft.Build.Locator`), so they are exactly MSBuild's; a log that cannot be read (MSBuild did not start, or the
  build was killed) falls back to the canonical lines of the console output. A multi-targeted project reports one
  result, and an error reported once per target framework is listed once. The output's last lines are Visual
  Studio's summary (`========== Build: 1 succeeded, 0 failed ==========`).
- **Cancel: `eludite/build/cancel`** kills the MSBuild process tree (on Linux and macOS `Process.Kill` of the whole
  tree; on Windows `taskkill /T /F /PID`, untested) and `eludite/build/finished` reports `canceled` within 2 s. A new
  generation (`eludite/solution/open` of another solution, or `eludite/solution/close`) cancels the running build the
  same way.
- **`system` (brief 0019).** The host builds .NET solutions only: `system` is `msbuild` or omitted, and the shell never
  sends `cargo`. The `eludite/build/*` shapes (start result, output, progress, finished) are also what the shell's own
  Cargo runner produces in process for a Cargo workspace (see "Generic language servers and Cargo"), with `system`
  `cargo` and toolchain kind `cargo`, so one Output and Error List path serves both build systems.
- **Status: `eludite/build/status`** (brief 0020) answers the running build, if any, with the start result's members,
  the time since it started, the last progress, and its output so far (`output.text`, the chunks `firstSeq` to
  `nextSeq - 1` concatenated; the host keeps the last 8 MiB of a running build's output and drops older whole chunks,
  setting `truncated`), plus the last finished build's result and summary (`last`, without its diagnostics). It
  never fails, and before `eludite/host/initialize` it reports no build. A shell that (re)connects calls it after
  `eludite/host/initialize`: it clears its Build output, appends `output.text`, and then applies only
  `eludite/build/output` chunks with `seq >= nextSeq` (a chunk the host sent before the reply is already in the text,
  whichever arrives first). When the shell believed a build was running and `running` is null (the host process was
  restarted, which ends its builds), the shell reports that build as ended. Today the host is the shell's child over
  stdio, so a restarted host never has the old build; the replay serves a host the shell reattaches to (D7's remote
  hosts) and is tested against the fake host.
- **Windows-only targets under Mono.** When a legacy project fails on a Windows-only target, the build's raw errors
  for it are replaced by **one diagnostic per project** with the brief 0003 code and message, and the same message
  is written to the output (instead of a task's stack trace, whose lines are dropped from the output):

  | Code | Severity | Recognized by |
  |---|---|---|
  | `ELUDITE0101` | warning | `ResolveComReference` failing (AxImp, TlbImp, MSB3283/MSB3290, `COMReference` items) |
  | `ELUDITE0102` | error | `Microsoft.Web.Publishing.targets` or `$(WebPublishingTasks)` missing (MSB4019, MSB4022, MSB4062) |
  | `ELUDITE0103` | warning | `Microsoft.WebApplication.targets` missing (MSB4019) |
  | `ELUDITE0108` | error | A `PreBuildEvent` or `PostBuildEvent` that is a Windows command script failing (MSB3073) |
  | `ELUDITE0109` | warning | `GenerateSerializationAssemblies` / SGen failing |
  | `ELUDITE0110` | warning | `aspnet_compiler` (`MvcBuildViews`, `AspNetCompiler`) failing |
  | `ELUDITE0111` | warning | The solution has legacy projects and no Mono or Build Tools MSBuild was located |

#### Tests (brief 0035)

The Test Explorer's .NET half (PLAN.md 4.6, D2, D3). The host discovers and runs tests **out of process** (CLAUDE.md
invariant 2): each test application, `vstest.console` and its testhosts are the host's child processes; the shell never
starts a .NET test runner itself, and runs `cargo test` for Rust through its own Cargo path (below).

- **Containers.** `eludite/test/discover` reads the open solution's projects (or the ones named) and keeps the test
  projects (`TestProjectInspector`, from the project file alone: MTP when an MTP runner property is true, the project
  uses `MSTest.Sdk`, or it references `Microsoft.Testing.Platform*` or `xunit.v3*`; VSTest when it references only
  `Microsoft.NET.Test.Sdk` or sets `UseVSTest`). Each test project is one **container per target framework** (a
  multi-targeted project is listed once per framework, `Name (net10.0)`, as Visual Studio's Test Explorer lists it), with
  its build output in `bin/<configuration>/<tfm>/`: the apphost (or the DLL, run with `dotnet`) for a CoreCLR MTP test
  application, the `.exe` run with the located `mono` for .NET Framework off Windows, the test DLL as VSTest's source.
  A container whose output does not exist (`not built`) or that needs a missing Mono is listed with `error` and finishes
  failed; the shell builds first when it wants a fresh build (the host discovers the outputs as they are).
- **Reply at once, then stream.** Discover and run answer with a `runId` (one counter for both), the generation and the
  containers, and stream `eludite/test/update` notifications, `seq` from 0 per run: `discovered` (a container's tests),
  `results` (outcomes as tests start and end), `output` (the runners' log lines for the Output window's Tests source),
  `launch` (a debug run's command line), `containerFinished` (each container, with its count, or `failed` with the
  reason) and exactly one `finished` (`completed`, `failed` or `canceled`, with the summary). Tests and results are
  batched: a burst is flushed as one update every 16 ms.
- **The model.** Both protocols map to one test item (`id`, `displayName`, `fullyQualifiedName`, `namespace`,
  `className`, `method`, `source`, `line`, `traits`) and one result (`outcome` running, passed, failed, skipped or
  notRun; `durationMs`, `message`, `stackTrace`, `output`):

  | Model | Microsoft.Testing.Platform node | VSTest |
  |---|---|---|
  | `id` | `uid` | TestCase `Id` |
  | `displayName` | `display-name` | `DisplayName` |
  | `fullyQualifiedName` | `location.type` + `.` + `location.method` without its parameter list | `TestCase.ManagedType` + `.` + `TestCase.ManagedMethod` (without parameters), else `FullyQualifiedName` |
  | `source`, `line` | `location.file`, `location.line-start` | `CodeFilePath`, `LineNumber` |
  | `traits` | `traits` (`[{ name: value }]`) | `TestObject.Traits` (`[{ Key, Value }]`) |
  | `outcome` | `execution-state`: `in-progress` running; `passed`; `failed`, `timed-out`, `error` failed; `skipped`; `cancelled` notRun | `Outcome` 1 passed, 2 failed, 3 skipped, 0 or 4 notRun; a test in `ActiveTests` is running |
  | `durationMs` | `time.duration-ms` | `Duration` (a TimeSpan) |
  | `message`, `stackTrace` | `error.message`, `error.stacktrace` | `ErrorMessage`, `ErrorStackTrace` |
  | `output` | `standardOutput`, `standardError` | `Messages` of category `StdOutMsgs`, `StdErrMsgs`, `AdditionalInfo` |

- **Microsoft.Testing.Platform's server mode** (preferred). The host listens on a loopback TCP port and starts the test
  application with `--server --client-port <port>` (plus `--settings <file>` for `runSettings`); the application connects
  and they speak JSON-RPC 2.0 with `Content-Length` framing: the host sends `initialize` (`processId`, `clientInfo`,
  `capabilities.testing.debuggerProvider: false`), then `testing/discoverTests` (`runId`) or `testing/runTests`
  (`runId`, `tests`: the discovered nodes, `uid` and `display-name`); the application sends `testing/testUpdates/tests`
  notifications (`changes: [{ node, parent }]`, and `changes: null` once the request's updates are done) and
  `client/log`, then answers the request; the host ends with the `exit` notification. One application process serves
  one request (a fresh build may have replaced it). Cancel is `$/cancelRequest` for the request (MTP has no
  `testing/cancel`); frameworks stop between tests, so the host kills the application's process tree when it has not
  answered within 2 s. `testing/runTests` with a tree-node `filter` is refused by xunit.v3, so runs always name nodes.
- **VSTest's translation-layer protocol** (projects that have not migrated). The host listens on a loopback TCP port and
  starts `dotnet <vstest.console.dll> --port:<port> --parentprocessid:<host pid>` (the SDK's, or `vstestConsolePath`);
  vstest.console connects, sends `TestSession.Connected`, and messages are JSON `{ MessageType, Version, Payload }` with a
  7-bit-encoded length prefix (BinaryWriter's string format), protocol version 7 after `ProtocolVersion`. Discovery is
  `TestDiscovery.Start` (`Sources`, `RunSettings`) answered by `TestDiscovery.TestFound` batches and
  `TestDiscovery.Completed`; a run is `TestExecution.RunAllWithDefaultHost` (`Sources`) or
  `TestExecution.RunSelectedWithDefaultHost` (`TestCases`: the discovered TestCase objects as vstest.console sent them),
  answered by `TestExecution.StatsChange` (new results and active tests) and `TestExecution.Completed`; logs arrive as
  `TestSession.Message`. Cancel is `TestExecution.Cancel` (`TestDiscovery.Cancel` for a discovery), then the process tree
  is killed after 2 s. One vstest.console serves one discovery or run and ends with `TestSession.Terminate`.
- **Debugging a test.** `eludite/test/run` with `debug: true` names one container and starts no test application
  itself.
  - MTP: the host listens and sends a `launch` update with the test application's command line plus `--server
    --client-port <port>` (`program` the DLL with `runtime: dotnet`, for netcoredbg, or the `.exe` with `runtime: mono`,
    for eludite-dbg-mono), and the shell launches it under its adapter. When it connects the host sends `initialize` and
    `testing/runTests` as for any run, so results flow while the person steps.
  - VSTest: the host sends `TestExecution.GetTestRunnerProcessStartInfoForRunAll` (or `...ForRunSelected` with the
    TestCases) with `DebuggingEnabled: true`. vstest.console 18 starts the testhost itself, paused, and asks for a
    debugger (`TestExecution.EditorAttachDebugger2` with the `ProcessID`): the host sends an `attach` update with that
    `processId`, the shell attaches its adapter to it (a brief 0027 attach) and answers `eludite/test/attached`, and the
    host answers vstest.console with `TestExecution.EditorAttachDebuggerCallback` (`Attached`). The testhost then runs the
    tests and results flow as for any run. vstest.console sends `TestExecution.CustomTestHostLaunch` (the start info to
    launch under a debugger) only to a launcher that cannot attach; the host answers it with a `launch` update and
    `TestExecution.CustomTestHostLaunchCallback`.
  - A debug run that gets no connection (MTP) or no `eludite/test/attached` (VSTest) within 60 s finishes failed.
- **One run at a time per container.** A run naming a container that a run is running fails with -32012
  (TestRunInProgress, `data: { runId, container }`). Discoveries run beside runs. Errors also: -32002 before
  `eludite/host/initialize`; -32602 when no solution is open, a project is not one of the solution's test projects, or a
  container id is unknown.
- **The generation rule.** Every update carries the generation its discovery or run started under. A new generation
  (`eludite/solution/open` or `close`) cancels every discovery and run (each ends with a `finished` update `canceled`
  under its old generation) and forgets the discovered tests. The shell drops updates whose generation is not current
  (CLAUDE.md invariant 12), and the host refuses a run naming containers of a previous generation's discovery (-32602).
  A run whose containers were not discovered under the current generation (the host restarted) is discovered first,
  silently, then run.
- **Status: `eludite/test/status`** answers the discoveries and runs that are going, with their tests and their latest
  result per test so far and `nextSeq`, and the last finished one with its summary. It never fails, and before
  `eludite/host/initialize` it reports nothing. A shell that (re)connects calls it after `eludite/host/initialize` with
  the build status: it replaces what it shows for each running one with the answer and then applies only updates with
  `seq >= nextSeq`. When the shell believed a run was going and it is not listed (the restarted host never had it), the
  shell reports the run as ended. Today the host is the shell's child over stdio, so a restarted host never has the old
  run; as for builds, the replay serves a host the shell reattaches to (D7) and is tested against the fake host.
- **Cargo.** `cargo test` never crosses this connection. The shell discovers a Cargo workspace's tests itself: `cargo
  test --no-run --message-format=json-diagnostic-rendered-ansi` through the Cargo build path (its output and errors are
  the Build pane's and the Error List's), then `cargo test -p <package> --lib --bins --tests -- --list --format terse`
  (doc tests are not listed), and runs them with `cargo test -p <package> --<target kind> [<name>] -- --exact <names>
  --nocapture --test-threads=1`, parsing libtest's `test <name> ... ok|FAILED|ignored` lines and its failure sections
  into the same model (protocol `cargo`).

### Forwarded LSP methods, typed

Forwarded to the Roslyn language server and typed in `eludite-protocol` (`lsp.rs`). Params and results are LSP 3.17;
requests additionally carry `eluditeGeneration`.

| Method | Kind | LSP 3.17 reference | Notes |
|---|---|---|---|
| `textDocument/didOpen` | notification | [didOpen](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didOpen) | The host keeps the text, starts a warming diagnostics pull at once |
| `textDocument/didChange` | notification | [didChange](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didChange) | Full or incremental (UTF-16 positions); the host applies it to its copy and schedules a debounced warming pull |
| `textDocument/didClose` | notification | [didClose](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_didClose) | Cancels warming for the document and publishes empty diagnostics |
| `textDocument/completion` | request | [completion](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_completion) | Result `CompletionList` or `CompletionItem[]` or `null` |
| `completionItem/resolve` | request | [resolve](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#completionItem_resolve) | Params are a `CompletionItem` plus `eluditeGeneration` |
| `textDocument/hover` | request | [hover](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_hover) | |
| `textDocument/signatureHelp` | request | [signatureHelp](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_signatureHelp) | Typed since brief 0013; schema [signature-help.json](host/signature-help.json). Result `SignatureHelp` or `null` |
| `textDocument/definition` | request | [definition](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition) | Result `Location`, `Location[]`, `LocationLink[]` or `null` |
| `textDocument/references` | request | [references](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_references) | |
| `textDocument/prepareRename` | request | [prepareRename](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_prepareRename) | Forwarded and typed since brief 0015; schema [prepare-rename.json](host/prepare-rename.json). Result `Range`, `{ range, placeholder }`, `{ defaultBehavior }` or `null` |
| `textDocument/rename` | request | [rename](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_rename) | Typed since brief 0015; schema [rename.json](host/rename.json). Result `WorkspaceEdit` or `null` |
| `textDocument/codeAction` | request | [codeAction](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_codeAction) | Typed since brief 0015; schema [code-action.json](host/code-action.json). Result `(Command \| CodeAction)[]` or `null` |
| `codeAction/resolve` | request | [resolve](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#codeAction_resolve) | Typed since brief 0015; schema [code-action-resolve.json](host/code-action-resolve.json). Params are a `CodeAction` plus `eluditeGeneration` |
| `textDocument/documentSymbol` | request | [documentSymbol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentSymbol) | Hierarchical `DocumentSymbol[]` (the host advertises hierarchical support) |
| `workspace/symbol` | request | [workspace symbol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_symbol) | |
| `textDocument/diagnostic` | request | [pull diagnostics](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_diagnostic) | The shell may pull itself; it usually relies on the host's published diagnostics |
| `$/cancelRequest` | notification | [cancel](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#cancelRequest) | Handled by the host; see Cancellation |

The typed requests are validated before forwarding: a missing `textDocument.uri` (or `query` for
`workspace/symbol`, `label` for `completionItem/resolve`, `title` for `codeAction/resolve`) is -32602.

Typed requests have no schema file of their own except where Eludite reads members LSP leaves loose:
[`host/signature-help.json`](host/signature-help.json) for `textDocument/signatureHelp` (brief 0013), and the rename and
code action requests of brief 0015 ([`prepare-rename.json`](host/prepare-rename.json),
[`rename.json`](host/rename.json), [`code-action.json`](host/code-action.json),
[`code-action-resolve.json`](host/code-action-resolve.json)), whose `WorkspaceEdit` shape is in
[`apply-edit.json`](host/apply-edit.json). The shell reads
`signatures[].label`, `parameters[].label` (a substring of the signature label, or `[start, end)` UTF-16 offsets into
it), `documentation` (a string or `MarkupContent`), `activeSignature` and `activeParameter` (also per signature).

How the shell uses the IntelliSense requests (brief 0013): `textDocument/completion`, `completionItem/resolve`,
`textDocument/hover` and `textDocument/signatureHelp` are sent after a pending `didChange` for the document, so they
see the text the user sees. A newer request of the same kind for a document cancels the older one with
`$/cancelRequest`; a result is dropped when it is not the newest request's, or when the generation or the document
version it was computed for is no longer current. Completion documentation is resolved lazily, for the selected item.

How the shell uses `textDocument/definition` and `textDocument/references` (brief 0014): both are sent after a
pending `didChange` for the document, as above. The shell keeps one of each in flight per window; a new Go To
Definition or Find All References cancels the previous one with `$/cancelRequest`, and a result is dropped when it is
not the newest request's, or when the generation or the document version it was computed for is no longer current.
`references` is sent with `context.includeDeclaration: true`. The shell reads `Location` and `Location[]` (and
`LocationLink[]`, using `targetUri` and `targetSelectionRange`).

How the shell uses rename and code actions (brief 0015), sent after a pending `didChange` like the others:

- **Rename** (Ctrl+R, Ctrl+R or F2): `textDocument/prepareRename` at the position first; `null` (or an error) means
  nothing there can be renamed, and no dialog opens. Then `textDocument/rename` for the name typed in the dialog,
  again whenever the name changes (a newer request cancels the older one); the preview shows the answer's edits, and
  Apply hands the newest answer to the workspace-edit applier (below). A `null` rename result means the server
  refused the name (the pinned Roslyn sends no reason).
- **Code actions**: `textDocument/codeAction` with an empty range at the caret and, in `context.diagnostics`, the
  diagnostics the shell shows on the caret's line. With `triggerKind` 2 it runs 50 ms after the caret rests on a
  new position (the light bulb; a newer request cancels the older one); Ctrl+. sends `triggerKind` 1 unless the
  light bulb's answer for that position is current. The chosen action is resolved with `codeAction/resolve` when it
  has no `edit`, then its edit goes to the applier.
- **Commands.** The pinned Roslyn's code actions carry only client-side commands: `roslyn.client.nestedCodeAction`
  (the first argument's `NestedCodeActions` are actions of their own, which the shell shows as a submenu) and
  `roslyn.client.fixAllCodeAction` (Fix All, which the shell does not offer). Completion items may carry
  `roslyn.client.completionComplexEdit` (override and partial-method completion), which the shell does not run. None
  of them is a server command, so `workspace/executeCommand` is not forwarded; an action whose only effect is a
  command is reported as unsupported.
- **The workspace-edit applier** takes a `WorkspaceEdit` (`changes`, or `documentChanges` with create, rename and
  delete operations, in order). Open documents change in their buffers, one undo step per document, followed at once
  by `didChange`; closed files are written atomically (a temporary file, then a rename) off the UI thread, followed by
  `workspace/didChangeWatchedFiles` (created 1, changed 2, deleted 3). A `TextDocumentEdit` whose `version` is not the
  open document's current version, or an edit computed for a document version or a generation that is no longer
  current, is refused as a whole: nothing is applied. Completion's `additionalTextEdits` (from the item or its
  `completionItem/resolve`) and the server's `workspace/applyEdit` use the same applier.

**Metadata as source.** For a symbol defined in a referenced assembly (no source in the solution), the pinned
Roslyn language server decompiles the type with ICSharpCode.Decompiler into a real file under its own temporary
directory and answers `textDocument/definition` with a plain `file://` URI to it:
`<temp>/MetadataAsSource/<session id>/DecompilationMetadataAsSourceFileProvider/<id>/<Type>.cs`, where `<temp>` is
the language server's `Path.GetTempPath()` (the host passes its environment on, so it is the shell's temporary
directory too). The file starts with a `#region Assembly <name>, Version=...` header naming the assembly and its
path. The host forwards the result unchanged and serves no text: the file is on the machine the shell runs on, so
the shell reads it from disk like any file. The shell recognizes a target under `<temp>/MetadataAsSource/`, opens
it in a read-only tab titled `<Type> [from metadata]` (Visual Studio's wording), and sends no `didOpen`,
`didChange` or `didClose` for it: Roslyn tracks these files in its own metadata workspace, so `hover` and
`definition` requests made inside the file are answered without them. A definition URI with another scheme (none
with the pinned server) is reported to the user as not navigable; serving such a document's text would need a new
host request with a schema here.

### Forwarded LSP methods, untyped

Forwarded verbatim (after the generation check) with no Eludite typing. `eludite-protocol` exposes them only as raw
JSON. Requests still require `eluditeGeneration`.

| Method | Kind |
|---|---|
| `textDocument/typeDefinition` | request, forwarded, untyped |
| `textDocument/implementation` | request, forwarded, untyped |
| `textDocument/documentHighlight` | request, forwarded, untyped |
| `textDocument/semanticTokens/full` | request, forwarded, untyped |
| `textDocument/semanticTokens/range` | request, forwarded, untyped |
| `textDocument/formatting` | request, forwarded, untyped |
| `textDocument/didSave` | notification, forwarded, untyped |
| `workspace/didChangeWatchedFiles` | notification, forwarded, untyped |

Any other method returns -32601 (MethodNotFound) and is not forwarded.

## Messages the host sends

| Method | Kind | Schema | Payload |
|---|---|---|---|
| `eludite/solution/status` | notification | [solution-status.json](host/solution-status.json) | `{ generation, path, state, ... }` |
| `eludite/languageServer/status` | notification | [language-server-status.json](host/language-server-status.json) | `{ state, ... }` |
| `textDocument/publishDiagnostics` | notification | [publish-diagnostics.json](host/publish-diagnostics.json) (LSP shape plus `eluditeGeneration`) | `{ uri, version, diagnostics, eluditeGeneration }` |
| `window/showMessage` | notification | LSP 3.17, relayed from the language server, untyped | |
| `$/progress` | notification | LSP 3.17, relayed from the language server, untyped | |
| `workspace/applyEdit` | request | [apply-edit.json](host/apply-edit.json) (LSP shape plus `eluditeGeneration`) | `{ label?, edit, eluditeGeneration }`; result `{ applied, failureReason?, failedChange? }` |
| `eludite/build/output` | notification | [build-output.json](host/build-output.json) | `{ buildId, seq, text }` |
| `eludite/build/progress` | notification | [build-progress.json](host/build-progress.json) | `{ buildId, elapsedMs, projectsTotal, projectsCompleted, errors, warnings, currentProject? }` |
| `eludite/build/finished` | notification | [build-finished.json](host/build-finished.json) | `{ buildId, generation, target, path, result, exitCode, elapsedMs, summary, projects, diagnostics, binlog?, message? }` |
| `eludite/test/update` | notification | [test-update.json](host/test-update.json) | `{ runId, generation, seq, kind, container?, tests?, results?, text?, launch?, processId?, state?, count?, summary?, elapsedMs?, message? }` |

`workspace/applyEdit` is the only request the host sends to the shell. Every other request from the host is answered
-32601 by the shell.

### `workspace/applyEdit`

Relayed from the language server (brief 0015). The host adds `eluditeGeneration` (the generation current when the
request arrived), sends the request to the shell and returns the shell's result to the language server unchanged. If
the shell answers with an error, or the connection to it is gone, the host answers
`{ applied: false, failureReason }`. When the language server cancels its request, the host cancels the relayed one
with `$/cancelRequest`. The shell applies the edit with its workspace-edit applier and answers `applied: false` with a
`failureReason`, applying nothing, when the generation is not current or a versioned document is not at that
version. The pinned Roslyn's C# server does not send this request (its code actions return edits through
`codeAction/resolve`); the relay exists for servers and features that do.

### `eludite/solution/status`

States:
- `loading`: `phase` is `legacyEvaluation` or `projectLoad`.
- `loaded`: `counts` (`projects`: C# projects (`.csproj`) listed by the solution file; `legacyProjects`: non-SDK
  projects among them; `legacyEvaluationFailures`), `msbuild` (the MSBuild used for legacy projects:
  `mono`, `buildTools` or `sdk`, with its path; `null` when the solution has no legacy projects, because Roslyn's
  build host then uses the .NET SDK's MSBuild), `corrections` (designer partials generated, Compile-item case fixups,
  COM references removed) and `diagnostics` (project-load problems for the Error List).
- `failed`: `diagnostics` holds at least one `error` explaining why (language server unavailable or exited during the
  load).
- `closed`: after `eludite/solution/close`.

`elapsedMs` is the time from `eludite/solution/open` to this notification. Diagnostic codes:

| Code | Severity | Meaning |
|---|---|---|
| `ELUDITE0001` | error | No language server is configured or it failed to start |
| `ELUDITE0002` | error | The language server exited while the solution was loading |
| `ELUDITE0003` | warning | A legacy project did not evaluate; `class` is the brief 0003 failure class |
| `ELUDITE0004` | warning | Legacy preparation failed as a whole; projects load as written |
| `ELUDITE0106` | warning | A Compile item differs in letter case from the file on disk; the file on disk is used |

### `eludite/languageServer/status`

`starting`, then `running` (with the server's `serverInfo` and LSP `capabilities` from its `initialize` result, so
the shell can see what Roslyn supports), or `unavailable` / `exited` with a `message`. A `restarting` state precedes
a deliberate restart on `eludite/solution/open` or `close`.

### `textDocument/publishDiagnostics`

The pinned Roslyn language server (tools/roslyn-pin/COMMIT) supports **pull** diagnostics only
(`textDocument/diagnostic`); it never pushes. The host turns its own warming pulls (below) into LSP push
notifications for the shell, so the shell receives diagnostics without polling. `version` is the document version
the pull ran on; a result for a version that is no longer current is not sent. `eluditeGeneration` is the generation
the pull ran under. On `didClose` the host publishes an empty list.

## Semantics warming (diagnostics pull contract)

Roslyn's LSP completion runs on frozen-partial semantics and the server does not compile dependencies on its own:
until something asks for full semantics, member completion on types from referenced projects stays empty
(brief 0002 report, "Frozen-partial semantics"). The host therefore pulls `textDocument/diagnostic` for every open
document:

- on `didOpen`, at once;
- on `didChange`, 150 ms after the last change to that document (debounce; `ELUDITE_DIAGNOSTICS_DEBOUNCE_MS`
  overrides it);
- for every open document when the solution reaches `loaded`, and when Roslyn sends `workspace/diagnostic/refresh`.

A newer pull for a document cancels the older one. Pulls run alongside shell requests and never delay them. The shell
does not need to pull diagnostics itself.

## Server-to-client requests from the language server

Answered by the host, never relayed:

| Method | Host answer |
|---|---|
| `workspace/configuration` | `projects.dotnet_enable_file_based_programs = false`; `null` (Roslyn default) for every other section. Shell-supplied settings are future work. |
| `client/registerCapability`, `client/unregisterCapability` | `null` (accepted, ignored) |
| `window/workDoneProgress/create` | `null` |
| `window/showMessageRequest` | `null` (no modal UI) |
| `workspace/diagnostic/refresh` | `null`, and the host re-pulls diagnostics for every open document |
| `workspace/semanticTokens/refresh`, `workspace/codeLens/refresh`, `workspace/inlayHint/refresh` | `null`; not relayed (no consumer yet) |

`workspace/applyEdit` is not answered by the host: it is relayed to the shell (see "Messages the host sends").

Language server notifications: `workspace/projectInitializationComplete` becomes `eludite/solution/status` `loaded`;
`window/logMessage` goes to the host log; `telemetry/event` is dropped; `window/showMessage` and `$/progress` are
relayed.

## LSP client capabilities the host advertises

The host owns the upstream handshake, so the shell cannot send its own `ClientCapabilities`. The host advertises
(and the shell must handle): `workspace.configuration`, `workspace.workspaceFolders`,
`textDocument.synchronization.didSave`, completion with `contextSupport`, `snippetSupport`, `insertReplaceSupport`,
`labelDetailsSupport`, `resolveSupport` (`documentation`, `detail`, `additionalTextEdits`) and `completionList.itemDefaults`
(`commitCharacters`, `editRange`, `insertTextFormat`, `data`), hierarchical document symbols, hover in markdown and
plaintext, `signatureHelp`, `definition`, `references`, `publishDiagnostics`, pull `diagnostic`, and
`window.workDoneProgress`. Since brief 0015 also: `workspace.applyEdit`, `workspace.workspaceEdit` with
`documentChanges`, `resourceOperations` (`create`, `rename`, `delete`) and `failureHandling` `abort`;
`textDocument.codeAction` with `codeActionLiteralSupport` (the kinds `quickfix`, `refactor`, `refactor.extract`,
`refactor.inline`, `refactor.rewrite`, `source`, `source.organizeImports`), `resolveSupport` for `edit`,
`dataSupport`, `isPreferredSupport` and `disabledSupport`; and `textDocument.rename` with `prepareSupport`. The server's resulting capabilities reach the shell in `eludite/languageServer/status`.

## Generic language servers and Cargo (brief 0019)

Not every language server runs inside `eludite-host`. For languages outside .NET the shell launches the server itself
and speaks **plain LSP 3.17** to it over the server's stdio, with the same `Content-Length` framing, request
correlation, `$/cancelRequest` cancellation and document notifications as the host connection (`crates/lsp`:
`ServerClient` and `HostClient` share one connection core). Roslyn stays behind the host (CLAUDE.md invariant 2 is
about .NET tooling); a generic server is a separate child process of the shell, supervised like the host. Nothing in
this section crosses the host connection, so none of it has a schema under [`host/`](host/); the method names are
LSP's own, and the Eludite-side contract is below.

**Registration is data.** Each generic server is one entry of `crates/lsp/src/servers.json` (`ServerRegistration`):
`id`, display `name`, LSP `languageId`, `fileGlobs` (`*.rs`), the root markers that name its workspace
(`Cargo.toml`), how to find the executable (`executable`, an environment override, a rustup component), its
`initializationOptions` and its `settings` (the answers to `workspace/configuration`). Adding a language is adding an
entry; the shell has no per-language code path for it. The pinned entry is rust-analyzer
(`tools/rust-analyzer/`, located beside the `eludite` executable, then `ELUDITE_RUST_ANALYZER`, then `PATH`, then
`rustup which rust-analyzer`; each candidate must answer `--version`, so a rustup proxy without the component is
skipped).

**Lifecycle.** One server per registration and workspace root. The root is the Cargo workspace root from
`cargo metadata` when a Cargo workspace is open (else the open folder, else the document's folder). The shell sends
`initialize` with `rootUri`, `workspaceFolders` (the root), `initializationOptions` from the registration and the
client capabilities below, then `initialized`, then replays `didOpen` for every open document the registration
matches. Shutdown is LSP `shutdown` then `exit`. A crash restarts the server under the brief 0007 policy (at most 3
restarts, 500 ms apart), re-initializes it and replays the open documents.

**Generations.** There is no `eluditeGeneration` on this connection: requests and notifications are plain LSP. The
client keeps its own generation for each server, raised on every (re)start; a result that arrives after the
generation moved is dropped (`Stale`), and diagnostics carry the generation they arrived under, so the shell never
renders a result from a previous server instance (CLAUDE.md invariant 12).

**What the shell sends** (the same requests the editor features send to the host, without the generation):

| Kind | Methods |
|---|---|
| Lifecycle | `initialize`, `initialized`, `shutdown`, `exit` |
| Document sync | `textDocument/didOpen`, `didChange` (incremental, UTF-16), `didSave`, `didClose`, `workspace/didChangeWatchedFiles` |
| Requests | `textDocument/completion`, `completionItem/resolve`, `textDocument/hover`, `textDocument/signatureHelp`, `textDocument/definition`, `textDocument/references`, `textDocument/prepareRename`, `textDocument/rename`, `textDocument/codeAction`, `codeAction/resolve` |
| Pull diagnostics | `textDocument/diagnostic`, sent by the client itself (below), never by the editor features |
| Cancellation | `$/cancelRequest` |

**Client capabilities** are the set the host advertises to Roslyn (section above), so the editor features need no
per-server branch, plus `workspace.diagnostics.refreshSupport: true` and `experimental.serverStatusNotification:
true`.

**What the server sends, and the shell's answer:**

| Kind | Method | Shell |
|---|---|---|
| notification | `textDocument/publishDiagnostics` | Merged with the pulled ones (below), then squiggles and Error List rows with source `live` |
| notification | `$/progress` | The status bar's slot for the server: the newest work-done progress title, message and percentage (`Indexing 120/300 (core)`) |
| notification | `experimental/serverStatus` | The slot's state: `health` (`ok`, `warning`, `error`) and `quiescent`; `ready` when quiescent and healthy. A rust-analyzer LSP extension ([lsp-extensions.md](https://github.com/rust-lang/rust-analyzer/blob/master/docs/book/src/contributing/lsp-extensions.md#server-status)); the shell enables it with the experimental client capability above |
| notification | `window/showMessage`, `window/logMessage` | The Output window's Language Servers source (with the server's stderr) |
| request | `workspace/applyEdit` | The workspace-edit applier, as for the host's relayed request |
| request | `workspace/configuration` | The registration's `settings` for each item's `section` (`null` when absent) |
| request | `window/workDoneProgress/create`, `client/registerCapability`, `client/unregisterCapability` | `null` (accepted) |
| request | `workspace/diagnostic/refresh` | `null`, and every open document is pulled again |
| request | `workspace/semanticTokens/refresh`, `workspace/inlayHint/refresh`, `workspace/codeLens/refresh` | `null` |
| request | anything else | -32601 (MethodNotFound) |

**Push and pull.** The editor features take one diagnostics list per document, as the host delivers them. The
pinned rust-analyzer computes its native diagnostics (syntax and semantic, codes such as `E0107`) only on pull
(`diagnosticProvider`) and pushes the results of its `cargo check` (`checkOnSave`). For a server that advertises
`diagnosticProvider`, the client pulls `textDocument/diagnostic` itself, like the host's warming: on `didOpen` at
once, 150 ms after the last `didChange`, and for every open document when `experimental/serverStatus` turns
quiescent or the server sends `workspace/diagnostic/refresh`. A newer pull for a document cancels the older one,
and a result for a document version that is no longer the last one sent is dropped. Each push or pull result is
delivered as the union of the document's pushed and pulled lists (a pushed diagnostic with the same range and code
as a pulled one, or the same range and message when it has no code, is the same problem and is dropped),
with the version the pull ran on. `didClose` delivers an empty list.

**Diagnostics and the Error List.** Live rows come from the server; `cargo build` rows come from the build. A build
row with the same file, line, column and code as a live row is shown once, as both, exactly as for MSBuild.

**Cargo.** The Cargo workspace model is read in the shell, off the UI thread, from
`cargo metadata --format-version 1 --no-deps --offline` (members, their targets and declared dependencies; no
network). Builds run `cargo build --message-format=json-diagnostic-rendered-ansi` (with `-p <package>` for one
package, `--release` for the Release configuration; `cargo clean` first for a rebuild, alone for a clean) in its own
process group. The runner streams the rendered messages and cargo's own stderr lines into the Output window through
the `eludite/build/output` shape (ANSI escapes removed), counts `compiler-artifact` messages into
`eludite/build/progress`, and ends with one `eludite/build/finished` whose diagnostics are the `compiler-message`
errors and warnings (primary span: file, line, column, end; code `E0308` or the lint name). Cancel kills the process
group (`taskkill /T /F` on Windows) and reports `canceled`.
