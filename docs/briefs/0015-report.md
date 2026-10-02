# Brief 0015 report: rename, code actions and the workspace-edit applier

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0015-rename-and-code-actions`, on `origin/main` at `8603e44` (main has not moved). Date: 2026-10-02.
Brief: [0015-rename-and-code-actions.md](0015-rename-and-code-actions.md).

## 1. Summary

- **It works end to end on `dotnet/Eludite.slnx`** with the real `eludite-host` and Roslyn, driven with real XTest keys
  in a nested KWin (section 6):
  - Ctrl+R, Ctrl+R on `HostRpcTarget.Ping` opens the Rename dialog; typing `PingHost` shows the preview
    (`HostRpcTarget.cs`, line 67, `public PingResult PingHost()`); Enter applies it as one undo step.
  - On `DotnetCliSdkDiscoverer`'s constructor the light bulb appears; Ctrl+. opens the menu (Fixes: "Use primary
    constructor", "Use primary constructor (and remove fields)", "Suppress or configure issues ▸"; Refactorings: "Use
    expression body for constructor"); Enter resolves and applies "Use primary constructor" (2 edits).
  - `var sb = new StringBuil`, Ctrl+Space, Tab commits `StringBuilder` (an unimported type) and adds
    `using System.Text;`.
  - Every file was restored with `git checkout -- dotnet/`; `git status` shows only this brief's changes.
- **Budgets** (release build, Wayland backend, nested KWin at 60 Hz, 3 runs of `--bench-refactor 100`; load average
  1.9 to 2.5):

  | Budget | Result |
  |---|---|
  | Light bulb visible < 150 ms p95 after the caret stops (host and UI separately) | **120.5 / 124.1 / 125.6 ms p95** caret-stop to the presented frame, including the 50 ms debounce. Host p50 30.6 ms, p95 66.6 to 71.0 ms; UI p50 4.5 to 4.6 ms, p95 5.6 to 6.0 ms. Pass |
  | Rename preview < 500 ms p95 (< 50 references) | Name change to preview shown **170.2 to 171.6 ms p95**, including the 150 ms debounce. Host p95 13.9 to 14.6 ms, UI p95 7.1 to 7.5 ms (`_dotnetPath`, 3 edits). `ISdkDiscoverer` (5 edits in 4 files, 3 of them closed): host p95 23.0 ms, UI p95 7.6 ms, total p95 178.8 ms. Pass |
  | Apply < 100 ms for 10 files | **18.4 to 18.8 ms p95** (`eludite.workspace.apply_edit`, 30 edits in 10 closed files, read, edited and written atomically off the UI thread, invoke to summary). Pass |
  | Keystroke frame cost < 8 ms p99 with the light bulb active | **p99 5.5 to 5.9 ms** (p50 2.6 ms; 12 to 13 light bulb requests during 300 keys). Pass |

  The first rename in a session is slower: Roslyn answered `Ping`'s rename in 417 ms in the manual run (cold).
- **Contract first.** Commit 1 holds only `protocol/schemas`: four command schemas, five host schemas
  (`prepare-rename.json`, `rename.json`, `code-action.json`, `code-action-resolve.json`, `apply-edit.json` with the
  shared `WorkspaceEdit` shape) and `host-rpc.md`.
- **Tests:** `cargo test --workspace` 303 passed, 0 failed, 1 ignored (brief 0014: 274). `dotnet test` 133 tests: 129
  passed, 4 skipped, 0 failed (brief 0014: 129, +4). fmt and clippy (`-D warnings`) clean; `dotnet build` 0 warnings.
- **No new dependency.** No third-party or vendored crate was added. New internal edges: none (the `eludite` crate
  already used `clock`, `eludite-protocol`, `eludite-lsp`, `eludite-editor`, `eludite-ui`, `eludite-commands`).

## 2. Protocol and host

**Host methods typed** (`host-rpc.md`, typed table; schemas in `protocol/schemas/host/`):
`textDocument/prepareRename` (new: it was not forwarded at all before), `textDocument/rename`,
`textDocument/codeAction`, `codeAction/resolve`. `codeAction` and `rename` left the untyped table.
`codeAction/resolve` is validated for a string `title`, the others for `textDocument.uri`.

**`workspace/applyEdit`, host to shell** ([apply-edit.json](../../protocol/schemas/host/apply-edit.json)): the host
relays the language server's request with `eluditeGeneration` added, and returns the shell's answer unchanged; a shell
error or a lost connection is `{ applied: false, failureReason }`; the server's cancellation is passed on. It is now
the one request the host sends the shell (`methods::HOST_TO_SHELL_REQUESTS`).

**Rust** (`eludite-protocol`): `TextEdit`, `TextDocumentEdit`, `OptionalVersionedTextDocumentIdentifier`,
`ResourceOperation` (create, rename, delete), `DocumentChange`, `WorkspaceEdit`, `PrepareRenameResponse`,
`RenameParams`, `CodeActionParams`/`Context`, `Command`, `CodeAction`, `CodeActionOrCommand`,
`ApplyWorkspaceEditParams`/`Result`, and the markers `PrepareRename`, `Rename`, `CodeActionRequest`,
`ResolveCodeAction`, `ApplyEdit`. Drift tests cover the method lists, the markers, and each schema with rejections.

**Client** (`eludite-lsp`): a request from the host is `Event::ApplyEdit { id, params }`, answered with
`HostClient::respond_apply_edit`; a malformed one is answered InvalidParams, any other method MethodNotFound. The fake
host gained `apply_edit` and `request_shell` (it now records responses to its own requests).

**Host changes** (`dotnet/src/Eludite.Host/Lsp/LspProxy.cs`, `HOST.md`, tests):
- the four requests in `TypedRequests`, `title` validation for `codeAction/resolve`;
- `RelayApplyEditAsync` registered for `workspace/applyEdit` on the upstream connection;
- client capabilities: `workspace.applyEdit`, `workspace.workspaceEdit` (`documentChanges`, `resourceOperations`
  create/rename/delete, `failureHandling: abort`), `textDocument.codeAction` (`codeActionLiteralSupport` with seven
  kinds, `resolveSupport: [edit]`, `dataSupport`, `isPreferredSupport`, `disabledSupport`), `textDocument.rename`
  (`prepareSupport`). Roslyn needs `resourceOperations` to return create, rename and delete file changes from
  `codeAction/resolve`.
- 4 new host tests: `RenameAndCodeActions_AreTypedValidatedAndForwarded`,
  `ClientCapabilities_AdvertiseWorkspaceEditsCodeActionsAndRename`,
  `ApplyEdit_IsRelayedToTheShellWithTheGenerationAndAnswered`,
  `ApplyEdit_ShellErrorIsNotAppliedAndCancellationIsRelayed`.

**Command schemas** (`protocol/schemas/`):

| Command | Input | Output | Permission |
|---|---|---|---|
| `eludite.editor.rename` | `{path?, line?, column?, new_name?, apply?}` (no name: the dialog) | `{path, line, column, state: loading\|dialog\|preview\|applied\|rejected\|failed, symbol?, new_name?, files[≤100]: {path, open, changes[≤100]: {line, before, after}}, total_edits, summary?, message?}` | edit_buffer |
| `eludite.editor.code_actions` | `{path?, line?, column?}` | `{path, line, column, state: loading\|open\|none\|failed, actions[≤200]: {index, title, group, kind?, preferred?, parent?, disabled?}, message?}` | read |
| `eludite.editor.apply_code_action` | `{index}` or `{title}` | `{state: resolving\|applying\|applied\|expanded\|unsupported\|failed, title, summary?, message?}` | edit_buffer |
| `eludite.workspace.apply_edit` | `{edit, label?}` | `{state: applying\|applied\|failed, applied, files, edits, open_documents, files_on_disk, created, renamed, deleted, paths[≤1000], message?}` | edit_buffer |

From another thread (an agent) each call waits up to 5 s for its outcome, like the IntelliSense commands.

## 3. What was built

**The workspace-edit applier** (`crates/eludite/src/shell/workspace_edit.rs`), used by rename, code actions,
completion's additional edits, `workspace/applyEdit` and `eludite.workspace.apply_edit`:
- **Open documents vs disk.** A document open in an editor (matched by normalized path, then by URI) changes in its
  buffer as one undo step (`Editor::apply_edits`, which never merges with typing) and gets `didChange` at once; it is
  left unsaved (dirty), as Visual Studio leaves it. Ranges are read in the text the server last saw (the document's
  sent snapshot) and anchored there, so unsent typing does not shift them. A closed file is read, edited and written
  off the UI thread with its BOM and line endings kept; every new content goes to a temporary file beside its target
  (with the target's permissions, `sync_all`) and only when all are written are they renamed into place; then
  `workspace/didChangeWatchedFiles` (created 1, changed 2, deleted 3).
- **File operations** run in order on a model of the files, so an edit after a create or rename lands in the new
  file. An open document a file operation touches must be saved first; its tab follows a rename and closes on a delete.
  Folder renames are refused (not supported).
- **Version mismatches refuse the whole edit before anything changes**: a `TextDocumentEdit.version` that is not the
  open document's current LSP version; a rename's or code action's edit when any open document it touches moved past
  the version the request was made at; an edit computed under another solution generation; a range past the end of
  a document, a reversed range or overlapping edits; a read-only (metadata) document. The pinned Roslyn sends
  `version: null`, so the request-time versions are what protects rename and code actions.
- **The summary**: files, edits, open documents, files on disk, created, renamed, deleted, sorted paths, and the
  refusal message; also in the status bar ("Rename 'Ping' to 'PingHost': 1 edit in 1 file").

**Rename** (`rename.rs`): prepareRename first (`null` is Visual Studio's "You must rename an identifier."), then the
dialog (`crates/ui` `dialog_panel`, `push_button`, `section_heading`): the name selected so typing replaces it, a
150 ms debounced `textDocument/rename` per name change (newest wins), the preview's changed lines computed off the UI
thread (closed files read from disk), Enter or Apply through `eludite.editor.rename`, Escape cancels. A `null`
answer for a name shows "The rename cannot be performed: …" in the dialog.

**Code actions** (`code_actions.rs`): 50 ms after the caret rests on a new position, `codeAction` (trigger 2) with the
diagnostics shown on the caret's line; a bulb in the new 20 px margin left of the line numbers (`EditorView::
set_lightbulb`, yellow with fixes, blue-gray with only refactorings; a click emits `EditorEvent::LightbulbClicked`).
Ctrl+. (also Alt+Enter) opens the menu from that answer when it is for the caret and the current text, else asks
with trigger 1. Groups: Fixes, Refactorings, Other actions; Roslyn's nested actions become a submenu (Right, Left);
Fix All actions are dropped. Applying resolves lazily when there is no edit; an action with only a command is
reported unsupported.

**Completion's `additionalTextEdits`** (`intellisense.rs`): on accept, from the item, or from the lazy documentation
resolve (now kept with the text it was computed on), or from a `completionItem/resolve` sent at commit; applied through
the applier and merged into the commit's undo step (`Editor::merge_transactions`) when nothing was typed in between.

**Editor** (`crates/editor`, no other internals): `Editor::apply_edits`, `last_transaction`, `merge_transactions`
(and the `Buffer` counterparts); the light bulb margin; `AcceptedCompletion` now names the list and index the
committed item came from (needed to find the server's item; additive).

**Keys and menus** (`crates/ui`): Ctrl+R, Ctrl+R and F2 (Refactor.Rename), Ctrl+. and Alt+Enter (View.QuickActions);
Edit > Rename... and Edit > Quick Actions and Refactorings....

## 4. Tests

| Where | New | What |
|---|---|---|
| `eludite-protocol` | 5 | WorkspaceEdit shapes with resource operations and annotations; rename and prepareRename (three result forms); Roslyn's code actions (nested, Fix All, bare command) and resolve; applyEdit; schema conformance and rejections for all five schemas |
| `eludite-lsp` `tests/in_process.rs` | 2 | typed prepareRename, rename, codeAction and resolve through the fake host with the generation; applyEdit request → event → answer, InvalidParams, MethodNotFound |
| `Eludite.Host.Tests` | 4 | section 2 |
| `eludite-editor` | 2 | workspace edits are one undo step that never merges with typing; merged steps undo together |
| `eludite-ui` | 1 (+1 updated) | bulb colors; the new keys in the keymap test |
| `eludite-commands` | 1 | parsing, rejections, output schemas, permissions, `is_loading` of the four commands |
| `eludite` unit | 7 | edit resolution (UTF-16, order, clipping, overlaps, past the end); plans; disk steps create/rename/edit/delete atomically and a failing step changes nothing; preview lines; preview of open and closed files; Roslyn answers to grouped menus |
| `eludite` `workspace_edit_tests.rs` | 5 headless | open buffers (one undo, didChange at once, unsent typing), closed files, mixed; stale versions, request versions, generations and ranges refuse the whole edit; create/rename/delete with didChangeWatchedFiles, a dirty open document refused, the open tab following the rename; host-initiated applyEdit applied and a stale one answered `applied: false`; the same edit gives the same summary |
| `eludite` `refactor_tests.rs` | 6 headless | below |

The 6 shell tests, against the fake host:
1. **Rename flow**: Ctrl+R, Ctrl+R → prepareRename at the caret → the dialog; typing → one rename request after the
   debounce; the preview (two files, the closed one first, lines before and after); Enter applies through the bus;
   the buffer changes unsaved, the closed file on disk; one undo; F2 then Escape cancels.
2. **Rejections**: prepareRename `null` → no dialog, "You must rename an identifier."; a refused name → the dialog
   shows why and Enter changes nothing.
3. **Light bulb and menu**: one request after the debounce with the line's diagnostic; the bulb on the caret's line;
   Ctrl+. opens from that answer (no second request), grouped, preferred selected, no Fix All; Right/Left on a nested
   action; Enter resolves (with `data`) and applies; one undo; a click on the bulb opens the menu; a command-only action
   is unsupported; a nested one expands; an action with an edit is applied without resolve.
4. **additionalTextEdits**: an item carrying its `using` and an item resolved at commit; both join the commit's undo
   step.
5. **Stale answers**: a light bulb request canceled by an edit never shows its late answer; a prepareRename answered
   after the solution reopened opens no dialog and the rename is `failed`.
6. **Agents on the bus** (another thread): rename preview with `apply: false`, rename with apply (closed file written),
   code actions, apply by title, and `eludite.workspace.apply_edit`.

The `eludite` tests passed 5 times in a row.

## 5. Commits

1. `874b994` Specify rename, code action and apply-edit commands and type rename, code actions and workspace/applyEdit
   in protocol/schemas (schemas and `host-rpc.md` only)
2. `fa23ed0` Type rename, code actions and workspace/applyEdit in eludite-protocol, relay applyEdit through the client
   and the host, and advertise the new client capabilities
3. `25b0042` Add the workspace-edit applier for open buffers, closed files and file operations, with one undo step per
   document and atomic writes
4. `723a572` Add the Rename dialog with prepareRename, a debounced preview of files and changed lines, and apply
   through the workspace-edit applier
5. `d01b735` Add the light bulb margin, the code action menu grouped as Visual Studio does, lazy codeAction/resolve
   and apply through the applier
6. `d0e1729` Apply completion additionalTextEdits on accept through the applier, resolving the item first when
   needed, as part of the commit's undo step
7. `dbe9a0d` Put rename, code actions, apply code action and apply edit on the command bus with Ctrl+R Ctrl+R, F2,
   Ctrl+. and Edit menu items
8. `8f46321` Test rename, the light bulb and its menu, additional completion edits, stale answers and agents headlessly
   against the fake host (also fixes a bug the tests found: moving the caret canceled a Ctrl+. request in flight)
9. `7e88822` Add the --bench-refactor harness and the rename and code action manual-run tooling, and record the run,
   its screenshots and measurements
10. This report and the brief's Status line.

Every commit builds and its tests pass. Commits 4 to 7 build with dead-code warnings (rename, then code actions, are
reached through the bus only from commit 7), and commit 7 fails `clippy --all-targets` on four test-only accessors
until commit 8 adds their tests. Commit 1 alone fails the protocol drift tests (by design: schemas first).

## 6. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/refactor-linux.sh OUT_DIR` (drive: `tools/refactor.py` on the X11 backend in a
  nested Xwayland with real XTest keys; bench: `--bench-refactor 100`, three runs on the Wayland backend). It refuses
  to run unless `dotnet/` is clean in git and restores it with `git checkout -- dotnet/` after each app.
