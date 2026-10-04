# Brief 0052: CodeLens for references and tests

Status: in progress
Phase: 2 (PLAN.md section 10: "CodeLens"; sections 4.1 and 4.3)
Plan reference: PLAN.md sections 2 (principles 1, 3, 5, 12), 4.1 (CodeLens in the editor core), 4.3 (CodeLens for references and tests from the Roslyn language server), 4.6 (the Test Explorer's run and debug at a test), 5.1, 8 (Visual Studio names: the CodeLens indicators above a member, "N references", "Run Test" and "Debug Test", the References popup), 9, 10 (Phase 2)
Related ADRs: ADR-0001, ADR-0003
Depends on: brief 0013 and 0014 (the editor's LSP features and Find All References), brief 0019 (generic servers and the refresh requests the client already accepts), brief 0035 (Test Explorer: run and debug a test by id), brief 0050 (several servers per document, so a TypeScript file's lenses come from its server too).

## Goal

A line above each class, method, property and test shows Visual Studio's CodeLens indicators without moving the text: "N references" (click: the References popup listing the locations with a preview line, Enter opens one, F12-style keyboard navigation) and, on a test method, "Run Test" and "Debug Test" with the last outcome's glyph and duration after a run, through the Test Explorer's model. The lenses come from `textDocument/codeLens` and `codeLens/resolve` of the Roslyn language server (which serves references and tests lenses) and of any generic server that offers them (rust-analyzer's "run test" and "implementations" lenses; TypeScript's references lens when the server enables it), rendered as a dedicated row above the member in the editor's display layer that scrolls with the text and never shifts the caret, with inlay-style rendering in the theme's lens colour; `workspace/codeLens/refresh` re-requests them; the setting `editor.codeLens` (on, references only, tests only, off) and per-language overrides. Lenses are requested lazily for the visible range plus a margin, resolved off-thread, and never block typing. Agents gain nothing new (references and tests are already commands), and `eludite.editor.references` answers what the lens shows.

## Files in scope

- `protocol/schemas/` first and alone: `settings.json` (`editor.codeLens`, `editor.codeLens.references`, `editor.codeLens.tests`, per-language overrides under `editor.languages.<id>.codeLens`), `host-rpc.md` (the forwarded `textDocument/codeLens` and `codeLens/resolve` as typed forwarded methods; the Roslyn lens command ids the host answers: `roslyn.client.peekReferences`, `roslyn.client.runTests` and the test run arguments), `host/forwarded-request.json` if the typed list needs the two methods.
- `dotnet/src/Eludite.Host/Lsp/**` (forward the two methods; the Roslyn server's lens `command`s are mapped to Eludite's: the references lens carries the position for `eludite.editor.find_references`, the tests lens carries the test's fully qualified name for `eludite.test.run` and `eludite.test.debug` with the project; `dotnet/tests/Eludite.Host.Tests/CodeLens*.cs` against the real language server on a corpus project), `crates/lsp/src/**` and `protocol/rust/src/host.rs` (the typed messages; the refresh request bumps a lens generation), `crates/editor/src/display.rs` and `view.rs` (the lens rows: a virtual line above a buffer line that takes layout height but no buffer text, scrolling, hit testing for clicks, keyboard access (Ctrl+K, Ctrl+Q as Visual Studio's "Show Code Lens Menu" opens the first lens of the current member), the References popup reusing the Find All References result list of brief 0014 in a popover, the run and debug lens glyphs with the last outcome), `crates/editor/src/intellisense.rs` (the request and resolve pipeline: visible range plus 50 lines each side, debounced 150 ms after an edit, resolve on demand when a lens row becomes visible, results dropped on a stale document version), `crates/eludite/src/shell/session.rs` and `servers.rs` (the fan-out to the document's servers, the refresh handling), `crates/eludite/src/shell/test_runs.rs` (the outcome per test id for the lens glyph; Run Test and Debug Test from a lens through `eludite.test.run` and `eludite.test.debug` with `ids`), `crates/eludite/src/shell/codelens_tests.rs` (headless, fake host and fake generic server), `crates/eludite/src/shell/options.rs` (the settings), `crates/ui/src/theme.rs` (the lens colour), `corpus/tests` (the xunit.v3 corpus project gains a class referenced from a second file so the references lens shows a count above 1), `crates/eludite/tools/codelens-linux.sh` (the Xvfb run on the corpus: the references lens, the popup, Run Test from the lens with the outcome glyph; screenshots), `docs/briefs/README.md`, `docs/briefs/0052-report.md` (new).

## Contract

- **Rendering.** A lens row is a display-only line above the member's first line, in the lens colour and a smaller size, with the indicators separated by a bar; it takes vertical space in layout so lines do not overlap, and the caret, selection, line numbers and gutter refer to buffer lines only; folding a member folds its lens row; `editor.codeLens` off removes the rows with no relayout jank (one reflow).
- **Requests.** Lenses are requested for the visible range plus the margin after the document is open and 150 ms after the last edit, resolved lazily per row, dropped when the document version moved; `workspace/codeLens/refresh` re-requests the visible range; a server without the capability contributes nothing and nothing else degrades.
- **References.** Click or Enter on "N references" opens the popup with the locations grouped by file with a preview line; Enter navigates; Escape closes; the count matches `eludite.editor.references` for the same symbol.
- **Tests.** "Run Test" runs the test through the Test Explorer's model (build gate, results stream, Error List) and the lens shows the outcome glyph and the duration after the run; "Debug Test" starts the session as brief 0035's does; a test with no discovered id yet triggers discovery first with the lens reading "Discovering...".
- The UI thread never waits on a request; typing under the editor's keystroke budget with 200 lens rows on screen; the popup is virtualized.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/editor`: lens rows take layout height and never shift buffer lines or the caret; folding; hit testing; the request window and the debounce; stale versions dropped; the keyboard menu.
- `dotnet/tests`: the real language server answers lenses on the corpus with the references count and the tests lens, and the host maps the commands.
- `crates/eludite` headless tests (fake host and fake generic server): lenses appear above members with counts, the popup lists and navigates, Run Test from the lens runs through the Test Explorer and the glyph shows the outcome, Debug Test starts the session, refresh re-requests, the settings hide references or tests or all, per-language overrides, a rust-analyzer-style "run test" lens from the fake generic server, the keystroke budget with 200 rows (`assert_budget`), an edit drops stale lenses.
- Real host: a test in the `real_host.rs` style on the corpus project.
- The Xvfb run's screenshots in the report.

## Budget

- Keystroke to frame under the existing 8 ms p99 with 200 lens rows visible; the lens request for a visible range answered and drawn within 300 ms warm from the Roslyn server (the server's own latency is reported separately).
- The References popup opens within 50 ms from a resolved lens.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green; `dotnet build` zero warnings and `dotnet test` green with the CodeLens host tests running.
2. The report records the budget numbers, the lens kinds each server offers, and what "implementations" and "overrides" lenses need next.
3. The briefs index and `host-rpc.md` match the repository.

## Out of scope

- Git blame lenses (the git margin and Blame document exist from brief 0040); "implementations" and "overrides" lenses beyond what a server offers unprompted; inline values while debugging (PLAN.md 4.5 names them under the debugger); Windows and macOS runs beyond CI.
