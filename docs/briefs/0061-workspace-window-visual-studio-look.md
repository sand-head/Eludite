# Brief 0061: The Workspace window's Visual Studio look: icons, colors and the search box

Status: in progress (implemented and tested on Linux; the four screenshots are owed, see the [report](0061-report.md))
Phase: 2
Plan reference: PLAN.md sections 2 (principles 3, 5), 4.2, 8 ("Themes and type": our own iconography; "Default
layout"), 9
Depends on: brief 0012 (the Workspace window), brief 0040 (the source control glyphs), brief 0048 (the Dependencies
node), brief 0056 (`TextInput`, for the search box)

## Goal

The Workspace window reads like Visual Studio 2022's Solution Explorer: colored icons for the solution, solution
folders, projects, the Dependencies node, folders and files by type; the existing change glyphs (modified, added, untracked, deleted, renamed, conflicted) kept
small beside the icons; and the search box "Search Workspace (Ctrl+;)" that filters the tree as the person types.
No toolbar: the owner does not want one (2026-10-05). Today the rows show monochrome text glyphs (`C#`, `Rs`, box
characters) and the window has no search. The icons are drawn by us (PLAN.md 8: no Zed icons, our own iconography)
and shipped as SVG assets through a GPUI asset source, which the shell lacks today; the Agents window's text glyphs
can move to the same set in a later brief.

## Files in scope

- `crates/ui/icons/*.svg` (new; see the icon list), `crates/ui/src/icons.rs` (new: `Icon` enum, the asset source,
  `icon(Icon, color)` element), `crates/ui/src/theme.rs` (the icon color tokens for the three themes),
  `crates/ui/src/tree.rs` (an icon instead of a glyph, the match highlight), `crates/ui/src/lib.rs`,
  `crates/ui/README.md` or the crate docs (the icon rules: 16 by 16, 1 px strokes, `currentColor` for monochrome
  shapes, at most three fixed colors for the colored ones, every icon listed with what it means).
- `crates/eludite/src/app.rs` (`Application::with_assets`), `crates/eludite/src/shell/explorer.rs` (icons and the search), `crates/eludite/src/shell/explorer_tests.rs` (new or the
  existing tests module), `crates/workspace/src/explorer.rs`
  (`Row::file_type`, the no-duplicates rule).
- `protocol/schemas/` first and alone: `workspace-search.input.json` and `.output.json` (new); then
  `crates/commands/src/workspace.rs`.
- `crates/eludite/tools/` (a driver for the screenshots), `docs/briefs/README.md`, this file, `docs/briefs/0061-report.md`
  (new), CLAUDE.md's `crates/ui` row.

## Contract

### Icons

- `eludite_ui::icons::Assets` implements `gpui::AssetSource` over `include_bytes!` of `crates/ui/icons/`; `app.rs`
  registers it (`Application::new().with_assets(Assets)`); `icon(Icon::Folder, color)` is a `gpui::svg()` 16 by 16
  with `text_color`. Monochrome icons use `currentColor`; the colored ones carry their colors in the file and get a
  `-light` variant only when the dark one does not read on VS Light (the report lists which).
- The set (each our own drawing, Visual Studio's metaphor, not its artwork): `solution` (purple), `workspace` (the
  folder root), `cargo-workspace` and `cargo-package` (the Rust gear-crate in the Rust orange), `solution-folder`
  (yellow folder) and `solution-folder-open`, `project-csharp` (green square with C#), `project-csharp-test` (the
  same with a flask, for projects referencing a test framework per the host's project kind), `project-web` (the
  globe variant for `web: true`), `project-unavailable` (muted, with the error badge), `dependencies` (the blue
  stack), `frameworks`, `packages` (the NuGet cube), `package` and `package-transitive`, `project-reference`,
  `framework`, `folder` and `folder-open` (yellow), `cargo-targets` and `cargo-target-bin`, `-lib`, `-test`, `-example`,
  `-bench`; files: `file-cs` (purple C#), `file-rs`, `file-json`, `file-md`, `file-xml` (`.csproj`, `.props`,
  `.targets`, `.xml`, `.config`, `.resx`), `file-sln`, `file-toml`, `file-ts`, `file-js`, `file-html`, `file-css`,
  `file-razor` and `file-cshtml`, `file-aspx`, `file-image` (`.png`, `.jpg`, `.gif`, `.svg`, `.ico`), `file-text`
  (`.txt`, `.log`), `file-shell` (`.sh`, `.ps1`), `file-yaml`, `file-lock` (lockfiles), `file` (anything else).
  The search box: `search`, `clear`.
- Theme tokens (`crates/ui/src/theme.rs`, all three themes): `icon_folder`, `icon_csharp_file`, `icon_csharp_project`,
  `icon_rust`, `icon_json`, `icon_markdown`, `icon_xml`, `icon_web`, `icon_package`, `icon_dependencies`,
  `icon_solution`, `icon_muted`. VS Dark's values start from Visual Studio's dark palette as the eye reads
  it (folder `DCB67A`, C# file `A179DC`, C# project `3BA25A`, JSON `CBCB41`, Markdown `519ABA`, XML `E37933`, package
  `6DAEE1`, dependencies `519ABA`, solution `A179DC`); the report has a screenshot beside Visual Studio's
  for the owner to judge.
- `NodeKind` to icon: `Solution` → solution; `FolderRoot` → workspace; `CargoWorkspace` → cargo-workspace;
  `CargoPackage` → cargo-package; `Project { kind, web, error }` → the C# variants by kind, web and error; `Folder` →
  folder or folder-open by the expanded state (solution folders from `.slnx` likewise); `Dependencies` →
  dependencies; `DependencyGroup` → frameworks, packages or project-reference by group; `Package { transitive }` →
  package or package-transitive, with brief 0048's warning badge kept; `Framework` → framework; `ProjectReference` →
  project-csharp; `CargoTargets` and `CargoTarget { kind }` → theirs; `File { item_type }` → by extension
  (`Row::file_type`), case-insensitive.

### The change glyphs

- Brief 0040's source control glyphs stay as they are and keep their colors, drawn small (the small type size) in
  the badge slot between the icon and the label: modified, added, untracked, deleted, renamed, conflicted and ignored.
  No glyph for an unchanged tracked file: the owner does not want Visual Studio's lock (2026-10-05).

### The command

- `eludite.workspace.search {query}` (agent-visible, class `read`): answers the matching rows `[{path, name, kind}]`,
  at most 500, with the same rule the box uses; `query: ""` clears the box.

### The search box

- Under the title bar, full width, a single-line `TextInput` (brief 0056) with the placeholder "Search Workspace
  (Ctrl+;)" and the search icon; Ctrl+; focuses it from anywhere in the shell (the Visual Studio key), Escape clears
  it and returns focus to the tree. As the person types (150 ms after the last keystroke), the tree shows the rows
  whose name contains the query (case-insensitive, every word of a multi-word query must match) and their ancestors,
  expanded; the matched characters are drawn bold in the accent color as the editor's completion list does; an
  empty query restores the tree and the expanded set from before the search. The search runs over the model's rows
  on the UI thread when under 20,000 rows, else on a background task with the stale-result rule (principle 12).

### Nothing listed twice

- A .NET project that belongs to the open solution appears under the solution only. Today the folder listing can
  show the same `.csproj` again at the workspace root (or under a plain folder) because the walk finds the project
  file on disk; the model drops a folder-walk project row whose path (canonicalized) is a project of the solution,
  and drops a plain folder row that holds nothing but such projects. The same rule applies to a Cargo package of the
  open Cargo workspace. `eludite.workspace.tree`'s `projects` lists each project once, under the same rule (the owner's
  request, 2026-10-05).

