# Brief 0071: The navigation set

Status: in progress (2026-10-11).
Phase: 2 (PLAN.md section 10: on par with Visual Studio and Rider for .NET work; accepted by the owner's direct request, as proposals 0004 and 0005 were)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 6, 12), 4.1 (the editor), 4.3 (language intelligence over LSP), 5.1 (every action a command), 9 (budgets)
Related ADRs: ADR-0002 (Roslyn stays in its own process; the host forwards), ADR-0003 (LSP, no bespoke hooks), ADR-0014 (tests fail on behavior only)
Depends on: brief 0014 (Go To Definition, Find All References, the navigation history, metadata as source), brief 0013 (the IntelliSense request pattern: one request per window, stale answers dropped), brief 0019 and 0050 (generic servers and the fan-out session), brief 0062 (the Workspace window's icons and tree), brief 0007 (the host bridge).

## Goal

The navigation a .NET developer uses many times an hour in Visual Studio and Rider works in Eludite with Visual Studio's names, menus and shortcuts, for the person and agents alike:

1. **Go To Implementation** (Ctrl+F12), **Go To Type Definition**, **Go To Base** (Alt+Home) and **Go To Derived Symbols** (Alt+End), with the picker F12 already has when there are several targets.
2. **Go To All** (Ctrl+T, Ctrl+,) with VS's filters (`f ` files, `t ` types, `m ` members, `# ` symbols, `: ` line) and chords (Ctrl+1, Ctrl+F / T / M / S), **Go To File** (Ctrl+Shift+T): files from the workspace's tree ranked with Zed's `fuzzy`, symbols from `workspace/symbol` of every server of the workspace streaming in, stale queries dropped, a preview of the selection.
3. **Reference highlighting**: the symbol under the caret and its references in the document, after ~200 ms idle, read and write references in two colors; Ctrl+Shift+Down / Up move between them; setting `editor.highlightReferences` (on).
4. **Document Outline** window (View > Other Windows > Document Outline, Ctrl+Alt+T) and Rider's **File Structure** popup (Ctrl+F11, Alt+\\ for VS's Go To Member), both from `textDocument/documentSymbol`: icons by kind, the current member selected as the caret moves, click to navigate, a filter box.
5. **The navigation bar** above every code editor: VS's three dropdowns (project, type, member) from `documentSymbol`, tracking the caret, choosing navigates; setting `editor.navigationBar` (on).
6. **Call Hierarchy** (Ctrl+K, Ctrl+T) tool window: "Calls To" and "Calls From" trees expanded lazily, call sites listed and navigable; **Type Hierarchy** (base types and derived types) the same way. The pinned Roslyn (`tools/roslyn-pin/COMMIT` 7c238e7) has `textDocument/prepareCallHierarchy`, `callHierarchy/incomingCalls`, `callHierarchy/outgoingCalls`, `textDocument/prepareTypeHierarchy`, `typeHierarchy/supertypes` and `typeHierarchy/subtypes` handlers (`src/LanguageServer/Protocol/Handler/CallHierarchy`, `TypeHierarchy`): the host forwards them; no Roslyn patch.
7. **Find All References depth**: a Kind column (Definition, Read, Write, Reference), a filter by kind, Group By (Project then File, File, Definition, none), a search box, Keep Results (the next search does not replace a kept result set), F8 / Shift+F8 stepping through the results.
8. **Source-generated documents**: F12 into a `roslyn-source-generated://` location opens a read-only tab whose text comes from the pin's LSP 3.18 `workspace/textDocumentContent` (`{uri}` to `{text}`), forwarded by the host; the client capability `workspace.textDocumentContent` is announced so Roslyn registers the scheme.

## Files in scope

- `docs/briefs/0071-navigation.md`, `docs/briefs/0071-report.md`.
- `protocol/schemas/navigation-*.json` (new): one input and one output schema per command below; `protocol/schemas/host/text-document-content.json` (new); `protocol/schemas/host-rpc.md` (the forwarded list gains the hierarchy methods and `workspace/textDocumentContent`; a paragraph on how the shell uses them).
- `crates/commands/src/navigation.rs` (new): ids, parsing, typed outputs, [`NavigationCommands`], `register`; `crates/commands/src/lib.rs` (the `mod` line and one doc sentence).
- `crates/lsp/src/navigation.rs` (new): typed parsing of implementation, type definition, document highlight, document symbol, workspace symbol, call and type hierarchy answers, and source-generated URIs; `crates/lsp/src/fanout.rs` (capabilities of the new methods, one marked block); `crates/lsp/src/lib.rs` (`mod` line); the fake host's and `FakeServer`'s scripting only if a method needs it.
- `crates/eludite/src/shell/navigation.rs` (Go To Implementation, Type Definition, Base and Derived over the definition machinery; the history API unchanged), `references.rs` (Kind, filter, Group By, search, Keep Results, F8), new shell modules `navigation_bus.rs` (the command bus on the UI thread), `goto_all.rs`, `highlights.rs`, `outline.rs`, `navbar.rs`, `call_hierarchy.rs`, `type_hierarchy.rs`, `generated_docs.rs`, and their headless tests `navigation_set_tests.rs`, `goto_all_tests.rs`, `hierarchy_tests.rs`.
- Shared files, small additive blocks marked "brief 0071": `crates/eludite/src/shell.rs` (mod lines, registration, the tool bodies, the document body's navigation bar), one line in `crates/eludite/src/shell/documents.rs` (`on_editor_changed` calls the caret tracker), `crates/eludite/Cargo.toml` (`fuzzy`, Apache-2.0, already in the build through `eludite-editor`), `crates/docking/src/model.rs` (window ids `document_outline`, `call_hierarchy`, `type_hierarchy`), `crates/ui/src/menu.rs` and `keymap.rs`, `crates/ui/src/theme.rs` (tokens `reference_highlight_read`, `reference_highlight_write`), `crates/commands/src/settings.rs` (`editor.highlightReferences`, `editor.navigationBar`), `dotnet/src/Eludite.Host/Lsp/LspProxy.cs` (forwarded methods, the client capability), host tests in `dotnet/tests/Eludite.Host.Tests/`.

Nothing in `crates/editor` (the highlights are the view's existing decoration layers), `README.md`, `CLAUDE.md`, `docs/PLAN.md`, `docs/briefs/README.md`, or generated bindings.

## Contract

**Commands** (schemas first; class `read`; agent-visible; from another thread an answer still loading is waited for up to 5 s, as IntelliSense's):

| Command | Keys, menu | Input | Output |
|---|---|---|---|
| `eludite.navigation.go_to_implementation` | Ctrl+F12; Edit > Go To Implementation | `{path?, line?, column?, target?}` | `{kind, path, line, column, state: loading\|navigated\|choose\|none\|failed, targets[≤100]: {path, line, column, metadata, generated, title}, navigated?, message?}` |
| `eludite.navigation.go_to_type_definition` | Edit > Go To Type Definition | same | same |
| `eludite.navigation.go_to_base` | Alt+Home; Edit > Go To Base | same | same (`typeHierarchy/supertypes` of the symbol's type, else `none`) |
| `eludite.navigation.go_to_derived` | Alt+End; Edit > Go To Derived Symbols | same | same (`textDocument/implementation`) |
| `eludite.navigation.go_to_all` | Ctrl+T, Ctrl+,; Ctrl+1 chords; Ctrl+Shift+T; Edit > Go To All | `{query?, filter?: all\|files\|types\|members\|symbols\|line, max_results? (≤500, default 100), select?, open_popup?}` | `{query, filter, state: loading\|done, results[]: {name, kind, container?, path, line?, column?, score}, total, servers_pending}` |
| `eludite.navigation.highlight_references` | Ctrl+Shift+Down / Up (`navigate`) | `{path?, line?, column?, navigate?: next\|previous}` | `{path, state: loading\|done\|none\|off, highlights[]: {line, column, end_line, end_column, kind: text\|read\|write}, current?}` |
| `eludite.navigation.document_outline` | Ctrl+Alt+T; View > Other Windows > Document Outline | `{path?, show?, filter?}` | `{path, state: loading\|done\|failed\|none, symbols[] (nested): {name, kind, detail?, line, column, end_line, children}, current?: [names]}` |
| `eludite.navigation.file_structure` | Ctrl+F11, Alt+\\; Edit > Go To Member | `{path?, filter?, select?, close?}` | the outline output plus `{open}` |
| `eludite.navigation.navigation_bar` | the bar's dropdowns | `{path?, select?: {dropdown: types\|members, index}}` | `{path, project, types[]: {name, kind, line}, members[]: …, type?, member?}` |
| `eludite.navigation.call_hierarchy` | Ctrl+K, Ctrl+T; View > Call Hierarchy | `{path?, line?, column?, expand?: node, navigate?: node}` | `{state, roots[]: {id, name, kind, detail?, path, line, column, calls_to: node[]?, calls_from: node[]?, sites[]}}` |
| `eludite.navigation.type_hierarchy` | View > Type Hierarchy | `{path?, line?, column?, expand?: node, navigate?: node}` | `{state, root?: {id, name, kind, path, line, column, base_types?, derived_types?}}` |
| `eludite.navigation.references` | F8 / Shift+F8 when Find All References is active; its toolbar | `{kinds?: [definition, read, write, reference], group_by?: project_file\|file\|definition\|none, search?, keep?, navigate?: next\|previous}` | `{symbol, kinds, group_by, search, kept, shown, total, references[≤1000]: {project?, path, line, column, text, kind}, selected?}` |

**Requests.** Every request goes through the document's session after its pending `didChange`; one of each kind is in flight per window, a new one cancels the old one, and an answer is applied only when it is the newest and its solution generation and document version are current (invariant 12). A server that does not offer a method (`implementationProvider`, `typeDefinitionProvider`, `documentHighlightProvider`, `documentSymbolProvider`, `workspaceSymbolProvider`, `callHierarchyProvider`, `typeHierarchyProvider`) is not asked; the command answers `none`/`failed` with the reason.

**Go To All.** The file index is the workspace tree's files, built off the UI thread when the tree changes; a query ranks them with `fuzzy::match_strings` on a background thread; the newest query wins, older answers are dropped. Symbols: `workspace/symbol` with the query (minus its filter prefix) to the host and to every running generic server; each answer merges in as it arrives (ranked by the same fuzzy score against the name); answers to an older query are dropped. `t ` keeps kinds class, struct, interface, enum, delegate-like (LSP 5, 10, 11, 23, 26); `m ` keeps method, property, field, constructor, event, enum member, constant (6, 7, 8, 9, 24, 22, 14); `# ` keeps all symbols; `: N` is Go To Line in the active document. The selected row is previewed (opened at its position without moving focus out of the popup); Enter navigates and pushes the history; Escape returns to where the popup opened.

**Highlights.** After the caret rests 200 ms in a document whose server offers `documentHighlightProvider`, `textDocument/documentHighlight` is sent; the answer becomes the view's `reference_highlights` decoration layer (Background fills: `reference_highlight_read` for text and read, `reference_highlight_write` for write). Any edit clears the layer at once; a caret move outside every highlighted range clears it; a stale answer is dropped. Ctrl+Shift+Down / Up move the caret to the next / previous range (wrapping), in document order.

**Outline, File Structure, navigation bar.** One `documentSymbol` answer per document version, shared by the three; requested when the document opens, after edits settle (500 ms), and when a view asks; flat `SymbolInformation` answers are nested by containment. The current symbol is the innermost one containing the caret. The navigation bar shows the document's project (from the workspace tree), its types (containers: namespace children flattened) and the selected type's members; it is hidden for documents without a symbol provider and when `editor.navigationBar` is off.

**Call and Type Hierarchy.** Prepare at the caret; each node expands on demand with one request; `fromRanges` become the node's call sites; double-click (or `navigate`) opens the item or the site and pushes the history. Base and derived types in the type hierarchy likewise. A new prepare replaces the window's roots and cancels pending expansions.

**Find All References.** The window keeps today's grouping by project and file as the default `project_file`. Kind: `definition` for a location equal to the symbol's definition (a `textDocument/definition` sent with the references), `write` when the reference is assigned (`=`, compound assignments, `++`, `--`, `out`, `ref`), else `read` for identifiers (`reference` for a location whose line cannot be read). VS's columns: Code, File, Line, Column, Kind, Project. Keep Results: the current results stay in a kept list shown under the new ones; F8 / Shift+F8 step through the visible references and open each.

**Source-generated documents.** A location with the `roslyn-source-generated` scheme opens tab id = the URI, titled `Name.g.cs [generated]`, read-only, no document notifications, text from `workspace/textDocumentContent` through the host. The host forwards the method and announces `workspace.textDocumentContent: {}` to Roslyn.

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-commands navigation`: every command parses its examples, rejects bad input, and its outputs conform to the schemas.
- `cargo test -p eludite-lsp navigation`: parsing of each answer shape (locations, links, flat and nested symbols, call and type hierarchy items, highlight kinds), source-generated URI recognition, fan-out capability checks.
- `cargo test -p eludite navigation_set_tests goto_all_tests hierarchy_tests`, headless against the fake host and `FakeServer`: Ctrl+F12 with two implementations shows the picker and Enter navigates with history; Go To Base on a class navigates to its base; Go To All ranks files and types, honors the prefixes, drops a stale query's answer and streams symbol answers; highlights get the read and write colors, Ctrl+Shift+Down walks them in order, an edit clears them; the outline tree, its caret tracking and click; the navigation bar's dropdown selection navigates; call hierarchy expansion lists callers and sites; type hierarchy lists bases and derived types; the references Kind column, kind filter, Group By, search and F8; a source-generated target opens read-only with the host's text.
- `dotnet test dotnet/tests/Eludite.Host.Tests`: the proxy forwards the hierarchy methods and `workspace/textDocumentContent`; with the pinned server built, Ctrl+F12's request on an interface method lists its implementations and the call hierarchy of a method lists its callers.

## Budget

- Go To All: first file results < 100 ms after a keystroke on a 10,000-file index (asserted through `assert_budget`; reported under CI); symbol results stream as they come.
- The editor's keystroke budget unchanged: the caret tracker does no work but a timer reset per caret move; highlight, outline and bar updates are applied in one frame each.
- No new dependency outside the build (`fuzzy` is Zed's, Apache-2.0, already in the build through `eludite-editor`).

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test` on every crate touched, `dotnet build dotnet/Eludite.slnx` and the host tests green on Linux here.
2. The report lists what was built, the numbers, what was not run, and the Integration notes (crate map lines, README status, briefs index row).

## Out of scope

Peek Definition (needs inline display rows, brief 0067); the editor's context menu (brief 0068; Integration notes name the entries); the preview tab (brief 0070; Go To All previews with a normal open until it lands); listing generator outputs under Dependencies > Analyzers in the Workspace window (another brief's window); VS's Go To Recent files and Go To Text; Rider's Recent Locations; Roslyn's `_vs_` reference extensions (they need `_vs_supportsVisualStudioExtensions`, which changes Roslyn's answers everywhere); changing the Roslyn pin.
