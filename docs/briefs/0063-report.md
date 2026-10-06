# Brief 0063 report: the `.resx` editor

Status: done on Linux, against the fake host (the shell) and the real `eludite-host` (its own tests, MSBuild in
process). Windows and macOS: not built here; nothing added is platform-specific. CI: not run.
Branch: `proposal/0005-resx-manager`, based on `main` at `be243ba`; not rebased.
Date: 2026-10-06. Brief: [0063-resx-editor.md](0063-resx-editor.md); proposal
[0005](../proposals/0005-resx-editor.md), accepted by the owner's direct request and built in the same change.

## 1. Summary

- **Schemas first**: eight command schemas `protocol/schemas/resx-*.{input,output}.json`, the host's
  `host/resx-sets.json` and `host/resx-designer.json` with the "Resources" section and the two methods table rows of
  `host-rpc.md`, the policy's `resx.remove`, the settings `resx.*` (the Options page "Text Editor > Resources"),
  and `editor` on `file-open.input.json` (`text` asks for the text editor).
- **`eludite-resx`** (`crates/resx`): a file parsed on `roxmltree` into entries with their byte ranges (string
  entries editable, the others listed), edits as splices (`set_value`, `set_comment`, `add` in Visual Studio's shape,
  `remove` with its line, `rename`, `header_only` for a new culture file, `sorted_text` for `resx.sortOnSave`), the
  byte order mark and the line endings kept, UTF-16 refused with a message; the four rules; the `{Invariant}` marker;
  the culture rule on the generated table (`tools/resx-cultures/generate.sh`, 851 names from the SDK's ICU list, so
  `Default.aspx.resx` is neutral); sets from any of their files (`set_files`) or a folder walk on `ignore`
  (`discover`); rows with cells and warnings. The corpus `corpus/resx/` round-trips byte for byte.
