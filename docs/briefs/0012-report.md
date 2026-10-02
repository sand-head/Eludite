# Brief 0012 report: open a solution end to end

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0012-open-solution`, rebased onto `origin/main` at `ff05449`. Date: 2026-10-02.
Brief: [0012-open-solution.md](0012-open-solution.md).

## 1. Summary

- **The pipes are real.** On Linux, Eludite opens `dotnet/Eludite.slnx` with the real `eludite-host` and Roslyn:
  - Solution Explorer shows the 8 projects and their files.
  - `HostRpcTarget.cs` opens in an `EditorView` document tab.
  - Typing an error sends a debounced `textDocument/didChange`. The host's diagnostics come back as red squiggles
    and as Error List rows. Deleting the error clears them.
  - Screenshots are in section 6.
- **Timings on `Eludite.slnx`** (release build, nested KWin; medians of the Wayland and X11 runs; all within the
  budget):

  | From `eludite.solution.open` to | Time |
  |---|---|
  | Editable text in the first file | **15.6 to 22 ms** (budget: under 1 s) |
  | The Solution Explorer tree | 490 to 499 ms |
  | The first diagnostics for the open file | 2.43 to 2.48 s |
  | The `loaded` status from Roslyn | 2.50 to 2.69 s |

  After a key press, the error's diagnostics arrive in 521 to 553 ms, and they clear 609 to 641 ms after the fix.
  That interval is the shell's 50 ms debounce, plus the host's 150 ms debounce, plus Roslyn's pull.
- **Contract first.** The schemas came first, in their own commit:
  - `eludite/solution/tree` in `protocol/schemas/host/solution-tree.json` and in `host-rpc.md`;
  - eight command schemas;
  - an optional `project` on the `diagnostics.list` output.

  Then, in order: the typed messages in `eludite-protocol`, the host, and the shell.
- **The UI thread never waits on the host.** A headless test stalls the fake host for 5 s. During the stall, 20
  edits take 44 ms in total (the worst edit 4.2 ms), and the bus answers. The edits reach the host after it recovers.
- **Keystroke frame cost with diagnostics arriving** (release build, 3 runs of 500 keys each, `LspProxy.cs`, 14
  `publishDiagnostics` per run):
  - p99 is 5.0 to 5.8 ms;
  - the same shell with no host and no diagnostics measures 4.8 to 5.5 ms;
  - both are under the 8 ms budget.

  Diagnostics do not change the keystroke cost. The shell around the editor costs about 2 ms at p99 more than
  brief 0009's bare viewer (1.9 to 3.0 ms): the menu, docks and status bar render every frame (section 7).
- **Tests.**
  - `cargo test --workspace`: 221 passed, 0 failed, 1 ignored (brief 0009 had 194; this brief adds 27).
  - `dotnet test dotnet/Eludite.slnx`: 128 tests, 127 passed, 1 skipped (the roslyn-200 bench solution is not
    generated here). There are 15 new host tests.
  - `cargo fmt --all --check` and `cargo clippy --workspace --all-targets -- -D warnings` are clean.
    `dotnet build` reports 0 warnings.
- **No new third-party dependency.**
  - File dialog: GPUI's own `App::prompt_for_paths`, which exists at the pin. On Linux it is the
    xdg-desktop-portal FileChooser through `ashpd` (MIT), already in GPUI's tree. `rfd` was not needed.
  - New internal edges, all already in the build:
    - `eludite` gains `clock`, `futures`, `eludite-editor`, `eludite-lsp` and `eludite-workspace`. `clock` is
      vendored, GPL-3.0-or-later. `futures` is MIT OR Apache-2.0.
    - `eludite-workspace` gains `eludite-protocol` (MIT).

## 2. Protocol: `eludite/solution/tree`

Schema: `protocol/schemas/host/solution-tree.json`. Prose: `protocol/schemas/host-rpc.md`, section
"`eludite/solution/tree`". Rust: `eludite_protocol::host::{SolutionTreeRequest, SolutionTree, TreeProject, TreeFile}`.

```
request  eludite/solution/tree   params: none
result   { generation, path | null,
           projects: [ { name, path, kind: "sdk" | "legacy", web?, targetFrameworks: ["net10.0"],
                         files: [ { path, itemType: "compile" | "content", dependentUpon?, link? } ],
                         error? } ] }