- **Host:** the Debug `eludite-host`, Roslyn from `~/.cache/eludite/roslyn`.
- **Raw results:** `crates/eludite/results/linux-refactor.json`.
- **Load average** (1-minute): 6.6 to 9.0 during the driven run (something else on the machine was busy then; the
  latencies below are from the trace), 1.9 to 2.5 for the benchmarks.

**Screenshots:**
- [`linux-rename-preview.png`](../../crates/eludite/screenshots/linux-rename-preview.png): HostRpcTarget.cs, the
  Rename dialog titled "Rename: Ping", New name `PingHost`, the preview "▾ HostRpcTarget.cs … (1 change, open)" with
  line 67 `public PingResult PingHost()`, "1 change in 1 file", Apply and Cancel; the light bulb in the margin of line
  67; the Error List shows IDE0290 "Use primary constructor" at line 27. Trace: prepareRename 72 ms, `rename` host
  417 ms (the first in the session); Enter: "applied true, 1 edits in 1 files (1 open, 0 on disk)" in 0.1 ms.
- [`linux-code-action-menu.png`](../../crates/eludite/screenshots/linux-code-action-menu.png):
  DotnetCliSdkDiscoverer.cs line 10, the yellow bulb in the margin, the menu below it: Fixes ("Use primary
  constructor" selected, "Use primary constructor (and remove fields)", "Suppress or configure issues ▸"),
  Refactorings ("Use expression body for constructor"); IDE0290 in the Error List. Trace: the bulb's request 185 ms
  (24 actions with nested ones), resolve 223 ms, "applied true, 2 edits in 1 files". `primary-constructor.png` in the
  run directory shows the result.
- [`linux-completion-using.png`](../../crates/eludite/screenshots/linux-completion-using.png): line 2
  `using System.Text;` added, line 18 `var sb = new StringBuilder();`; trace: `accept completion "StringBuilder"`,
  "completion: 1 additional edits applied" (from the commit-time resolve). The line was typed at column 0 by the
  driver (it pressed Home before Enter), hence its indentation.

The run used `Use primary constructor` on `DotnetCliSdkDiscoverer` rather than on `HostRpcTarget` (both report
IDE0290); the action is the same.

## 7. Budgets and measurements

Method (`--bench-refactor N`, details in `bench.rs`): (1) the caret alternates between `DotnetCliSdkDiscoverer(` (24
actions) and `DiscoverAsync` (2 actions), N times plus 3 warm-up; (2) 300 keys in a comment with the bulb active;
(3) one letter typed per run in the Rename dialog on `_dotnetPath`; (4) 20 applies of three empty inserts into each of
10 closed files.

| Measure | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| Light bulb host p50 / p95 / p99 (ms) | 30.6 / 66.6 / 112.2 | 30.6 / 69.8 / 118.8 | 30.6 / 71.0 / 110.3 |
| Light bulb UI p50 / p95 / p99 (ms) | 4.6 / 6.0 / 6.5 | 4.6 / 5.6 / 5.9 | 4.5 / 5.6 / 5.9 |
| **Caret stop to bulb visible p50 / p95** (ms, incl. 50 ms debounce) | 85.9 / **120.5** | 86.0 / **124.1** | 85.7 / **125.6** |
| **Keystroke frame cost p50 / p95 / p99** (ms) | 2.6 / 3.9 / **5.5** | 2.7 / 4.1 / **5.7** | 2.6 / 4.1 / **5.9** |
| Rename host p50 / p95 (ms) | 11.9 / 14.6 | 11.7 / 13.9 | 12.0 / 14.6 |
| Rename UI p50 / p95 (ms) | 6.2 / 7.2 | 6.3 / 7.5 | 6.2 / 7.1 |
| **Name change to preview visible p95** (ms, incl. 150 ms debounce) | **171.6** | **170.2** | **171.2** |
| **Apply to 10 closed files p50 / p95** (ms) | 17.4 / **18.7** | 17.6 / **18.8** | 17.6 / **18.4** |
| RSS at the end (MiB) | 99.8 | 99.6 | 100.4 |

No timeouts. The light bulb's p99 (165 to 173 ms) is Roslyn's tail on the constructor (24 actions). Rename of
`ISdkDiscoverer` (5 edits in 4 files, one run of 50): host p95 23.0 ms, UI p95 7.6 ms, total p95 178.8 ms. The apply
budget's "10 files" was measured with the applier alone (no symbol in the solution is referenced in exactly 10 files);
a real multi-file rename's apply is the same code (the manual run's renames applied in 0.1 to 0.2 ms when only open
buffers were involved).

