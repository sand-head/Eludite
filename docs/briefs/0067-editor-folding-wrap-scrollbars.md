# Brief 0067: The editor's vertical display map: outlining, word wrap, scroll bars, sticky scroll and the visual aids

Status: in progress
Phase: 2 (PLAN.md section 10: the editor on par with Visual Studio and Rider for .NET work)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 12), 4.1 (the editor core: "folding", "soft wrap", "sticky scroll" are the display map's), 5.1 (every action a command), 8 (Visual Studio names, menus and chords)
Related ADRs: ADR-0003 (protocols: `textDocument/foldingRange` over LSP, no bespoke hook), ADR-0014 (tests fail on behavior only)
Depends on: brief 0009 (the editor core and its display-map gap, `docs/briefs/0009-report.md`), brief 0052 (CodeLens rows and `VerticalLayout`), brief 0013 (the document's server session), brief 0020 (settings and Tools > Options).

## Goal

The editor view gets the vertical display map brief 0009 left out: a buffer row can be hidden (a collapsed outlining region) or take several display rows (word wrap), beside CodeLens's display-only rows, and every row and pixel mapping goes through it. On top of it: Visual Studio's outlining (the margin's expanders, the collapsed adornment with its tooltip, Edit > Outlining's commands and chords) from the language server's `textDocument/foldingRange` when it advertises it and otherwise from tree-sitter fold queries; Edit > Advanced > Word Wrap; vertical and horizontal scroll bars with Visual Studio's bar-mode overview markers; sticky scroll; brace matching with Go To Brace and Extend to Brace; View White Space; indent guides; and the editor's font, size and zoom.

## Files in scope

- `crates/editor/src/display.rs`: `VerticalLayout` extended into the display map (hidden rows, wrapped rows, lens rows) with prefix sums; `crates/editor/src/folding.rs`, `wrap.rs`, `scrollbar.rs`, `sticky.rs`, `brace_match.rs`, `visual_aids.rs` (new); `crates/editor/src/lib.rs` (mod lines, re-exports, crate docs); hooks in `crates/editor/src/view.rs` in blocks marked `brief 0067`; one hook in `crates/editor/src/codelens.rs` (the layout cache builds the display map); `crates/editor/queries/<language>/folds.scm` (new) for C#, VB, F#, Razor, Rust, TypeScript, JavaScript, HTML, CSS and JSON; `crates/editor/src/syntax/language.rs` (the `folds_query` field and its registration only); `crates/editor/tests/folding.rs` (new).
- `protocol/schemas/editor-<command>.input.json` (new, one per command below) and `protocol/schemas/editor-view-state.output.json` (new); `protocol/schemas/settings.json` (new keys, additive); `protocol/schemas/host-rpc.md` (the `textDocument/foldingRange` row).
- `crates/commands/src/outlining.rs` (new) and its `pub mod` line in `crates/commands/src/lib.rs`.
- `crates/eludite/src/shell/folding.rs` and `folding_tests.rs` (new); in `crates/eludite/src/shell.rs`, `shell/settings.rs` and `shell/documents.rs` only one-line hooks (mod lines, registration, the job loop, the UI-thread staging, settings application, document open and close).
- `crates/ui/src/menu.rs` (Edit > Outlining, Edit > Advanced > Word Wrap, View White Space, Go To Brace; View > Zoom In and Zoom Out) and `crates/ui/src/keymap.rs` (the chords), additive.
- `dotnet/src/Eludite.Host/Lsp/LspProxy.cs` (`textDocument/foldingRange` forwarded untyped) and its test.
- `docs/briefs/0067-report.md`, this file.

Nothing else: not `editor.rs` (brief 0068), not the shaping of a line beyond splitting it into wrapped segments (brief 0066), not `README.md`, `CLAUDE.md`, `docs/briefs/README.md` or `docs/PLAN.md` (the report's integration notes carry their lines).

## Contract

**Display map** (`display.rs`). `VerticalLayout::new(line_height, lens_height, line_count, lens_rows)` keeps its meaning; `with_display(hidden, wraps)` adds sorted disjoint hidden row ranges and per-row extra wrap rows. A hidden row has height 0 and no lens row; a row wrapped into `n` segments is `n * line_height` tall. `slot_top`, `line_top`, `hit` (now also returning the wrap segment), `visible_rows`, `total_height`, `max_scroll` hold for every combination; `is_hidden`, `row_height`, `segments`, `next_visible`, `prev_visible` and `display_row_count` are new. Lookups are binary searches over prefix sums rebuilt per change in O(rows); a 50k-row map with 1,000 folds rebuilds in under 2 ms.

**Folding** (`folding.rs`). A fold is a row range `start..=end` (`end > start`) with a kind (`region`, `comment`, `imports`, `namespace`, `type`, `member`, `block`), a collapsed flag and the label a collapsed fold shows (`...`, or a `#region`'s name). Collapsing hides rows `start + 1..=end`; the header row stays and shows the adornment after its text. Fold ranges come from a source the owner sets (`EditorView::set_fold_ranges(FoldSource::Server, ..)`) or, without one, from the language's `folds.scm` run on the syntax thread (captures `@fold`, `@fold.<kind>`; consecutive one-line comments and imports merge; a capture with a `name` field starts at the name's row) plus `#region`/`#endregion` (C#), `#Region`/`#End Region` (VB), `// #region` (TypeScript, JavaScript, Rust's `// region:`), `/* #region */` (CSS) and `<!-- #region -->` (HTML) found in the text. Folds are anchored so they follow edits; a new set keeps the collapsed state of folds with the same header row. The caret, a selection head, an edit or the execution point inside hidden rows expands the folds hiding it (moving the caret, find, go to definition, a breakpoint stop). `#region`s start collapsed when `editor.outlining.collapseRegionsOnOpen` is on (Visual Studio's C# default: on). The margin between the line numbers and the text shows `-` and `+` expanders and the expanded region's extent line; a click toggles. Hovering the adornment shows the hidden text (up to 25 lines).

**Commands** (`crates/commands/src/outlining.rs`, each with `{"path"?: string}` input, more where listed, and the shared `editor-view-state.output.json` output; class `read` except none edits the buffer):

| Command | Visual Studio | Chord |
|---|---|---|
| `eludite.editor.toggle_outlining_expansion` (`line`?) | Edit.ToggleOutliningExpansion | Ctrl+M, Ctrl+M |
| `eludite.editor.toggle_all_outlining` | Edit.ToggleAllOutlining | Ctrl+M, Ctrl+L |
| `eludite.editor.stop_outlining` | Edit.StopOutlining | Ctrl+M, Ctrl+P |
| `eludite.editor.start_automatic_outlining` | Edit.StartAutomaticOutlining | none |
| `eludite.editor.collapse_to_definitions` | Edit.CollapseToDefinitions | Ctrl+M, Ctrl+O |
| `eludite.editor.expand_all_outlining` | Edit.ExpandAllOutlining | Ctrl+M, Ctrl+X |
| `eludite.editor.collapse_all_outlining` | Edit.CollapseAllOutlining | Ctrl+M, Ctrl+A |
| `eludite.editor.collapse_current_region` | Edit.CollapseCurrentRegion | Ctrl+M, Ctrl+S |
| `eludite.editor.expand_current_region` | Edit.ExpandCurrentRegion | Ctrl+M, Ctrl+E |
| `eludite.editor.hide_selection` | Edit.HideSelection | Ctrl+M, Ctrl+H |
| `eludite.editor.stop_hiding_current` | Edit.StopHidingCurrent | Ctrl+M, Ctrl+U |
| `eludite.editor.toggle_word_wrap` | Edit.ToggleWordWrap | Ctrl+E, Ctrl+W |
| `eludite.editor.toggle_white_space` | Edit.ViewWhiteSpace | Ctrl+R, Ctrl+W |
| `eludite.editor.go_to_brace` | Edit.GotoBrace | Ctrl+] |
| `eludite.editor.extend_to_brace` | Edit.GotoBraceExtend | Ctrl+Shift+] |
| `eludite.editor.zoom_in` | View.ZoomIn | Ctrl+Shift+. |
| `eludite.editor.zoom_out` | View.ZoomOut | Ctrl+Shift+, |
| `eludite.editor.zoom` (`level` 20 to 400) | the zoom box | none |
| `eludite.editor.view_state` | (read) | none |

The output: `{path, outlining, folds: [{startLine, endLine, kind, collapsed, label}] (1-based lines, at most 2,000), foldSource, wordWrap, whiteSpace, zoom, caret: {line, column}, scroll: {firstLine, lastLine}, stickyLines: [line]}`.

**Word wrap** (`wrap.rs`). Wrapping is by columns at the text area's width (the editor font is monospaced): at the last space or punctuation before the edge, else at the edge, with continuation rows indented as the line. Up and Down move by display row keeping the goal column; Home and End go to the display row's start and end first (Visual Studio). With `editor.wordWrap.glyphs` a `↵` glyph ends each wrapped segment. `editor.wordWrap` (off) and `editor.languages.markdown.wordWrap` (`on`).

**Scroll bars** (`scrollbar.rs`). A vertical bar at the right and a horizontal one at the bottom (shown when a line is wider than the text area and wrap is off), 14 px. The thumb is `track * viewport / content` (at least 18 px) at `(track - thumb) * scroll / max_scroll`; dragging it scrolls, a click on the track pages. Markers (`ScrollMarker { row anchor, kind }`, layers set with `EditorView::set_scroll_markers(layer, markers)`, kinds `error`, `warning`, `info`, `find`, `added`, `modified`, `deleted`, `breakpoint`, `bookmark`, `caret`, `custom(color)`) are drawn at the row's display position; the view itself contributes the caret line, find matches, breakpoints and every wavy-underline decoration (errors and warnings). A click on a marker scrolls its row to the middle and puts the caret there. `editor.scrollBar.markers` (on).

**Sticky scroll** (`sticky.rs`). The header rows of the namespace, type, member and block folds that contain the first visible row (hidden rows aside) are drawn pinned at the top of the text area, outermost first, at most `editor.stickyScroll.maxLines` (5); a click moves the caret to that header and scrolls to it. `editor.stickyScroll` (on).

**Visual aids.** The bracket at or before the caret and its match (scanned outside strings and comments, at most 100,000 bytes away) get Visual Studio's brace highlight; Go To Brace moves the caret to the match, Extend to Brace selects through it. View White Space draws `·` for spaces and `→` for tabs. Indent guides at each tab stop inside a line's indentation (`editor.indentGuides`, on). The caret blinks (530 ms) while the editor has focus and stops blinking while typing. `editor.fontFamily` (empty: JetBrains Mono) and `editor.fontSize` (14) in Tools > Options > Environment > Fonts and Colors; zoom per editor from 20 to 400 percent by Ctrl+wheel, Zoom In and Zoom Out (10 percent steps), shown at the bottom left of the editor (a click resets to 100).

**Host.** `textDocument/foldingRange` is forwarded untyped and generation-checked like `textDocument/semanticTokens/full`; the shell asks it after a pending `didChange` when the document opens and 300 ms after the last edit, only when the server advertises `foldingRangeProvider`, and drops an answer whose generation or document version moved.

## Proving test

- `crates/editor/src/display.rs` unit tests: hidden, wrapped and lens rows together against a linear walk; `crates/editor/src/folding.rs`, `wrap.rs`, `scrollbar.rs`, `sticky.rs`, `brace_match.rs` unit tests.
- `crates/editor/tests/folding.rs` (headless GPUI): fold ranges from a fake server and from tree-sitter on corpus C# with `#region`s; collapsing hides rows and the caret moved into a fold unfolds it; a click and Up and Down across a collapsed fold and across wrapped rows land on the right offsets; the scroll bar's thumb geometry and a marker click; the sticky header lines at a scroll position; brace pairs and Go To Brace; zoom by Ctrl+wheel.
- `crates/eludite/src/shell/folding_tests.rs`: the commands through the bus on an open document (`toggle_all_outlining`, `collapse_to_definitions`, `toggle_word_wrap`, `zoom`, `view_state`), the server's `foldingRange` answer through a `FakeServer`, a stale answer dropped.
- The keystroke budget: typing in a 50k-line file with 1,000 collapsed folds and word wrap on stays under 8 ms per frame at p99 (`assert_budget`).

## Budget

Keystroke to frame submitted < 8 ms p99 on the 50k-line file with 1,000 folds and wrap on; the display map's rebuild < 2 ms there; nothing on the UI thread scans the whole buffer per keystroke except that rebuild (fold queries run on the syntax thread).

## Exit criterion

The proving tests pass; `cargo clippy --all-targets -- -D warnings` and `cargo fmt --check` are clean for every touched crate; the host's test of the forwarded list passes.

## Out of scope

Map mode (the minimap), folding from indentation for languages without a query, persisting fold state across restarts (noted in the report), word wrap by pixel width for proportional fonts, structure guides from the syntax tree (indent guides here are by indentation), and the Fonts and Colors page's per-item colors.
