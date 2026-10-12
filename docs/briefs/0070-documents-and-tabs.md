# Brief 0070: The document lifecycle and the tab well

Status: in progress
Phase: 2 (PLAN.md section 10: on par with Visual Studio and Rider for .NET work; section 8, the document well)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 12), 4.1 (the editor: encodings, line endings), 5.1 (every action a command), 8 (docking, document tabs, the status bar, Visual Studio's names and shortcuts), 9 (budgets)
Related ADRs: ADR-0003 (protocols), ADR-0014 (deterministic tests)
Depends on: brief 0008 (docking: document tabs with pinned and preview states), brief 0009 (editor core: `Buffer`, line endings, BOM), brief 0012 (open documents, `eludite.file.open`/`close`, `eludite.editor.save`), brief 0014 (the navigation history), brief 0047 (the per-workspace user state folder), brief 0049 and proposal 0005 (the property pages and the `.resx` grid, which track their own unsaved changes).

## Goal

Nothing a person types is lost and nothing stale is built or written: closing the window, quitting, closing a dirty tab and Close Workspace ask Visual Studio's "Save changes to the following items?" (Save, Don't Save, Cancel) over every dirty document; Save All (Ctrl+Shift+S) exists and builds, F5, Ctrl+F5 and test runs save first (`build.saveBeforeBuild`, on); a file changed on disk reloads a clean buffer in place (caret, selections and scroll kept) and asks for a dirty one (Yes, Yes to All, No, No to All), and a save never overwrites a newer disk version without asking. Open File (Ctrl+O), Save As, New File (Ctrl+N), Close (Ctrl+F4) and Close All exist. The tab well behaves as Visual Studio's: overflow dropdown and scrolling, middle-click close, the tab context menu, pinned tabs at the left, the preview tab from a single click in the Workspace window, the IDE Navigator on Ctrl+Tab in most-recently-used order, Ctrl+Alt+PgUp/PgDn, drag to reorder. The open documents, the active one and each caret and scroll position come back after a restart without delaying the first frame. Files in UTF-16, UTF-32 and legacy code pages (Windows-1252 by default) open and save byte for byte; the status bar shows Ln, Col, Ch, the selection length, the encoding and the line endings (the last two clickable). Navigate Backward/Forward covers Error List and Find Results jumps, tab switches, Go To Line and large caret jumps and the mouse's back and forward buttons; Go To Last Edit Location exists. Bookmarks (Ctrl+K, Ctrl+K and the rest) with the Bookmarks window, kept through edits and restarts.

Every user-visible action is a command with a schema in `protocol/schemas/` first, reachable by agents over MCP. Agents never see a modal: commands that would ask take an explicit `save` (`save` or `discard`) argument and fail with the list of dirty documents without it.

## Files in scope