### Nothing else moves

- Selection, double-click, the context menus, the startup project's bold, the warning badge and the git glyph
  colors stay as they are; the row height stays `TREE_ROW_HEIGHT`.

## Proving test

- `crates/ui`: every `Icon` has a file and the file parses as SVG (a sanity check with the asset source); the
  monochrome ones contain `currentColor`; the three themes define every icon token and each reads against its
  theme's panel background (the readability test brief 0058 added, extended).
- `crates/eludite` explorer tests: the icon chosen for each `NodeKind` and for 20 extensions; the change glyph
  on a modified file and none on an unchanged one; a workspace folder holding `Eludite.slnx` and its projects lists
  each project once, under the solution, and `eludite.workspace.tree` answers each once; a search for `host rpc` shows
  `HostRpcTarget.cs` under its expanded ancestors with the matches bold, Escape restores the previous expanded set;
  Ctrl+; focuses the box from the editor; `eludite.workspace.search` on the bus answers its schema and `""` clears.
- Benchmarks: brief 0040's "1,000 changed files draw in a frame" and brief 0012's tree draw stay within 5 percent.
- Manual, three screenshots by a driver on Xvfb (`linux-workspace-vs-look-<theme>.png` for dark, light and blue) of
  `dotnet/Eludite.slnx` opened as the workspace with `debuggers`, `src` and `tests` expanded, a search in a fourth
  (`linux-workspace-search.png`).

## Budget

- A frame with 60 visible rows and icons under 2 ms more than today's glyph rows on the reference machine (the icons
  rasterize once per size and are cached by GPUI).
- The search over 20,000 rows under 10 ms per keystroke.
- No new dependency (SVG rendering is GPUI's).

## Exit criterion

1. Every test above is green; fmt, clippy and the workspace tests pass.
2. The four screenshots exist; the report has the VS Dark one beside the Visual Studio screenshot the owner gave
   (`docs/briefs/0061-run/visual-studio-reference.png`, copied from the owner's upload) and notes every difference left.
3. `docs/briefs/README.md` has this brief's row; CLAUDE.md's `crates/ui` row names the icon set.

## Out of scope

- A toolbar (Back, Forward, Home, Sync with Active Document, Refresh, Collapse All, the filters): the owner does not
  want one. Sync with Active Document may return later as a command without a button. The multi-select of the tree.
- Icons in the document tabs, the Agents window, the Error List and the Git windows (a later brief moves them to the set).
- User-installable icon themes (PLAN.md 8's theme system, Phase 2 with extensions).
- Any change to the tree's model beyond `file_type` and the no-duplicates rule.
