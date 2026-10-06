# Proposal 0005: ResX Manager and the `.resx` editor

Status: Proposed, 2026-10-05
Plan reference: PLAN.md sections 2 (principles 1 to 4, 6), 3 (D2, D4), 4.2, 4.11 ("`.resx` editor"), 5.1 to 5.4, 5.6, 8, 9, 10 (Phase 2: "resx editor"), 11
Related: ADR-0002, ADR-0004, brief 0042 (`eludite-search`, the reference scan), brief 0048 and 0049 (the host's formatting-preserving project edit), brief 0040 (`eludite-git`, changes since HEAD), brief 0016 (the Agents window, translation by an agent)
New paths: `crates/resx` (`eludite-resx`), `protocol/schemas/resx-*.json`, `protocol/schemas/host/resx-*.json`, `corpus/resx/`, `docs/agents/resx.md`

## 1. Goal

Every ResX string resource in the workspace, in one grid: a row per key, a column per culture, edited in place, with
the untranslated cells, the orphaned keys and the inconsistent translations visible at a glance. This is what the
ResX Resource Manager extension (Tom Englert, MIT, a .NET Foundation project) gives Visual Studio users, and the
person who asked for it works that way. PLAN.md 4.11 names a `.resx` editor for Phase 2 without detail; this
proposal details both surfaces over one model:

- **The ResX Manager window** (View > Other Windows > ResX Manager, the extension's own name since Visual Studio has
  no equivalent): a tree of resource sets on the left (project, folder, base name), the grid on the right, and the
  Translate view. Several sets can be selected and shown in one grid.
- **The `.resx` editor** (Visual Studio's Managed Resources Editor, the tab titled by the file): double-click a
  `.resx` in the Workspace window and get the same grid scoped to that file's set, with the Access Modifier list.

Agents read and edit the same model through `eludite.resx.*`; a hosted agent is the first machine translator.

A **resource set** is a neutral file `Name.resx` with its culture files `Name.<culture>.resx` beside it. A key's
**cell** is its value and comment in one culture. A key is **invariant** when it is never to be translated.

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
  ResX Manager window, the .resx editor  ──────▶  eludite/resx/sets: the EmbeddedResource items per project,
  eludite.resx.*                                   NeutralLanguage, root namespace, manifest names, Generator,
        │                                          CustomToolNamespace, LastGenOutput
        ▼                                         eludite/resx/designer: writes Name.Designer.cs
  eludite-resx on the shell's worker threads      eludite/resx/culture: adds the item to a non-SDK project
  (parse, splice, rules, references via            (brief 0049's formatting-preserving edit)
   eludite-search, changes via eludite-git)
```

- The shell parses, edits and validates: `.resx` is XML, and the model must work in a folder workspace with no host
  (a Rust or web repository with `.resx` files is the same grid). Every file is read and parsed off the UI thread
  and the grid renders the sets as they arrive; a 5,000-key set paints its first rows before the rest is parsed.
- The host answers what only the project system knows: which `.resx` files are resources of which project (an
  `EmbeddedResource` that is not under `bin/` or `obj/`), the neutral language, the designer's namespace and manifest
  name, and it writes the designer file and the non-SDK project item. Without a host, sets are found by walking
  the workspace for `*.resx` (through `ignore`, as search does), designers are left alone and the window says so.
- Writes go through the workspace-edit applier (brief 0015): an open `.resx` document changes in its buffer, a
  closed one is written atomically off-thread; the editor's dirty-file rules, the language servers' file
  notifications and the undo stack are the same as any other edit. The grid marks dirty cells and Save (Ctrl+S)
  writes every dirty file of the selection in one applier call; closing asks. An external change to a parsed file
  reloads the set unless its grid is dirty, in which case the person is asked, as the editor does.
- Nothing runs at startup. The first parse happens when the window opens, a `.resx` is opened, or an agent calls.

## 4. The command surface

All ids are `eludite.resx.*`, schemas in `protocol/schemas/resx-<name>.{input,output}.json`, every command reachable
from the window's toolbar, grid or context menu. A set is named by its neutral file's path; `cultures` default to
all of the set's. Cell writes are class `edit_buffer`, like `eludite.project.set_property` and Replace in Files: an
agent's change is a pending change until accepted, or applied at once under the policy's `edit_buffer: accept`.

| Command | Class | Does |
|---|---|---|
| `manage` | read | shows the ResX Manager window, selecting the given sets |
| `sets` | read | the resource sets of the workspace or a project: path, project, base name, neutral language, cultures with their files, string count, missing and warning counts per culture, designer file and access modifier |
| `entries` | read | a set's rows: key, invariant, per culture the value and comment, warnings, reference count when scanned, changed since `HEAD` or the snapshot; filters `query` (key, value or comment), `missing`, `warnings`, `unused`, `changed`, `cultures`, `invariant`; paged with `skip` and `take` (default 500) |
| `set` | edit_buffer | writes cells `[{ set, key, culture, value?, comment?, invariant? }]`, creating the key's entry in a culture file on the fly; a missing culture file is an error unless `create_culture` is set |
| `add` | edit_buffer | a new key with its neutral value and comment, in the given sets |
| `remove` | edit_buffer; `resx.remove` | removes keys from every culture file of the set |
| `rename` | edit_buffer | renames a key in every culture file; the designer property follows; code is not touched, the answer carries the reference count so the caller renames uses with `eludite.editor.rename` on the designer property |
| `cultures` | edit_buffer for `add`; `resx.remove` for `remove` | adds a culture file (and the non-SDK project item) or removes one, which deletes the file |
| `validate` | read | runs the rules over the given sets and answers the warnings |
| `references` | read | scans code for the keys' uses, streaming counts; cancelable; `wait_ms` waits for the scan |
| `changes` | read | the cells that differ from `HEAD` or the snapshot |
| `snapshot` | execute | writes `.eludite/resx/snapshot.json` for the workspace |
| `export` | read | the selection or sets as TSV, CSV or (later) XLSX, to a path or the answer |
| `import` | edit_buffer; `resx.import` | reads TSV, CSV or XLSX; `preview: true` answers the cells it would change without writing |
| `access_modifier` | edit_buffer | `internal`, `public` or `none` for a set: the `Generator` metadata through the host, the designer regenerated or deleted |
| `translate` (later brief) | dangerous | fills missing cells from a machine translation provider, the external network call the person chooses |

The window's Translate view lists the missing cells of the selection by target culture with the neutral text beside
each and an edit box; "Ask the agent" sends the Agents window a prompt holding the set, the cultures and the keys,
and the hosted agent fills them with `set`, which land as pending changes the person accepts in the grid. This is the
first machine translation and costs no provider, key or dependency; providers (DeepL, Azure Translator, Google Cloud
Translation; keys through the credential store brief 0046 built) are a later brief.

**Policy.** `agents-policy.json` gains `resx`: `remove` (`prompt` default: keys and culture files are deleted;
`allow`, `deny`), `import` (`prompt` default: bulk; `allow`, `deny`), `translate` (`prompt` default: external
network; `deny`). Reads are always allowed. Every write is audited with the set, the keys and the cultures.

**Settings** (`settings.json`, under Tools > Options > Text Editor > Resources): `resx.rules.*` (the four rules),
`resx.sortOnSave`, `resx.referencePatterns`, `resx.showNonStringFiles` (off: a set with no string entry, such as a
WinForms form's, is hidden from the tree), `resx.translationPrefix` (`#TODO_` as the extension's; prefixed to values
a provider wrote, never to an agent's), `resx.columns` (which cultures the grid shows by default), and in the
later brief `resx.translation.provider`.

**Visual Studio surface.** Double-click or Enter on a `.resx` in the Workspace window opens the `.resx` editor;
Open With offers the text editor. The Workspace window shows `Name.<culture>.resx` and `Name.Designer.cs` under
`Name.resx`, as it does today. The window's toolbar: Add Key, Delete, Add Language, the Missing, Warnings, Unused,
Changed and Invariant filters, the search box (300 ms debounce), Columns, Find References, Translate, Export,
Import, Snapshot. F2 renames a key; Delete removes; Ctrl+C and Ctrl+V move cells as TSV; the Error List is not
used (the rules are the window's, as the extension's are).

## 5. Protocol artifacts

`host/resx-sets.json` (`eludite/resx/sets`: `{ generation, projects? }` to `{ generation, sets: [{ neutral, project,
baseName, neutralLanguage?, cultures: [{ name, path }], designer?, accessModifier?, manifestName?, namespace? }] }`),
`host/resx-designer.json` (`eludite/resx/designer`: regenerate or delete), `host/resx-culture.json` (`eludite/resx/culture`:
the project item for a new or removed culture file), `host-rpc.md`'s "Resources" section with the generation rule;
`resx-manage`, `resx-sets`, `resx-entries`, `resx-set`, `resx-add`, `resx-remove`, `resx-rename`, `resx-cultures`,
`resx-validate`, `resx-references`, `resx-changes`, `resx-snapshot`, `resx-export`, `resx-import`,
`resx-access-modifier` (`.input.json` and `.output.json` each), `agents-policy.json` (`resx`), `settings.json`
(`resx.*`), `view-show.input.json` (`resx_manager`), `workspace-tree.output.json` if a set's missing count becomes a
glyph (section 9). The schemas land first and alone, as every brief here does.

## 6. Budgets

- A workspace with 200 `.resx` files in 20 cultures (4,000 files, 100,000 cells) lists its sets within 300 ms of
  the window opening and paints a selected 5,000-key set's first rows within 100 ms of the selection, the rest
  streaming (parsing on the worker threads, the grid virtualized).
- Keystroke to frame p99 under 8 ms while editing a cell of that grid (`assert_budget`), and while a reference scan
  streams counts into it (coalesced per frame).
- A cell write round-trips (splice, applier, the grid's mark) within 50 ms for a 5,000-key file; Save of 20 dirty
  files within 500 ms plus the designer's regeneration on the host.
- A reference scan over a 100,000-file repository streams and cancels at the next match, as Find in Files does.
- Cold start unchanged: nothing parses before the window or a `.resx` opens. No new Rust dependency in the first two
  briefs; the `.xlsx` crates and their SPDX ids come with their brief.

## 7. Briefs

- **R1, the model and the editor.** `crates/resx` (parse, splice, rules, cultures, the round trip on `corpus/resx/`),
  the host's `eludite/resx/sets` and `eludite/resx/designer`, the `.resx` editor as a document tab, `sets`,
  `entries`, `set`, `add`, `remove`, `rename`, `validate`, `access_modifier`, the policy and settings, `corpus/resx/`
  (MIT: neutral and culture files with a BOM, CRLF, comments, a `ResXFileRef`, a WinForms form's file, a designer
  file from each generator, a non-SDK project with the items), `docs/agents/resx.md`.
- **R2, the window.** The ResX Manager window with its tree, the multi-set grid, the filters, Columns, Add
  Language with the non-SDK project item (`eludite/resx/culture`), `cultures`, `references` on `eludite-search`,
  `changes` and `snapshot` on `eludite-git`, TSV copy and paste, `export` and `import` for TSV and CSV, the Translate
  view with "Ask the agent".
- **R3, Excel.** `.xlsx` export and import (`calamine`, `rust_xlsxwriter`, licenses in the PR).
- **R4, machine translation.** The provider trait, DeepL, Azure Translator and Google Cloud Translation over `ureq`
  with keys in the credential store, `translate` under `resx.translate`, the prefix rule, the Translate view's
  provider list.

Brief numbers are assigned on acceptance (0057 to 0062 are taken on open branches).

## 8. Changes to PLAN.md on acceptance

Section 4.11: "`.resx` editor" becomes "`.resx` editor and the ResX Manager window: every set's cultures in one
grid, missing, unused and inconsistent translations, references, changes, Excel exchange, translation by the hosted
agent or a provider (proposal 0005)". Section 10, Phase 2: "resx editor" becomes "resx editor and ResX Manager
(proposal 0005)". Section 12: `crates/resx/` under the Rust workspace. Section 14: a decision row, "ResX: the file
model in the shell (`eludite-resx`, splices that keep every untouched byte), the project knowledge and the designer
generation in the host; the ResX Manager window carries the extension's name since Visual Studio has none; its
`{Invariant}` marker and reference patterns are kept for interchange (proposal 0005)". No ADR: nothing structural
changes; the split follows ADR-0002 and ADR-0004.

## 9. Later, named so they are not forgotten

- Roslyn's exact references for C# and VB keys (find references on the designer property through the host), and
  `rename` renaming uses through it.
- A missing-translation glyph on the set in the Workspace window and an Error List source, if the owner wants them.
- Other resource formats behind the same grid: JSON resource files (`i18n/*.json`, `Microsoft.Extensions.Localization`
  JSON providers), XLIFF and Angular's `messages.xlf`; the model's `ResourceFormat` boundary is drawn in R1 so a
  second format is a file, not a redesign.
- A spell checker over the grid, once the editor has one.
- Pseudo-localization (`qps-ploc`) as a generated culture for testing layouts.

## 10. Risks

- **Designer fidelity.** `ResXFileCodeGenerator`'s output has changed between Visual Studio versions (header text,
  the `GeneratedCodeAttribute` version); regenerating a file written by another version produces a diff once. The
  generator writes the current shape and the report lists the differences the corpus shows; the owner decides whether
  to keep an existing file's header.
- **Culture files without a host** are found by walking, so a `.resx` under an excluded folder or a build output is
  listed if `search.excludes` does not cover it; the defaults do.
- **WinForms files** hold designer state the window must never touch; hiding them by default and never editing a
  non-string entry is the protection, and the round trip test includes one.
- **Agent translations** are only as good as the agent; they land as pending changes, never as applied edits, under
  the default policy, and the Changed filter shows what it wrote.