- `protocol/schemas/file-*.json`, `window-*.json`, `bookmarks-*.json` (new): the schemas below, written first.
- `crates/commands/src/documents.rs` (new) and its `pub mod` line in `crates/commands/src/lib.rs`: the commands' ids, parsing, typed outputs, specs and `register` over a `DocumentsTarget`.
- `crates/commands/src/settings.rs`: the settings keys below (additive, one block).
- `crates/eludite/src/shell/documents.rs` (owned): encoding-aware load and save, the disk stamp, untitled documents, the API other briefs call (`rename_open_document`, `close_document`, `open_document_paths`).
- `crates/eludite/src/shell/file_ops.rs` (new): the bus side (`DocumentsBus`, jobs from other threads), Save All, the dirty list, the unsaved-changes question, Exit and window close, Close All and its variants, Open File, Save As, New File, Advanced Save Options (set encoding), save before build, Reopen Closed Tab, Copy Full Path, Open Containing Folder, the status bar's document items.
- `crates/eludite/src/shell/reload.rs` (new): the disk watcher (stat polling off the UI thread, no `notify`), reload of clean buffers, the question for dirty ones, deleted files, autosave.
- `crates/eludite/src/shell/session_state.rs` (new): the open documents per workspace in the brief 0047 state folder, restored lazily.
- `crates/eludite/src/shell/navigator.rs` (new): the IDE Navigator popup (Ctrl+Tab) and next/previous document.
- `crates/eludite/src/shell/bookmarks.rs` (new): the bookmark model, commands, the Bookmarks window, persistence.
- `crates/eludite/src/shell/documents_tests.rs` (new, headless tests of every flow) and its `mod` line.
- `crates/eludite/src/shell.rs`: `mod` lines, one `Services` field, one `Shell` field, the install call, the `run`/`invoke` dispatch, the tool body for the Bookmarks window: each a marked block (brief 0070).
- `crates/eludite/src/app.rs`: the window-close and quit flow.
- `crates/eludite/src/shell/build.rs`, `debug.rs`, `test_runs.rs`: one marked `save_before_build` call each at their start entry points.
- `crates/eludite/src/shell/error_list.rs`, `search.rs` (Find Results click-through): one marked navigation-history push each, if not already pushed.
- `crates/editor/src/buffer.rs` (encodings), `crates/editor/Cargo.toml` (`encoding_rs`), `crates/editor/src/bookmarks.rs` (new: the margin glyph) with a marked hook in `crates/editor/src/view.rs`.
- `crates/docking/**`: the tab well (overflow, scrolling, middle click, context menu, drag to reorder), the MRU order, the Bookmarks tool window descriptor.
- `crates/ui/src/menu.rs`, `crates/ui/src/keymap.rs`: additive entries (one marked block each where possible).
- `docs/briefs/0070-report.md`, this file.

