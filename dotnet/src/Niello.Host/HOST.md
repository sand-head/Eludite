# niello-host

The long-lived .NET process the Niello shell starts once per solution
(PLAN.md D2). It speaks JSON-RPC 2.0 over stdio with LSP-style
`Content-Length` framing (StreamJsonRpc `HeaderDelimitedMessageHandler`,
System.Text.Json, camelCase). All logging goes to stderr; stdout is
reserved for protocol traffic.

Usage: `niello-host [--stdio]` (stdio is the only transport for now).

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

## Planned (not yet added)

- **Roslyn language server.** `Microsoft.CodeAnalysis.LanguageServer` from
  dotnet/roslyn (MIT) will be embedded here in Phase 0 spike 2, built from
  source at a pinned commit rather than consumed from the Azure feeds.
- **MSBuild evaluation** will locate the user's SDK or Build Tools through
  `Microsoft.Build.Locator` (PLAN.md D4).

No Roslyn or MSBuild packages are referenced yet.