```

- **Not generational in the forwarded-request sense.** The result carries the generation it was computed under, and
  the shell drops a stale one.
- **Errors:**
  - -32002 before `eludite/host/initialize`;
  - -32801 with the usual data when the generation moves on before the tree is ready;
  - -32800 when the request is canceled.
- **Host implementation:**
  - `Projects/SolutionTreeProvider.cs` caches one evaluation per generation and runs it on the thread pool. It does
    not wait for Roslyn: on `Eludite.slnx` the tree is ready in about 0.5 s, and the load takes 2.5 s.
  - `Projects/MsBuildProjectTreeEvaluator.cs` evaluates every project with the .NET SDK's MSBuild in-process, through
    the brief 0003 locator registration and design-time properties. It is evaluation only: no targets run, and
    missing imports are ignored.
  - A multi-targeted project's items come from its first inner evaluation, because the outer evaluation has no
    default items.
  - `bin/` and `obj/` items are skipped. `DependentUpon` is resolved to an absolute path.
  - Web projects get their `Content` items. A project is web when it uses `Microsoft.NET.Sdk.Web`, has a WebForms or
    MVC project type GUID, or has markup items.
  - The legacy design-time preparation (designer partials, case fixups) is deliberately not part of the tree.
- **Tests:**
  - `SolutionTreeTests`, 15 tests:
    - SDK fixture: multi-targeting, folders, a removed item, a linked file, and a web project's content;
    - a synthetic legacy WebForms project: `net472` and code-behind under markup;
    - an unreadable project;
    - `dotnet/Eludite.slnx`: 8 projects, with `HostRpcTarget.cs` listed;
    - the corpus project `ChangePK/PrimaryKeysConfigTest`: `net45`, `Login.aspx.cs` and the designer under
      `Login.aspx`. It skips cleanly without `corpus/legacy/.checkout`. It ran here after a sparse fetch of
      `aspnet-samples` only.
    - the wire: before initialize, no solution, once per generation, ContentModified on close mid-evaluation, and
      cancel then reuse.
  - The `eludite-lsp` real-host test now also requests the tree.

## 3. Command bus

| Command | Input (schema) | Output | Permission |
|---|---|---|---|
| `eludite.solution.open` | `{path}` (`solution-open.input.json`) | `{path, state: "loading"}` | execute (design-time builds run) |
| `eludite.solution.close` | `{}` | `{closed, path?}` | read |
| `eludite.file.open` | `{path, line?, column?}` (relative to the solution directory allowed) | `{path, already_open}` | read |
| `eludite.file.close` | `{path, save?: "save" \| "discard"}` | `{path, closed, saved}` | edit_buffer |
| `eludite.editor.save` | `{path?}` (default: active document) | `{path, bytes}` | edit_buffer |
| `eludite.editor.undo`, `eludite.editor.redo` | `{path?}` | `{path, applied, dirty}` (`editor-history.output.json`) | edit_buffer |
| `eludite.editor.find` | `{path?, query?, case_sensitive?}` | `{path, found, line?, column?, find_bar_open?}` | read |
| `diagnostics.list` (existing) | `{severity?}` | rows, now with `project` | read |

- **`eludite.file.close` is beyond the brief's list.** The Contract needs it: closing a tab is a user action, sends
  `didClose`, and must ask about unsaved changes, so invariant 3 requires a command. This is the same reasoning by
  which brief 0008 added `eludite.view.dock`.
- **`eludite.file.open` replaces the stub.** `builtins` keeps a placeholder with the real spec, which fails with "no
  workspace is attached" until the shell registers the real handler. `workspace::register` replaces it through the
  new `CommandRegistry::replace`. This keeps `crates/mcp`'s mapping test (outside scope) green.
- **The existing Error List command is `diagnostics.list`,** not `eludite.diagnostics.list` as the brief says. It now
  reads the shell's real rows. A test asserts real CS diagnostics through the bus.
- **Menus:**
  - File > Open Project/Solution, File > Save and File > Close Solution;
  - Edit > Undo, Edit > Redo and Edit > Find and Replace.

  The ids of these items changed from stubs to these commands.
- **Keys:**
  - Ctrl+Shift+O (open solution), Ctrl+S, Ctrl+Z, Ctrl+Y (also Ctrl+Shift+Z) and Ctrl+F.
  - In the editor, the editor's own Undo, Redo and Find bindings are replaced by `RunCommand` bindings in the
    editor's key context, so these keys go through the bus first.
- **Threading:**
  - `eludite.solution.*` only hands work to the session worker, so they run on any thread.
  - File and editor commands touch GPUI entities:
    - On the UI thread, the shell applies the request and stages the typed result for the bus handler, so the audit
      log records every call.
    - From another thread (an agent), the request is posted to the UI and the caller waits. A request that names a
      file still loading waits for the load.

## 4. Shell

`crates/eludite/src/shell/`:

- **`session.rs`: host supervision.**
  - Where the host is found:
    1. `eludite-host`, or `eludite-host.dll` run with `dotnet`, beside the executable;
    2. else `ELUDITE_HOST`;
    3. else `PATH`.

    It is started with `--stdio` through `eludite_lsp::HostClient` with its default restart policy.
  - The host starts on the first solution open, never at startup, so cold start is unchanged.
  - A worker thread does every write to the host. A pump thread turns host events into `SessionEvent`s on a
    `futures` channel, which a foreground task drains.
  - After a host restart, the worker reopens the solution and replays the open documents.
- **Status bar.** A `solution` slot on the left shows Opening..., loading projects, "N projects loaded in X s", or
  "did not load: ...". A `language_server` slot on the right shows the eludite-host version, then
  "C#: starting / running / unavailable / exited". Both come from `eludite/solution/status` and
  `eludite/languageServer/status`.
- **`explorer.rs`: Solution Explorer.**
  - The model is `eludite_workspace::explorer::SolutionModel`, built from the tree and unit tested.
  - Labels: `Solution 'Eludite' (8 of 8 projects)`, then `Eludite.Host (net10.0)` with the target frameworks in
    parentheses, or `(load failed)`.
  - Folders mirror the file system. Linked files appear at their `Link` path.
  - Nesting: `DependentUpon` first, then by name: `X.aspx.cs` under `X.aspx`, and `X.designer.cs` under `X`, `X.cs`
    or `X.resx`.
  - Folders come before files, each sorted case-insensitively.
  - The rows are virtualized (`uniform_list`, using `eludite_ui::tree_row`).
  - Interaction: a click selects. The triangle, or a double-click on a folder or project, expands and collapses. A
    double-click on a file runs `eludite.file.open`.
  - When a file opens, its node is revealed and selected.
- **`documents.rs`: documents and LSP.**
  - Opening reads the file off the UI thread, creates an `EditorView`, focuses it, and sends `didOpen` (`csharp`,
    version 1) for `.cs` files.
  - The tab shows `Name*` while dirty.
  - **Debounce: `DIDCHANGE_DEBOUNCE` = 50 ms** after the last edit. The UI hands the whole text to the worker. The
    worker diffs it against what it last sent and sends one incremental change in UTF-16 positions.
  - Ctrl+S runs `eludite.editor.save`. It keeps the BOM and line endings (brief 0009's `to_file_bytes`), then sends
    `didSave`.
  - Closing a dirty tab asks Save / Don't Save / Cancel (`Window::prompt`). Closing sends `didClose`.
- **Diagnostics.**
  - Each result is anchored in the buffer snapshot of the version it was computed for, so it follows later edits.
    It becomes a wavy underline in the `diagnostics` decoration layer: red for errors, green for warnings, grey for
    messages. Hints are not drawn, as in Visual Studio.
  - A result for an older version than the last `didChange` is dropped (invariant 12).
  - A new generation clears everything.
- **`error_list.rs`: the Error List.**
  - Columns: Code, Description, Project, File, Line and Col. The header reads "N Errors  N Warnings  N Messages", and
    the tab stays "Error List".
  - Solution-load diagnostics (`ELUDITE000x`) appear with their project.
  - Double-clicking a row (Visual Studio's gesture) runs `eludite.file.open {path, line, column}`, which moves the
    caret and scrolls to it.
  - The explorer and the Error List are cached views, so they do not re-render on every keystroke frame.
- **Other components of this brief:**
  - `crates/docking`:
    - document-tab API: `open_document`, `close_document`, `retain_documents` and `set_document_dirty`;
    - a close button that dispatches `eludite.file.close`;
    - a dirty marker as view state (not persisted);
    - the document body no longer adds padding;
    - Reset Window Layout keeps open documents.
  - `crates/editor`: one read-only getter, `EditorView::decorations(layer)`, for the tests. No internals changed.
  - `crates/lsp`:
    - `HostClient::start_in_process(Connector)`, a host running in this process;
    - `eludite_lsp::fake::FakeHost` (feature `fake`). This is the scripted fake host the shell's tests use. It
      records every message, serves the tree, and can inject diagnostics and stall.

## 5. Tests

| Where | New | What |
|---|---|---|
| `eludite-protocol` | 1, plus schema conformance | tree round trip (including `path: null`), conformance and rejections against `solution-tree.json` |
| `eludite-lsp` `tests/in_process.rs` | 3 | tree, status and recorded notifications with injected diagnostics; a stalled host does not block `request`/`notify`; an in-process host restarts after `kill` |
| `eludite-commands` | 6 | specs from the protocol files, parsing every command, rejections, outputs against their schemas, the placeholder replaced, `CommandRegistry::replace` |
| `eludite-workspace` | 4 | Visual Studio tree (folders, `DependentUpon` and name nesting, links, sorting, failed project), default expansion, project lookup and reveal, no solution |
| `eludite-docking` | 2 headless | dirty marker drawn and not persisted, tab click goes through `eludite.view.show`, the close button dispatches `eludite.file.close` without activating the tab; reset keeps documents, `retain_documents` |
| `eludite` | 11 (5 headless GPUI) | see below |
| `Eludite.Host.Tests` | 15 | section 2 |

The 5 headless GPUI tests in `eludite` run against the in-process fake host, reached through the command bus:

1. **`open_solution_edit_diagnostics_and_error_list`.**
   - `eludite.solution.open` builds the tree. The status shows "1 project loaded".
   - Expand clicks and a double-click on a folder change the tree.
   - Double-clicking `Program.cs` opens it, and the fake records `didOpen`.
   - Typing marks the tab dirty. No `didChange` is sent at debounce minus 1 ms; at debounce it is sent with the exact
     incremental change.
   - Injected diagnostics produce 2 Error List rows. A hint is excluded. Code, project, file and line/column are
     checked, the header counts are checked, and squiggles cover exactly "class" and "Main".
   - `diagnostics.list` returns relative paths with the project.
   - A stale version is dropped.
   - Double-clicking a row moves the caret.
   - Ctrl+S writes the file and the fake records `didSave`.
   - Ctrl+Z goes through `eludite.editor.undo`.
   - The close button prompts. Don't Save closes the tab, the fake records `didClose`, and the rows go away.
   - `eludite.solution.close` clears the explorer.
2. **`ui_stays_responsive_while_the_host_stalls_for_5_s`.** The proving test for the budget:
   - 20 edits, each with an advanced debounce and a bus read, while the fake host reads nothing for 5 s;
   - total 44 ms, worst 4.2 ms (test asserts under 2 s and under 500 ms);
   - the stall is confirmed (no `didChange` reached the host during it);
   - the last version arrives after the stall.
3. **`agents_open_save_and_find_from_another_thread`.** From another thread: `eludite.file.open` with a relative path
   and a line, and `eludite.editor.find` right after, which waits for the load and selects "Main" at 3:17. A save of
   a file that is not open fails.
4. **`file_open_solution_menu_uses_the_path_prompt`.** File > Open Project/Solution shows the path prompt. The chosen
   file goes through the bus exactly once. Ctrl+Shift+O prompts too.
5. **`missing_host_is_reported_not_fatal`.** A bad path and a bad extension are rejected. A missing host shows in the
   status bar.

There are also unit tests for the diff (UTF-16 and astral characters), host location order, URI round trips, decoration
anchoring through edits, caret clipping and relative paths. The existing `eludite` tests are unchanged and pass.

## 6. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/open-solution-linux.sh OUT_DIR`. It runs a nested `kwin_wayland --virtual` at
  1280x960 and 60 Hz, because the session was locked.
