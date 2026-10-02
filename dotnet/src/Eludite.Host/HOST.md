# eludite-host

The long-lived .NET process the Eludite shell starts once per solution
(PLAN.md D2). It speaks JSON-RPC 2.0 over stdio with LSP-style
`Content-Length` framing (StreamJsonRpc `HeaderDelimitedMessageHandler`,
System.Text.Json, camelCase). All logging goes to stderr; stdout is
reserved for protocol traffic.

Usage: `eludite-host [--stdio] [--roslyn-ls <path> | --no-roslyn]` (stdio is the only transport for now; no side
channel yet, so large completion lists travel on stdio).

## Contract

The full contract is `protocol/schemas/host-rpc.md` (brief 0007), with a JSON schema per Eludite message in
`protocol/schemas/host/`. In short:

| Method | Kind | Where |
|---|---|---|
| `eludite/host/initialize` | request | `Rpc/HostRpcTarget.cs`; starts the language server in the background |
| `eludite/ping`, `eludite/host/info` | request | `Rpc/HostRpcTarget.cs` |
| `eludite/host/shutdown` | request | `Rpc/HostRpcTarget.cs` |
| `eludite/host/exit` | notification | process exits (0 if `eludite/host/shutdown` came first, else 1) |
| `eludite/solution/open`, `eludite/solution/close` | request | `Lsp/LspProxy.cs`; return the new solution generation |
| `eludite/solution/tree` | request | `Projects/SolutionTreeProvider.cs`; projects and source files from `Projects/MsBuildProjectTreeEvaluator.cs`, once per generation |
| forwarded LSP (typed and untyped lists) | request / notification | `Lsp/LspProxy.cs` (`TypedRequests`, `UntypedRequests`, `UntypedNotifications`) |
| `eludite/solution/status`, `eludite/languageServer/status`, `textDocument/publishDiagnostics` | host-to-shell notification | `Lsp/LspProxy.cs` |
| `workspace/applyEdit` | host-to-shell request, relayed from the language server with `eluditeGeneration` added | `Lsp/LspProxy.cs` (`RelayApplyEditAsync`) |

Plain LSP `initialize`, `shutdown` and `exit` are not host methods (renamed in brief 0007; they return
MethodNotFound). SDK discovery is behind `ISdkDiscoverer` so tests never spawn `dotnet`.

## Logs and stdout

- stdout carries JSON-RPC only. `Program` keeps the raw stdout stream for the protocol and points `Console.Out` at
  stderr, so a library that prints cannot corrupt the stream (`HostProcessTests` checks every stdout byte is a frame).
- Host logs go to **stderr**. The Roslyn server's stderr is copied there with a `[roslyn-ls]` prefix, and its
  `window/logMessage` with `[roslyn-ls log]`. Roslyn's own log files go to `<temp>/eludite-host/roslyn-logs/`
  (`--extensionLogDirectory`; `/tmp/eludite-host/roslyn-logs/` on Linux).
- `ELUDITE_LSP_TRACE=1` traces upstream JSON-RPC to stderr (`warn` for warnings only); `ELUDITE_ROSLYN_LOGLEVEL`
  overrides the server's `--logLevel` (default Information).

## Roslyn language server

`Microsoft.CodeAnalysis.LanguageServer` is built from source at the commit in `tools/roslyn-pin/COMMIT` and runs as
a **child process** of eludite-host (`dotnet <dll> --stdio --clientProcessId <host pid> --telemetryLevel off`).
In-process loading was tried and rejected; see `docs/briefs/0002-report.md`.

- Located from `--roslyn-ls`, else `ELUDITE_ROSLYN_LS`, else the tools/roslyn-pin output under `ROSLYN_SRC_DIR`
  (default `~/.cache/eludite/roslyn`). If none exists (or `--no-roslyn`), the bridge runs without a server:
  `eludite/languageServer/status` says `unavailable`, forwarded requests fail with -32803 and `eludite/solution/open`
  reports `failed` (ELUDITE0001).
- `eludite/host/initialize` launches the server and performs the LSP `initialize`/`initialized` handshake itself,
  with the client capabilities listed in host-rpc.md. The server's capabilities reach the shell in
  `eludite/languageServer/status` `running`.
- `eludite/solution/open` sends Roslyn's `solution/open` (or `project/open` for a project file). A second open, or a
  close, restarts the server (Roslyn cannot unload a solution) and replays the shell's open documents from the
  host's copy (`Lsp/OpenDocuments.cs`, which applies incremental `didChange` edits in UTF-16 positions).
- Readiness: Roslyn's `workspace/projectInitializationComplete` becomes `eludite/solution/status` `loaded`; it is not
  relayed. The load does not change the generation.
- Ordering: the shell connection runs handlers on a `NonConcurrentSynchronizationContext`, so they start in arrival
  order; document notifications, restarts and warming pulls are chained, and forwarded requests wait for the chain,
  so a completion after a `didChange` always sees the new text.
- Cancellation: `$/cancelRequest` from the shell fails the forwarded request at once with -32800 and is passed on to
  Roslyn.
