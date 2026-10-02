# Brief 0013: Completion, hover and signature help in the editor

Status: done on Linux; Windows and macOS not run. Report: [0013-report.md](0013-report.md)
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 3), 4.3, 5.1, 8, 9
Related ADR: ADR-0003
Depends on: briefs 0007 (bridge), 0009 (editor), 0011 (editor memory), 0012 (open solution)

## Goal

The editor gains the three features a C# developer uses every minute: IntelliSense completion with a VS-style popup, hover with documentation, and signature help. All three flow through the brief 0007 bridge to Roslyn, are cancelable, respect solution generations, and fall back to tree-sitter-derived identifier completion while the host is still loading. After this brief, typing in a C# file in Eludite feels like Visual Studio for the basic editing loop.

## Files in scope

- `protocol/schemas/` for the new command schemas (`eludite.editor.complete`, `eludite.editor.hover`, `eludite.editor.signature_help`, `eludite.editor.accept_completion`), first and alone
- `crates/editor/**`: the popup, hover and signature-help display layers, the trigger logic, the decoration hooks they need
- `crates/eludite/**`: wiring the editor to the `crates/lsp` client for `textDocument/completion`, `completionItem/resolve`, `textDocument/hover`, `textDocument/signatureHelp`
- `crates/lsp/**` only if a typed message is missing (signatureHelp is currently pass-through; type it, with its schema and drift test in `protocol/`)
- `crates/commands/src/**` for the new commands
- `crates/ui/**` for the popup and tooltip widgets, styled from the theme tokens
- `docs/briefs/0013-report.md` (new)

Do not touch the host unless a protocol gap forces it; record gaps in the report. Do not touch `vendor/`, `crates/docking`, `crates/workspace`.

## Contract

- Completion: triggered by typing an identifier character, `.`, `(` and `<` and by Ctrl+Space; the popup lists items with kind icons (VS iconography by kind), the filter text updates as the user types, fuzzy matching through the vendored `fuzzy` crate, Tab or Enter accepts, Escape dismisses, arrows navigate; `completionItem/resolve` fetches documentation for the selected item lazily; text edits apply via the item's `textEdit` or `insertText` with snippet placeholders reduced to plain text for this brief. Each keystroke cancels the previous request. Stale responses (older generation or older document version) are dropped, never shown.
- Fallback: while the language server reports `starting` or the solution is `loading`, completion lists identifiers from the current buffer's tree-sitter tree, marked as such in the popup, and switches to Roslyn results the moment they arrive.
- Hover: after a documented delay over an identifier, a tooltip shows the hover markup rendered as plain text with simple Markdown (code spans, paragraphs); moves with scrolling; dismisses on mouse leave or typing.
- Signature help: triggered by `(` and `,`, shows the active signature with the active parameter highlighted, updates as the caret moves between arguments, dismisses on `)` or Escape.
- Keys follow Visual Studio: Ctrl+Space, Ctrl+Shift+Space, Ctrl+K Ctrl+I.
- Every trigger and acceptance is a command on the bus so an agent can drive and observe them (PLAN.md 5.1).
- The UI thread never waits on the host; requests run on the client's threads and results arrive as events.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless GPUI tests against the fake host from `crates/lsp`: popup opens on trigger, filters on typing, cancels the previous request, drops a stale response, accepts via Tab with the right edit, resolves documentation lazily, falls back to tree-sitter items while loading and swaps to host items; hover appears after the delay and dismisses; signature help tracks the active parameter.
- Manual, recorded in the report with three screenshots: in `dotnet/Eludite.slnx`, completion after `_sdkDiscoverer.` in `HostRpcTarget.cs`, hover over `JsonRpc`, signature help inside a method call. Nested compositor if the session is locked.

## Budget

- Completion popup visible under 50 ms p95 after the trigger when the host answers (PLAN.md section 9); report the host latency and the UI latency separately over 200 triggers.
- Keystroke frame cost unchanged from brief 0012 while the popup is open and filtering (under 8 ms p99).
- No growth in resident memory over 500 completion cycles beyond 5 MB.

## Exit criterion

1. The manual flow works with screenshots.
2. Headless tests green; workspace fmt, clippy and tests green.
3. Numbers in the report against the budgets.
4. The report sizes brief 0014 (go to definition, references, rename, code actions, Error List filtering).

## Out of scope

- Go to definition, references, rename, code actions, CodeLens, inlay hints (brief 0014 and later).
- Snippet expansion with tab stops.
- Completion for languages other than C# (the mechanism must not be C#-specific).
- Windows and macOS runs.