- **Host:** the Debug `eludite-host`, set through `ELUDITE_HOST`, with Roslyn from `~/.cache/eludite/roslyn`.
- **Raw results:** `crates/eludite/results/linux-open-solution.json`. The 1-minute load average was 6.8 at the start
  and 2.4 at the end; a sibling agent was running its own benchmarks.

The steps:

1. **Wayland backend.**
   `eludite --solution dotnet/Eludite.slnx --open-file dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs --timings-out ...`
   - `--solution` now opens the solution through `eludite.solution.open`.
   - `--open-file` runs `eludite.file.open`, the same command a double-click runs.
2. **X11 backend on the nested Xwayland**, the same command line.
   - `tools/edit_error.py` waits for the `loaded` status and the file's diagnostics after load.
   - It types `x` at the top of the file with a real XTest key event. This gives `xusing System.Globalization;` and
     45 errors.
   - It waits for the diagnostics and takes screenshot 1.
   - It presses BackSpace, waits for the diagnostics to clear, and takes screenshot 2.
   - The timestamps come from `ELUDITE_TRACE_LSP=1` (stderr lines with wall-clock milliseconds).
3. **Typing cost.** Three runs each of `--bench-type 500` in `LspProxy.cs` (1,068 lines), with the solution and host,
   and without them.

