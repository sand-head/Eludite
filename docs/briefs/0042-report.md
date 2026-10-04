# Brief 0042 report: Find in Files and Replace in Files

Status: done on Linux. Windows and macOS: not built here (no targets or machines); nothing in the search is
platform-specific beyond paths (the walker and matcher are ripgrep's, which run on all three). CI: not run (nothing
pushed).
Branch: `brief/0042-find-in-files`, based on `main` at `ed8f1a1`; not rebased (the coordinator merges).
Date: 2026-10-04. Brief: [0042-find-in-files.md](0042-find-in-files.md).

## 1. Summary

- **Edit > Find and Replace > Find in Files (Ctrl+Shift+F)** opens Visual Studio 2022's modeless dialog with the
  Find in Files and Replace in Files tabs: Find what (with the last 20 queries of the workspace), Look in (Entire
  Solution, Current Project, Current Document, All Open Documents, each project of the solution, a folder), File types
  (`*.cs;*.cshtml;!*.g.cs`), Match case, Match whole word, Use regular expressions (with a link to the notes on Rust's
  syntax, section 5), Find Results 1 or 2, Append results, and Find All. The editor's selection (on one line), else the
  word at the caret, is the query, selected so typing replaces it. Visual Studio's access keys work (Alt+C, Alt+W,
  Alt+E; in Replace in Files Alt+R Replace Next, Alt+S Skip File, Alt+A Replace All).
- **Find Results 1 and Find Results 2** (View > Other Windows > Find Results 1, 2; closed until a search shows one,
  then tabbed at the bottom; layout schema 5 migrates older layouts) show each search as a block: Visual Studio's count
  line (`Find all "fn main", Subfolders, Find Results 1, Entire Solution, "*.*" — Matching lines: 40 Matching files:
  30 Total files searched: 1191`), the matching files in path order with their counts, each matching line with its
  `(line, column)` and the matches highlighted, the files Replace All skipped and why, and the elapsed time (or that the
  search was stopped, or capped). Append adds a block under a separator. Previous (Shift+F8), Next (F8), Clear All and
  Stop Search are on the toolbar; F8 and Shift+F8 anywhere step through the window of the last search and open the
  editor at the match with the match selected; a double-click on a result does the same. The list is virtualized.
- **Replace in Files (Ctrl+Shift+H)** adds Replace with (`$1`, `${name}`, `$0` for regular expressions) and Keep
  modified files open after Replace All, and Replace Next (replaces the match selected in the editor through
  `eludite.workspace.apply_edit`, then steps to the next result), Skip File, Replace All. **Replace All holds every
  file's replacements as a pending change in brief 0016's review view** (Accept, Reject, Accept All from the Agents
  window or `eludite.agents.review`). Accepting checks the file against the text that was searched again (a file
  changed on disk since is refused with a row in the results and never overwritten), applies through the workspace-edit
  applier as **one undo step per open document**, and with Keep modified files open opens an unopened file first so its
  replacements land in an editor (undoable, unsaved); with it off, unopened files are written directly.
- **ripgrep's engines** in the new crate `eludite-search` (section 3): `ignore`'s parallel walker (one thread per
  core) with `.gitignore`, `search.excludes` and the File types; `grep-regex` and `grep-searcher` line by line,
  multiline off, binary files skipped by the NUL rule, UTF-8 and UTF-16 by their byte order mark; the open documents'
  buffers (unsaved text) searched instead of their files, once. Each file streams to the window as soon as it is
  searched, applied once per frame; Stop ends the search within one file; a newer search in a window cancels the older
  one, whose late files are dropped.
- **Agents** call the same four commands (`eludite.search.find`, `replace`, `cancel`, `results`; schemas
  `protocol/schemas/search-*.json`). `find` answers the matches grouped by file, capped (`max_results`, default 1,000,
  at most 10,000; more is capped, not refused) and paged through `results` by `search_id`; with `results_window` it
  also shows in that window, else it is answered only. `replace` with `preview` (the default) answers the pending
  changes it made and writes nothing until `eludite.agents.review` accepts them; `preview: false` applies at once. The
  MCP server's instructions point agents at `eludite.search.find` for searching files.
