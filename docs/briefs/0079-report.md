# Brief 0079 report: Razor and Blazor IntelliSense through Roslyn's Razor cohosting

Status: done on Linux (2026-10-12; Windows and macOS by CI). Brief: [0079-razor-intellisense.md](0079-razor-intellisense.md).
Branch: `brief/0079-razor-intellisense` from `main` at `f312628`.

## Summary

- **The hosting path is cohosting, and the pinned build already is the Razor server** (ADR-0015). At the pin
  (`7c238e7c`) dotnet/razor lives inside dotnet/roslyn, `Microsoft.CodeAnalysis.LanguageServer.csproj` references
  `Microsoft.VisualStudioCode.RazorExtension` as a runtime dependency, and `LanguageServerExportProviderBuilder`
  composes it unconditionally; the design-time targets Razor projects need ship in the server's `Targets/`. So no
  `--extension` argument, no `rzls`, no second pin, no patch and no new build step: `tools/roslyn-pin/build.sh`'s
  output (built here in 231 s) serves Razor as it is. `RoslynProcessLauncher` is unchanged and `COMMIT` is unchanged.
- **Razor works end to end through the real host** on `corpus/web/blazor-app` (`RazorTests`, passing here against the
  real build): C# completion inside `@code` (999 items, with the component's `IncrementAmount` and `currentCount`), the
  `Counter` component after `<` in `Home.razor`, its `IncrementAmount` parameter inside `<Counter ...>`, `@bind`
  (with `@bind:format`, `@bind:event`, `@bind:get`/`set`/`after`) and `@onclick` on an element, hover on a field used
  in markup (`(field) int Counter.currentCount`), go to definition from `<Counter` to `Counter.razor`, CS0029 for a
  C# error inside `@code` by pull (so the host's warming publishes it to the editor and the Error List), and Format
  Document on a badly indented `.razor` returning 6 edits that re-indent the `@code` block with every statement and
  brace kept. Razor called the shell's HTML side (`eludite/razor/html`) for the tag completion (its HTML item was
  merged into Razor's list) and for formatting.
- **Completion latency in `@code`, warm, from the host: p50 58 ms, p95 96 ms over 50 runs** (after 5 warm-up runs;
  Debug host, 4-core Linux container, Information log level). Budget 150 ms: met. A first run over 20 samples measured
  p95 156 ms (p50 77 ms) while two Rust builds shared the machine; the test now samples 50 and reports through
  `Budget.Assert`, so CI prints the number. Nothing changed on the `.cs` path: no capability, argument or handler the
  C# documents use was touched (the new forwarded methods are ones C# did not forward before).
- **The HTML parts go through the shell's HTML server.** Razor publishes each document's HTML projection
  (`razor/updateHtml`) and asks the client for HTML results under the LSP method's own name. The host keeps the
  projections and relays the requests to the shell as one new host-to-shell request, `eludite/razor/html` (schema and
  host-rpc.md first). The shell (`crates/eludite/src/shell/razor.rs`) answers from vscode-html-language-server on a
  virtual document `<path>__virtual.html`, sends the text again only when Razor's checksum changes, maps the virtual
  uri back in the answer, and answers `null` when there is no HTML server, the server fails or the generation is
  stale, so C# and Razor features keep working without the web servers installed.
- **Routing**: `servers.json` sends `*.razor` and `*.cshtml` to the `roslyn` registration (the host) with the language
  id `aspnetcorerazor`; the `html` registration no longer claims them, so the HTML server never sees Razor source and
  the HTML formatter no longer formats `.razor` (Format Document goes to the host, which is Razor's formatter, which
  formats the markup through the projection).

## What was built

