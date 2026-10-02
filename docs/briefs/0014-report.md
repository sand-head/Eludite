# Brief 0014 report: go to definition, Find All References and Error List filtering

Status: done on Linux, except the Find All References latency budget, which the pinned Roslyn cannot meet (section 7).
Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0014-navigation`, on `origin/main` at `40c64e7` (main has not moved). Date: 2026-10-02.
Brief: [0014-navigation.md](0014-navigation.md).

## 1. Summary

- **It works end to end on `dotnet/Eludite.slnx`** with the real `eludite-host` and Roslyn, driven with real XTest keys
  and pointer clicks in a nested KWin (section 6):
  - F12 on `JsonRpc` in `HostServer.cs` opens a read-only tab titled `JsonRpc [from metadata]` with Roslyn's
    decompiled `StreamJsonRpc.JsonRpc`, the caret on the class name at line 31; Ctrl+- returns to `HostServer.cs`
    line 18.
  - Shift+F12 on `HostRpcTarget` fills the Find All References window, tabbed beside the Error List: "'HostRpcTarget'
    references: 18 results", grouped by project and file, the symbol highlighted in each line.
  - The Error List's Errors and Messages buttons clicked off leave the two warnings (CS0219, CS0168); the buttons
    still read "1 Error", "2 Warnings", "2 Messages".
- **Metadata as source with the pinned Roslyn is a real file.** The definition answer is a plain `file://` URI to
  `<temp>/MetadataAsSource/<session>/DecompilationMetadataAsSourceFileProvider/<id>/JsonRpc.cs`, decompiled with
  ICSharpCode.Decompiler. No host request was needed; the shell reads the file, opens it read-only and sends no
  document notifications for it. This is documented in `host-rpc.md` (section 2).
