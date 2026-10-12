# ADR-0015: Razor through Roslyn's cohosting, with the HTML parts answered by the shell's HTML server

Status: Accepted, 2026-10-12 (brief 0079, by the owner's request for Visual Studio and Rider parity)
Plan reference: PLAN.md sections 2 (principles 2, 4, 6, 12), 3 (D2, D3), 4.3, 4.9 ("Razor and Blazor: the Razor
language server from dotnet/razor (MIT), hosted beside Roslyn")

## Context

`.razor` and `.cshtml` files had Razor highlighting (brief 0056) and the HTML server's completion on their raw text,
and nothing else: no C# in `@code` or `@expr`, no component, parameter, directive or tag-helper completion, no
diagnostics, and Format Document handed Razor source to the HTML formatter, which broke code blocks.

Razor's language features have had two shapes. Until 2025 VS Code's C# extension ran `rzls`, a separate Razor
language server, beside Roslyn: Razor asked the client for the C# and HTML parts of a document, and the client
forwarded them to Roslyn (on generated C# documents it had to keep in step) and to its HTML server. Since then
dotnet/razor has moved into dotnet/roslyn (`src/Razor/`) and Razor is "cohosted": its features run inside the Roslyn
language server, against the Razor source generator's output in Roslyn's own workspace, and only the HTML parts go
to the client.

At the pinned commit (`tools/roslyn-pin/COMMIT`, 7c238e7c) the server project references
`Microsoft.VisualStudioCode.RazorExtension` as a runtime dependency, `LanguageServerExportProviderBuilder` adds it to
the MEF composition unconditionally, and `LanguageServerProjectLoader` points Razor projects at the design-time
targets in the server's own `Targets/` folder. `tools/roslyn-pin/build.sh` has built all of it since brief 0002 (whose
report noted that scoped builds "pull in Razor"). `rzls` is no longer built from this tree.

Razor's callbacks to the client in cohosting are few: `razor/updateHtml` publishes a document's HTML projection (the
document with its C# and Razor parts blanked out, positions unchanged), and an HTML-forwarded request
(`{ textDocument, checksum, request }` under the LSP method's own name) asks the client's HTML server about it.

## Decision

- Serve Razor by cohosting: `.razor` and `.cshtml` documents go to `eludite-host` as C# does (`servers.json`'s
  `roslyn` entry, language id `aspnetcorerazor`), and the host forwards them to the Roslyn language server it already
  runs. Add no argument, process, build step or patch: the pinned build is the Razor server.
- The host keeps each document's projection from `razor/updateHtml` and relays Razor's HTML requests to the shell as
  one host-to-shell request, `eludite/razor/html` (schema `protocol/schemas/host/razor-html.json`), carrying the
  projection's text, the request and the solution generation. The host answers `razor/updateHtml` itself.
- The shell answers from the HTML language server it already runs for `.html` (brief 0050), on a virtual document
  `<path>__virtual.html` holding the projection, mapping the virtual uri back in the answer. With no HTML server it
  answers `null`, which Razor treats as "no HTML result".
- Take `.razor` and `.cshtml` away from the HTML server's registration: it sees only projections.

## Alternatives considered

- **`rzls` beside Roslyn** (VS Code's pre-2025 shape): needs `rzls` built from dotnet/razor at a second pin, a second
  process, and the client-side delegation of C# ranges to Roslyn on generated documents kept in step with every edit,
  which is most of what VS Code's old Razor client was. Cohosting does all of that inside Roslyn already. Rejected.
- **The HTML server inside the host**: the host would locate and run Node.js and the npm servers, which the shell
  already does (brief 0050: discovery, the cache, the fetch script, the status slot). Duplicating it in .NET buys one
  hop per HTML request. Rejected; the hop is a local pipe and the projection is a file's size.
- **The shell keeps projections, the host relays only requests**: two messages where one does, and a projection
  could arrive after the request that needs it. Rejected; the host relays the text with each request and the shell
  sends the HTML server a new version only when the checksum changes.
- **Keep the HTML server on raw Razor source beside Razor** (the fan-out of brief 0050): its completions and edits
  are wrong inside code (it formats C# as text), and Razor already merges HTML completions with its own. Rejected.

## Consequences

Positive:
- C#, component, directive, tag-helper and HTML IntelliSense, diagnostics, navigation and formatting in Razor
  files from one pinned build, with nothing new to build, ship or locate.
- Razor lives in Roslyn's child process (invariant 2); the shell only answers HTML questions on a server it already
  runs.

Negative:
- Razor's capabilities (its trigger characters: `@`, `<`, `:`) arrive as dynamic registrations, which the host
  accepts and ignores; the shell's completion uses Roslyn's static C# triggers until a brief relays them.
- Each HTML request carries the projection's text over the host's stdio.
- A Roslyn pin bump can change Razor's client contract (method names, params); `RazorTests` against the real build
  is the check.

## Revisit when

- The pinned Roslyn changes the Razor client callbacks (`HtmlRequestInvoker`, `HtmlDocumentPublisher`).
- Razor's dynamic registrations are needed by the shell (trigger characters, document colors, linked editing).
