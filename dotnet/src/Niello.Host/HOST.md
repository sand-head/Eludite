# niello-host

The long-lived .NET process the Niello shell starts once per solution
(PLAN.md D2). It speaks JSON-RPC 2.0 over stdio with LSP-style
`Content-Length` framing (StreamJsonRpc `HeaderDelimitedMessageHandler`,
System.Text.Json, camelCase). All logging goes to stderr; stdout is
reserved for protocol traffic.

Usage: `niello-host [--stdio] [--roslyn-ls <path> | --no-roslyn]` (stdio is the only transport for now).

## Current methods

| Method | Kind | Result |
|---|---|---|
| `initialize` | request | `{ hostName, hostVersion, capabilities }` |
| `niello/ping` | request | `{ pong: true, timestamp }` (ISO-8601 UTC) |
| `niello/host/info` | request | `{ dotnetSdks: [{ version, path }], runtime, os }` |
| `shutdown` | request | `null` |
| `exit` | notification | process exits (0 if `shutdown` came first, else 1) |

The method surface lives in `Rpc/HostRpcTarget.cs`; SDK discovery is behind
`ISdkDiscoverer` so tests never spawn `dotnet`.

## Roslyn language server (brief 0002 spike)

`Microsoft.CodeAnalysis.LanguageServer` is built from source at the commit in
`tools/roslyn-pin/COMMIT` and runs as a **child process** of niello-host
(`dotnet <dll> --stdio --clientProcessId <host pid> --telemetryLevel off`). In-process
loading was tried and rejected; see `docs/briefs/0002-report.md`.

- Located from `--roslyn-ls`, else `NIELLO_ROSLYN_LS`, else the tools/roslyn-pin output under
  `ROSLYN_SRC_DIR` (default `~/.cache/niello/roslyn`). If none exists, LSP forwarding is off and
  the host serves only the methods above.
- `initialize` (the niello handshake) starts the server in the background and returns at once.
  The host then sends LSP `initialize`/`initialized` itself and Roslyn's `solution/open`
  with `solutionPath`.
- Forwarded verbatim on the same connection, with plain LSP names: requests such as
  `textDocument/completion`, `completionItem/resolve`, `textDocument/documentSymbol`, `hover`,
  `definition`, `references`, `semanticTokens/*`, `textDocument/diagnostic`, `workspace/symbol`;
  notifications `textDocument/didOpen|didChange|didClose|didSave`. The full list is in
  `Lsp/LspProxy.cs`.
- Relayed from Roslyn to the shell: `workspace/projectInitializationComplete`,
  `textDocument/publishDiagnostics`, `window/showMessage`, `$/progress`. `window/logMessage`
  goes to stderr. The host answers `workspace/configuration` itself, with
  `projects.dotnet_enable_file_based_programs = false` and Roslyn defaults for everything else.
- Cancellation: `$/cancelRequest` from the shell fails the forwarded request at once with
  -32800 and is passed on to Roslyn.
- Solution generation: counts completed solution loads (`projectInitializationComplete`).
  A forwarded request may carry `params.nielloGeneration`; the host strips it. A stale value, or
  a load that finishes while the request is in flight, fails the request with -32801
  (ContentModified) and cancels it upstream.
- Diagnostics: `NIELLO_LSP_TRACE=1` traces upstream JSON-RPC to stderr (`warn` for warnings
  only); `NIELLO_ROSLYN_LOGLEVEL` overrides the server's `--logLevel` (default Information).

None of this is in `protocol/schemas/host-rpc.md` yet; the gaps are listed in the brief 0002
report.

## Legacy projects (brief 0003 spike)

`Legacy/` evaluates non-SDK `.csproj` files at design time (evaluation plus `ResolveReferences`, never a compile;
see `docs/briefs/0003-report.md`):

- `MonoInstallation` locates Mono's MSBuild (`NIELLO_MONO_PREFIX`, `mono` on `PATH`, `~/.local/opt/mono-root/usr`,
  `/usr`, ...); `BuildToolsInstallation` locates Build Tools' `MSBuild.exe` with `vswhere` (Windows, untested).
- `CommandLineMsBuildEvaluator` runs a located MSBuild with an injected dump target; `InProcessMsBuildEvaluator`
  uses the .NET SDK's MSBuild in-process through `Microsoft.Build.Locator`, ignoring Visual Studio-only imports.
- `ReferenceAssemblies` supplies `TargetFrameworkRootPath` from the `Microsoft.NETFramework.ReferenceAssemblies.net4*`
  packages in the NuGet cache.
- `LegacyDesignTime` runs before the Roslyn server starts: it puts Mono on the server's `PATH` (Roslyn then loads
  non-SDK projects with its Mono build host), sets `TargetFrameworkRootPath`, generates WebForms designer partials
  (`WebFormsDesignerService`, Niello.Web) and injects them through `CustomAfterMicrosoftCommonTargets`. By default a
  checked-in `.designer.cs` is kept and the partial adds only the fields it lacks (checked-in designers can be stale).
  The same targets file fixes Compile items whose letter case differs from the disk and drops `COMReference` items
  off Windows.
- Switches: `NIELLO_LEGACY=0` (all off), `NIELLO_LEGACY_MONO=0` (hide Mono), `NIELLO_LEGACY_DESIGNERS=0`
  (no designer partials), `NIELLO_CACHE_DIR` (default `~/.cache/niello`).
- A bare `.csproj` as `solutionPath` is opened with Roslyn's `project/open`.

No `niello/*` method exposes evaluation results yet; the gaps are listed in the brief 0003 report.

## Planned (not yet added)

- **MSBuild evaluation for SDK-style projects** through `Microsoft.Build.Locator` (PLAN.md D4).
