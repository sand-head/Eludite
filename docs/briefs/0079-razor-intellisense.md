# Brief 0079: Razor and Blazor IntelliSense through Roslyn's Razor cohosting

Status: open
Phase: 2 (PLAN.md section 10; the owner's request for Visual Studio and Rider parity in .NET work)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 6, 12), 4.3 (the host bridge), 4.9 ("Razor and Blazor: the Razor language server from dotnet/razor (MIT), hosted beside Roslyn"), 7 (the web row)
Related ADRs: ADR-0002 (process topology: Razor runs inside Roslyn's language server process, a child of `eludite-host`, never in the shell), ADR-0003 (protocols: LSP; the HTML delegation is one host-to-shell request), ADR-0005 (licensing: everything MIT), ADR-0015 (new: Razor by cohosting, HTML through the shell)
Depends on: brief 0002 (the pinned Roslyn built from source), brief 0007 (the host bridge), brief 0050 (the HTML language server, several servers per file), brief 0056 (the Razor grammar), brief 0063 (the `roslyn` registration with per-glob language ids).

## Goal

A person opens a Blazor or Razor Pages project and gets, in `.razor` and `.cshtml` files, what Visual Studio and Rider give: C# completion, hover, signature help, go to definition, references, rename and highlights inside `@code`, `@{ }` and `@expr`; component tags and their parameters, directive attributes (`@bind`, `@onclick`), directives and tag helpers completed; diagnostics for C# and Razor errors in the Error List and the editor; and Format Document that formats the C# and the markup without breaking code blocks. The work is done by the Razor cohost extension that the pinned Roslyn language server already composes (`Microsoft.VisualStudioCode.RazorExtension`, built by `tools/roslyn-pin/build.sh` since brief 0002): `eludite-host` routes `.razor` and `.cshtml` documents to Roslyn and answers Razor's callbacks to the client. Razor asks the client for the HTML parts (HTML completion, hover, formatting) on a generated HTML projection of the document; the host relays these to the shell as one new request, and the shell answers them from `vscode-html-language-server` on a virtual document, or with `null` when the HTML server is not installed (C# and Razor features keep working).

## Files in scope

- `docs/briefs/0079-razor-intellisense.md` (this file), `docs/briefs/0079-report.md`, `docs/adr/0015-razor-cohosting.md` (new).
- **Host** (`dotnet/src/Eludite.Host/Lsp/`): `RazorDocuments.cs` (new: which documents are Razor, the HTML projections Razor publishes, the forwarded HTML requests), `LspProxy.cs` (marked `// Brief 0079` blocks only: register the Razor callbacks on the upstream connection, `razor/documentClosed` on close, the extra forwarded methods, the client capabilities Razor reads), `RoslynProcessLauncher.cs` (only if an argument is needed; see Contract: none is).
- **Host tests** (`dotnet/tests/Eludite.Host.Tests/`): `RazorDocumentsTests.cs` (new, unit: projections, checksums, relay shapes against a fake upstream), `RazorTests.cs` (new, against the real pinned server; skips with `CodeLensTests`' message when it is not built).
- **Protocol**: `protocol/schemas/host/razor-html.json` (new), `protocol/schemas/host-rpc.md` (a "Razor" section and the rows), `protocol/rust/src/host.rs` (the method constant and `RazorHtmlParams`; hand-written mirror), `protocol/rust/src/schema_tests.rs` (the new schema's conformance).
- **Shell LSP client** (`crates/lsp/src/`, marked blocks): `servers.json` (`*.razor` and `*.cshtml` move from `html` to `roslyn` with language id `aspnetcorerazor`), `client.rs` and `connection.rs` (`eludite/razor/html` as a deferred host request, `Event::RazorHtml`, `HostClient::respond_razor_html`), `fake.rs` (only if the fake needs a helper), registry tests for the new routing.
- **Shell** (`crates/eludite/src/shell/`): `razor.rs` (new: the virtual HTML documents and the answers), `razor_tests.rs` (new, headless with the fake host and a fake HTML server), `session.rs` (marked: `SessionEvent::RazorHtml`, `ServerSession::respond_razor_html`), `shell.rs` (marked: the `mod` lines and the event dispatch).
- **Corpus**: `corpus/web/blazor-app/` (new, MIT): a Blazor Web App trimmed from `dotnet new blazor`, with `_Imports.razor`, `Components/Pages/Counter.razor` (a parameter, `@code`, `@onclick`), a component using `<Counter>` with `@bind`, a Razor Pages page and an MVC view with a tag helper; `corpus/README.md` row through the integrator.
- `tools/roslyn-pin/README.md` (a "Razor" section: what the pinned build already contains and what the host needs).

Not touched: `crates/eludite/src/shell/intellisense.rs`, `format.rs`, `code_actions.rs` (brief 0069), the navigation paths (brief 0071), `tools/roslyn-pin/COMMIT`, `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, `docs/PLAN.md`.

## Contract

**Hosting path: cohosting (decided, ADR-0015).** At the pinned commit (`tools/roslyn-pin/COMMIT` = `7c238e7c`) dotnet/razor lives inside dotnet/roslyn (`src/Razor/`), and `Microsoft.CodeAnalysis.LanguageServer.csproj` references `Microsoft.VisualStudioCode.RazorExtension` as a runtime dependency; `LanguageServerExportProviderBuilder` puts it in the MEF composition unconditionally. No `--extension` argument, no separate `rzls`, no second build step: the existing `tools/roslyn-pin/build.sh` output is the Razor server. `RoslynProcessLauncher`'s arguments are unchanged. The standalone `rzls` (option b) is rejected: it is no longer built from this tree and needs its own C# delegation back through the client.

**Routing (shell).** `servers.json`'s `roslyn` entry gains `*.razor` and `*.cshtml` with `languageIds` `aspnetcorerazor` (what VS Code's C# extension sends; Roslyn's `LanguageInfoProvider` maps the two extensions to its Razor language before the id). The `html` entry loses both globs: the HTML server never sees Razor source again (it mangled code blocks on Format Document). Every editor feature reaches Razor documents through the host as it reaches C#: no Razor-specific path in the editor features.

**Routing (host).** `didOpen`, `didChange` and `didClose` of `.razor` and `.cshtml` are forwarded like C#'s (the host's open-document copy and warming pulls included, so diagnostics are pulled and published the same way). On `didClose` of a Razor document the host also sends Roslyn `razor/documentClosed` `{ uri }` (VS Code's client does; Razor drops its HTML synchronization state). The forwarded request list gains, in a marked block, `textDocument/rangeFormatting`, `textDocument/onTypeFormatting`, `textDocument/foldingRange` and `textDocument/linkedEditingRange` (deduplicated against the lists other briefs extend). The client capabilities stay as they are, so nothing changes for `.cs` and `.vb`; Razor's endpoints answer without dynamic registration (its `client/registerCapability` is answered `null` as today).

**Razor's callbacks to the client (host).**
- `razor/updateHtml` (a request from Roslyn: `{ textDocument: { uri }, checksum, text }`): the host keeps the latest projection per Razor document uri (`RazorDocuments`), answers `null` at once, and sends the shell nothing.
- An HTML-forwarded request (method one of `textDocument/completion`, `completionItem/resolve`, `textDocument/hover`, `textDocument/signatureHelp`, `textDocument/definition`, `textDocument/implementation`, `textDocument/documentHighlight`, `textDocument/references`, `textDocument/prepareRename`, `textDocument/rename`, `textDocument/formatting`, `textDocument/rangeFormatting`, `textDocument/onTypeFormatting`, `textDocument/foldingRange`, `textDocument/documentColor`, `textDocument/colorPresentation`, `textDocument/codeAction`, `codeAction/resolve`, `textDocument/linkedEditingRange`, `textDocument/_vs_onAutoInsert`, `textDocument/_vs_textPresentation`, `textDocument/_vs_uriPresentation`; params `{ textDocument: { uri }, checksum, request }`): the host looks up the projection for `uri`; when it has none or its checksum differs it answers `null` (Razor treats that as "no HTML result"); otherwise it sends the shell `eludite/razor/html` `{ method, uri, checksum, text, params: request, eluditeGeneration }` and returns the shell's result unchanged. A shell error, a lost shell or a generation change answers `null`; Roslyn's cancellation cancels the relayed request.
- `workspace/configuration` items Razor asks for (`razor.*`, `html.*`) are `null` (Razor's defaults), as every unknown section is today.

**`eludite/razor/html` (shell).** Schema `protocol/schemas/host/razor-html.json` (host-to-shell request): params `{ method (string), uri (the Razor document), checksum (string), text (the whole HTML projection), params (the LSP params of `method`, whose `textDocument.uri` is the Razor document), eluditeGeneration }`; result the LSP result of `method`, or `null`. The shell (`razor.rs`):
- keeps one virtual document per Razor uri at `<razor path>__virtual.html` (VS Code's naming), opened in the HTML server the `html` registration starts for that path (`server_for_path`), `languageId` `html`; a new checksum sends the whole text as the next `didChange` version, the same checksum sends nothing; the Razor document's close closes it.
- sends `method` to that server with `params.textDocument.uri` replaced by the virtual uri, maps every occurrence of the virtual uri in the result back to the Razor uri, and answers the host with it.
- answers `null` at once, without starting anything, when the HTML server is not found, does not offer `method`, fails, or the generation is stale; never blocks the UI thread (the request runs on the session's threads, the answer is sent from the reply).

**Corpus.** `corpus/web/blazor-app/BlazorApp.csproj` (`Microsoft.NET.Sdk.Web`, `net10.0`, no package references, so it restores offline), `Components/_Imports.razor`, `Components/App.razor`, `Components/Routes.razor`, `Components/Layout/MainLayout.razor`, `Components/Pages/Counter.razor` (`[Parameter] public int IncrementAmount`, `@code`, `@onclick`), `Components/Pages/Home.razor` (`<Counter IncrementAmount="2" />`, an `<input @bind="name" />`), `Pages/Privacy.cshtml` with its `PageModel` and `_ViewImports.cshtml` (`@addTagHelper *, Microsoft.AspNetCore.Mvc.TagHelpers`), `Views/Home/Index.cshtml` with an `<a asp-controller>` tag helper, `Program.cs`. MIT, hand-trimmed from the template.

## Proving test

- `dotnet test dotnet/Eludite.slnx --filter RazorDocumentsTests`: the projection store keeps the latest checksum per uri; a forwarded request with a matching checksum reaches a fake shell as `eludite/razor/html` with the text and the generation, and the shell's result comes back unchanged; a mismatched or missing projection answers `null` without asking the shell; a shell error answers `null`; `didClose` of a `.razor` sends `razor/documentClosed` and of a `.cs` does not.
- `dotnet test dotnet/Eludite.slnx --filter RazorTests` against the real pinned server (skips when it is not built), on `corpus/web/blazor-app`: completion inside `@code` lists a member of the component (`IncrementAmount`, `currentCount`); completion after `<` in `Home.razor` lists `Counter`, and inside `<Counter ` lists `IncrementAmount`; completion after `@` in an attribute position lists `@bind` / `@onclick`; hover on `currentCount` names the field's type; definition from `<Counter` in `Home.razor` lands in `Counter.razor`; a C# error inside `@code` is reported by `textDocument/diagnostic` (and published by the warmer); Format Document on a `.razor` with a code block returns edits that leave the `@code { }` block and its statements intact (the formatted text still contains every statement, braces balanced); the host's `eludite/razor/html` reached the test's shell for an HTML position. The report records completion latency p95 over 20 warm runs.
- `cargo test -p eludite-lsp`: `.razor` and `.cshtml` resolve to `roslyn` with `aspnetcorerazor` and to no HTML entry; `eludite/razor/html` from the fake host arrives as `Event::RazorHtml` and the answer goes back.
- `cargo test -p eludite razor_tests` (headless, fake host plus a `FakeServer` as the HTML server): a `.razor` document opens on the host with `aspnetcorerazor`; an `eludite/razor/html` completion opens `__virtual.html` in the HTML server with the projection, the HTML server's items come back to the host; a second request with the same checksum sends no `didChange`, a new checksum sends one; the virtual uri in a definition result is mapped back; with no HTML server the answer is `null`.
- `cargo test -p eludite-protocol`: `razor-html.json` conforms and its samples round-trip.
- `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings` on `eludite-lsp`, `eludite-protocol`, `eludite`; `dotnet build dotnet/Eludite.slnx` (warnings as errors).

## Budget

- Completion in a `.razor` document: p95 under 150 ms from the host, warm (reported, asserted through `assert_budget`'s C# counterpart `Budget.Assert`, so CI prints it).
- No change on `.cs` and `.vb`: no new capability, argument or handler touches them (the forwarded-request additions are methods C# did not forward before).
- No network at startup; nothing new is downloaded at run time. No new dependency in the shell or the host.
- The relay's extra cost per HTML request: one host-to-shell round trip plus the projection's text (a Razor file's size).

## Exit criterion

1. The proving tests above pass on Linux here; the report says which ran against the real Roslyn with Razor and which did not.
2. `docs/adr/0015-razor-cohosting.md` records the decision and the rejected option.
3. The report lists what Razor features were seen through the host, the completion p95, and what is owed (trigger characters from Razor's registrations, HTML diagnostics, Windows and macOS).
4. Integration notes give the exact lines for `CLAUDE.md`'s crate map (`crates/lsp`, `dotnet/`), `README.md` and the briefs index.

## Out of scope

- A `rzls` process, a Razor build step or patch in `tools/roslyn-pin` beyond the README, changing `COMMIT`.
- Razor's trigger characters in the shell's completion (they come from Razor's dynamic registrations, which the host does not relay yet; Ctrl+Space and the C# triggers work), HTML diagnostics on the projection, CSS and JavaScript inside `<style>` and `<script>` beyond what the HTML server gives on the projection, Razor's code actions that need `razor/*` custom client methods (`razor/applyRenameEdit`, `razor/addNestedFile`), Hot Reload of Razor, WebForms (`.aspx`), the Razor highlighting (brief 0056), semantic tokens (brief 0066).
- Editing intellisense, formatting, code-action and navigation files owned by briefs 0069 and 0071.