## 8. Protocol gaps and findings

1. **The pinned Roslyn never sends `workspace/applyEdit`** for C#: code actions return edits through
   `codeAction/resolve`; only Razor's cohosting uses the request. The relay is built and tested with the fakes, but
   was not exercised by the real server.
2. **Roslyn's code-action commands are client-side.** `roslyn.client.nestedCodeAction` (shown as a submenu) and
   `roslyn.client.fixAllCodeAction` (Fix All, not offered: out of scope). No server command goes through
   `workspace/executeCommand`, so the host does not forward it; a command-only action is reported unsupported.
3. **`roslyn.client.completionComplexEdit`** (override and partial-method completion) is not run: such items insert
   their label only. Supporting it means applying the command's `TextEdit` argument through the applier.
4. **Rename has no reason when it fails.** Roslyn answers `null` for a conflict or an invalid name; the dialog says
   so generically. Visual Studio's conflict display, "include comments/strings" and "rename overloads/file" need
   Roslyn's VS-only rename extensions.
5. **Version checks rest on request-time versions**, because the pinned Roslyn sends `version: null` in every
   `TextDocumentEdit`. A host-initiated edit with null versions is applied against the text the server last saw.
6. **Closed-file changes reach Roslyn twice**: the host forwards `didChangeWatchedFiles`, but advertises no dynamic
   registration, so Roslyn relies on its own file watcher; the notification is harmless but probably unused.
