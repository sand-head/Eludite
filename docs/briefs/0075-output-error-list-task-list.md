# Brief 0075: the Output window, the Error List and the Task List at Visual Studio's depth

Status: in progress
Phase: 2 (PLAN.md section 10: the alpha's ".NET on par with Visual Studio and Rider" wave, accepted by the owner's direct request)
Plan reference: PLAN.md sections 2 (principles 1, 3, 4, 5, 12), 4.4 (build, the Output window, the Error List), 5.1, 5.4, 8
Related ADRs: ADR-0002 (MSBuild out of process), ADR-0014 (tests fail on behavior only)
Depends on: brief 0012 (the Error List), brief 0014 (its toolbar and `eludite.error_list.filter`), brief 0017 (builds, the Output window, its 100k-line frame budget), brief 0042 (`crates/search`'s engine, F8 in Find Results), brief 0020 (settings), brief 0028 (the Startup Projects dialog)

## Goal

The three windows a .NET developer reads all day work the way Visual Studio's do. The Output window is a real text
view: select and copy, find, word wrap, colors, `\r` progress, and double-click (or F8) on a compiler location opens
it. The Error List filters by source (Build, IntelliSense) and scope (open documents, current project, current
document), sorts by any column, copies rows, steps with Ctrl+Shift+F12, links each code to its help page and
suppresses a diagnostic through the language server's own code action. A Task List window lists the workspace's
`TODO`, `HACK`, `UNDONE` and `FIXME` comments. Around them: Ctrl+Break cancels the build, the build slot has a Cancel
button, Build > Open Binary Log reveals the last `.binlog`, `build.verbosity` reaches MSBuild's `-v:`, and the
workspace root's context menu has Build, Rebuild, Clean and Configure Startup Projects....

## Files in scope

- `crates/eludite/src/shell/output.rs` and `output_tests.rs` (new): the Output window.
- `crates/eludite/src/shell/error_list.rs` and `error_list_tests.rs` (new): the Error List.
- `crates/eludite/src/shell/task_list.rs` and `task_list_tests.rs` (new): the Task List window and its scanner.
- `crates/eludite/src/shell.rs`: `mod` lines, the windows' creation and dispatch, command arms, the Error List row
  collection's `help` member and the sort (one block marked "brief 0075"); nothing else restructured.
- `crates/eludite/src/shell/build.rs`: cancel, binlog and verbosity plumbing only (marked blocks).
- `crates/eludite/src/shell/explorer.rs`: the root node's context items (one marked block).
- `crates/eludite/src/app.rs`, `crates/eludite/src/tests.rs`: binding the Output window's key context (one line each).
- `crates/commands/src/build.rs`, `workspace.rs`, `view.rs`, `settings.rs` (the key list test): additive.
- `protocol/schemas/`: `error-list-filter.{input,output}.json` (source, scope, sort), `error-list-next.*`,
  `error-list-suppress.*`, `output-find.*`, `output-go-to-location.*`, `output-word-wrap.*`, `task-list-list.*`,
  `build-open-binary-log.*`, `build-result.output.json` (`binlog`), `settings.json` (`build.verbosity`,
  `taskList.tokens`), `host/build-start.json` (`verbosity`), `host/build-finished.json` (`helpLink`), `host-rpc.md`.
- `protocol/rust/src/host.rs` (the hand-written mirror): `verbosity`, `help_link`; its schema tests.
- `dotnet/src/Eludite.Host/Build/BuildPlan.cs`, `BuildService.cs`, `BuildMessages.cs`, `BinlogReader.cs` and their
  tests in `dotnet/tests/Eludite.Host.Tests/`.
- `crates/docking/src/model.rs`: the `task_list` window id (additive).
- `crates/ui/src/menu.rs`, `crates/ui/src/keymap.rs`: entries (additive, marked).

## Contract

**Output window.**
- Storage per source: lines of text plus styled runs (foreground, background, bold) from ANSI SGR (`30`-`37`,
  `90`-`97`, `40`-`47`, `100`-`107`, `38;5;n`, `48;5;n`, `38;2;r;g;b`, `48;2;r;g;b`, `1`, `22`, `39`, `49`, `0`);
  other escape sequences are dropped. A `\r` not followed by `\n` returns to the start of the line, so the next text
  overwrites it (progress bars). The trailing partial line is a row at once and grows in place. Each source keeps at
  most 200,000 lines; past that the oldest are dropped (in blocks, so dropping is amortized) and selection, find and
  scroll positions shift with them. `eludite.output.show`'s `line_count` and `tail` see the same lines.
- Selection: press and drag selects characters; Shift+click extends; double-click on a line holding a location opens
  it, elsewhere selects the word; Ctrl+A selects all; Ctrl+C copies the selection (or the current line when nothing is
  selected, as Visual Studio does); the context menu has Copy, Select All, Clear All, Find, Word Wrap and Go To
  Location. Up/Down/PageUp/PageDown/Home/End move the caret line; Shift extends.
- Find: Ctrl+F while the Output window has focus opens its find bar (the editor's Ctrl+F is unchanged elsewhere);
  Enter / F3 next, Shift+Enter / Shift+F3 previous, a Match case toggle, every match highlighted, the current one
  selected and scrolled into view, Escape closes. The bar runs `eludite.output.find`.
- Word wrap: the toolbar's Toggle Word Wrap button runs `eludite.output.word_wrap`; off (the default) the body scrolls
  horizontally to the longest line; on, long lines wrap (rows have variable height).
- Locations: `parse_location(line)` recognizes MSBuild and compiler canonical form `path(line,col): error CODE: ...`
  and `path(line): ...` and `path(line,col,endLine,endCol): ...` (with an optional `N>` node prefix), `path:line:col`
  and `path:line` (rustc's `--> path:line:col`, tsc, gcc), and .NET stack frames `at ... in path:line N`. A relative
  path resolves against the workspace root. Double-click or Enter on such a line, or `eludite.output.go_to_location`
  with `line`, opens it through `eludite.file.open`; F8 / Shift+F8 while the Output window has focus run it with
  `navigate` next / previous (Find Results keep F8 elsewhere), selecting the line.
- Frame budget: 100k lines, scrolled and with a selection, keeps brief 0017's keystroke-to-frame p99 under 8 ms.

**Error List.**
- `eludite.error_list.filter` gains `source` (`all`, `build`, `intellisense`: Visual Studio's "Build + IntelliSense",
  "Build Only", "IntelliSense Only"; a `Both` row passes all three; Test and NuGet rows count as build), `scope`
  (`solution`, `open_documents`, `current_project`, `current_document`), and `sort` (`column` one of `severity`,
  `code`, `description`, `project`, `file`, `line`, `column`, `source`; `descending`). The output echoes them. The
  toolbar gets the scope dropdown (left, as Visual Studio's) and the source dropdown (right of the toggles).
- Sorting: a click on a column header sorts by it, a second click reverses; the header shows ▲ or ▼. The default
  order is today's (severity, then path, line, column) and stays the order rows are collected in.
- Selection: click selects, Ctrl+click toggles, Shift+click extends; Up/Down move, Shift extends, Enter opens; Ctrl+C
  copies the selected rows as Visual Studio does: a header line `Severity\tCode\tDescription\tProject\tFile\tLine\tSuppression State`
  then one tab-separated line per row (File is the full path, Suppression State is `Active`).
- Next/previous: `eludite.error_list.next` (`direction` `next` or `previous`) moves the selection through the shown
  rows (wrapping) and opens the row; View > Next Error is Ctrl+Shift+F12, View > Previous Error has no default key.
- Help: rows carry `help` (LSP `codeDescription.href`; for build rows the host's `helpLink`, from MSBuild's
  `HelpLink`). The Code cell of a row with help is drawn as a link; clicking it, or the context menu's Show Error
  Help, runs `eludite.browser.open_external` with the URL. `diagnostics.list` is unchanged.
- Context menu on rows: Copy, Go To Location, Show Error Help, Suppress > In Source, Suppress > In Suppression File.
  `eludite.error_list.suppress` (`rows`: shown-row indices, `kind` `in_source` or `in_suppression_file`) asks the
  language server for code actions at each row's position (`eludite.editor.code_actions`' path), picks the action
  whose title says "in Source" (or "in Suppression File"), resolves and applies it through the workspace-edit
  applier, and answers with what was applied and what had no such action. Set Severity is not built.

**Task List.**
- Window id `task_list`, title "Task List", bottom dock (hidden until shown); View > Task List, Ctrl+\, Ctrl+T.
- `taskList.tokens` (Tools > Options > Environment > Task List): an array of `{name, priority}` with priority
  `high`, `normal` or `low`; default TODO, HACK, UNDONE, FIXME, all `normal`.
- Scan: the workspace folder with `crates/search`'s walker (`.gitignore`, `search.excludes`), off the UI thread,
  cancelable, files over 2 MB and binaries skipped, at most 10,000 items. A token counts when it starts a comment's
  text after `//`, `///`, `'`, `#`, `/*`, `*` (a block comment's continuation), `<!--`, `@*`, `--`, `;` and is followed
  by a non-word character, case-sensitive like Visual Studio's. The description is the comment's text from the token
  to the end of the line (without a closing `*/`, `-->`, `*@`).
- Columns: Priority (! for high, ↓ for low), Description, Project, File, Line; double-click opens; sorted by
  priority then file and line; a token filter dropdown "All tokens". Saving a file rescans just that file; a full
  rescan runs when the window is first shown, when the workspace opens and on `refresh`.
- `eludite.task_list.list` (`refresh`, `token`, `limit`): the items `{token, priority, description, path, project,
  line, column}`, `total`, `truncated`, `scanning`. Agent visible.

**Build chrome.**
- Ctrl+Break (`ctrl-pause`) runs `eludite.build.cancel` (Build > Cancel shows it); while a build runs the status bar's
  build slot shows a Cancel link that runs the same command.
- `build.verbosity` (`quiet`, `minimal`, `normal`, `detailed`, `diagnostic`; default `minimal`; Projects and
  Solutions > Build and Run) is passed as `eludite/build/start`'s `verbosity` and becomes `-v:q|m|n|d|diag`.
- `build-result.output.json` gains `binlog` (absolute path of the binary log, MSBuild builds only).
  `eludite.build.open_binary_log` (Build > Open Binary Log) opens the last build's log in MSBuild Structured Log
  Viewer when `StructuredLogViewer` or `msbuildlog` is on PATH, else reveals it in the file manager; it answers
  `{path, opened_with}` or fails when no build wrote one.
- The root node (Solution) context menu: Build Solution, Rebuild Solution, Clean Solution, a separator, Configure
  Startup Projects..., then Manage NuGet Packages for Solution...; the Debug menu gains Configure Startup Projects...
  (both run `eludite.workspace.set_startup_project` without arguments, which opens the dialog).

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite output`: ANSI runs (16, 256, truecolor, bold, reset), `\r` overwrite, the partial row, the
  cap drops the oldest lines; selection by drag and Shift+click, Ctrl+A, Ctrl+C on the clipboard; Ctrl+F in Output
  opens the bar (the editor's find is not run), next/previous wrap, match case; word wrap on and off; each location
  format parsed, with a relative path and a node prefix; double-click and Enter open the file, F8 / Shift+F8 step;
  100k lines under the frame budget (ADR-0014's `assert_budget`).
- `cargo test -p eludite error_list`: source and scope filters, each column's sort and its reversal, copy text,
  Ctrl+Shift+F12 order and wrap, the help link opened, suppress through a fake host's code action.
- `cargo test -p eludite task_list`: a temp workspace with each token in C#, VB, F#, Rust, XML and Razor comments,
  a token outside a comment ignored, a custom token from the setting, live update on save, `eludite.task_list.list`.
- `cargo test -p eludite build`: Ctrl+Break cancels a fake-host build; the result carries `binlog`; the fake host
  sees `verbosity` from the setting; the root node's items run their commands.
- `cargo test -p eludite-commands`, `cargo test -p eludite-protocol`: parsing, specs and schema conformance.
- `dotnet test dotnet/tests/Eludite.Host.Tests`: `BuildPlan`'s `-v:` per verbosity, an invalid verbosity refused,
  `BinlogReader` reads `HelpLink`.

## Budget

- Output: keystroke to frame p99 < 8 ms with 100k lines; appending 100k lines < 2 s; memory bounded by the cap.
- Error List: refilter and sort of 10k rows < 16 ms.
- Task List: the scan never runs on the UI thread; a save's rescan is one file.

## Exit criterion

Every proving test passes on Linux; `cargo fmt --check` and clippy with `-D warnings` are clean on the touched crates;
the host builds with warnings as errors and its tests pass; the report lists what was not run.

## Out of scope

Set Severity (writing `.editorconfig`), the Error List's column chooser and grouping, custom Task List shortcuts
(user tasks), Output panes created by extensions, a find in Output across sources, binary log viewing inside
Eludite.
