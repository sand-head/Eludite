# Brief 0015: Rename, code actions and the workspace-edit applier

Status: open
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 3), 4.3, 5.1, 5.6, 8, 9
Related ADR: ADR-0003
Depends on: briefs 0013 (completion; its gap list items 1 and 2), 0014 (navigation; sizes this as brief 0015)

## Goal

The editing loop closes: Ctrl+R Ctrl+R renames a symbol across the solution with a preview, Ctrl+. shows Roslyn's code actions and refactorings as a VS-style lightbulb menu and applies the chosen one, and completion items that carry `additionalTextEdits` (for example a `using` directive for an unimported type) apply them. All three sit on one shared workspace-edit applier that edits open buffers in place (undoable as one step) and files on disk atomically, and that an agent can call through the command bus.

## Files in scope

- `protocol/schemas/` first and alone: command schemas for `eludite.editor.rename`, `eludite.editor.code_actions`, `eludite.editor.apply_code_action`, `eludite.workspace.apply_edit`; typed `textDocument/rename`, `textDocument/prepareRename`, `textDocument/codeAction`, `codeAction/resolve` and `workspace/applyEdit` (host to shell) in `host-rpc.md` with schemas and drift tests
- `protocol/rust/**`, `crates/lsp/**` for the typed messages and the host-to-shell `workspace/applyEdit` request path
- `dotnet/src/Eludite.Host/**` and its tests: move the four requests to the typed list, forward `workspace/applyEdit` from Roslyn to the shell and relay the response, advertise the client capabilities the shell now has (`workspace.workspaceEdit.documentChanges`, `resourceOperations`, `codeAction.resolveSupport`, `rename.prepareSupport`)
- `crates/eludite/**`: the applier, the rename dialog with preview, the lightbulb and its menu, the wiring
- `crates/editor/**` only for a lightbulb margin indicator and a "transaction" API to group edits as one undo step if it does not exist yet; no other internals
- `crates/ui/**` for the dialog and menu widgets
- `crates/commands/src/**` for the new commands
- `docs/briefs/0015-report.md` (new)

## Contract

- Applier: takes an LSP `WorkspaceEdit` (`changes` and `documentChanges`, including create, rename and delete file operations); for open documents it applies edits to the buffer as one undo transaction and sends `didChange`; for closed files it writes atomically (temp file plus rename) and tells the host via `didChangeWatchedFiles` if that notification is supported, else reopens as needed; it refuses stale edits (document version mismatch) with a clear error and applies nothing; it reports a summary (files touched, edits applied). Exposed as `eludite.workspace.apply_edit` with permission class edit-in-buffer.
- Rename: Ctrl+R Ctrl+R or F2 on a symbol opens a VS-style dialog with the new name, a preview tree of files and changed lines (from `textDocument/rename`'s edit), and Apply; `prepareRename` validates the position first; Escape cancels; the rename is one undo step per document.
- Code actions: a lightbulb in the margin on lines with available actions (requested with a documented debounce after the caret moves, cancelable), Ctrl+. opens the menu with Roslyn's titles grouped as VS does (fixes, refactorings), arrows and Enter apply, resolved lazily through `codeAction/resolve` when the edit is missing; actions that are commands rather than edits are executed through `workspace/executeCommand` if Roslyn uses them, else reported as unsupported.
- `additionalTextEdits` from completion are applied by the applier on accept (closes brief 0013 gap 2).
- Host-initiated `workspace/applyEdit` (Roslyn uses it for some fixes) goes through the same applier and answers `applied: true/false`.
- Cancellation and generations as before; the UI thread never waits on the host.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Applier unit and headless tests: edits to open buffers as one undo, to closed files on disk, mixed, version mismatch refused, create/rename/delete file operations, idempotent summary.
- Headless tests against the fake host: rename flow with preview and apply, prepareRename rejection, code action lightbulb appears and the menu applies a resolved action, host-initiated applyEdit, additionalTextEdits on completion accept, stale results dropped.
- Host tests for the typed messages and the applyEdit relay.
- Manual, recorded with three screenshots: in `dotnet/Eludite.slnx`, rename `HostRpcTarget.Ping` with its preview, apply "Use primary constructor" (IDE0290 appears in the Error List already), and accept a completion for an unimported type that adds a `using`. Nothing committed; revert the files after.

## Budget

- Lightbulb visible under 150 ms p95 after the caret stops (report host and UI separately).
- Rename preview under 500 ms p95 for a symbol with under 50 references; apply under 100 ms for 10 files.
- Keystroke frame cost unchanged (under 8 ms p99) with the lightbulb polling active.

## Exit criterion

1. The manual flow works with screenshots.
2. All tests green; workspace fmt, clippy, tests green; `dotnet build` zero warnings and `dotnet test` green.
3. Numbers in the report.
4. The report sizes the next briefs: the Agents window moved from the spikes into `crates/` (brief 0016), and build/run with netcoredbg (brief 0017).

## Out of scope

- Inline rename (VS 2022 style, editing in place) beyond the dialog.
- Fix-all across the solution, suppression actions, refactoring previews with diffs beyond the rename preview.
- Snippet tab stops.
- Windows and macOS runs.