7. **Light bulb range.** The request uses an empty range at the caret with the line's diagnostics. Fixes for a
   diagnostic elsewhere on the line need the caret inside its span, as in Visual Studio Code; Visual Studio itself
   looks at the whole line.
8. **First requests are slow** (rename 417 ms, resolve 223 ms, light bulb 185 ms on first use in a session); warm
   requests meet the budgets.
9. **Two applies in flight are not serialized.** Each validates and writes independently; an overlap would be caught
   only by version checks on open documents.

## 9. Sizing the next briefs

**0016: the Agents window from the spikes into `crates/`.** What exists: `spikes/0005-acp-panel` (2.2 k lines: the
panel, session, transcript, bench), `crates/acp` (client and a fake agent), `crates/mcp` (server over the command bus),
`agents/claude-acp` (the native adapter, brief 0006), and a command bus whose every feature, including this brief's
rename, code actions and `workspace.apply_edit`, an agent can already call. Missing: a production `agents` tool window
in `crates/docking` (VS-style, docked right), transcript rendering with Markdown (reusing `crates/ui::markdown`),
permission prompts mapped to the command permission classes (edit_buffer edits shown as a pending diff, which needs a
diff view), session lifecycle and restart, MCP tool exposure of the workspace commands, persistence of sessions, and
headless tests against the fake agent. Estimate: one agent-week, about this brief's size; the pending-diff view for
edit_buffer permissions is the uncertain part (half a week alone if built properly; it could start as a summary of
the applier's paths).

**0017: build and run with netcoredbg.** Missing: `eludite.build.*` commands running `dotnet build` (and MSBuild for
legacy) in the host with output streaming to the Output window and MSBuild diagnostics into the Error List
(< 100 ms Ctrl+Shift+B to first line); `crates/dap` (89 lines today) grown into a DAP client with the stdio
transport; netcoredbg located (not bundled unless its license review is done: MIT, so bundling is allowed), launch
profiles from `launchSettings.json`, F5/Ctrl+F5/Shift+F5, breakpoints in the editor margin (the light bulb margin is
the first margin; breakpoints need their own glyph column), the call stack, locals and the debug toolbar. Estimate: two
agent-weeks; split into 0017a build with Output and Error List (one week) and 0017b run and debug with netcoredbg (one
week), each with a schema-first protocol step for the build messages and the DAP subset.

## 10. Not done, caveats

- **Windows and macOS:** not run. **CI:** not run.
- **Files beyond the brief's list:** `crates/lsp` tests and fake (in scope as `crates/lsp/**`), `dotnet/tests` (host
  tests, in scope), `HOST.md` (the host's own contract summary). `crates/editor` changed beyond the margin and the
  transaction API in one place: `AcceptedCompletion` gained `list` and `index`, which the accept path needs to find the
  server's item.
- **Not updated:** `docs/briefs/README.md` (index row) and `CLAUDE.md`, which are outside this brief's files. The crate
  map still matches.
- **Inline rename, fix-all, suppression previews and snippet tab stops** are out of scope and not built.