| Measure | Wayland | X11 |
|---|---|---|
| open command to editable text (frame drawn) | 15.6 ms | 17.1 ms |
| to Solution Explorer tree | 494 ms | 491 ms |
| to first diagnostics for the file | 2,427 ms | 2,443 ms |
| to `loaded` (8 projects) | 2,505 ms | 2,613 ms |
| shell RSS at that point | 88.9 MB | 88.5 MB |
| key press to error diagnostics | | 521 ms |
| key press (fix) to cleared diagnostics | | 641 ms |

Three earlier runs on the same day gave the same picture:
- editable: 17 to 42 ms;
- tree: 479 to 519 ms;
- first diagnostics: 2.43 to 2.58 s;
- error shown: 522 to 598 ms;
- error cleared: 450 to 626 ms.

Screenshots (X11 backend, nested KWin):
- [`crates/eludite/screenshots/linux-open-solution-error.png`](../../crates/eludite/screenshots/linux-open-solution-error.png):
  - the tab is `HostRpcTarget.cs*`, and squiggles cover the file;
  - Solution Explorer shows Eludite.Host (net10.0), then Rpc, with HostRpcTarget.cs revealed and selected;
  - the Error List header reads "45 Errors  0 Warnings  1 Message";
  - the first row is CS0246, "The type or namespace name 'xusing' could not be found", in Eludite.Host,
    HostRpcTarget.cs, line 1, column 1;
  - the status bar reads "Eludite.slnx: 8 projects loaded in 2.5 s" and "C#: running".