- **Budgets** (Ubuntu container, 4 cores, debug build with optimized dependencies; the other worktree's agent built
  beside this one, so the load average is given with each figure):

  | Budget | Result |
  |---|---|
  | A literal search over 50,000 files (500 MB) within 1.5 times `rg` (ripgrep 14.1.0 on PATH) | **1.15x** in the test profile (eludite-search median 155.9 ms, `rg` 135.2 ms, 5 runs each after a warm-up, load 0.4; a later run 1.14x, 212.3 ms vs 187.0 ms at load 2.7) and **0.90x** in the release profile (134.4 ms vs 148.9 ms, load 5.4). Pass |
  | The first result within 50 ms of Enter on this repository | the engine's first file **1.8 ms** median (5 runs; the whole search of the repository 15.5 ms, 1,191 files); in the shell, Enter to the first file drawn **16.5 ms** with an artificial 5 ms delay per file (`the_first_result_streams_before_the_search_ends`). Pass |
  | Stop cancels within 50 ms | **23.7 ms** from the click on Stop to the window saying so, with a 20 ms sleep per file on 4 walker threads, at load 2.5 (49.5 ms at load 7.3, beside the other worktree's build; the test asserts 50 ms when the load is under the core count). Pass |
  | Results window with 10,000 matches, frame p99 under 8 ms | **2.23 to 2.40 ms p99** (median 1.96 to 2.01 ms) over 40 frames of the whole window with Find Results 1 shown and scrolled, load 5.8 (`ten_thousand_matches_draw_in_a_frame`; under the full suite's load 26.8 ms, which `assert_budget` does not assert). Pass |
  | Three new dependencies | `ignore` 0.4.33, `grep-regex` 0.1.14, `grep-searcher` 0.1.17 (Unlicense OR MIT); `grep-matcher` used directly too, which they bring (section 8). Pass, with that note |

- **Tests**: section 6; counts in section 11.

## 2. What was built

Commits on top of `ed8f1a1`, in order:

1. `8519afd` Mark brief 0042 in progress.
2. `d2027fd` The schemas, first and alone: `search-{find,replace,cancel,results}.{input,output}.json`, the four
   `search.*` settings (the Options page "Environment > Find and Replace", listed last), `view-show`'s
   `find_results_1` and `find_results_2`.
3. `68e225c` `crates/search` (section 3) with its fixture tests and the benchmark against `rg`.
4. `edb8a78` `crates/commands/src/search.rs` (parsing, the cap, typed outputs, the classes and `replace`'s escalation
   hook), the settings' string lists, `crates/mcp`: the instructions and the search tools' test.
5. `2c0b53b` `search-results.input.json` gains `select`, `next_file` and `clear` (the double-click, Skip File and Clear
   All needed commands; schema first again).
6. `65a062a` `crates/docking` (`ids::FIND_RESULTS_1`, `FIND_RESULTS_2`, schema 5 and its migration) and `crates/ui`
   (Edit > Find and Replace > Quick Find, Find in Files, Replace in Files; View > Other Windows > Find Results 1, 2;
   Ctrl+Shift+F, Ctrl+Shift+H, F8, Shift+F8), and the commands' `select`, `next_file` and `clear`.
7. `1cac250` The shell: `shell/search.rs` (service, jobs, events, Replace All, the accept hook, navigation),
   `shell/search/dialog.rs`, `shell/search/results.rs`, `shell/search_tests.rs`, and the hooks (section 7).
8. `ca5b23d` `crates/eludite/tools/find-linux.sh` and its screenshots, the first-result benchmark on this repository.
9. This report, the brief's status, the index row, `CLAUDE.md` and `README.md`.

## 3. `crates/search` (`eludite-search`)

- `Query` (text, `regex`, `case_sensitive`, `whole_word`) compiles into a `grep_regex::RegexMatcher` (case folding,
  `\n` never matched, CRLF mode so `$` matches before `\r\n`) and the same pattern in `regex::bytes` for the matches'
  ranges on a matching line and their groups. Literal text is escaped. **Whole word is Visual Studio's**: a word
  boundary where the query's own first or last character is an identifier character (letters and digits, Unicode
  included, and `_`), so `.Foo` still matches `x.Foo`; a regular expression gets `\b(?:…)\b`. A pattern that would
  match a line ending is refused (multiline is off).
- `search(request, overlay, cancel, sink)`: the scope's folders (a folder inside another is searched once; a file named
  by the scope is searched whatever the filters say) walked by `ignore::WalkBuilder::build_parallel` with
  `available_parallelism` threads, hidden files included, `.gitignore`, `.git/info/exclude`, the global excludes and
  `.ignore` honored when `use_gitignore` (also outside a git repository), the excludes as overrides (`x/**` also prunes
  the folder `x`), the File types' positive globs as a filter that never brings back an ignored file, `max_file_size`
  (an open document is searched whatever its size), symbolic links followed only when asked. Each file is read whole
  into the thread's buffer and searched with `grep-searcher` (line numbers, context lines, `BinaryDetection::quit(0)`:
  a NUL in the first 64 KB skips the file); a UTF-8 BOM is stripped, a UTF-16 BOM decodes the file.
