# Brief 0014: Go to definition, Find All References and Error List filtering

Status: open
Phase: 1
Plan reference: PLAN.md sections 2 (principles 1, 3), 4.3, 4.11, 5.1, 8, 9
Related ADR: ADR-0003
Depends on: briefs 0012 (open solution), 0013 (completion and hover; its report sizes this as "0014a")

## Goal

Navigation the way Visual Studio does it: F12 goes to a definition, including into decompiled or metadata-as-source for symbols without source; Shift+F12 fills a Find All References tool window grouped by project and file with click-through; Ctrl+- and Ctrl+Shift+- walk the navigation history; the Error List gains the VS filter buttons (errors, warnings, messages), a project filter and a text filter, and keeps its counts in the tab header. Everything flows through the brief 0007 bridge with cancellation and generations, and every action is a command.

## Files in scope

- `protocol/schemas/` first and alone: command schemas for `eludite.editor.go_to_definition`, `eludite.editor.find_references`, `eludite.navigation.back`, `eludite.navigation.forward`, `eludite.error_list.filter`; typed `textDocument/definition` and `textDocument/references` already exist in the bridge, check `host-rpc.md`; if metadata-as-source needs a Roslyn-specific request (the `roslyn/...` or `sourceGenerated`/`metadata` document scheme), document it in `host-rpc.md` with a schema and forward it
- `protocol/rust/**`, `crates/lsp/**` for any new typed message
- `dotnet/src/Eludite.Host/**` and its tests only if a Roslyn-specific request must be forwarded or a virtual document (`metadata:` scheme) must be served
- `crates/eludite/**`: navigation, history, the Find All References tool window, Error List filters
- `crates/editor/**` only for a read-only "metadata" document mode and a navigation hook; no editor internals changes
- `crates/ui/**` for the references tree and filter buttons
- `crates/docking/**` only to register the new tool window
- `crates/commands/src/**` for the new commands
- `docs/briefs/0014-report.md` (new)

## Contract

- Go to definition: F12 and Ctrl+click; opens the target file at the position, or a read-only tab titled like VS ("Foo [from metadata]") when Roslyn returns a non-file URI; the host serves that document's text through the bridge (document how); multiple targets show a picker. The current position is pushed on the navigation history first.
- Find All References: Shift+F12 opens the "Find All References" tool window docked at the bottom beside the Error List, rows grouped by project then file, showing the line text with the symbol highlighted, read and write kinds if Roslyn provides them; double-click navigates; the window shows the symbol name and a result count; a new search replaces the old results.
- Navigation history: per window, 50 entries, Ctrl+- back and Ctrl+Shift+- forward, as commands.
- Error List: toggle buttons for errors, warnings and messages with counts, a project dropdown, a text filter over code and description; filtering is instant and local; `diagnostics.list` is unaffected by UI filters (agents get everything) but accepts an optional `severity` filter as it already does.
- Cancellation and generations as in brief 0013; stale results are never shown.
- The UI thread never waits on the host.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless tests against the fake host: definition in a file opens and positions the caret and pushes history; definition to a metadata URI opens a read-only tab with the served text; multiple targets show a picker; references fill the tool window grouped correctly and double-click navigates; back and forward walk history; each Error List filter hides and shows the right rows and the counts stay correct; stale results are dropped.
- Manual, recorded with three screenshots: in `dotnet/Eludite.slnx`, F12 on `JsonRpc` (metadata-as-source), Shift+F12 on `HostRpcTarget`, and the Error List filtered to warnings only.

## Budget

- Definition in a file: caret at the target under 100 ms p95 after F12 when the host answers; report host and UI latency separately over 100 runs.
- References window populated under 300 ms p95 for a symbol with under 100 references; report the count.
- Keystroke frame cost unchanged (under 8 ms p99) with the references window open and holding 1000 rows.

## Exit criterion

1. The manual flow works with screenshots.
2. Headless tests green; workspace fmt, clippy and tests green; `dotnet test` green if the host changed.
3. Numbers in the report.
4. The report sizes brief 0015 (rename, code actions, the shared workspace-edit applier, additionalTextEdits from completion).

## Out of scope

- Rename, code actions, CodeLens, inlay hints, call hierarchy, go to implementation (brief 0015 and later).
- Decompilation beyond what Roslyn's metadata-as-source already returns.
- Windows and macOS runs.