**Host (`dotnet/src/Eludite.Host/Lsp/`).** `RazorDocuments.cs` (new): `IsRazor`, the projection store (latest
`checksum` and `text` per Razor uri, forgotten on close and on a server restart), `Update` (`razor/updateHtml`,
answered `null`), `RelayAsync` (an HTML-forwarded request to `eludite/razor/html` when the projection with that
checksum is here; `null` otherwise, and on a shell error, a lost shell or a generation change; Razor's cancellation
cancels the relay), the 22 HTML methods Razor forwards at the pin (the callers of
`IHtmlRequestInvoker.MakeHtmlLspRequestAsync`), and four extra forwarded requests Razor documents use
(`textDocument/rangeFormatting`, `onTypeFormatting`, `foldingRange`, `linkedEditingRange`). `LspProxy.cs`, in blocks
marked `Brief 0079`: the field and its accessor, the extra forwarded requests (deduplicated against the lists briefs
0069 and 0071 extend), `razor/documentClosed` `{ uri }` after `didClose` of a Razor document (VS Code's client sends
it; Razor drops its HTML synchronization state), clearing the projections when a server starts, and the upstream
handlers for `razor/updateHtml` and the HTML methods. Razor's `client/registerCapability` and its
`workspace/configuration` sections are answered as before (`null`), which Razor accepts.

**Protocol.** `protocol/schemas/host/razor-html.json` (host-to-shell request: `{ method, uri, checksum, text,
params, eluditeGeneration }`, result any LSP result or `null`); `host-rpc.md`: the four untyped forwarded rows, the
`eludite/razor/html` row, the `razor/updateHtml` and HTML-request rows of the server-to-client table, a "Razor"
section, and the `html` registration row; `protocol/rust/src/host.rs` (hand-written mirror): `methods::RAZOR_HTML` in
`HOST_TO_SHELL` and `HOST_TO_SHELL_REQUESTS`, the four untyped requests, `RazorHtmlParams` and the `RazorHtml` marker;
`schema_tests.rs`: conformance, round trip and the method sets.

**Shell LSP client (`crates/lsp`).** `servers.json` (above); `client.rs`: `eludite/razor/html` as a deferred host
request (`Event::RazorHtml`, `HostClient::respond_razor_html`); `connection.rs`: the `Event::RazorHtml` variant;
`registry.rs`: the routing tests.

