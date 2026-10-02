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
