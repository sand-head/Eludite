# Brief 0012: Open a solution end to end

Status: done on Linux ([report](0012-report.md)); Windows and macOS not run
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 2, 3), 4.2, 5.1, 8, 9, 10 (Phase 1)
Related ADRs: ADR-0002, ADR-0003
Depends on: briefs 0007 (host bridge), 0008 (docking), 0009 (editor core)

## Goal

The first end-to-end Phase 1 flow: File > Open Solution picks a `.sln` or `.slnx`, the shell starts `eludite-host`, the host loads the solution, Solution Explorer shows the real project and file tree, double-clicking a file opens it in the document area in an `EditorView`, edits are sent to the host as LSP document notifications, and the host's diagnostics show as squiggles in the editor and as rows in the Error List. Completion, hover and go to definition come in the next brief; this one makes the pipes real.

## Files in scope

- `protocol/schemas/**` and `protocol/schemas/host-rpc.md`: one new request `eludite/solution/tree` (schema first, own commit) returning projects (name, path, kind, target frameworks) with their source files (Compile items as absolute paths, plus content files for web projects), and command schemas for the new commands below
- `protocol/rust/**` for the new message
- `dotnet/src/Eludite.Host/**`, `dotnet/tests/Eludite.Host.Tests/**`: implement `eludite/solution/tree` from the existing evaluation (SDK-style via Roslyn's workspace or MSBuild evaluation; legacy via the brief 0003 evaluator)
- `crates/eludite/**` except `src/main.rs` (brief 0011 owns one line there; coordinate by not touching it)
- `crates/workspace/**` (client-side solution model fed from the tree response)
- `crates/lsp/**` (only if the client needs new surface; the brief 0007 client should suffice)
- `crates/docking/**` and `crates/ui/**` only for the document-area API needed to host an `EditorView` and for a tree widget
- `crates/editor/**` only to expose what the shell needs (decoration API for diagnostics is already there); do not change editor internals, brief 0011 owns them
- `crates/commands/src/**` for new commands: `eludite.solution.open` (path), `eludite.solution.close`, `eludite.file.open` (path; replaces the stub), `eludite.editor.*` for the editor actions that must be commands (save, undo, redo, find), `eludite.diagnostics.list` already exists and must now read real data
- `docs/briefs/0012-report.md` (new)

## Contract

- Native file dialog through the `rfd` crate (MIT) or GPUI's own prompt if it exists at the pin; record which and its SPDX id.
- Host supervision: the shell starts `eludite-host` found beside its own executable (then `ELUDITE_HOST` env, then PATH), passes `--stdio`, uses the `crates/lsp` client with its restart policy, and shows host and language-server status in the status bar from `eludite/languageServer/status` and `eludite/solution/status`. Loading never blocks the UI thread; the tree streams in when ready.
- Solution Explorer: VS semantics for this brief: solution node, projects (with target framework in parentheses as VS does), folders mirroring the file system under each project, files; nested `.designer.cs` and `.aspx.cs` under their parents; expand and collapse; double-click opens. No context menus yet.
- Document area: one tab per open file with the file name and a dirty marker; the `EditorView` from brief 0009 inside; Ctrl+S saves through `eludite.editor.save`; closing a dirty tab prompts.
- LSP flow: `didOpen` on open, `didChange` debounced on edit (document the interval), `didClose` on close, `eludite/...` generation handling via the client. Diagnostics from `textDocument/publishDiagnostics` become editor decorations (wavy underline by severity) and Error List rows (file, line, column, code, message, project), click-through to the location. The Error List's counts update the bottom tab label as VS does ("Error List" with the error and warning counts in the body header).
- `eludite.diagnostics.list` returns the real diagnostics, so the MCP tool from brief 0005 reports real errors.
- Every user action is a command with a schema in `protocol/` first (CLAUDE.md invariant 3 and 4).
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless GPUI tests: open a solution through the command bus against a fake host (reuse the `crates/lsp` fake), assert the tree model, open a file, edit, assert the didChange, inject a publishDiagnostics, assert the decoration and the Error List row and click-through.
- Host tests for `eludite/solution/tree` on the SDK-style fixture and on a legacy WebForms corpus project (skip cleanly when the corpus is absent).
- Manual, recorded in the report with two screenshots: open the `dotnet/Eludite.slnx` solution itself in Eludite with the real host, see its projects and files, open `HostRpcTarget.cs`, introduce an error, see the squiggle and the Error List row within the host's diagnostics debounce, fix it, see them clear. Nested compositor if the session is locked.

## Budget

- `dotnet/Eludite.slnx` (8 projects) to editable text in the first opened file under 1 s after the open command (PLAN.md section 9), with the tree and diagnostics streaming in after; report the time to tree and to first diagnostics.
- Keystroke frame cost unchanged from brief 0009 while diagnostics arrive.
- The UI thread never waits on the host; prove with a test that the fake host stalling for 5 s leaves the editor responsive.

## Exit criterion

1. The manual flow works on Linux with screenshots.
2. Headless and host tests green; workspace fmt, clippy, tests green; `dotnet test` green.
3. The schema commits precede their code.
4. The report lists protocol gaps found and sizes brief 0013 (completion, hover, definition, references in the editor; Error List filtering; status bar details).

## Out of scope

- Completion, hover, go to definition, references, rename, code actions (brief 0013).
- Solution Explorer context menus, drag and drop, Show All Files, Properties window contents.
- Building, running, debugging, tests, NuGet, git.
- Editor internals (brief 0011) and any new editor features.
- Windows and macOS runs.
