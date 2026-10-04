# Brief 0052 report: CodeLens for references and tests

Status: done on Linux against the fake host, the fake generic server and the real rust-analyzer. The pinned Roslyn
language server was not available here (not built, and the disk had 1.3 to 4 GB free while a Roslyn build needs
several), so the real-host C# tests skip with their message and the C# lenses were not seen on screen (section 7).
Windows and macOS: not run. CI: not run (nothing pushed).
Branch: `brief/0052-codelens`, based on `main` at `470f11f`; not rebased (the coordinator merges).
Date: 2026-10-04. Brief: [0052-codelens.md](0052-codelens.md).

## 1. Summary

- **A line above each class, method, property and test** shows Visual Studio's indicators (`3 references`,
  `Run Test | Debug Test`, rust-analyzer's `2 implementations`), in the theme's new `code_lens` colour (a quiet grey
  per theme) and a smaller size, separated by bars, at the member's indentation and above its attributes (`[Fact]`,
  `#[test]`). The row takes layout height in the editor's display layer and holds no buffer text: the caret,
  selections, line numbers, the gutter, the breakpoint margin, the light bulb and the overlays beside the text (the Git
  change margin, the forge comment marks, the agents' pending marks) refer to buffer rows only and line up with the
  text. Adding or removing rows keeps the first visible line where it is on screen (one reflow).
- **Requests** go to the document's server after a pending `didChange`: when the document opens, 150 ms after the
  last edit, on `workspace/codeLens/refresh` (the host relays Roslyn's as `eludite/codeLens/refresh`), when a server's
  capabilities arrive, on a new generation and when the solution finishes loading. A newer request cancels the older;
  an answer for a moved document version or generation is dropped, by the shell and again by the editor. Unresolved
  lenses are resolved, at most 8 at a time per document, when their line comes within 50 lines of the visible range.
  A server without `codeLensProvider` is never asked and contributes nothing.
- **References**: a click, or Ctrl+K, Ctrl+Q (Visual Studio's Show CodeLens Menu: the first indicator of the member at
  the caret), opens the References popup at once (`'Calculator': searching…`), then the locations grouped by file with
  a preview line (brief 0014's rows, read off the UI thread), virtualized; Up, Down, Page Up, Page Down, Enter (and a
  double-click) navigate through `eludite.file.open` with the navigation history; Escape closes it.
- **Tests**: Run Test and Debug Test run through the Test Explorer's commands (`eludite.test.run` and
  `eludite.test.debug` with `ids`), so the build gate, the streamed results, the Error List rows and the status bar are
  the Test Explorer's; the Run lens then shows the last outcome's glyph (aggregated for a class or a module) and the
  duration. A test not discovered yet is discovered first while the lens reads "Discovering…".
- **Settings** (`protocol/schemas/settings.json`, Tools > Options > Text Editor > All Languages > CodeLens, generated):
  `editor.codeLens` (Enable CodeLens), `editor.codeLens.references` (Show References), `editor.codeLens.tests` (Show
  Test Status), and per-language overrides `editor.languages.<csharp|rust|typescript|javascript>.codeLens`
  (`default`, `on`, `references`, `tests`, `off`; TSX and JSX follow TypeScript and JavaScript). A change asks again;
  off removes the rows.
- **Agents gain nothing new**: `eludite.editor.find_references` and `eludite.test.*` answer what the lenses show.
- **No new dependency** (`Cargo.lock` and the .NET packages unchanged).

## 2. Protocol (schemas first, `31c54c7`, alone)

- `host-rpc.md`: `textDocument/codeLens` and `codeLens/resolve` in the typed forwarded table (schemas
  [`host/code-lens.json`](../../protocol/schemas/host/code-lens.json) and
  [`host/code-lens-resolve.json`](../../protocol/schemas/host/code-lens-resolve.json); `codeLens/resolve` requires a
  `range` object, else -32602); a "CodeLens" paragraph (when the shell asks, what the pinned Roslyn answers, the
  command mapping table); `eludite/codeLens/refresh` among the messages the host sends
  ([`host/code-lens-refresh.json`](../../protocol/schemas/host/code-lens-refresh.json), `{ eluditeGeneration }`);
  `workspace/codeLens/refresh` in its own server-to-client row; the capabilities `textDocument.codeLens` and
  `workspace.codeLens.refreshSupport`; for generic servers the two requests, the refresh, the client commands
  rust-analyzer needs listed (`experimental.commands.commands`: `rust-analyzer.runSingle`, `rust-analyzer.debugSingle`,
  `rust-analyzer.showReferences`) and the lens commands the shell recognizes.
- **What the pinned Roslyn answers** (read from its source at `tools/roslyn-pin/COMMIT`, `CodeLensHandler` and
  `CodeLensResolveHandler`; not run here): for every type and member declaration an unresolved references lens (range
  = the identifier, `data` = `{ syntaxVersion, listIndex, textDocument }`), resolved to `"N references"` (`"1
  reference"`, `"99+ references"` past 99, `"- references"` when not counted) with the client command
  `roslyn.client.peekReferences [uri, position]`, ContentModified when the syntax version moved; for every test
  method (attributes, syntactically) `"Run Test"` and `"Debug Test"` with the command **`dotnet.test.run`** and one
  argument `{ textDocument, range, attachDebugger, runSettingsPath }`, and `"Run All Tests"` / `"Debug All Tests"` on
  the class. The reference count excludes the declaration.
- **Deviation from the brief's wording**: the brief names `roslyn.client.runTests` and a test's fully qualified name.
  The pinned server's test command is `dotnet.test.run`, and its argument names no test, only the document and the
  member's identifier range. The host therefore maps it to `eludite.test.run` / `eludite.test.debug` with
  `{ uri, path, range, member }` (the identifier read from its copy of the document), and the shell finds the
  discovered tests of that file whose method (or, for Run All Tests, class) is `member` in the Test Explorer's model
  (`lens_test_ids`), which knows the fully qualified name and the project. `roslyn.client.peekReferences` becomes
  `eludite.editor.find_references` with `{ uri, path, position }`.

## 3. What was built, by step

1. **Host** (`1464084`): `Lsp/CodeLensCommands.cs` (the mapping, applied to both answers; other commands pass through
   unchanged), the two methods in `LspProxy.TypedRequests` with the `range` check, the capabilities, the refresh answered
   at once and relayed (both the bare and the object shape), `HOST.md`. The fake language server answers Roslyn's lens
   shapes.
2. **Typed messages** (`8ed25e8`): `lsp::CodeLens`, `CodeLensParams`, `CodeLensRequest`, `ResolveCodeLens`;
   `host::CodeLensRefreshNotification`; drift and conformance tests. In `eludite-lsp`: `codelens::classify` (what a lens
   command does: `LensKind` References, Implementations, RunTest, DebugTest; `LensTarget` References with or without
   locations, Test by member, CargoTest by libtest name, exact or module, and package), `title_count`;
   `Event::CodeLensRefresh` and `Connection::code_lens_generation` (one more per refresh, from a generic server's
   `workspace/codeLens/refresh` or the host's relay; a relay for an older generation is dropped); the generic client's
   capabilities.
3. **Editor** (`8ca1850`, `c78f210`): `display::VerticalLayout` (where every buffer row and lens row is: binary searches
   over the sorted lens rows, `hit` for the pointer, `visible_rows`, `max_scroll`), `codelens.rs` (the rows anchored to
   their members, the layout cached per text version, `set_code_lenses` / `update_code_lens` /
   `set_code_lens_enabled` / `refresh_code_lenses` / `code_lens_bounds` / `row_top`, the scroll-keeping replace, the
   debounce, the resolve window, Ctrl+K, Ctrl+Q), `intellisense::CodeLensPipeline` (pure: when to ask, whether an
   answer is current, which to resolve), the three `EditorEvent`s, and the element: lens rows shaped in the UI font at
   0.85 of the editor's size, a pointing-hand hitbox per indicator, the hovered one underlined. Every row-to-pixel
   computation in the view, popups, margins and data tips goes through the layout.
4. **Shell** (`a7f3d7e`, `f166ab2`): `shell/codelens.rs` (requests, resolves, activation, the References popup,
   Run/Debug through the Test Explorer, settings, glyphs from the model, lens placement above attributes, timings),
   hooks in `session.rs` (`SessionEvent::CodeLensRefresh`), `servers.rs` (refresh, capabilities, generation,
   readiness), `shell.rs`, `documents.rs` (open, close), `test_runs.rs` (`after_tests_change`), `settings.rs`; the theme
   colour; `ServerFeatures::code_lens` / `code_lens_resolve`.
5. **Corpus and real servers** (`b0467bc`, `2ef89bb`): `corpus/tests/Corpus.XunitV3`'s `Calculator` moved into
   `Calculator.cs`, so its references come from another file (the failing line `CalculatorTests.cs:line 25` is
   unchanged); `crates/lsp/tests/real_host.rs::real_host_code_lens_on_the_corpus` and
   `RealCodeLensTests` in the host tests (both skip without Roslyn); `real_analyzer.rs::
   real_rust_analyzer_offers_run_debug_and_implementations_lenses`.
6. **Manual run** (`f166ab2`, `86a199e`): `crates/eludite/tools/codelens-linux.sh` and the screenshots.

## 4. Tests and what each proves

| Where | Test | Proves |
|---|---|---|
| `eludite-editor` `display` | `lens_rows_take_height_and_never_move_buffer_rows_relative_to_each_other` | Each text line stays `line_height` tall; a lens row only pushes later rows; hit testing on text and lens rows and at the edges; with 200 lens rows the lookups match a linear walk |
| `eludite-editor` `intellisense` | `the_lens_window_and_pipeline` | Visible ± 50 clamped; nothing asked while off; an answer applied once, dropped for another version; the debounce asks only after an edit; resolves asked once, retried after a failure |
| `eludite-editor` `tests/codelens.rs` | `lens_rows_take_layout_height_and_never_shift_buffer_lines_or_the_caret` | Rows 0, 2 and 4 move down by exactly the lens rows above them; the caret does not move; the text is drawn below its lens row; a click on text lands on the right buffer position, a click on a lens row's blank part does nothing; an edit above moves the rows with their members |
| | `indicators_activate_on_click_and_from_the_keyboard_menu` | Indicators laid out left to right; hover underlines; a click activates without moving the caret; Ctrl+K, Ctrl+Q activates the first indicator of the member at the caret (two members); a glyph and title change in place without relayout |
| | `requests_are_debounced_after_edits_and_stale_answers_are_dropped` | Nothing asked while off; 149 ms after the last of two keys nothing, at 150 ms one request; an edit makes the answer in flight stale (dropped, rows kept); a refresh supersedes the request in flight |
| | `unresolved_lenses_resolve_within_50_lines_of_the_visible_range` | Exactly the unresolved lenses within 50 lines are asked for; scrolling asks for the new ones once; a failed resolve is asked again |
| | `turning_lenses_off_removes_the_rows_in_one_reflow_that_keeps_the_text_in_place` | 200 lens rows arriving and then going leave the first visible line where it is; off asks for nothing more |
| `eludite-lsp` | `codelens::tests` (3) | The mapped Roslyn commands, rust-analyzer's runnables (test, module, not `cargo run`, not doctests) and `showReferences`, title counts |
| | `generic_server::code_lens_requests_and_the_refresh_move_the_lens_generation` | The capabilities (codeLens, refresh, the experimental client commands); typed lenses and resolve; the server's refresh answered `null`, an event, the lens generation moved |
| | `in_process::typed_code_lens_and_the_relayed_refresh` | Typed lens requests carry the generation through the fake host; the relayed refresh is an event; one for an older generation is dropped |
| | `real_analyzer::real_rust_analyzer_offers_run_debug_and_implementations_lenses` | Against the real rust-analyzer (skips without it): the kinds in section 5 |
| | `real_host::real_host_code_lens_on_the_corpus` | Against the real host and Roslyn (skipped here: not located) |
| `eludite-protocol` | `code_lens_messages_conform_to_their_schemas`, drift tests | Params, results, the mapped arguments and the refresh conform; rejections; method lists and `host-rpc.md` agree |
| `eludite-commands` | settings schema test | The seven keys, their section, defaults and enum values |
| `Eludite.Host.Tests` `CodeLensTests` (4) | forwarding and mapping, refresh relay, capabilities, mapping edge cases | Lenses forwarded without the generation; `dotnet.test.run` mapped by `attachDebugger` with the member (class and method); `peekReferences` mapped with the position; unknown commands and `null` untouched; verbatim identifiers, CRLF, an empty range; -32602 without a range |
| `Eludite.Host.Tests` `RealCodeLensTests` (1) | the real Roslyn on `Corpus.XunitV3` | Skipped here with "Roslyn language server not built" |
| `eludite` `shell/codelens.rs` (4 unit) | settings, titles and durations, popup rows, placement above attributes | |
| `eludite` `shell/codelens_tests.rs` (7 headless) | `lenses_appear_above_members_and_the_references_popup_lists_and_navigates` | Lenses above `class Program` and `Main` with counts after `didOpen`, under the current generation, one resolve each; rows take height; a click opens the popup within 50 ms, grouped by file with lines, `includeDeclaration: false`; Down, Up, Enter navigate (history kept, Ctrl+- returns); Ctrl+K, Ctrl+Q opens the member's popup and Escape closes it; Find All References lists 3 = the lens's 2 + the declaration |
| | `a_server_without_lenses_contributes_nothing` | No `codeLensProvider`: no request, no rows |
| | `run_test_from_a_lens_discovers_first_runs_through_the_test_explorer_and_shows_the_outcome` | "Discovering…", the build gate, discovery then `eludite.test.run` with `[u-adds]`; ✔ and "(12 ms)"; Subtracts' ✖ and its Error List row; Run All Tests runs both and aggregates; Debug Test asks the host for a debug run and the fake adapter stops; an unknown command shows nothing |
| | `refresh_asks_again_and_an_edit_drops_a_stale_answer` | `eludite/codeLens/refresh` asks again; one for an older generation does not; a late answer for an older version is never shown; the next request follows the edit's `didChange` |
| | `the_settings_hide_references_or_tests_or_all_and_per_language_overrides_win` | Tests off, references off, all off (no rows, no more requests), a C# override over the switch, another language's override not applied to C# |
| | `a_rust_analyzer_run_test_lens_from_the_generic_server_runs_the_cargo_test` | rust-analyzer-style lenses from the fake generic server in Visual Studio's words; the implementations lens resolves and opens the popup with its own locations (no references request); Run Test discovers the Cargo tests with the real cargo and runs `tests::adds`, ✔ on the lens |
| | `typing_stays_under_the_keystroke_budget_with_200_lens_rows_on_screen` | Section 6; the lens requests made while typing are held by the host and nothing waits |
| `eludite` existing | `settings_tests` | The Options test now finds the Agents page by name (a page was added before it) |

Final checks on this tree: `cargo fmt --check` and `cargo clippy --workspace --all-targets --features
eludite-chromium/cef -- -D warnings` clean; `dotnet build dotnet/Eludite.slnx` with 0 warnings; `dotnet test
dotnet/Eludite.slnx --no-build` 258 tests, 248 passed, 8 skipped (Roslyn not built, and the other environment skips),
2 Mono adapter timing tests failed under load and passed when run alone; `cargo test --workspace --no-fail-fast
--features eludite-chromium/cef` with the full environment 1207 passed, 1 ignored, 1 failed under load (the Web
Browser window's View menu test, on the known load-flaky list) that passed when run alone.

## 5. The lens kinds each server offered

| Server | Kinds | Notes |
|---|---|---|
| Roslyn (pinned, from its source; not run) | references (all type and member declarations, resolved lazily, `N references`, counts to 99), Run Test / Debug Test per test method, Run All Tests / Debug All Tests per test class | Gated by `dotnet_enable_references_code_lens` and `dotnet_enable_tests_code_lens` (default true; the host answers `null`, the default); tests lenses are not sent when `dotnet_lsp_using_devkit` is set (it is not) |
| rust-analyzer 2026-08-31 (real, `real_analyzer.rs` and the Xvfb run) | `N implementation(s)` on traits, structs and enums (`rust-analyzer.showReferences` with the locations), `▶︎ Run Test` / `⚙︎ Debug` on tests and `▶︎ Run Tests` / `⚙︎ Debug` on test modules (`runSingle`/`debugSingle` with Cargo runnables) | Only offered because the client lists the three client commands. Its references lenses (`lens.references.*`) are off by default and the registration (`servers.json`, brief 0050's file) does not turn them on. A `fn main`'s `▶︎ Run` (`cargo run`) and doctests are not shown |
| TypeScript and JavaScript (brief 0050, not merged here) | — | `editor.action.showReferences` `[uri, position, locations]` is recognized; whether 0050's server offers lenses depends on its preferences |

**What implementations and overrides lenses need next.** rust-analyzer's implementations lens works today (shown with
the references indicators). Roslyn's LSP has no implementations or overrides lens: Visual Studio draws those from its
own providers. They need a lens source in the host: for each visible type or virtual member, `textDocument/
implementation` (forwarded today, untyped) asked lazily as the references lens is resolved, a lens kind and a setting
(`editor.codeLens.implementations`), and the popup listing the answer; overrides need either the same request on
`virtual`/`abstract` members or a Roslyn handler the pin does not have.

## 6. Budgets

Measured headless (debug build, software rendering, the GPUI test platform), 4 cores shared with another agent's builds
(load average 3 to 5); `assert_budget` asserts only below the core count and prints otherwise.

| Budget | Result |
|---|---|
| Keystroke to frame < 8 ms p99 with 200 lens rows | **p99 4.6 to 7.5 ms** (median 2.6 to 3.0) over 200 keys in a 1280 × 800 window with 200 lens rows in the document (the best of up to three runs). With all 200 lens rows and their 400 lines on an 11,000 px window: median 7.2 to 9.3 ms against 6.0 to 7.0 ms for the same screen without lenses: the 200 lens rows cost 0.8 to 2.3 ms, less than the text lines under them do line for line (asserted) |
| Lens request answered and drawn < 300 ms warm from the Roslyn server | Not measurable here (no Roslyn). The real rust-analyzer: 2.9 to 7.9 ms server time warm (one 2.1 s answer while it loaded); the fake host: request to applied under 10 ms. The host's own share is the mapping of the answer (a JSON rewrite) |
| The References popup within 50 ms from a resolved lens | **11 ms** from the click to the frame showing the popup (created 0.08 ms after the click; its rows 2.8 ms with the fake host's answer) |
| No new dependency | Pass |

The pinned Roslyn batches `textDocument/references` with a fixed 500 ms delay (brief 0014), so with Roslyn the
popup's rows would follow about 500 ms after it opens; the popup itself opens at once.

## 7. Manual run (Linux, Xvfb) and screenshots

`crates/eludite/tools/codelens-linux.sh OUT_DIR`, with `ELUDITE_RUST_ANALYZER` pointing at the pinned rust-analyzer
(`tools/rust-analyzer/fetch.sh`). It writes a crate with a trait implemented in two files and two tests, opens it with
`--folder`, and drives it with xdotool. [Screenshots](0052-run/screenshots/):

- [`codelens-rows.png`](0052-run/screenshots/codelens-rows.png): `2 implementations` above `pub trait Shape`,
  `1 implementation` above `Square`, `Run Tests | Debug Tests` above `#[cfg(test)] mod tests`, `Run Test | Debug Test`
  above `#[test] fn squares`; line numbers and the text are not moved by the rows.
- [`codelens-popup.png`](0052-run/screenshots/codelens-popup.png): Ctrl+K, Ctrl+Q on the trait: "'Shape': 2
  implementations", circle.rs (1) and lib.rs (1) with their lines.
- [`codelens-run.png`](0052-run/screenshots/codelens-run.png): Ctrl+K, Ctrl+Q in `squares`: `cargo test --no-run` in
  the Build pane, then the run; the lens reads ✔ Run Test (< 1 ms), the module's ○ Run Tests (one of its tests not
  run), the status bar "Tests: 1 passed, 0 failed (0.3 s)".

The script's C# step opens `corpus/tests/Corpus.XunitV3` with `Calculator.cs`; here it reports that the Roslyn language
server is not located, so there is no C# screenshot.

## 8. Not done, and why

- **The real Roslyn**: not built here (disk); `real_host_code_lens_on_the_corpus` and `RealCodeLensTests` skip with
  their message. The mapping is tested against Roslyn's shapes copied from its source at the pinned commit.
- **Folding**: the editor has no folding yet (`crates/editor/src/lib.rs`, Known gaps). A lens row belongs to the buffer
  row below it, so folding will hide it with that row; nothing to test until then.
- **Requests "for the visible range plus the margin"**: LSP's `textDocument/codeLens` has no range, so the whole
  document is asked (rows for every lens keep the layout stable while scrolling); the 50-line margin governs the
  resolves.
- **The keyboard menu** opens the member's first indicator (the brief's wording); moving between a row's indicators
  from the keyboard is not built.
- **"Discovering…"** is written with the ellipsis character, as the shell's other states are.
- **Lens requests at open** can go out twice (the editor's own and the capabilities' refresh); the first is canceled.

## 9. For the coordinator: merging beside brief 0050

- **Per-document servers.** `request_code_lenses` and the resolves use the document's one session
  (`documents[id].session`, `doc_features(id)`) through 0050's single-server API as it is on `main`. 0050's fan-out
  should ask each of a document's servers that advertises `codeLensProvider` and merge the answers into the one
  `DocLenses` (an entry per lens already carries its own `lsp::CodeLens`; it would also carry its server so the
  resolve goes back to it). The hook is `codelens.rs`'s `request_code_lenses` / `pump_lens_resolves`; `session.rs` and
  `servers.rs` gained only `SessionEvent::CodeLensRefresh` and four one-line calls to `code_lens_server_changed`.
- **Shared files touched**: `protocol/schemas/settings.json` (seven `editor.*` keys and the section
  "Text Editor > All Languages > CodeLens" after "Text Editor > Rust"), `crates/commands/src/settings.rs` (the key list
  in its test), `crates/ui/src/theme.rs` (`code_lens`), `docs/briefs/README.md` (row 0052),
  `crates/eludite/src/shell/intellisense.rs` (`ServerFeatures` gained `code_lens`, `code_lens_resolve`; three
  `EditorEvent` arms), `server.rs`'s `client_capabilities()` (codeLens, its refresh, `experimental.commands`). 0050's
  `servers.json` was not touched; enabling rust-analyzer's references lenses would be a `settings` entry there.
- **Files beyond the brief's list**, each forced by it: `crates/editor/src/{codelens.rs (new), lib.rs, popups.rs,
  debugging.rs}` (every row-to-pixel computation goes through the layout), `protocol/rust/src/lsp.rs` and
  `schema_tests.rs` (the typed messages), `crates/eludite/src/shell.rs`, `documents.rs` (two calls),
  `references.rs` (`reference_inputs` visible to the popup), `settings.rs` (one call), `git/gutter.rs`,
  `forge/margin.rs`, `agents/review.rs` (their marks placed with `row_top`, else they drift below lens rows),
  `tests.rs` (`setup_services`), `test_runs_tests.rs` (helpers `pub(super)`, `setup_scripted`), `settings_tests.rs`
  (the Agents page by name), `dotnet/src/Eludite.Host/HOST.md`, `FakeLanguageServer.cs`, `corpus/tests/README.md`.
  `options.rs` needed no change: the page is generated from the schema.
- **A pre-existing failure** seen only with rust-analyzer installed: `real_analyzer::real_rust_analyzer_diagnoses_
  and_completes` expects `E0107` where the pinned rust-analyzer now says `E0061` for a missing argument. It skips in
  this environment's normal runs (rust-analyzer is not on PATH); the pinned binary was fetched to
  `~/.cache/eludite/rust-analyzer/2026-08-31/` for the Xvfb run.

## 10. Commits

1. `f890ef1` Mark brief 0052 in progress
2. `31c54c7` Specify the CodeLens settings, the forwarded codeLens requests, the Roslyn lens command mapping and the
   refresh notification (schemas and `host-rpc.md` only)
3. `1464084` Forward codeLens and codeLens/resolve in eludite-host, map Roslyn's lens commands and relay the refresh
4. `8ed25e8` Type textDocument/codeLens and codeLens/resolve, classify lens commands and raise a lens generation on
   refresh
5. `8ca1850` Draw CodeLens rows in the editor's display layer with hit testing, the keyboard menu and the debounced
   request and resolve pipeline
6. `92f79a8` Format the typed lens messages and their tests
7. `a7f3d7e` Request, resolve and draw CodeLens in the shell with the References popup, Run Test and Debug Test through
   the Test Explorer, the settings and headless tests
8. `c78f210` Cache the lens rows' buffer rows per text version and reuse the laid-out lines for their indentation
9. `b0467bc` Move the corpus's Calculator into its own file and test the real Roslyn's lenses through the real host
10. `2ef89bb` Test the lenses the real rust-analyzer offers the generic client
11. `f166ab2` Show lenses above the member's attributes and name the popup's locations by the lens (with
    `crates/eludite/tools/codelens-linux.sh`)
12. `86a199e` Add the CodeLens Xvfb run script and its screenshots with the real rust-analyzer (the screenshots)
13. This report, the brief's status and the briefs index.
