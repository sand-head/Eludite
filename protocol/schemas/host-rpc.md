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

The projects of the open solution with their source files, for Solution Explorer (brief 0012). The host computes it
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
| `textDocument/definition` | request | [definition](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_definition) | Result `Location`, `Location[]`, `LocationLink[]` or `null` |
| `textDocument/references` | request | [references](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_references) | |
| `textDocument/documentSymbol` | request | [documentSymbol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_documentSymbol) | Hierarchical `DocumentSymbol[]` (the host advertises hierarchical support) |
| `workspace/symbol` | request | [workspace symbol](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#workspace_symbol) | |
| `textDocument/diagnostic` | request | [pull diagnostics](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#textDocument_diagnostic) | The shell may pull itself; it usually relies on the host's published diagnostics |
| `$/cancelRequest` | notification | [cancel](https://microsoft.github.io/language-server-protocol/specifications/lsp/3.17/specification/#cancelRequest) | Handled by the host; see Cancellation |

The typed requests are validated before forwarding: a missing `textDocument.uri` (or `query` for
`workspace/symbol`, `label` for `completionItem/resolve`) is -32602.

### Forwarded LSP methods, untyped

Forwarded verbatim (after the generation check) with no Eludite typing. `eludite-protocol` exposes them only as raw
JSON. Requests still require `eluditeGeneration`.

| Method | Kind |
|---|---|
| `textDocument/signatureHelp` | request, forwarded, untyped |
| `textDocument/typeDefinition` | request, forwarded, untyped |
| `textDocument/implementation` | request, forwarded, untyped |
| `textDocument/documentHighlight` | request, forwarded, untyped |
| `textDocument/semanticTokens/full` | request, forwarded, untyped |
| `textDocument/semanticTokens/range` | request, forwarded, untyped |
| `textDocument/codeAction` | request, forwarded, untyped |
| `textDocument/formatting` | request, forwarded, untyped |
| `textDocument/rename` | request, forwarded, untyped |
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

The host sends no requests to the shell.

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
`window.workDoneProgress`. The server's resulting capabilities reach the shell in `eludite/languageServer/status`.