**Shell (`crates/eludite/src/shell/`).** `razor.rs` (new; module docs are the contract): the virtual documents per
Razor uri (opened through `server_for_path` on the virtual path, so the `html` registration's discovery, status slot
and restarts apply unchanged), the checksum rule, the request with the virtual uri, the answer mapped back and sent
from a background task (the UI thread only queues), `null` answers, stale virtual documents closed when their Razor
document is no longer open, `is_virtual_uri`. Marked one-line blocks: `session.rs` (`SessionEvent::RazorHtml`,
`Cmd::RespondRazorHtml`, `ServerSession::respond_razor_html`, the event mapping), `shell.rs` (the `mod` lines, the
`razor` field and its initializer, the event dispatch), `servers.rs` (a generic server's diagnostics for a virtual
document are dropped, so the HTML server's findings on a projection never reach the Error List).

**Corpus.** `corpus/web/blazor-app/` (MIT, trimmed by hand from `dotnet new blazor`): `BlazorApp.csproj`
(`Microsoft.NET.Sdk.Web`, `net10.0`, no package references, so it restores offline), `Program.cs` (components, Razor
Pages, MVC), `Components/_Imports.razor`, `App.razor`, `Routes.razor`, `Layout/MainLayout.razor`,
`Pages/Counter.razor` (`@code`, a `[Parameter] IncrementAmount`, `@onclick`), `Pages/Home.razor` (`<Counter
IncrementAmount="2" />`, `@bind`), `Pages/Privacy.cshtml` with its `PageModel` and `_ViewImports.cshtml`
(`@addTagHelper`), `Views/Home/Index.cshtml` with an `asp-controller` tag helper, `Controllers/HomeController.cs`,
`README.md`. It builds with `dotnet build` (0 warnings).

**Docs.** This report, the brief, `docs/adr/0015-razor-cohosting.md`, `tools/roslyn-pin/README.md` ("Razor").

## Tests (what each proves)

- `dotnet/tests/Eludite.Host.Tests/RazorTests.cs` (real Roslyn with Razor through the real `eludite-host` process;
  skips with `CodeLensTests`' message when the server is not built): the features and the latency in the Summary, and
  that `eludite/razor/html` reached the test's shell with the projection's text and the generation. **Ran here against
  the real build: passed.**
- `dotnet/tests/Eludite.Host.Tests/RazorDocumentsTests.cs` (8 tests, fake language server): `IsRazor`; the latest
  projection kept and `null` answered; a forwarded HTML request reaches the shell as `eludite/razor/html` with the
  text, checksum, generation and the inner params, and the shell's result comes back unchanged; no projection or
  another checksum answers `null` without asking the shell; a shell error answers `null`; a generation change while
  the shell holds the request answers `null`; `didClose` of a `.razor` sends `razor/documentClosed` after
  `didClose`, of a `.cs` does not, and the projection is forgotten; the extra forwarded requests reach the server.
- `crates/lsp/src/registry.rs`: `.razor` and `.cshtml` (any case) resolve to `roslyn` only, as `aspnetcorerazor`;
  a `.cshtml.cs` stays C#; a virtual `__virtual.html` resolves to `html`; no formatter claims a Razor file.
- `crates/eludite/src/shell/razor_tests.rs` (headless GPUI, the fake host and a `FakeServer` as the HTML server):
  a `.razor` opens on the host as `aspnetcorerazor` with no generic server and no HTML server started; the host's
  `eludite/razor/html` opens `Home.razor__virtual.html` in the HTML server (`languageId` `html`, the projection's
  text), the completion goes there with the virtual uri and its items come back; the same checksum sends no
  `didChange`, a new one sends version 2; a definition into the virtual document is answered with the Razor uri; the
  HTML server's diagnostics for the virtual document never reach the Error List while a real file's do; a stale
  generation answers `null` without starting the HTML server; with no HTML server the answer is `null` (twice).
  `razor.rs`'s unit tests: the virtual path, the retargeting and the uri mapping (only whole strings).
- `crates/eludite/src/shell/web_tests.rs`: the two brief 0056 tests that asserted the old routing (a `.cshtml` and a
  `.razor` served by the HTML server) now assert the new one: Razor highlighting unchanged, and neither the HTML server
  nor any generic server starts for a Razor document.
- `protocol/rust/src/schema_tests.rs`: `razor-html.json` conforms, rejects params without the generation, round-trips;
  the method sets match `host-rpc.md`.

## Commands run here

<!-- filled below -->

## Not done, and what is owed

- **Razor's trigger characters.** Razor declares its completion triggers (`@`, `<`, `:`, `"`, ...) in dynamic
  registrations (`client/registerCapability` with the `aspnetcorerazor` selector), which the host accepts and
  ignores; the shell's completion on a Razor document uses Roslyn's static C# capabilities, so typing `<` or `@` does
  not open the list by itself (Ctrl+Space and `.` do). Relaying the registrations (or a Razor trigger set in the
  shell's IntelliSense, brief 0069's file) is the follow-up.
- **HTML diagnostics on the projection** are dropped, not shown (the brief's out of scope).
- **Razor features needing custom client methods** (`razor/applyRenameEdit`, `razor/addNestedFile`, Visual Studio's
  `_vs_*` HTML extensions, which the HTML server does not offer and are answered `null`).
- **Not run**: the real HTML server (`tools/web-servers/fetch.sh`) against the real Razor in one session (the host test
  plays the shell; the shell test uses a fake HTML server); Windows and macOS (CI); the shell's keystroke-to-popup
  measurement on a `.razor` document with the real host.
- The packaging of the Roslyn server must include its whole output folder, `Targets/` and the Razor DLLs included
  (Integration notes).

## New dependencies

None. (The Razor cohost extension is part of the already pinned Roslyn build, MIT.)

## Integration notes

<!-- filled below -->