- The sink of a file collects its matching lines (text without the line ending, byte ranges, context before and after)
  and hands the file over whole as `FileMatches` (with a fingerprint of the bytes searched and the encoding) from the
  walker's thread. A shared counter enforces `max_results` (the last line is cut to the cap; `truncated`); the cancel
  token is checked before each file and at each match.
- `Overlay`: the open documents' text by path, searched instead of the file when the walker reaches it (All Open
  Documents searches the overlay alone, with the File types).
- `replace::replacements` turns a file's matches into edits (byte ranges in the searched text, and line with UTF-16
  columns for LSP), expanding `$1`, `${name}` and `$0` for a regular expression; `file_edits` gives
  `(path, Vec<(Range, String)>)`; `apply` previews. `fingerprint` hashes what was searched.

## 4. The command surface

| Command | Class | For the person | For an agent |
|---|---|---|---|
| `eludite.search.find` | read | without `query` (Ctrl+Shift+F) the dialog opens; with one, a search on its own thread, shown in Find Results 1 (or 2) | always allowed; answered (capped), shown only with `results_window` |
| `eludite.search.replace` | edit (held for review); `preview: false` raised to execute | Ctrl+Shift+H opens the dialog; Replace All always previews | `preview` (default) answers pending changes; `preview: false` applies at once (the policy's `execute` decides, a prompt by default) |
| `eludite.search.cancel` | read | Stop Search | stop its own or every search |
| `eludite.search.results` | read | F8, Shift+F8, Skip File, a double-click, Clear All | pages of a window or of one of the last 16 searches by id |

The scope `solution` is the workspace's folder plus every project folder outside it; `project` the named project's
folder (else the active document's project); `document` a file (else the active document); `folder` a folder,
absolute or relative to the workspace. Paths in answers are relative to the workspace's folder with `/` when inside
it. Long lines are cut to 1,000 characters around their first match in answers (400 in the window).

## 5. Regular expression compatibility (Rust's `regex` against .NET)

The dialog's link shows these notes (`dialog::REGEX_NOTES`):

- **The same**: character classes (`\d \w \s [a-z] [^…]`, Unicode categories `\p{L}`), quantifiers (`* + ? {n,m}` and
  their lazy forms), groups (`(…)`, `(?:…)`, named `(?<name>…)` and `(?P<name>…)`), anchors (`^ $ \b \B`), alternation,
  inline flags (`(?i)`, `(?m)` is moot as each line is matched alone).
- **Not supported**: lookahead and lookbehind (`(?=…) (?!…) (?<=…) (?<!…)`), backreferences in the pattern (`\1`,
  `\k<name>`), atomic groups and possessive quantifiers, balancing groups, `RegexOptions.RightToLeft`. Such a pattern
  is refused with the parser's message, before anything is searched.
- **Different**: `\w` and `\b` are Unicode-aware as in .NET; `.` never matches a line ending (there are none: a match
  never spans lines); in a replacement, `$1a` means the group named `1a` (write `${1}a`), `$$` is a dollar sign, and
  .NET's `$+`, `` $` ``, `$'` and `$_` are not available.

## 6. Tests

- **`crates/search`** (5 unit, 10 fixture, 2 benchmark): the query (escaping, case folding, **Visual Studio's whole
  word on identifiers** with Unicode letters and digits and `.Foo`, groups and named groups in replacements, a line
  ending refused); decoding (BOMs, UTF-16 both ways), columns, fingerprints. `tests/fixture.rs` on a tree with
  `bin/`, `obj/`, `node_modules/`, a `.gitignore`d folder and `*.log`, a binary file, a 10 MB file, a CRLF file and a
  UTF-16 file: **the excluded, ignored, binary and large files are skipped** (and found with the settings off or
  raised), the counts; **CRLF lines without their ending and `$` before `\r\n`; UTF-16 decoded**; context lines;
  File types with `!`, an include never bringing back an ignored file, a file named by the scope searched anyway, a bad
  glob and a missing path refused; whole word; symbolic links only when asked; **the overlay beats the disk and counts
  once**, All Open Documents with File types; **groups in replacements** with UTF-16 columns and the preview; **the cap
  and `truncated`**; **cancellation mid-walk stops within one file per thread**. `tests/bench.rs`: the 50,000-file tree
  against `rg` (asserted when `rg` is present and the machine is not loaded) and the first result on this repository.
- **`crates/commands`** (5 new, 2 updated): every schema parses and names its command, the classes; parsing and
  validation (the dialog without a query, ranges, File types split, one of `navigate`, `select`, `clear`); **`max_results`
  capped at 10,000, not refused**; long lines cut around the match; **every output conforms to its schema**; **through
  the registry: `replace` is held for review and `preview: false` is execute and asks**; the settings' four keys, their
  page last and string list entries checked.
- **`crates/mcp`** (1 new): the search tools listed with their classes, a `find` answered in its schema with the cap
  applied, the instructions naming them.
- **`crates/docking`** (1 new, 2 updated): **a version 4 layout gets Find Results 1 and 2 closed at the bottom**
  (nothing else moves; a window placed already stays; showing one tabs it at the bottom; version 3 gets them too); the
  default layout's closed windows; the reset count.
- **`crates/ui`** (1 new, 1 updated): the keys (Ctrl+Shift+F, Ctrl+Shift+H, F8, Shift+F8) and the menu items with them.
- **`crates/eludite` headless** (`shell/search_tests.rs`, 12 tests, the real engine on the test solution's files; 2 unit
  tests in `shell/search.rs` and `search/results.rs`):
  - `ctrl_shift_f_opens_the_dialog_with_the_selection_and_enter_searches_the_solution`: the selection `Main` preloaded
    and selected, typing replaces it, **Enter searches the Entire Solution, Find Results 1 groups by file in path
    order under the exact count line**, the rows drawn, the history; Escape closes the dialog; **F8 opens the editor
    at the first match with `Order` selected**, F8 the next, Shift+F8 back and around, **a double-click on a result
    opens it there**; the audit.
  - `append_keeps_the_previous_block_and_find_results_2_is_its_own`: **Append keeps the previous block** under a
    separator, a plain search starts over, **Find Results 2** shows its own and F8 follows it, Clear All, View > Other
    Windows shows the window.
  - `stop_during_a_slow_search_cancels_and_the_window_says_so`: a 20 ms delay per file (the slow overlay); **Stop
    ends the search** before the end, the footer says it was stopped; the 50 ms budget.
  - `an_unsaved_edit_in_an_open_document_is_found_once`: **an unsaved line in an open editor is found, under its path,
    once**, and not on disk; an agent's search without a window is answered only; Current Document, All Open
    Documents, a project by name (and an unknown one named), a folder; a bad regex refused; paging by `search_id`.
  - `a_superseded_search_never_draws`: a slow search superseded by a newer one in the same window; **its block and
    late files never draw**, and a late file is refused by the window.
  - `replace_all_with_preview_holds_changes_and_accept_all_is_one_undo_step_per_document`: through the dialog
    (Ctrl+Shift+H, Match case, Match whole word): **three pending changes, nothing applied, the review view open**, the
    count line; **Accept All applies them**, the closed file opened (Keep modified files open) and edited unsaved;
    **Ctrl+Z (`eludite.editor.undo`) takes a file's two replacements back at once**.
  - `reject_leaves_a_file_untouched_and_a_file_changed_on_disk_is_skipped_with_a_row`: **Reject leaves a file as it
    was; a file changed on disk after Replace All is refused on Accept, never overwritten, with the skipped row in
    Find Results** and the change failed; with Keep modified files open off, accepted files are written directly.
  - `an_agents_replace_with_preview_answers_pending_changes_and_review_applies_them`: **an agent's `replace` with
    `$1` groups answers `pending` with the change ids** in the schema, nothing written; **`eludite.agents.review`
    applies it**; **`preview: false` applies at once** (an open document's buffer and a closed file on disk); the
    classes; an agent cannot open the dialog.
  - `replace_next_and_skip_file_step_through_the_results`: Replace Next finds first, selects the first match, **then
    replaces the selected match and selects the next**; **Skip File** goes to the next file's first match.
  - `the_history_keeps_twenty_queries_per_workspace`: 23 searches keep the newest 20; the history list fills Find what;
    the file written off the UI thread; the Look in list with the solution's project; the regex notes.
  - `ten_thousand_matches_draw_in_a_frame`: **the 10,000-match frame budget**.
  - `the_first_result_streams_before_the_search_ends`: **the first file is drawn while the search still runs**.
- **The Xvfb run** (section 12).

## 7. Deviations and gaps

1. **Files outside the brief's list**, each a hook: `crates/eludite/src/shell/agents/review.rs` (`capture_edit` takes
   the person's Replace All with a call id of its own instead of refusing a non-agent caller; `decide` asks
   `search_before_accept` first when accepting; `finish_change` visible in the shell), `shell/settings.rs` (one line:
   `search_apply_settings`), `app.rs` and `shell/tests.rs` (one line each: `search::bind_keys`, which leaves F8 and
   Shift+F8 to Compare with Unmodified's own handler; without it the global F8 took them and brief 0040's compare test
   failed), `crates/eludite/Cargo.toml`, `crates/docking/src/controller.rs` (the reset test's window count),
   `crates/commands/src/settings.rs` (string entries in list settings, which `search.excludes` is the first to have),
   `crates/mcp/src/server.rs` (the instructions' sentence). `shell.rs` gains the module, the `Services` fields, the
   registration, the `tool_body` parameter, the field, the install call, the `run` hook and the dialog in `render`.
2. **`replace`'s class**: the brief says "`replace` execute" in the file list and "`preview: false` is `execute`" in
   the schema list. I read the second as the rule: `replace` is declared edit (its replacements are held for review,
   as `eludite.workspace.apply_edit`'s are) and raised to execute by its escalation hook when `preview` is false. With
   execute declared, every preview would ask first and then be reviewed again.
3. **`grep-matcher` is a direct dependency** besides the three: the searcher and the matcher must agree on CRLF mode
   (so `$` matches before `\r\n` in .NET's CRLF files) and the `LineTerminator` value is only nameable from
   `grep-matcher`. It adds no crate: `grep-regex` and `grep-searcher` depend on it (Unlicense OR MIT). `regex` (already
   in the build) is used directly for the ranges and groups of a matching line.
4. **The cap keeps the first matches found**, which are not the first by path when files are searched in parallel;
   the answer says `truncated` and its description says to narrow the search. Results within a window and in answers
   are sorted by path.
5. **Replace Next** replaces through `eludite.workspace.apply_edit` (no review: it is one match the person sees
   selected); the results' columns on that line are not shifted afterwards (as in Visual Studio until the next search).
6. **UTF-16 files are searched but not replaced in** (skipped with the reason): the editor and the applier read UTF-8
   only.
7. **The solution's excluded folders**: the workspace model has no excluded-folder list today, so the scope is the
   workspace's folder and the project folders with `search.excludes` and `.gitignore`; when the project system reports
   excluded folders they can be added to `Filters::exclude`.
8. **Several roots** (projects outside the workspace's folder) are walked one after another, each with its own
   overrides, so a relative glob like `src/**` is relative to each root.
9. **The dialog is anchored at the top right** of the window and does not move; it can cover the review view's buttons
   (the Agents window's Accept and Reject and `eludite.agents.review` are always there). The text boxes append and
   delete at the end (Ctrl+A, Ctrl+V, Backspace), like the Rename dialog's; there is no caret movement inside them.
10. **The query history** is kept per workspace folder in `<config dir>/eludite/search/history.json` (brief 0021 has no
    state store; the git drafts of brief 0040 use the same pattern).
11. **Windows and macOS** were not built.

## 8. The dependencies

All from crates.io, by ripgrep's author, **SPDX `Unlicense OR MIT`**:

| Crate | Version | SPDX | New in `Cargo.lock` |
|---|---|---|---|
| `ignore` | 0.4.33 | Unlicense OR MIT | yes |
| `grep-regex` | 0.1.14 | Unlicense OR MIT | yes |
| `grep-searcher` | 0.1.17 | Unlicense OR MIT | yes |
| `grep-matcher` (through both, used directly) | 0.1.9 | Unlicense OR MIT | yes |
| `encoding_rs_io` (through `grep-searcher`) | 0.1.8 | MIT OR Apache-2.0 | yes |

Their other dependencies were already in the build: `globset` 0.4.20 (Unlicense OR MIT), `walkdir` 2.5.0 and
`same-file` 1.0.6 (Unlicense/MIT), `memchr` 2 (Unlicense OR MIT), `crossbeam-deque` 0.8.8, `bstr` 1.13.1, `memmap2`
0.9.11, `log` 0.4, `regex-automata` 0.4.18, `regex-syntax` 0.8.11 (all MIT OR Apache-2.0), `encoding_rs` 0.8.42
((Apache-2.0 OR MIT) AND BSD-3-Clause), `winapi-util` 0.1.11 on Windows (Unlicense OR MIT). All are compatible with
GPL-3.0-or-later.

## 9. What Roslyn structural search will add (PLAN.md 4.11)

Find in Files matches text. Structural search, a later brief, matches syntax: a pattern such as `$x.Count() > 0` that
finds every call of `Count()` compared with zero whatever the receiver's spelling, spacing or line breaks, constrained
by types (`$x` is an `IEnumerable<T>`) and symbols (only `System.Linq.Enumerable.Count`, not a `Count()` of the
project's own), with a replacement template (`$x.Any()`) whose result is formatted and simplified by Roslyn. It runs in
`eludite-host` over the workspace's compilations (out of the shell process, invariant 2), answers the same Find Results
windows (a block per search, matches grouped by file), and its replacements go through the same pending changes and
applier, with the same "checked against what was searched" rule as a document version. What this brief leaves ready
for it: the results windows' model takes any file and line matches, `eludite.search.replace`'s pending changes and
accept hook are not text-specific, and the command surface has room for a `structural` mode beside `regex`.

## 10. Notes for merging with brief 0045

- `Cargo.toml`: the workspace member line (`"crates/search"` after `"crates/terminal"`), the internal dependency line
  and the three third-party lines, which I put after `toml = "0.8"`, away from the `git2` line 0045 edits. `Cargo.lock`
  is best regenerated (`cargo update -w` or a build) after both are merged.
- `protocol/schemas/settings.json` and `crates/commands/src/settings.rs`: the four `search.*` keys and the
  "Environment > Find and Replace" section were appended last; if 0045 also appends settings, the settings test's key
  list and its "last section" assertion need both groups in merge order.
- `crates/docking`: layout schema 5 is this brief's; if another branch also bumps it, the migrations need renumbering.
- No file under `crates/git`, `crates/eludite/src/shell/git*.rs` or `shell/git/` was touched; F8 inside Compare with
  Unmodified is left to it by a key binding in the `GitCompare` context bound from `shell/search.rs`.

## 11. Counts

This machine, `DISPLAY=:99`, `CEF_PATH`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`, `ELUDITE_DBG_MONO` and
`ELUDITE_JS_DEBUG` set, Node 22 on PATH, `dotnet build dotnet/Eludite.slnx` and `bash corpus/tests/build.sh` first:

- `cargo test --workspace --no-fail-fast --features eludite-chromium/cef`: 1,014 passed, 1 failed, 1 ignored (the
  editor's doc example) over 76 test targets. The failure was `crates/browser/tests/chrome.rs`'s
  `acting_on_the_page_in_a_headless_chrome` (not this brief's: a `favicon.ico` 404 reached its console ring during the
  check, under the load of the run); it passes alone (rerun right after, exit 0).
- `cargo fmt --check`: clean. `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`:
  clean (exit 0). `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.

## 12. Screenshots (Xvfb run)

`crates/eludite/tools/find-linux.sh OUT_DIR` opens this repository as the workspace in the real `eludite` binary
under Xvfb and drives it with xdotool keys; the repository is checked unchanged afterwards. In
[0042-run/screenshots](0042-run/screenshots/):

- `find-dialog.png`: Ctrl+Shift+F: the Find in Files dialog, Find in Files tab, Entire Solution.
- `find-results.png`: `fn main`, Enter: Find Results 1 at the bottom with the count line (**40 matching lines in 30
  files, 1,191 files searched**: `target/` and the other excludes skipped) and the files with their highlighted lines.
- `find-f8.png`: F8: `agents/claude-acp/examples/bench.rs` at line 195 with `fn main` selected, the result selected
  in the window.
- `replace-preview.png`: Ctrl+Shift+H, `fn main` with `fn main_renamed`, Alt+A: **41 replacements in 30 files held for
  review** as pending changes, the first review view open (`- fn main() {` / `+ fn main_renamed() {`), the block
  titled `Replace all "fn main", "fn main_renamed", Subfolders, Keep modified files open, ...`; nothing accepted.

## 13. How to reproduce

```
cargo test -p eludite-search                                    # the engine, the fixture tree, the benchmark
cargo test -p eludite-search --release --test bench -- --nocapture   # the numbers against rg
cargo test -p eludite --bin eludite search -- --nocapture       # the shell; prints the timing lines
crates/eludite/tools/find-linux.sh OUT_DIR                      # Xvfb, xdotool, ImageMagick; the screenshots
```