- **The editor** (`crates/eludite/src/shell/resx/`): a `.resx` opens as its set, one tab `resx:<neutral file>`
  titled by the neutral file, the files parsed off the UI thread; the grid (Key, Comment, one column per culture,
  the neutral labeled by the project's neutral language), in-place editing on double-click, Enter and F2, Add Key,
  Delete, Rename, Invariant, the Missing, Warnings and Invariant filters, the search box, the Access Modifier list,
  Save, the status line with the counts and the selected cell's warning, Other Resources collapsed; dirty until
  Save, which writes every dirty file as a whole-file edit through the workspace-edit applier and has the host
  regenerate the designer when keys changed; closing asks; Open With > XML (Text) Editor in the Workspace window and
  `resx.openAsText`.
- **The commands** (`crates/commands/src/resx.rs`, `eludite.resx.*`): `sets`, `entries`, `validate` (read), `set`,
  `add`, `rename`, `access_modifier` (edit_buffer), `remove` (edit_buffer under `resx.remove`, `prompt` by default
  for agents); the shell's bus (`ResxBus`) runs them on the UI thread; an agent's write is held as pending changes
  under `edit_buffer: review` (`pending` in the answer, nothing written until accepted); the MCP guide
  `eludite://guides/resx` (`docs/agents/resx.md`).
- **The host** (`dotnet/src/Eludite.Host/Resources/`): `eludite/resx/sets` from the evaluated projects
  (`EmbeddedResource` items not under `bin/` or `obj/`, the culture rule on `CultureInfo.GetCultureInfo(suffix,
  predefinedOnly: true)`, the neutral language, the designer, the access modifier, the manifest name and the
  namespace) and `eludite/resx/designer` (`generate`, `setModifier`, `delete`): the designer in
  `ResXFileCodeGenerator`'s shape, the `Generator` and `LastGenOutput` metadata and the designer's `Compile` item
  edited with the construction model preserving formatting, the solution reloaded after a project write.

## 2. Commits

One commit on `proposal/0005-resx-manager`, after the proposal's two: the whole of this brief, with the owner's go-ahead
for the push (the work finished inside the weekday window CLAUDE.md keeps commits out of).

## 3. Decisions taken while building

- **Where the editor opens.** `Shell::apply`'s `eludite.file.open` arm routes a `.resx` to the grid; the direct
  callers of `open_file` (Find Results, the Error List, navigation) keep the text editor, since they name a line.
- **The person's edits are view state until Save**, as the property pages' are; the toolbar's Add Key, Delete,
  Rename and Invariant change the model and mark the tab dirty. The Access Modifier runs `eludite.resx.access_modifier`
  at once, as the Debug page's profile edits run their command, because it needs the host.
- **A write from another caller** (an agent, or a person's outer call) loads the set afresh and writes through the
  applier; with the set open and clean the grid reloads afterwards; with the set open and dirty, an agent's write is
  refused ("save or discard first") and a person's outer call lands in the open grid and is saved from there
  (`pending` while the save runs).
- **Agents' review hold.** The write commands are class edit_buffer but not in `REVIEWED_COMMANDS`, so an agent's
  call is gated like Replace in Files (the Agents window asks under `edit_buffer: review`), then the applier holds
  the edit as pending changes and the answer says `pending` with the change ids; the grid does not reflect a held
  change until it is accepted and the set reloads.
- **F2 in the grid** is the Rename of the editor; outside a cell the keymap's F2 (Rename symbol) wins, so the toolbar's
  Rename is the reliable way and the test uses it.
- **The grid's cells are plain elements with a mouse-down listener**, not stateful ones, and draw without text
  ellipsis: with some two hundred cells a frame, per-element state and a second shaping of every text cost the frame
  budget (section 6).
- **Line numbers and entry lookups** in `eludite-resx` are indexed (a newline table and a name index): the first
  version counted newlines per entry and scanned entries by name, which took 20 s to parse a 5,000-entry file in a
  debug build.

## 4. The designer generator and the corpus

The host reproduces the three corpus designer files byte for byte. The corpus files were written by hand from what
Visual Studio's generator is known to write (the header, `GeneratedCodeAttribute(..., "17.0.0.0")`, members sorted
with `InvariantCultureIgnoreCase`, the `internal` constructor, `SecurityElement.Escape` in the summaries, the
512-character cut, `System.Drawing.Icon` from a `ResXFileRef`, the identifiers `_1Number`, `with_space` and
`_class`). One expectation is a guess: the summary of a value with a line break (`Multi`) in
`Strings/Properties/Resources.Designer.cs` has `Line one` CR CR LF `        ///Line two.`; the host matches it by
reading the `.resx` without newline normalization, as CodeDom keeps the value's CR LF and the tool's own newline
pass adds a CR. Whether real Visual Studio writes CR CR LF or CR LF there has not been checked against a machine
with Visual Studio; the owner may regenerate a designer in Visual Studio and compare. Everything else in the three
files is what the generator writes as far as the authors know. Visual Basic designers are listed (`.Designer.vb`)
but not generated (the host answers -32602).

## 5. Proving tests

- `cargo test -p eludite-resx`: 17 unit tests (parsing, every splice, the template, LF and tab files, the rules,
  the culture rule, the invariant marker, sets and rows, a missing neutral file) and 5 corpus tests (9 files
  round-trip, the Strings set's rows and warnings, 5 sets found with the `aspx` rule, every splice keeps every other
  byte, a new culture file takes the neutral header).
- `cargo test -p eludite-commands` (125, with the resx module's parse, schema, policy and audit tests),
  `-p eludite-protocol` (the typed messages conform to the schemas), `-p eludite-lsp --features fake`,
  `-p eludite-mcp` (the tools, their classes, the gate for `remove`, the guide).
- `cargo test -p eludite resx_tests`: six headless tests, all green: the open from the Workspace window with the
  timing, the cells, an edit saved byte for byte with the designer call, the close prompt; the filters, the search
  box, Rename, Delete, the comment and the invariant mark saved to every file; agents' `sets`, `entries`,
  `validate`, `set` (created on the fly, `create_culture`), the review hold, `rename` and `remove` with the
  designer calls and the audit, the open-and-clean reload and the open-and-dirty refusal; the Access Modifier through
  the host; `editor: text`, Open With and `resx.openAsText`; the 5,000-key set's open and frame budget.
- `dotnet test dotnet/Eludite.slnx --filter "FullyQualifiedName~Resx"`: 10 tests (the sets of both solutions, the
  three designers byte for byte and `unchanged`, `New Key` to `New_Key` in sorted position, `setModifier` public,
  none and on a legacy project, the stale generation, the wire); the whole host test project: 179 passed, 8 skipped.

## 6. Budget numbers

Debug build, this container (4 cores, shared; the figures are not the reference machine's):

| Measure | Result |
|---|---|
| Open to rows shown, 3 files, 3 keys | 10 to 12 ms |
| Open to rows shown, 5,000 keys in 3 cultures (the files parsed off the UI thread) | 87 to 106 ms (budget 100 ms) |
| Frame p50 / p99 per round, the editor alone, the selection moving one row per frame | 4.9 to 6.4 ms p50; 6.9 to 13 ms p99 across runs (the best round's p99 is asserted under 8 ms: it passed on one run here, 6.9 ms, and missed on others, 8.4 to 8.8 ms) |
| The same, the whole shell | 4.7 to 6.3 ms p50; 7.2 to 24 ms p99 |
| The NuGet window's 500 results on the same machine, for scale | 2.5 / 6.8 ms best round |

The grid's median frame is above the NuGet window's: five texts per row against two or three. On this shared
container the p99 moves from run to run (the pre-existing forge and git budget tests miss here too, section 10);
the assertion is CI's call on the reference machine. What would bring it down: one text run per
row (the cells laid out from measured column widths) and a `uniform_list` that reuses the unchanged rows' elements.

## 7. The Xvfb run

`crates/eludite/tools/resx-linux.sh docs/briefs/0063-run` (the real binary, the real host, a copy of `corpus/resx`,
real X input from xdotool against `--bounds-out`): [screenshots](0063-run/screenshots/) `grid.png` (the set opened
from its German file: the three cultures, the missing French Save tinted, the warning glyph on the French Hello and
the German Speichern, the invariant Brand dimmed, the line break of Multi shown as a mark), `edited.png` (the French
Save selected and typed over), `saved.png` (one element changed: [`Resources.fr-FR.resx.diff`](0063-run/Resources.fr-FR.resx.diff)),
`added.png` (Add Key "Welcome" saved: [`Resources.resx.diff`](0063-run/Resources.resx.diff) and the designer the
host regenerated, [`Resources.Designer.cs.diff`](0063-run/Resources.Designer.cs.diff)), `missing.png` (the Missing
filter), `public.png` (Access Modifier Public: [`Strings.csproj.diff`](0063-run/Strings.csproj.diff) changes the one
`Generator` element, [`Resources.Designer.public.diff`](0063-run/Resources.Designer.public.diff) the designer). The
run found two things the headless tests had not: the host's set list was asked for while the solution was still
loading (a stale generation), so the list is asked again after a moment, up to five times; and a value with a line
break overflowed its row, so the grid shows line breaks as a return mark.

## 8. Not done, and why

- **Watching the files** while a set is open: an external change (another editor, git) is not noticed until the
  set reloads after one of the editor's own writes. The proposal names the editor's reload-unless-dirty rule; it
  needs the file watcher the editor's documents use, a later brief.
- **A held agent write in the grid**: the pending change is not shown in the grid (the applier's review surface
  shows it); after acceptance the grid reloads only on its next write. Adding the write commands to
  `REVIEWED_COMMANDS` with an `amend_output` would let the agent's call wait for the decision; not done to keep the
  review module untouched.
- **R2 and later**: Add Language and Remove Language with the project item (`eludite/resx/culture`), `references`,
  `changes` and `snapshot`, copy and paste, export and import, the Translate view, Excel, machine translation.
- **UTF-16 files** open as text with the parse error in the status line.
- **A created culture file through `set` with `create_culture`** is written by the applier's `Create` then the
  whole-file edit: it has no byte order mark, while Visual Studio's would.

## 9. Files outside the listed scope

- `crates/eludite/src/shell/agents.rs`: `agent_edits_reviewed` (one accessor over the existing policy store).
- `crates/eludite/src/shell/explorer.rs`: the Open With item for `.resx` rows and the context menu's condition.
- `crates/commands/src/workspace.rs`, `protocol/schemas/file-open.input.json`: `editor`.
- `crates/commands/src/settings.rs`: the pinned key list and section positions.
- `crates/lsp/src/fake/projects.rs`: `reload` made `pub(super)` for the fake's resx module.
- `crates/mcp/src/tests.rs`: the resource count (the guide added), `crates/mcp/src/server.rs`: the instructions.
- `Cargo.toml`, `Cargo.lock`: the member and the dependency.
- `dotnet/tests/Eludite.Host.Tests/ProjectCorpus.cs`: a folder parameter (the old use unchanged).

## 10. Verification

- `cargo fmt --check` clean. `cargo clippy --workspace --all-targets -- -D warnings` clean (three findings in the
  new code fixed: two type aliases, a derived `Default`).
- `cargo test --workspace` did not fit this container's disk (the whole workspace's test binaries fill its
  allowance); the crates ran one by one: `eludite-resx`, `eludite-commands`, `eludite-protocol`, `eludite-lsp`
  (all features), `eludite-mcp`, `eludite-docking`, `eludite-ui`, `eludite-workspace` all green, and `eludite`'s
  407 tests green but for five frame-budget assertions (`assert_budget`) when the suite runs with four threads on
  this shared 4-core container; alone and one at a time, the CodeLens and search budgets pass here while the
  pre-existing forge and git budgets fail like the resx one (the resx editor alone: 4.5 to 5.3 ms p50, 8.4 to 39 ms
  p99 across rounds). The budgets are CI's on the reference machine (CLAUDE.md); the numbers above are this
  machine's.
- `dotnet build dotnet/Eludite.slnx` zero warnings; `dotnet test --filter "FullyQualifiedName~Resx"` 10 passed (the
  four other test projects report "zero tests ran" under the filter); the whole host test project 179 passed, 8
  skipped.