- [`crates/eludite/screenshots/linux-open-solution-fixed.png`](../../crates/eludite/screenshots/linux-open-solution-fixed.png):
  the squiggles are gone, and only Roslyn's two IDE suggestions (IDE0290, IDE0305) remain as Messages.

**Not exercised with real input:**
- **Double-click in Solution Explorer.** The release build exposes no element bounds for the explorer rows, so the
  file was opened by `--open-file`, the same command. The headless tests drive the double-click with GPUI mouse
  events.
- **The portal file dialog.** The nested session has no portal set up. It is covered headlessly with GPUI's test
  platform.

## 7. Budgets

| Budget | Result |
|---|---|
| `Eludite.slnx` to editable text in the first file under 1 s after the open command | **15.6 to 22 ms: pass.** The file opens while Roslyn loads; the tree follows at about 0.5 s and diagnostics at about 2.4 s |
| Keystroke frame cost unchanged from brief 0009 while diagnostics arrive | **With host: p99 5.0, 5.6, 5.8 ms; without host: 4.8, 5.2, 5.5 ms; p50 2.4 vs 2.4 to 2.6 ms.** Diagnostics do not change it, and it is under the 8 ms budget. It is above brief 0009's bare viewer (p99 1.9 to 3.0 ms) because the shell renders its menu, docks and status bar each frame |
| UI thread never waits on the host (5 s stall test) | **Pass:** 20 edits in 44 ms during the stall |

- **Method.** The keystroke frame cost is brief 0009's: `Window::dispatch_keystroke`, then the key handler plus the
  next frame's render to end of present, without the wait for the next refresh.
- **Pacing.** Keys came in bursts of 25, 15 to 45 ms apart, with 250 ms pauses. That gave 14 `publishDiagnostics` per
  run while typing.
- **What made the difference.** The first version re-rendered Solution Explorer and the Error List on every editor
  frame. It measured p99 5.9 to 6.5 ms with the host against 4.7 ms without. Caching those two views (`.cached()`)
  closed the gap.

## 8. Protocol gaps found

1. **Solution folders are not in the tree.** `.slnx` `<Folder>` and `.sln` nested projects are missing, so Solution
   Explorer lists projects flat. VS shows `src` and `tests` for `Eludite.slnx`. The tree needs a `folders` member.
2. **No tree change notification.** The tree is computed once per generation, so a file added on disk or to the
   project needs a reopen. Needed: `eludite/solution/treeChanged`, or file watching plus `workspace/didChangeWatchedFiles`.
3. **No References, Dependencies or Packages nodes** (PLAN.md 4.2). The tree carries no project references or
   packages.