Not in scope here: `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, `docs/PLAN.md` (their lines go to the report's Integration notes); other briefs' files (0065 to 0079).

## Contract

**Commands** (schemas `protocol/schemas/<name>.input.json` and `.output.json`, `title` `"<id> input"`/`"<id> output"`, `additionalProperties: false`):

| Id | Input | Output | Class | Key, menu |
|---|---|---|---|---|
| `eludite.file.save_all` | `{}` | `{saved: [path], failed: [{path, message}]}` | edit_buffer | Ctrl+Shift+S, File > Save All |
| `eludite.file.save_as` | `{path?, new_path}` | `{path, new_path, bytes}` | edit_buffer | File > Save As... (a dialog without `new_path`; Ctrl+S on an untitled document) |
| `eludite.file.new` | `{language?}` (a language id or a file extension) | `{path, language}` | read | Ctrl+N, File > New File... |
| `eludite.file.open_file_dialog` | `{}` | `{shown}` | read | Ctrl+O, File > Open File... (agents use `eludite.file.open`) |
| `eludite.file.documents` | `{dirty_only?}` | `{documents: [{path, title, kind, dirty, pinned, preview, deleted, encoding, bom, line_ending, active}]}` | read | (agents' list of dirty documents) |
| `eludite.file.close_all` | `{except?, keep_pinned?, save?}` | `{closed: [path], saved: [path]}` | edit_buffer | Window > Close All Documents, the tab menu's Close All, Close All But This, Close All But Pinned |
| `eludite.file.reload` | `{path?, discard?}` | `{path, reloaded, dirty}` | edit_buffer | (the external-change question's Yes) |
| `eludite.file.set_encoding` | `{path?, encoding?, bom?, line_ending?}` | `{path, encoding, bom, line_ending, dirty}` | edit_buffer | File > Advanced Save Options..., the status bar's encoding and line-ending items |
| `eludite.file.exit` | `{save?}` | `{exiting, saved: [path]}` | dangerous | File > Exit, Alt+F4 |
| `eludite.window.pin_tab` | `{path?, pinned?}` | `{path, pinned}` | read | the tab menu's Pin Tab / Unpin Tab |
| `eludite.window.next_document` | `{backward?}` | `{path}` | read | Ctrl+Alt+PgDn / Ctrl+Alt+PgUp |
| `eludite.window.navigator` | `{backward?}` | `{items: [{id, title, kind}], selected}` | read | Ctrl+Tab / Ctrl+Shift+Tab |
| `eludite.window.reopen_closed` | `{}` | `{path?}` | read | Ctrl+Shift+T, the tab menu |
| `eludite.window.copy_full_path` | `{path?}` | `{path}` | read | the tab menu's Copy Full Path |
| `eludite.window.open_containing_folder` | `{path?}` | `{folder}` | read | the tab menu's Open Containing Folder |
| `eludite.bookmarks.toggle` | `{path?, line?}` | `{path, line, added}` | edit_buffer (bookmarks are workspace state) | Ctrl+K, Ctrl+K |
| `eludite.bookmarks.next` / `previous` | `{}` | `{path?, line?}` | read | Ctrl+K, Ctrl+N / Ctrl+K, Ctrl+P |
| `eludite.bookmarks.clear` | `{path?}` | `{removed}` | edit_buffer | Ctrl+K, Ctrl+L |
| `eludite.bookmarks.list` | `{}` | `{bookmarks: [{id, name, path, line, enabled}]}` | read | |
| `eludite.bookmarks.update` | `{id, name?, enabled?}` | the row | edit_buffer | the Bookmarks window's rename and check box |
| `eludite.bookmarks.go_to` | `{id}` | `{path, line}` | read | double-click in the Bookmarks window |
| `eludite.editor.go_to_last_edit` | `{}` | `{path?, line?, column?}` | read | Ctrl+Shift+Backspace |

View > Bookmark Window (Ctrl+K, Ctrl+W) is `eludite.view.show` with id `bookmarks` (a tool window registered in `ToolWindowRegistry::vs_default`). Close (Ctrl+F4) is `eludite.file.close` on the active document.

**Settings** (additive): `build.saveBeforeBuild` (bool, true), `files.autoReload` (bool, true: "Reload modified files unless there are unsaved changes"), `files.autoSave` (`off` | `afterDelay` | `onFocusChange`, `off`), `files.autoSaveDelay` (ms, 1000), `files.encoding.fallback` (an encoding label, `windows-1252`), `files.restoreDocuments` (bool, true), `editor.previewTab` (bool, true).

**Unsaved changes.** The dirty items are every document whose buffer differs from its last save, every `.resx` grid and project property page that reports unsaved changes. Closing the main window, `eludite.file.exit`, Close Workspace (`eludite.workspace.close`) and Close All without `save` ask "Save changes to the following items?" with the items in the prompt's detail and Save, Don't Save, Cancel; Save writes them all (an untitled document asks for its path; a failure cancels the quit and says why), Don't Save drops them, Cancel keeps everything. From an agent (another thread) the same commands without `save` fail with the list. A dirty tab's close keeps brief 0012's question.

**Save before build.** `build.saveBeforeBuild` on: `eludite.build.solution`/`project`/`rebuild`, `eludite.debug.start` (both modes) and `eludite.test.run`/`debug` save every dirty document (not untitled ones) before they start, on the UI thread, before the build reads files. Each entry point gets one marked call, `self.save_before_build(cx)`.

**Encodings** (`eludite_editor::Buffer`). Detection order: BOM (UTF-8, UTF-16 LE/BE, UTF-32 LE/BE), else valid UTF-8, else the fallback code page (`encoding_rs`, `windows-1252` unless set). `Buffer::encoding()` names it (`utf-8`, `utf-16le`, `utf-16be`, `utf-32le`, `utf-32be`, or the `encoding_rs` label); `to_file_bytes` re-encodes with the same BOM and line endings: a file opened and saved unchanged is byte-identical. `set_encoding` changes it (Save With Encoding). A character the code page cannot encode makes the save fail with a message naming the encoding, never a silent `?`.

**Reload.** A watcher thread stats every open document's file (mtime and length) every 1 s and on window activation; changes are read off the UI thread. A clean buffer whose file changed reloads in place as one undoable edit keeping caret, selections and scroll (`files.autoReload` on; off asks too). A dirty buffer asks "This file has been modified outside of the source editor. Do you want to reload it?" with Yes, Yes to All, No, No to All. A deleted file marks its tab `[deleted]` and leaves the buffer dirty. `eludite.editor.save` and Save All refuse (the UI asks "overwrite?") when the disk stamp differs from the one last read or written and the person has not answered No.

**Tab well** (`crates/docking`). Tabs scroll horizontally; a dropdown at the right lists every open document (active marked) and activates one; middle click closes (through `eludite.file.close`); right click opens the context menu: Close, Close All, Close All But This, Close All But Pinned, Pin Tab / Unpin Tab, Copy Full Path, Open Containing Folder, Reopen Closed Tab. Pinned tabs stay left with a pin glyph; the preview tab is italic and last (brief 0008's model); a single click on a file in the Workspace window opens it as the preview (`editor.previewTab`), double click or an edit promotes it. Dragging a tab onto another reorders (pinned and unpinned stay in their bands). The controller keeps the MRU order of documents and tool windows (not persisted).

**Session state.** `<config>/eludite/workspaces/<name>-<hash>/documents.json` (brief 0047's folder): `{version: 1, documents: [{path, line, column, scroll_row, pinned}], active}` written on change (debounced, off the UI thread) and on exit; after a workspace opens, the active document opens first, the others become tabs whose files load when first shown. Missing files are skipped.

**Navigation.** The history (brief 0014's `NavigationHistory`) gains entries from Error List and Find Results jumps, tab switches, Go To Line and caret jumps of more than 10 lines; the mouse's back and forward buttons run `eludite.navigation.back`/`forward`. Go To Last Edit Location returns to the last edit's anchor.

**Bookmarks.** Anchored in the buffer (they follow edits), kept per workspace in the state folder (`bookmarks.json`, `{path, line, name, enabled}`), drawn as a glyph in the indicator margin, listed in the Bookmarks window (name, file, line, an enable box; double-click goes to it). Next and Previous wrap across documents in list order, skipping disabled ones.

**API for other briefs.** `Shell::rename_open_document(old, new)` (retargets an open document and its tab after a rename), `Shell::close_document(path, discard)`, `Shell::open_document_paths()`.

## Proving test

`cargo test -p eludite documents_tests` (headless, against the fake host):
- quit with a dirty document prompts; Cancel keeps the window and the edit; Save writes the file and quits; Don't Save quits without writing; an agent's `eludite.file.exit` without `save` fails listing the file.
- Save All writes every dirty buffer and clears every dirty marker; `eludite.file.documents {dirty_only}` lists them first.
- a build with `build.saveBeforeBuild` saves the dirty document before the host sees the build request; with it off, it does not.
- an external edit reloads a clean buffer keeping the caret's line; a dirty buffer gets the question; No keeps the edit and the next save writes it; a deleted file marks the tab.
- the tab context menu's Close All But This, Close All But Pinned, Pin Tab, Copy Full Path; middle-click close; Ctrl+Tab's MRU order; Ctrl+Shift+T reopens the last closed tab.
- documents restored with caret after a simulated restart (a second shell on the same state folder).
- bookmarks: toggle, next/previous across files, survive an edit above them and a restart.
`cargo test -p eludite-editor buffer`: UTF-16 LE with BOM, UTF-16 BE, UTF-32, Windows-1252 and UTF-8 without BOM fixtures round-trip byte for byte; an unencodable character fails the save.
`cargo test -p eludite-docking`: the MRU order, tab reorder, close variants.
`cargo test -p eludite-commands documents`: schemas parse, name their command, input validation.

## Budget

- Startup to interactive window unchanged (< 300 ms); restoring 20 documents reads none of them before the first frame except the active one, and that off the UI thread.
- Reloading a 10 MB file runs its read and decode off the UI thread; the UI thread applies one edit.
- The watcher's stat pass over 100 open files stays off the UI thread.
- New dependency: `encoding_rs` 0.8 (MIT OR Apache-2.0 AND BSD-3-Clause), already in `Cargo.lock`.

## Exit criterion

`cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test -p` for each touched crate green on Linux; the report lists what was built, measured and left.

## Out of scope

Split document groups (stretch; a layout schema bump), a Find in Files over non-UTF-8 files' encodings (brief 0042's engine decodes on its own), the Windows file dialogs' encoding drop-down, Rider's local history, remote files.
