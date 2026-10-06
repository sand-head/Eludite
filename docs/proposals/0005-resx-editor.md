# Proposal 0005: The `.resx` editor

Status: Accepted by the owner's direct request, 2026-10-06 (revised 2026-10-05 on the owner's direction: the grid is what a `.resx` file looks like when opened, there is no separate window; R1 built in the same change as brief 0063, which applies the PLAN.md edits of section 9)
Plan reference: PLAN.md sections 2 (principles 1 to 4, 6), 3 (D2, D4), 4.2, 4.11 ("`.resx` editor"), 5.1 to 5.4, 5.6, 8, 9, 10 (Phase 2: "resx editor"), 11
Related: ADR-0002, ADR-0004, brief 0042 (`eludite-search`, the reference scan), brief 0048 and 0049 (the host's formatting-preserving project edit), brief 0040 (`eludite-git`, changes since HEAD), brief 0016 (the Agents window, translation by an agent)
New paths: `crates/resx` (`eludite-resx`), `protocol/schemas/resx-*.json`, `protocol/schemas/host/resx-*.json`, `corpus/resx/`, `docs/agents/resx.md`

## 1. Goal

Open a `.resx` file and see its whole resource set: a row per key, a column per culture, the neutral text beside
every translation, edited in place, with the untranslated cells, the orphaned keys and the inconsistent
translations visible at a glance. This is what the ResX Resource Manager extension (Tom Englert, MIT, a .NET
Foundation project) gives Visual Studio users, and the person who asked for it works that way. Eludite makes it the
editor itself, not a window beside the editor: double-click `Resources.resx`, or `Resources.de.resx`, in the
Workspace window and the tab titled `Resources.resx` opens with the grid. Open With offers the text editor, as
Visual Studio does for its Managed Resources Editor. PLAN.md 4.11 names a `.resx` editor for Phase 2 without detail;
this proposal details it.

Agents read and edit the same model through `eludite.resx.*`; a hosted agent is the first machine translator.

A **resource set** is a neutral file `Name.resx` with its culture files `Name.<culture>.resx` beside it. The editor
opens one set per tab, whichever of its files was opened. A key's **cell** is its value and comment in one culture.
A key is **invariant** when it is never to be translated.

## 2. The file model

`eludite-resx` (`crates/resx`, GPL) owns the format. It knows nothing of the shell, the command bus or the host.

- **Parsing** with `roxmltree` (already a workspace dependency, through `crates/workspace`), keeping each node's byte
  range. A file yields its string entries (`data` with no `type` or `mimetype`, or `type` naming `System.String`),
  their `value`, `comment` and `xml:space`, in file order; its non-string entries (`ResXFileRef` files, binary
  data, WinForms designer entries named `>>...` or `$this...`) are listed with their type and never edited; the
  header (`xsd:schema`, `resheader`, `assembly` aliases), comments and whitespace are opaque bytes.
- **Edits are splices.** A changed value or comment replaces exactly that element's text, escaped as the file
  escapes; a new key is appended before `</root>` in the file's indentation and Visual Studio's shape (`data` with
  `name` and `xml:space="preserve"`, `value`, `comment` only when set); a removed key removes its element and the
  line it sat on; a renamed key changes the `name` attribute. The byte order mark, the XML declaration, the line
  endings and every untouched byte survive: the proving test is a byte-for-byte round trip on `corpus/resx/`.
  `resx.sortOnSave` (off by default, as the extension's) writes the string entries sorted by key instead.
- **Culture detection.** `Name.<suffix>.resx` is a culture file only when `<suffix>` is a .NET culture name;
  `Default.aspx.resx` is a neutral file named `Default.aspx`. The culture table is generated once from
  `CultureInfo.GetCultures(CultureTypes.AllCultures)` by `tools/resx-cultures/generate.sh` into
  `crates/resx/src/cultures.rs` (a generated file, never hand-edited; the script and the SDK it ran are named in the
  file's header). The neutral column's label is the project's `NeutralLanguage` property when the host knows it,
  "Neutral" otherwise.
- **Rules** (the extension's consistency checks, each a setting under `resx.rules.*`, all on by default): string
  format placeholders (`{0}`, `{name}`, `%s`) in a translation match the neutral value's; leading and trailing
  punctuation match; leading and trailing white space match; a translation equal to the neutral value in a
  different culture is flagged as untranslated-looking. A violation is a warning on the cell and a row in the
  Warnings filter; nothing is refused.
- **Invariant** keys carry the `{Invariant}` marker in the neutral entry's comment, as the extension stores it, so a
  workspace moving between the two tools keeps its marks. Invariant keys are excluded from the missing counts.
- **Designer files.** The host regenerates `Name.Designer.cs` (or `.vb`) after a key is added, removed or renamed in
  a neutral file that has one: the same shape `ResXFileCodeGenerator` and `PublicResXFileCodeGenerator` write (the
  class, `ResourceManager`, `Culture`, one property per string key, the file's non-string keys typed), the
  namespace from `CustomToolNamespace` or the root namespace and the folder, the manifest name from the project's
  evaluation, `internal` or `public` from the `Generator` metadata or the existing file's modifier. The proving
  test regenerates the corpus's existing designer files and gets the same bytes. A `.resx` without a designer gets
  none (Blazor and `IStringLocalizer` sets).
- **References.** Each key's uses in code are counted by `eludite-search` (ripgrep's engines, brief 0042) with the
  extension's own pattern scheme, `resx.referencePatterns`: per file extension, regular expressions with `$Key` and
  `$File` (the set's base name) placeholders. Defaults: `.cs`, `.vb` (`\b$File\.$Key\b`, `\["$Key"\]`), `.xaml`
  (`x:Static\s+\w+:$File\.$Key\b`), `.cshtml`, `.razor` (`$File\.$Key\b`, `\["$Key"\]`), `.aspx`, `.ascx`,
  `.master` (`$File,\s*$Key`, `\$\s*Resources:\s*$File,\s*$Key`), `.ts`, `.js` (`\b$Key\b`). The scan runs
  off-thread, streams counts into the grid, is cancelable, and is cached per set until a file under the scope
  changes. Zero references is the Unused filter. Roslyn's exact references on the designer property come later
  (section 9).
- **Changes.** A cell's current value is compared with the same file at `HEAD` through `eludite-git` (brief 0040)
  when the workspace is a repository, else with a snapshot file (`.eludite/resx/snapshot.json`, written on request,
  the extension's snapshot feature); changed cells carry a mark and the Changed filter lists them.
- **Export and import.** The grid's selection copies as tab-separated text with a header row (key, comment, one
  column per culture), which pastes into Excel and back; Export writes the same as `.csv` or `.xlsx`, Import reads
  either, shows every cell it would change as a preview, and applies on confirmation, creating keys and culture
  cells on the fly and never deleting. The `.xlsx` reader and writer are a later brief with their dependency named
  there (`calamine`, MIT; `rust_xlsxwriter`, MIT or Apache-2.0).

## 3. Process model

```
eludite (shell, UI thread)                         eludite-host (brief 0012's host, out of process)
  the .resx editor (a document tab per set) ───▶  eludite/resx/sets: the EmbeddedResource items per project,
  eludite.resx.*                                   NeutralLanguage, root namespace, manifest names, Generator,
        │                                          CustomToolNamespace, LastGenOutput
        ▼                                         eludite/resx/designer: writes Name.Designer.cs
  eludite-resx on the shell's worker threads      eludite/resx/culture: adds the item to a non-SDK project
  (parse, splice, rules, references via            (brief 0049's formatting-preserving edit)
   eludite-search, changes via eludite-git)
```

- The shell parses, edits and validates: `.resx` is XML, and the editor must work in a folder workspace with no
  host (a Rust or web repository with `.resx` files opens the same grid). The set's files are read and parsed off
  the UI thread when the tab opens and the grid renders rows as they arrive; a 5,000-key set paints its first rows
  before the rest is parsed.
- The host answers what only the project system knows: which `.resx` files are resources of which project (an
  `EmbeddedResource` that is not under `bin/` or `obj/`), the neutral language, the designer's namespace and manifest
  name, and it writes the designer file and the non-SDK project item. Without a host, the set is the neutral file
  and the culture files found beside it, designers are left alone and the editor says so.
- Writes go through the workspace-edit applier (brief 0015): a `.resx` open as text changes in its buffer, a closed
  one is written atomically off-thread; the editor's dirty-file rules, the language servers' file notifications and
  the undo stack are the same as any other edit. The grid marks dirty cells and Save (Ctrl+S) writes every dirty
  file of the set in one applier call; closing asks, as a dirty text document does. An external change to one of
  the set's files reloads the grid unless it is dirty, in which case the person is asked, as the text editor does.
- Nothing runs at startup. The first parse happens when a `.resx` opens or an agent calls.

## 4. The editor

The tab is a document tab like the project property pages (brief 0049), titled by the neutral file's name, with
the dirty mark, Ctrl+S and the close prompt a text document has. Its parts:

- **The grid**, virtualized: the Key column, the Comment column, then one column per culture, the neutral culture
  first; cells edit in place (F2, Enter, or typing), an edit in an empty culture cell creates the entry in that
  culture's file on the fly; a missing cell is tinted, a warning cell carries a glyph with the rule in its tooltip,
  a changed cell carries a mark, an invariant key is dimmed in the culture columns; the References column shows the
  count once Find References has run. Ctrl+C and Ctrl+V move cells as TSV with Excel. Columns hides and shows
  cultures and the Comment and References columns, remembered per workspace in brief 0021's state.
- **The toolbar**: Add Key, Delete, Rename (F2 on the key), Add Language (a culture picker over the generated
  table; creates `Name.<culture>.resx` and the non-SDK project item), Remove Language, the Missing, Warnings,
  Unused, Changed and Invariant filters, the search box (300 ms debounce, over keys, values and comments), Find
  References, Translate, Export, Import, Snapshot, Columns, and the Access Modifier list (Internal, Public, No code
  generation) that Visual Studio's editor has, which edits the `Generator` metadata through the host and
  regenerates or deletes the designer.
- **Non-string entries** (icons, files, a WinForms form's designer state) are listed under a collapsed Other
  Resources section with their type and file reference, read-only; a file with only such entries opens with that
  section expanded and an empty strings grid.
- **The Translate view** (the toolbar's Translate) replaces the grid in the tab while open: the missing cells by
  target culture with the neutral text beside each and an edit box; "Ask the agent" sends the Agents window a prompt
  holding the set, the cultures and the keys, and the hosted agent fills them with `set`, which land as pending
  changes the person accepts in the grid. This is the first machine translation and costs no provider, key or
  dependency; providers (DeepL, Azure Translator, Google Cloud Translation; keys through the credential store brief
  0046 built) are a later brief and appear here as a list beside the agent.
- **The Workspace window** keeps showing `Name.<culture>.resx` and `Name.Designer.cs` under `Name.resx`; opening
  any of the culture files opens the set's tab and selects that culture's column. Open With > XML (Text) Editor
  opens the file as text, and the two stay consistent through the applier.

## 5. The command surface

All ids are `eludite.resx.*`, schemas in `protocol/schemas/resx-<name>.{input,output}.json`, every command reachable
from the editor's toolbar, grid or context menu. A set is named by its neutral file's path (a culture file's path
is accepted and resolved); `cultures` default to all of the set's. Cell writes are class `edit_buffer`, like
`eludite.solution.select_configuration` and Replace in Files: an agent's change is a pending change until accepted,
or applied at once under the policy's `edit_buffer: accept`.

| Command | Class | Does |
|---|---|---|
| `sets` | read | the resource sets of the workspace or a project: path, project, base name, neutral language, cultures with their files, string count, missing and warning counts per culture, designer file and access modifier |
| `entries` | read | a set's rows: key, invariant, per culture the value and comment, warnings, reference count when scanned, changed since `HEAD` or the snapshot; filters `query` (key, value or comment), `missing`, `warnings`, `unused`, `changed`, `cultures`, `invariant`; paged with `skip` and `take` (default 500) |
| `set` | edit_buffer | writes cells `[{ set, key, culture, value?, comment?, invariant? }]`, creating the key's entry in a culture file on the fly; a missing culture file is an error unless `create_culture` is set |
| `add` | edit_buffer | a new key with its neutral value and comment |
| `remove` | edit_buffer; `resx.remove` | removes keys from every culture file of the set |
| `rename` | edit_buffer | renames a key in every culture file; the designer property follows; code is not touched, the answer carries the reference count so the caller renames uses with `eludite.editor.rename` on the designer property |
| `cultures` | edit_buffer for `add`; `resx.remove` for `remove` | adds a culture file (and the non-SDK project item) or removes one, which deletes the file |
| `validate` | read | runs the rules over the set and answers the warnings |
| `references` | read | scans code for the keys' uses, streaming counts; cancelable; `wait_ms` waits for the scan |
| `changes` | read | the cells that differ from `HEAD` or the snapshot |
| `snapshot` | execute | writes `.eludite/resx/snapshot.json` for the workspace |
| `export` | read | the selection or the set as TSV, CSV or (later) XLSX, to a path or the answer |
| `import` | edit_buffer; `resx.import` | reads TSV, CSV or XLSX; `preview: true` answers the cells it would change without writing |
| `access_modifier` | edit_buffer | `internal`, `public` or `none` for a set: the `Generator` metadata through the host, the designer regenerated or deleted |
| `translate` (later brief) | dangerous | fills missing cells from a machine translation provider, the external network call the person chooses |

Opening the editor is `eludite.file.open` on any file of the set, as for every document (`editor: text` asks for
the text editor); there is no command of its own.

**Policy.** `agents-policy.json` gains `resx`: `remove` (`prompt` default: keys and culture files are deleted;
`allow`, `deny`), `import` (`prompt` default: bulk; `allow`, `deny`), `translate` (`prompt` default: external
network; `deny`). Reads are always allowed. Every write is audited with the set, the keys and the cultures.

**Settings** (`settings.json`, under Tools > Options > Text Editor > Resources): `resx.rules.*` (the four rules),
`resx.sortOnSave`, `resx.referencePatterns`, `resx.translationPrefix` (`#TODO_` as the extension's; prefixed to
values a provider wrote, never to an agent's), `resx.openAsText` (off: a `.resx` opens in the grid; on: as text,
with Open With offering the grid), and in the later brief `resx.translation.provider`.

## 6. Protocol artifacts

`host/resx-sets.json` (`eludite/resx/sets`: `{ generation, projects? }` to `{ generation, sets: [{ neutral, project,
baseName, neutralLanguage?, cultures: [{ name, path }], designer?, accessModifier?, manifestName?, namespace? }] }`),
`host/resx-designer.json` (`eludite/resx/designer`: regenerate or delete), `host/resx-culture.json` (`eludite/resx/culture`:
the project item for a new or removed culture file), `host-rpc.md`'s "Resources" section with the generation rule;
`resx-sets`, `resx-entries`, `resx-set`, `resx-add`, `resx-remove`, `resx-rename`, `resx-cultures`, `resx-validate`,
`resx-references`, `resx-changes`, `resx-snapshot`, `resx-export`, `resx-import`, `resx-access-modifier`
(`.input.json` and `.output.json` each), `agents-policy.json` (`resx`), `settings.json` (`resx.*`),
`workspace-tree.output.json` if a set's missing count becomes a glyph (section 9). The schemas land first and alone,
as every brief here does.

## 7. Budgets

- A 5,000-key set in 20 cultures (100,000 cells) paints its first rows within 100 ms of the tab opening, the rest
  streaming (parsing on the worker threads, the grid virtualized); a workspace with 200 such sets answers `sets`
  within 300 ms.
- Keystroke to frame p99 under 8 ms while editing a cell of that grid (`assert_budget`), and while a reference scan
  streams counts into it (coalesced per frame).
- A cell write round-trips (splice, applier, the grid's mark) within 50 ms for a 5,000-key file; Save of 20 dirty
  files within 500 ms plus the designer's regeneration on the host.
- A reference scan over a 100,000-file repository streams and cancels at the next match, as Find in Files does.
- Cold start unchanged: nothing parses before a `.resx` opens. No new Rust dependency in the first two briefs; the
  `.xlsx` crates and their SPDX ids come with their brief.

## 8. Briefs

- **R1, the model and the editor.** `crates/resx` (parse, splice, rules, cultures, the round trip on `corpus/resx/`),
  the host's `eludite/resx/sets` and `eludite/resx/designer`, the editor as the document a `.resx` opens as (the
  grid, in-place editing, the filters, the search box, Columns, Add Key, Delete, Rename, the Access Modifier list,
  Other Resources), `sets`, `entries`, `set`, `add`, `remove`, `rename`, `validate`, `access_modifier`, the policy
  and settings, `corpus/resx/` (MIT: neutral and culture files with a BOM, CRLF, comments, a `ResXFileRef`, a
  WinForms form's file, a designer file from each generator, a non-SDK project with the items),
  `docs/agents/resx.md`.
- **R2, languages, references and exchange.** Add Language and Remove Language with the non-SDK project item
  (`eludite/resx/culture`), `cultures`, `references` on `eludite-search`, `changes` and `snapshot` on
  `eludite-git`, TSV copy and paste, `export` and `import` for TSV and CSV, the Translate view with "Ask the agent".
- **R3, Excel.** `.xlsx` export and import (`calamine`, `rust_xlsxwriter`, licenses in the PR).
- **R4, machine translation.** The provider trait, DeepL, Azure Translator and Google Cloud Translation over `ureq`
  with keys in the credential store, `translate` under `resx.translate`, the prefix rule, the Translate view's
  provider list.

Brief numbers are assigned on acceptance (0057 to 0062 are taken on open branches).

## 9. Changes to PLAN.md on acceptance

Section 4.11: "`.resx` editor" becomes "`.resx` editor: a file opens as its whole set, every culture a column,
missing, unused and inconsistent translations, references, changes, Excel exchange, translation by the hosted agent
or a provider (proposal 0005)". Section 10, Phase 2: "resx editor" becomes "resx editor (proposal 0005)". Section
12: `crates/resx/` under the Rust workspace. Section 14: a decision row, "ResX: a `.resx` opens as its resource set
in one grid, there is no separate manager window (owner's decision, 2026-10-05); the file model in the shell
(`eludite-resx`, splices that keep every untouched byte), the project knowledge and the designer generation in the
host; the extension's `{Invariant}` marker and reference patterns are kept for interchange (proposal 0005)". No
ADR: nothing structural changes; the split follows ADR-0002 and ADR-0004.

## 10. Later, named so they are not forgotten

- Roslyn's exact references for C# and VB keys (find references on the designer property through the host), and
  `rename` renaming uses through it.
- A missing-translation glyph on the set in the Workspace window and an Error List source, if the owner wants them.
- Several sets in one grid (the extension's multi-select), should a workspace with many small sets need it; the
  model and the commands already take a set each, so it is a tab, not a redesign.
- Other resource formats behind the same grid: JSON resource files (`i18n/*.json`, `Microsoft.Extensions.Localization`
  JSON providers), XLIFF and Angular's `messages.xlf`; the model's `ResourceFormat` boundary is drawn in R1 so a
  second format is a file, not a redesign.
- A spell checker over the grid, once the editor has one.
- Pseudo-localization (`qps-ploc`) as a generated culture for testing layouts.

## 11. Risks

- **Designer fidelity.** `ResXFileCodeGenerator`'s output has changed between Visual Studio versions (header text,
  the `GeneratedCodeAttribute` version); regenerating a file written by another version produces a diff once. The
  generator writes the current shape and the report lists the differences the corpus shows; the owner decides whether
  to keep an existing file's header.
- **A culture file opened alone** still opens the whole set, which is the point, but a person who wanted the XML
  reaches it through Open With or `resx.openAsText`; the report says whether that was the right default.
- **Sets without a host** are found beside the neutral file, so a stray `Name.de.resx` under a build output is
  listed only if it sits beside its neutral file; `search.excludes` governs the reference scan, not the set.
- **WinForms files** hold designer state the editor must never touch; listing them read-only under Other Resources
  and never editing a non-string entry is the protection, and the round trip test includes one.
- **Agent translations** are only as good as the agent; they land as pending changes, never as applied edits, under
  the default policy, and the Changed filter shows what it wrote.