4. **`None` items are not listed** (for example `appsettings.json` in a non-web project), nor are other item types.
   VS lists them. Show All Files is out of scope.
5. **Diagnostics exist only for open documents.** The host pulls per open document, so the Error List shows open
   files and project-load problems, not the whole solution. VS-like full-solution analysis needs `workspace/diagnostic`
   or a build.
6. **Project-load diagnostics have no location.** `ELUDITE000x` carry a project, not a file and line, so the Error
   List points at the project file, line 1.
7. **Error List columns are UTF-16 code units.** LSP columns are UTF-16; VS counts characters. They differ only for
   astral characters.
8. **Command id mismatch.** The brief names `eludite.diagnostics.list`; the command is `diagnostics.list` (brief 0005).
   Renaming it would change the MCP tool name, so it was left alone.
9. **No cancellation of a superseded tree request.** The shell does not cancel an outstanding tree request when a new
   open supersedes it. The host answers -32801 anyway, so this is cheap, but it is a gap.

## 9. Sizing brief 0013

Brief 0013 covers completion, hover, definition and references in the editor, Error List filtering, and status bar
details.

**What exists:**
- the typed requests in `eludite-protocol`;
- generation pinning and cancellation in the client;
- `EditorView::pixel_position_for_offset` for popups;
- `eludite.file.open {line, column}` for navigation;
- the document and diagnostics plumbing from this brief.

**What is missing:**
- a completion popup: list, filtering with the vendored `fuzzy`, commit characters, text edits and
  `additionalTextEdits`, resolve on selection, and a tree-sitter fallback;
- a hover popup: markdown as plain text at first;
- a request scheduler per document: cancel on edit, drop stale versions;
- a Find All References tool window: new registration, grouped rows, click-through;
- Error List filters: current document, open documents or entire solution, severity toggles, a search box;
- status bar details: Ln/Col, encoding and line endings from the active editor, and host memory;
- commands with schemas for each: `eludite.editor.complete`, `hover`, `go_to_definition`, `find_references`, and
  `eludite.error_list.filter`.

**Estimate:** about 1.5 times this brief, or roughly two agent-weeks. Split it in two:
- **0013a:** the completion and hover popups with their commands and the scheduler.
- **0013b:** definition, references, Error List filtering and the status bar.

## 10. Commits

1. `eb99426` Specify eludite/solution/tree and the solution, file and editor command schemas (schemas only, first)
2. `fb122b6` Type eludite/solution/tree in eludite-protocol with round trip and schema tests
3. `d30c6ee` Serve eludite/solution/tree from the host's in-process MSBuild evaluation
4. `b3f5d9b` Let eludite-lsp run an in-process host and ship the scripted fake host behind a feature
5. `bcc855f` Add the solution, file and editor commands to the command bus
6. `5a018af` Build Solution Explorer's tree from the host's solution tree in eludite-workspace
7. `1384de8` Add tree rows, document tab close and dirty markers, and the workspace keys and menu commands
8. `e080a0b` Open solutions in the shell: host session, Solution Explorer, editor documents, diagnostics and the Error List
9. `3c686aa` Test opening a solution headlessly against the fake host, including a 5 s host stall
10. `1286101` Run the open-solution flow on Eludite.slnx with timing and typing harnesses, and fix the Error List columns, tree reveal and tool view caching it exposed
11. This report and the brief's Status line.

## 11. Not done, caveats

- **Windows and macOS:** not run.
- **CI:** not run.
- **Saving writes the file on the UI thread.** That is fine for source files, but a very large file or a slow disk
  would block a frame. It should move to a background write with the result reported back (invariant 1).
- **The MCP server is not running in the `eludite` binary yet** (brief 0008 noted this). `diagnostics.list` returns
  real data to whoever holds the shell's registry. This is tested through the bus, not over MCP.
- **Undo back to the saved text still shows the tab dirty.** Dirty means the buffer version differs from the saved one.
- **Host test flake.** `HostProcessTests.Stdout_IsProtocolOnly_AndRenamedLifecycleExitsCleanly` failed once in one
  full `dotnet test` run. It passed in the next 2 full runs and in 3 isolated runs. It looks timing-sensitive under
  load and is unrelated to this brief's code.
- **Outside this brief's files:** `docs/briefs/README.md` (index row) and `CLAUDE.md` were not touched. The crate map
  still matches.
- **Corpus checkout.** It was fetched for `aspnet-samples` only, under the gitignored `corpus/legacy/.checkout`. On a
  fresh clone, the corpus test skips.
