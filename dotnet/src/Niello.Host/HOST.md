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

## Planned (not yet added)

- **MSBuild evaluation** will locate the user's SDK or Build Tools through
  `Microsoft.Build.Locator` (PLAN.md D4).