- Solution generation: 0 after initialize, +1 on each open and on each close of an open solution. Every forwarded
  request must carry `params.eluditeGeneration` (the host strips it): missing is -32602, stale is -32801 with
  `data { requestedGeneration, currentGeneration }`, and a generation change while in flight cancels the request
  upstream and answers -32801.
- Server-to-client requests (`workspace/configuration`, `client/registerCapability`, the refresh requests, ...) are
  answered by the host; `workspace/configuration` turns `projects.dotnet_enable_file_based_programs` off.
  `workspace/applyEdit` is the exception: it is relayed to the shell, whose answer goes back to the server (a shell
  error or a lost connection is `applied: false`; the server's cancellation is passed on).

## Semantics warming (brief 0002 finding)

Roslyn's LSP completion runs on frozen-partial semantics, and the server has no background compiler: a document
version whose first completion runs before its dependency compilations exist gets no members from referenced
projects, and stays that way until a request computes full semantics (brief 0002 report, "Frozen-partial
semantics"). The pinned server supports pull diagnostics only (`textDocument/diagnostic`), never
`publishDiagnostics`, so the host pulls for the shell (`Lsp/DiagnosticsWarmer.cs`):

- on `textDocument/didOpen`, at once;
- on `textDocument/didChange`, **150 ms** after the last change to that document (`ELUDITE_DIAGNOSTICS_DEBOUNCE_MS`
  overrides it);
- for every open document when the solution reaches `loaded`, and on Roslyn's `workspace/diagnostic/refresh`.

A newer pull cancels the older one for the same document. Pulls run on the thread pool alongside shell requests and
never delay them. A result is published to the shell as `textDocument/publishDiagnostics` (with `version` and
`eluditeGeneration`) only if the document version and the generation are unchanged.

## Legacy projects (brief 0003 spike)

`Legacy/` evaluates non-SDK `.csproj` files at design time (evaluation plus `ResolveReferences`, never a compile;
see `docs/briefs/0003-report.md`):

- `MonoInstallation` locates Mono's MSBuild (`ELUDITE_MONO_PREFIX`, `mono` on `PATH`, `~/.local/opt/mono-root/usr`,
  `/usr`, ...); `BuildToolsInstallation` locates Build Tools' `MSBuild.exe` with `vswhere` (Windows, untested).
- `CommandLineMsBuildEvaluator` runs a located MSBuild with an injected dump target; `InProcessMsBuildEvaluator`
  uses the .NET SDK's MSBuild in-process through `Microsoft.Build.Locator`, ignoring Visual Studio-only imports.
- `ReferenceAssemblies` supplies `TargetFrameworkRootPath` from the `Microsoft.NETFramework.ReferenceAssemblies.net4*`
  packages in the NuGet cache.
- `LegacyDesignTime` sets the Roslyn server's environment when the server starts and prepares projects before Roslyn
  opens them: it puts Mono on the server's `PATH` (Roslyn then loads
  non-SDK projects with its Mono build host), sets `TargetFrameworkRootPath`, generates WebForms designer partials
  (`WebFormsDesignerService`, Eludite.Web) and injects them through `CustomAfterMicrosoftCommonTargets`. By default a
  checked-in `.designer.cs` is kept and the partial adds only the fields it lacks (checked-in designers can be stale).
  The same targets file fixes Compile items whose letter case differs from the disk and drops `COMReference` items
  off Windows.
- Switches: `ELUDITE_LEGACY=0` (all off), `ELUDITE_LEGACY_MONO=0` (hide Mono), `ELUDITE_LEGACY_DESIGNERS=0`
  (no designer partials), `ELUDITE_CACHE_DIR` (default `~/.cache/eludite`).
- A project file passed to `eludite/solution/open` is opened with Roslyn's `project/open`.
- `LegacyDesignTime` is the bridge's `ISolutionPreparer`: it runs on `eludite/solution/open` before Roslyn opens the
  solution, only when the solution has legacy projects, and its result (MSBuild used, designer partials, case
  fixups, COM references removed, evaluation failures) is reported in `eludite/solution/status` `loaded`.

## Solution Explorer tree (brief 0012)

`eludite/solution/tree` lists the open solution's C# projects (`Legacy/SolutionProjects.cs`) with their `Compile`
items, plus `Content` items for web projects. `Projects/MsBuildProjectTreeEvaluator.cs` evaluates every project with
the .NET SDK's MSBuild in-process (registered by `Microsoft.Build.Locator`, as the brief 0003 in-process evaluator
does): evaluation only, no targets, missing imports ignored, one evaluation at a time. A multi-targeted project's
items come from the inner evaluation of its first target framework. Legacy projects get the brief 0003 design-time
properties. `Projects/SolutionTreeProvider.cs` runs the evaluation once per generation on the thread pool, answers
every request for that generation from it, and fails a waiting request with -32801 when the generation moves on.
It does not wait for Roslyn: on `dotnet/Eludite.slnx` the tree is ready long before the language server's load.

## Planned (not yet added)

- **MSBuild evaluation for SDK-style projects** through `Microsoft.Build.Locator` (PLAN.md D4).