- **Budgets** (release build, Wayland backend, nested KWin at 60 Hz, 3 runs; load average 1.2 to 1.9):

  | Budget | Result |
  |---|---|
  | Definition in a file: caret at the target < 100 ms p95 after F12, host and UI separately over 100 runs | **F12 to caret p95 3.6 to 4.1 ms.** Host p50 1.5 ms, p95 2.3 to 2.6 ms; UI p50 4.4 to 4.6 ms, p95 5.6 to 5.7 ms. F12 to the presented frame p95 42 to 53 ms (the nested compositor's frame scheduling, as in brief 0013). Pass |
  | References window populated < 300 ms p95 for a symbol with < 100 references (18 for `HostRpcTarget`) | **507.7 to 509.2 ms p95. Fail**, all of it in Roslyn: host p95 503.0 to 503.6 ms, because the pinned server batches reference results with a fixed 500 ms delay (section 7). The shell's own share is p95 4.0 to 6.0 ms |
  | Keystroke frame cost < 8 ms p99 with the references window open holding 1000 rows | **p99 5.2 to 5.8 ms** (p50 2.5 ms, max 6.5 ms). Pass |

- **Tests:** `cargo test --workspace` 274 passed, 0 failed, 1 ignored (brief 0013: 257). fmt and clippy (`-D warnings`)
  are clean. The host did not change, so `dotnet test` was not required; `dotnet build dotnet/Eludite.slnx` reports
  0 warnings.
- **No new dependency.** No third-party or vendored crate was added; no new internal edge.

## 2. Protocol and metadata as source

**Command schemas** (`protocol/schemas/`, committed first and alone):

| Command | Input | Output | Permission |
|---|---|---|---|
| `eludite.editor.go_to_definition` | `{path?, line?, column?, target?}` (`target` chooses from the open picker) | `{path, line, column, state: loading\|navigated\|choose\|none\|failed, targets[≤100]: {path, line, column, metadata, title?}, navigated?, message?}` | read |
| `eludite.editor.find_references` | `{path?, line?, column?}` | `{path, line, column, state: loading\|done\|failed, symbol, total, references[≤1000]: {project?, path, line, column, text}, message?}` | read |
| `eludite.navigation.back`, `eludite.navigation.forward` | `{}` | `{navigated, path?, line?, column?, back, forward}` (`navigation.output.json`) | read |
| `eludite.error_list.filter` | `{errors?, warnings?, messages?, project?: string\|null, text?}` (omitted members keep their value) | `{errors, warnings, messages, project?, text, counts: {errors, warnings, messages}, shown, total}` | read |

As with the IntelliSense commands, a call from another thread (an agent) waits up to 5 s for the definition or the
references; from the UI thread it returns at once with `loading`.

**`host-rpc.md`** gains a paragraph on how the shell uses `textDocument/definition` and `textDocument/references`
(after a pending `didChange`, one of each in flight per window, superseded requests canceled, stale answers dropped,
`includeDeclaration: true`, `LocationLink` read as `targetUri` plus `targetSelectionRange`), and a section
"Metadata as source":

- Found empirically with a real request (F12 on `JsonRpc` in `HostServer.cs` against `Eludite.slnx`). The pinned
  Roslyn's `AbstractGoToDefinitionHandler` calls `IMetadataAsSourceFileService.GetGeneratedFileAsync`, which writes
  the decompiled type under `Path.Combine(Path.GetTempPath(), "MetadataAsSource")`, and answers with
  `ProtocolConversions.CreateAbsoluteDocumentUri(path)`: a plain `file://` URI. Answer seen:
  `file:///tmp/MetadataAsSource/006fad54.../DecompilationMetadataAsSourceFileProvider/c19c8186.../JsonRpc.cs`, range
  line 30, characters 13 to 20. The file starts with `#region Assembly StreamJsonRpc, Version=2.25.0.0` and the path of
  the assembly. First request 1.4 to 1.5 s (decompilation), then 3 to 54 ms.
- The host forwards it unchanged and serves no text: the file is on the shell's machine, and the host passes its
  environment to Roslyn, so `<temp>` is the shell's `std::env::temp_dir()`.
- The shell treats a target under `<temp>/MetadataAsSource/` as metadata: read-only, titled `<Type> [from metadata]`,
  no `didOpen`, `didChange`, `didSave` or `didClose`. Roslyn tracks these files in its own metadata workspace: a hover
  request inside the file without `didOpen` was answered (`class StreamJsonRpc.JsonRpc`).
- A definition URI with another scheme is reported as not navigable. Serving such text would need a new host request
  with a schema; none exists because the pinned server never sends one.

**Rust:** `DefinitionResponse::into_locations` flattens `Location`, `Location[]` and `LocationLink[]`, tested with
Roslyn's real metadata answer. No new typed message was needed: `definition` and `references` were already typed in
the bridge and validated by the host, so the host is unchanged.

## 3. What was built

**Editor (`crates/editor`, no internals changed):**
- A read-only mode: `EditorView::set_read_only`. The caret, selection, find, copy and requests work; typing, every
  editing action, paste and cut do nothing (cut copies), and completion and Parameter Info do not open. Programmatic
  edits through `update_editor` still apply.
- A navigation hook: Ctrl+click (without Alt or Shift) moves the caret and emits `EditorEvent::GoToDefinition`.
  Ctrl+Alt+click still adds a caret.

**Shell (`crates/eludite/src/shell/`):**
- `navigation.rs`:
  - Go To Definition through `HostSession::request` after the pending `didChange`. One request in flight per window;
    a new one cancels the old with `$/cancelRequest`. An answer is applied only when it is the newest and its
    generation and document version are current; a newest-but-outdated answer becomes `failed` with a message,
    never a navigation.
  - One target: the request position is pushed on the history, then the file opens at the target through
    `eludite.file.open`.
  - Several targets: a picker (a `DefinitionPicker` view anchored below the symbol) titled "N definitions of X".
    Up, Down, Enter, Escape and clicks work. Choosing runs `eludite.editor.go_to_definition {target}`.
  - No target: "Cannot navigate to the symbol under the caret." in the status bar (Visual Studio's message).
  - The history: per window, 50 positions back, a forward list cleared by any new navigation, positions in deleted
    files skipped, closed documents reopened (metadata ones read-only again).
- `references.rs`: the Find All References tool window.
  - Shift+F12 shows the window at once ("searching…") through `eludite.view.show`, then sends `references` with the
    declaration included.
  - Rows: project (or "Miscellaneous Files"), then file, then reference, each with the trimmed line text, the symbol
    highlighted and `(line, column)`. Line text is read off the UI thread: open documents from a buffer snapshot,
    other files from disk. UTF-16 columns are converted to characters.
  - Groups collapse and expand (triangle or double-click). Double-clicking a reference pushes the caret's position
    and opens the reference. A new search replaces the results.
  - The rows are a virtualized `uniform_list` in a cached view.
- `error_list.rs`: the toolbar.
  - A project dropdown (All Projects plus the projects of the rows).
  - Errors, Warnings and Messages toggle buttons with counts.
  - A search box over code and description, case-insensitive.
  - Every change runs `eludite.error_list.filter`; filtering is local and instant.
  - The counts are those of the rows passing the project and search filters, whatever the toggles, as in Visual
    Studio. `diagnostics.list` is unaffected and still takes `severity`.
- `documents.rs`: metadata documents (read-only, no notifications, no save, no undo, no `didClose`); tab titles come
  from `navigation::document_title`.

**Other crates:**
- `crates/docking`: the `find_all_references` tool window ("Find All References", bottom). It starts closed and opens
  as a tab of the Error List's group.
- `crates/ui`: `toggle_button`, `text_box` and `highlighted_code`; the keys F12, Shift+F12, Ctrl+- and Ctrl+Shift+-
  (also `ctrl-_` for layouts that report Shift+- as `_`); Edit > Go To Definition and Find All References;
  View > Navigate Backward and Navigate Forward.
- `crates/commands`: the five commands, their parsing and validation, and outputs checked against the schemas.

## 4. Tests

| Where | New | What |
|---|---|---|
| `eludite-protocol` | 1 | `into_locations` for Roslyn's metadata answer, a scalar, links (selection range) and an empty array |
| `eludite-lsp` `tests/in_process.rs` | 1 | typed definition and references through the fake host, with the generation |
| `eludite-editor` `tests/view.rs` | 2 headless | read-only (typing, Enter, Backspace, Delete, Tab, paste, cut and undo do nothing; movement, selection and find work); Ctrl+click moves the caret and emits the event, a plain click and Ctrl+Alt+click do not |
| `eludite-commands` | 1 | parsing, defaults, `project: null`/`""`, rejections, output schemas, permissions, `is_loading` |
| `eludite-docking` | (2 updated) | the default layout has Find All References closed, and showing it tabs it beside the Error List |
| `eludite-ui` | (1 updated) | the new keys and their display text |
| `eludite` unit | 4 | history (back, forward, cap of 50, skipping, duplicates); metadata titles and targets; reference rows (line text, trimming, highlight, UTF-16 to characters, CRLF); grouping and collapsing |
| `eludite` unit | 1 | Error List filters combined (severity, project, text on code and description) |
| `eludite` `shell/navigation_tests.rs` | 7 headless, against the fake host | below |

The 7 shell tests cover the Proving test:

1. **`definition_in_a_file_opens_positions_pushes_history_and_back_and_forward_walk_it`:**
   - F12 runs the command; the request carries the caret's position;
   - the target file opens through `eludite.file.open` with the caret on the definition;
   - the origin is pushed;
   - Ctrl+- and Ctrl+Shift+- walk back and forward, through the commands;
   - the output counts are checked; a closed document reopens from the history;
   - Ctrl+click goes to the definition of the clicked symbol.
2. **`definition_into_metadata_opens_a_read_only_tab_with_its_text`:**
   - a metadata-as-source file under `<temp>/MetadataAsSource/` opens titled `JsonRpc [from metadata]`, with the
     file's text and the caret at the target;
   - typing does nothing, Save fails, no `didOpen` or `didClose`;
   - Quick Info inside it still goes to the server;
   - Ctrl+- returns.
3. **`several_definitions_show_a_picker`:**
   - two targets open the picker (state `choose`, nothing pushed yet);
   - Down and Enter go to the second target;
   - Escape dismisses without navigating; a click on a row navigates;
   - `target` with no picker open is an error.
4. **`references_fill_the_window_grouped_and_double_click_navigates`:**
   - the window shows at once, tabbed with the Error List;
   - "'Main' references: 3 results";
   - rows `App (2)` / `Order.cs` / `Program.cs`, then `Miscellaneous Files (1)` / `Loose.cs`, with text,
     highlights and positions (a tab-indented line, trimmed);
   - collapsing a group;
   - double-clicks navigate and push the history;
   - a new search replaces the results.
5. **`stale_navigation_answers_are_dropped`:**
   - a superseded definition request is canceled, and its late answer (the host ignores the cancel) never
     navigates;
   - an answer for a document version that changed is dropped (`failed`);
   - a references answer computed under generation 1 never reaches the window after the solution reopens.
6. **`error_list_filters_hide_and_show_rows_and_keep_counts`:**
   - each toggle hides and shows the right rows, and the counts stay;
   - the project dropdown restricts to App (Loose.cs is in no project) and the counts follow;
   - typing in the search box filters by description and by code, and Escape clears;
   - the same through the bus returns the documented output;
   - `diagnostics.list` still returns everything (and filters by `severity`);
   - new diagnostics are filtered as they arrive.
7. **`agents_go_to_definition_and_find_references_on_the_bus`:** from another thread, `find_references` waits and
   returns `done` with the row; `go_to_definition` returns `navigated`; `navigation.back` returns.

The shell tests passed 5 times in a row.

## 5. Commits

1. `9ee8cf4` Specify the go to definition, find references, navigation and Error List filter commands and document
   metadata-as-source in host-rpc.md (schemas and `host-rpc.md` only)
2. `d84a1d6` Flatten definition results into locations in eludite-protocol and test typed definition and references
   through the fake host
3. `286f091` Add go to definition with the navigation history, read-only metadata-as-source tabs and the multi-target
   picker
4. `66e7b9b` Add the Find All References tool window grouped by project and file with click-through
5. `5b78de1` Add the Error List severity toggles, project dropdown and search filter
6. `25318da` Put navigation, references and Error List filtering on the command bus with Visual Studio's keys and menu
   items
7. `af14576` Test navigation, references, history, Error List filters and stale-answer dropping headlessly against the
   fake host
8. `a0475a4` Add the --bench-navigate harness and the navigation manual-run tooling
9. `a85fa5e` Record the navigation manual run, its screenshots and measurements
10. This report and the brief's Status line.

Commits 3 to 5 were split by file from one working tree.
- They do not build on their own: the shell's wiring and the command types land in commit 6, which builds.
- `cargo test` builds from commit 7 on, because commit 6 declares the test module that commit 7 adds.

## 6. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/navigation-linux.sh OUT_DIR`.
  - The drive phase uses the X11 backend on a nested Xwayland with real XTest input (`tools/navigation.py`).
  - The bench phase is `--bench-navigate 100`, three runs on the Wayland backend.
- **Host:** the Debug `eludite-host`, with Roslyn from `~/.cache/eludite/roslyn`.
- **Raw results:** `crates/eludite/results/linux-navigation.json`.
- **Load average** (1-minute): 1.3 to 1.5 for the driven run, 1.2 to 1.9 for the benchmarks. No other agent was
  running.

**Screenshots:**

- [`crates/eludite/screenshots/linux-go-to-definition-metadata.png`](../../crates/eludite/screenshots/linux-go-to-definition-metadata.png):
  - the input: Ctrl+F `JsonRpc CreateConnection`, Escape, Left, then F12;
  - tabs: `HostServer.cs` and the active `JsonRpc [from metadata]`;
  - the decompiled `public class JsonRpc : IDisposableObservable, IDisposable, ...` with the caret on `JsonRpc`,
    line 31;
  - trace: "definition reply 1: 1 locations (host 1381.9 ms)", the first request, which decompiles.
  - Ctrl+- then returned to `HostServer.cs` line 18 with the caret on `JsonRpc`.
- [`crates/eludite/screenshots/linux-find-all-references.png`](../../crates/eludite/screenshots/linux-find-all-references.png):
  - the input: the caret in the `HostRpcTarget` declaration, then Shift+F12;
  - the Find All References tab is active beside Error List and Output;
  - "'HostRpcTarget' references: 18 results", then Eludite.Host (10), LspProxy.cs (2) with both
    `HostRpcTarget` occurrences of line 763 highlighted, then Program.cs (3);
  - host 699 ms for this first search.
- [`crates/eludite/screenshots/linux-error-list-warnings.png`](../../crates/eludite/screenshots/linux-error-list-warnings.png):
  - the input: `int unusedLocal = 42; int neverRead; undefinedName();` typed into `HostRpcTarget.cs` line 77;
  - the Errors and Messages buttons were then clicked off with the real pointer;
  - the rows: CS0219 and CS0168, Eludite.Host, HostRpcTarget.cs, line 77;
  - the buttons read "1 Error", "2 Warnings" (pressed), "2 Messages".
  - `error-list-all.png` in the run directory shows all five rows before the clicks.

**A caveat on the warnings scene.** `dotnet/Directory.Build.props` sets `TreatWarningsAsErrors`, so in
`Eludite.slnx` every compiler warning arrives as an error, and the unfiltered list has no warnings to filter to. For
that scene only, the script sets `WarningsNotAsErrors=CS0168;CS0219` in the environment. MSBuild reads environment
variables as properties in Roslyn's design-time build, so those two stay warnings. No file was changed. The files the
driver typed into were not saved; the script checks this.

## 7. Budgets and measurements

**Method** (`eludite --solution dotnet/Eludite.slnx --open-file …/HostRpcTarget.cs --bench-navigate 100`). The run
waits for the load, the file's first diagnostics and 2 s more, then measures three things.

**1. Definition, 100 runs plus 3 of warm-up.**
- Each run presses F12 on the first `ISdkDiscoverer` (defined in `Sdk/ISdkDiscoverer.cs`), waits for the caret at the
  target and the frame that shows it, then presses Ctrl+- back.
- Host = the request written to the reply read.
- UI = key to request written + reply read to caret placed + that frame's render to end of present.

**2. References, 25 runs plus 3 of warm-up.** Each run presses Shift+F12 on `HostRpcTarget`. The split is the same,
with "rows in the window" in place of "caret placed".

**3. Typing.** The window is filled with 1000 synthetic rows built from the file's lines, in 5 projects and 25 files,
and shown. Then 300 keys are typed into a comment in the editor, in `--bench-type`'s rhythm. The keystroke frame
cost is brief 0009's method: the key handler plus the next frame's render to end of present.

| Measure | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Definition host p50 / p95 / p99 (ms) | 1.5 / 2.3 / 2.7 | 1.5 / 2.6 / 3.6 | 1.5 / 2.3 / 3.3 |
| Definition UI p50 / p95 / p99 (ms) | 4.4 / 5.7 / 6.2 | 4.6 / 5.6 / 6.0 | 4.5 / 5.6 / 6.7 |
| **F12 to caret p50 / p95** (ms) | 3.0 / **4.1** | 3.1 / **4.1** | 3.0 / **3.6** |
| F12 to presented frame p50 / p95 (ms) | 19.8 / 53.2 | 19.8 / 42.1 | 19.7 / 52.4 |
| References (18) host p50 / p95 (ms) | 502.7 / 503.6 | 502.1 / 503.0 | 502.4 / 503.1 |
| References UI p50 / p95 (ms) | 3.7 / 4.0 | 3.8 / 6.0 | 3.8 / 6.0 |
| **Shift+F12 to populated and drawn p95** (ms) | **507.7** | **508.4** | **509.2** |
| **Keystroke frame cost with 1000 rows p50 / p99** (ms) | 2.5 / **5.5** | 2.5 / **5.2** | 2.5 / **5.8** |
| First frame with 1000 rows, render to present (ms) | 3.4 | 4.1 | 3.1 |
| RSS at the end (MiB) | 96.2 | 96.2 | 96.4 |

There were no timeouts.

**Why references take 500 ms.** The pinned Roslyn's `FindUsagesLSPContext` reports references through an
`AsyncBatchingWorkQueue` created with `DelayTimeSpan.Medium`, which is 500 ms. Its `OnCompletedAsync` waits for that
batch. Every `textDocument/references` therefore answers no sooner than 500 ms after the first reference is found,
whatever the count; streaming with `partialResultToken` would only deliver the first batch at the same moment.
- The 300 ms budget cannot be met with this server through the shell or the host.
- Meeting it needs either a Roslyn patch (the delay is not configurable), or a different source: for example the host
  calling Roslyn's find-references service directly, which goes beyond forwarding LSP.
- This should go back to design (an ADR note, or a brief on the Roslyn pin).
- The shell's own share is 4 to 6 ms at p95.

**The definition frame tail.** The tail of "F12 to presented frame" (p95 42 to 53 ms) is the nested KWin's frame
scheduling in a freshly configured session, as brief 0013 found for completion. The caret is placed within 3.6 to
4.1 ms p95.

## 8. Protocol gaps and other findings

1. **References are batched by a fixed 500 ms delay in Roslyn** (section 7). This is the one budget miss.
2. **No read and write kinds.** Roslyn gives `VSInternalReferenceItem` (with `Kind`: read, write, reference) only to
   clients that advertise `SupportsVisualStudioExtensions`. The host advertises plain LSP, so references are bare
   `Location`s and the window shows no kind column.
   - Switching the host to the Visual Studio extensions would change other responses too.
   - Getting kinds through `textDocument/documentHighlight` per file is possible but untried.
3. **Metadata as source is recognized by its path.** `<temp>/MetadataAsSource/` is a Roslyn implementation detail. A
   future Roslyn that changed the directory, or a host on another machine, would break it. A typed marker from the
   host, or a `textDocument/metadataSource` request, would make it explicit.
4. **The decompiled file is never cleaned up by the shell.** Roslyn's own workspace manages `<temp>/MetadataAsSource`.
5. **Columns are UTF-16 code units for definition targets.** Reference rows are converted to characters; the
   definition target passes the LSP character as a column. These differ only after astral characters (the same gap as
   brief 0012's Error List columns).
6. **Find All References shows no kind, no column chooser and no grouping options.** Visual Studio's toolbar
   (grouping, "Entire solution" scope, search within results) is not built.
7. **Error List scope.** The project dropdown lists the projects of the current rows. Visual Studio's "Current
   document / Open documents / Entire solution" scope is not built, and diagnostics still exist only for open
   documents (brief 0012 gap 5).
8. **A focus gap in tests.** After `eludite.file.close` through the bus, keys reach the shell only after the next
   frame is drawn. The test redraws before pressing keys. Not seen with real input.

## 9. Sizing brief 0015

Brief 0015 covers rename, code actions, the shared workspace-edit applier, and `additionalTextEdits` from completion.

**What exists now:**
- the request pattern (one in flight, cancel, stale-drop with an "outdated" message);
- commands through the bus with agent waits;
- `eludite.file.open` positioning;
- the navigation history;
- the editor's anchored popups;
- the picker pattern (an anchored, focusable list), for the light bulb menu.

**What is missing:**
- **Protocol:**
  - type `textDocument/rename`, `prepareRename`, `codeAction` and `codeAction/resolve`; they are forwarded untyped
    today, and `prepareRename` is not forwarded at all, which needs a host change;
  - schemas for `WorkspaceEdit` with `documentChanges`, and for create, rename and delete file operations;
  - commands `eludite.editor.rename`, `eludite.editor.code_actions` and `eludite.editor.apply_code_action`.
- **The workspace-edit applier:**
  - apply to open buffers as one undoable transaction per document;
  - apply to closed files by opening, editing, sending `didChange`, and either saving or leaving them dirty as Visual
    Studio does;
  - version checks against `textDocument.version`;
  - resource operations;
  - a summary in the status bar.
- **Rename UI:** an inline rename box, Visual Studio's dashboard (include comments, strings, preview), Escape and Enter,
  and the result in the history.
- **Code actions:** Ctrl+. and a light bulb at the caret's line, `codeAction` with the line's diagnostics, lazy
  resolve, and preview.
- **Completion:** `additionalTextEdits` (resolve on commit, then the shared applier), and commit characters (brief
  0013 gaps 2 and 3).

**Estimate:** about the size of this brief, one agent-week if the applier is shared from the start. Rename and code
actions both depend on the applier, so it should come first, with its own headless tests over open and closed files.

## 10. Not done, caveats

- **Windows and macOS:** not run. **CI:** not run.
- **Not met:** the Find All References latency budget (section 7).
- **Files touched beyond the brief's list:** none.
  - The host is unchanged.
  - `crates/lsp` gained one test only.
  - `crates/eludite/tools` and `crates/eludite/results` are under `crates/eludite/**`.
- **Not updated:** `docs/briefs/README.md` (index row) and `CLAUDE.md`, which are outside this brief's files. The crate
  map still matches.
- **The 1000-row measure uses synthetic rows,** because no symbol in `Eludite.slnx` has 1000 references. The rows go
  through the same window, grouping and drawing as real results.
