# Brief 0073: The Workspace window as a working tool

Status: open
Phase: 2 (PLAN.md section 10: "on par with Visual Studio for .NET work"; issued by the owner's direct request, the way proposals 0004 and 0005 were accepted)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 5, 12), 4.2 (the Workspace window, Visual Studio's Solution Explorer), 4.3, 5.1 (every action a command), 9 (budgets)
Related ADRs: ADR-0002 (MSBuild stays in `eludite-host`), ADR-0003 (protocols), ADR-0014 (tests fail on behavior only)
Depends on: brief 0012 (the tree from `eludite/solution/tree`), 0015 (the workspace-edit applier and its rules), 0019 (opened folders), 0040 (the polling watcher pattern), 0048 (Dependencies node), 0049 (the formatting-preserving project editor and the reload after a project write), 0062 (the window's look, icons and search), 0063 (VB and F# projects).

## Goal

The Workspace window stops being a read-only view. Every node has Visual Studio's context menu, built from one table so later briefs add an item with one line; files and folders are renamed in place (F2), deleted to the OS trash (Del) after a confirmation, cut, copied, pasted (Ctrl+X, C, V) and dragged (Ctrl-drag copies), with open tabs following and the project file kept right for projects that list their files; New Folder adds a folder with an inline name box; the arrow keys, Enter, `*`, type-ahead, Shift+F10 and Ctrl/Shift multi-select work as in Visual Studio; a polling watcher keeps the tree current when files change outside Eludite and has the host re-evaluate when a project, `.props`, `.targets` or solution file changes; the host lists every visible item type (None, EmbeddedResource, Content, Folder and the rest), so `appsettings.json`, `Properties/launchSettings.json`, a library's `.resx` and a Razor class library's `.razor` files appear, nested by Visual Studio's default file-nesting rules (with a toggle); Show All Files shows what is on disk but not in the project, with Include In Project and Exclude From Project written by the host; Edit Project File, Collapse All, Sync with Active Document (Ctrl+[, S), Copy Full Path and Copy Relative Path; F# projects keep their compile order with Move Up and Move Down; VB and F# projects and files have their own icons.

## Files in scope

- `crates/workspace/src/explorer.rs`: folder nodes carry their folder's path; `NodeKind::Excluded { dir }` (Show All Files); the widened item types; Visual Studio's default nesting rules (`nest_by_name`, a nesting switch); F# compile order (no sort for `.fsproj` files); [`SolutionModel::with_disk_files`] (merges a project folder listing for Show All Files) and [`SolutionModel::unnested`]; tests.
- `crates/workspace/src/folder.rs`: only the listing functions (a listing of several project folders for Show All Files, including `bin` and `obj`); brief 0072 owns the project-only-folder fallback in this file.
- `crates/eludite/src/shell/explorer.rs`, `crates/eludite/src/shell/explorer_tests.rs`: the menu table, keys, multi-select, inline rename and New Folder, cut/copy/paste, drag and drop, toggles, Copy Path, Sync.
- `crates/eludite/src/shell/workspace_files.rs` (new, + `workspace_files_tests.rs`): the file operations (rename, move, copy, delete, new folder, include, exclude, F# order) behind the commands, with the applier's rules.
- `crates/eludite/src/shell/workspace_watch.rs` (new, + tests): the polling watcher and the refresh it drives.
- `crates/eludite/src/shell/session.rs`: marked blocks for the two new host requests.
- `crates/eludite/src/shell.rs`: one marked block each for the mod lines, the service registration, `run`'s hook and `invoke`'s staging.
- `crates/ui/src/tree.rs`: multi-select drawing, an inline-edit slot, the drop-target highlight.
- `crates/ui/icons/` and `crates/ui/src/icons.rs` (additive): `project-vb.svg`, `project-fsharp.svg`, `file-vb.svg`, `file-fs.svg`, `file-excluded.svg`.
- `crates/commands/src/workspace_files.rs` (new) and its `pub mod` line in `crates/commands/src/lib.rs`; `protocol/schemas/workspace-files-*.json` (new).
- `protocol/schemas/host/solution-tree.json` (the `file.itemType` enum and `msbuildItemType` only; brief 0081 owns the solution-folder additions), `protocol/schemas/host/project-edit-items.json` and `protocol/schemas/host/solution-reload.json` (new), their rows in `protocol/schemas/host-rpc.md`, `protocol/rust/src/host.rs` (the mirror types, additive), `crates/lsp/src/fake*.rs` (the fake host answers the two new methods; additive).
- `dotnet/src/Eludite.Host/Projects/MsBuildProjectTreeEvaluator.cs` (the item listing only), `dotnet/src/Eludite.Host/Projects/ProjectItemEditor.cs` and `ProjectItemsService.cs` (new), one marked block in `dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs`, `dotnet/tests/Eludite.Host.Tests/ProjectItemsTests.cs` (new) and `SolutionTreeTests.cs` (additions).
- `docs/briefs/0073-report.md`, this file.

Nothing else. `README.md`, `CLAUDE.md`, `docs/briefs/README.md` and `docs/PLAN.md` are the integrator's: the report's Integration notes hold their lines.

## Contract

**The menu table.** `explorer.rs` has one `MENU: &[MenuEntry]` table: `MenuEntry { id, label, nodes: &[MenuNode], group, when, command }`, where `MenuNode` is `Workspace`, `Solution`, `SolutionFolder`, `Project`, `CargoPackage`, `Folder`, `File`, `Dependencies`, `Package`, `Reference`; `group` orders the items and puts a separator between groups; `when(&MenuContext) -> bool` hides an item that does not apply (a Git item outside a repository, Show All Files' check mark, F# Move Up); `command(&MenuContext) -> Option<(&'static str, Value)>` is what the item runs through the bus (`None`: handled by the window, such as Rename's inline box). Another brief adds an item with one line in the table, inside a block comment naming the brief. The menus, in Visual Studio's order:

- Workspace root: Show All Files (check), File Nesting (check), Collapse All, Sync with Active Document, Refresh; New Folder; Open Folder in File Explorer, Open in Terminal; Copy Full Path.
- Solution: Build/Rebuild/Clean stay brief 0075's; Manage NuGet Packages for Solution...; Collapse All; Open Folder in File Explorer, Open in Terminal; Copy Full Path.
- Project (.NET): Build, Rebuild, Clean; Manage NuGet Packages...; Set as Startup Project; Edit Project File; New Folder; Paste; Rename; Open Folder in File Explorer, Open in Terminal; Copy Full Path, Copy Relative Path; Properties.
- Cargo package: Build, Rebuild, Clean; Set as Startup Project; Open Folder in File Explorer, Open in Terminal; Copy Full Path.
- Folder: New Folder; Cut, Copy, Paste, Delete, Rename; Exclude From Project (in a project) or Include In Project (an excluded folder); Open Folder in File Explorer, Open in Terminal; Copy Full Path, Copy Relative Path.
- File: Open; Open With XML (Text) Editor (`.resx`); Cut, Copy, Delete, Rename; Exclude From Project / Include In Project; Move Up, Move Down (F# compile items); Git items (brief 0040, in a repository); Open Containing Folder; Copy Full Path, Copy Relative Path.
- Dependencies and Packages node: Manage NuGet Packages...; a package: Update, Remove, Manage NuGet Packages....
- Project reference: Remove (later briefs), Copy Full Path.

Shift+F10 and the Menu key open the selected row's menu at the row. With several rows selected the menu holds the items that apply to every selected row and take several paths (Cut, Copy, Delete, Exclude From Project, Include In Project).

**Commands** (`crates/commands/src/workspace_files.rs`, schemas `protocol/schemas/workspace-files-<name>.input.json` and one shared `workspace-files.output.json`, agent-visible; reads `read`, writes `edit_disk`):

| Id | Input | What it does |
|---|---|---|
| `eludite.workspace.rename` | `path`, `name` (a file name, no separators) | Renames a file or folder in place. |
| `eludite.workspace.move` | `paths` (1 to 1000), `to` (a folder) | Moves files and folders into `to`. |
| `eludite.workspace.copy` | `paths`, `to` | Copies them; a name that exists becomes `Name - Copy.ext` (Visual Studio's rule), then `Name - Copy (2).ext`. |
| `eludite.workspace.delete` | `paths`, `permanent` (default false) | To the OS trash, or removed for good with `permanent`. |
| `eludite.workspace.new_folder` | `parent` (a folder or a project file), `name` | Creates the folder; in an SDK project it also adds `<Folder Include="name\" />` so it shows while empty. |
| `eludite.workspace.include` | `paths` | Include In Project (the host edits the project). |
| `eludite.workspace.exclude` | `paths` | Exclude From Project. |
| `eludite.workspace.fsharp_move` | `path`, `direction` (`up`, `down`) | Moves an F# file's `Compile` item one place in the project file. |
| `eludite.workspace.view` | `showAllFiles`, `fileNesting`, `trackActiveItem` (each optional bool), `collapseAll`, `syncActive`, `refresh` (each optional bool) | Sets the window's switches and runs its actions; answers the switches. |
| `eludite.workspace.copy_path` | `path`, `relative` (default false) | Puts the full path, or the path relative to its project's (else the workspace's) folder, on the clipboard and answers it. |
| `eludite.workspace.edit_project_file` | `project` | Opens the project file in the text editor. |

The write commands share one output: `applied`, `paths` (each `{from, to}` or `{path}`), `projectEdits` (project files the host changed), `openDocuments` (tabs that followed), `message`. The rules are the applier's (brief 0015): all or nothing (every source exists, no target exists, no target inside its own source, no open document with unsaved changes among the touched files, else nothing changes and the answer says why), the disk work off the UI thread, open tabs follow a rename or move at the same caret and close on a delete, `workspace/didChangeWatchedFiles` to every server. Files are moved with `std::fs::rename` (a copy and delete across file systems), so bytes, binary files and folders survive unchanged; the applier's text path rewrites contents through a buffer and refuses folders, so it is not used for these. A person's Delete asks first ("'Name' will be deleted." with OK and Cancel; several rows: "The N selected items will be deleted."); an agent's call does not ask (the `edit_disk` class and the agents' policy decide). Trash: freedesktop.org's trash specification on Linux (`$XDG_DATA_HOME/Trash`, or the volume's `.Trash-$UID`), the Recycle Bin on Windows through PowerShell's `Microsoft.VisualBasic.FileIO.FileSystem` (`SendToRecycleBin`), the Finder's trash on macOS through `osascript` (no new dependency on either); a file that cannot be trashed is not deleted and the answer says so.

**Renaming a C#, VB or F# file** whose name matches a type it declares (`Foo.cs` declaring `class Foo`) asks Visual Studio's question "You are renaming a file. Would you also like to perform a rename in this project of all references to the code element 'Foo'?" (Yes/No); Yes runs `eludite.editor.rename` at the declaration after the file rename. An agent's `eludite.workspace.rename` takes `renameSymbol` (default false). **Moving a `.cs` file** to another folder of its project offers nothing in this brief (Roslyn's Sync Namespace code action is out of scope; see Out of scope).

**Project files.** A rename, move or delete in a project that lists files explicitly (legacy projects, F# projects, and SDK projects with explicit items or `Remove`/`Update` elements naming the path) is followed by `eludite/project/editItems` with `rename` or `remove` for each project, before the shell reloads the tree; nothing is written when the project has no element naming the path (SDK globs pick the change up on re-evaluation).

**Host methods** (schemas first: `protocol/schemas/host/project-edit-items.json`, `solution-reload.json`, rows in `host-rpc.md`):

- `eludite/project/editItems` `{project, edits: [{op, path, to?, itemType?}]}` → `{changed, generation, edits: [{op, path, applied, message?}]}`. Ops: `include` (SDK: drop a `Remove` that excludes it, else add `<T Include>`; legacy: add the item; `T` is `itemType` or by extension: `.cs`/`.vb`/`.fs` Compile, `.resx` EmbeddedResource, web content Content, else None; a folder includes its files), `exclude` (SDK: `<T Remove="rel" />` for a globbed item or delete an explicit element; legacy: delete the element; a folder excludes `rel\**`), `rename` (every `Include`, `Remove`, `Update` and `DependentUpon` naming `path`, or a path under it, rewritten to `to`), `remove` (every element naming the path, or a path under it, deleted), `moveUp`/`moveDown` (an F# `Compile` element swapped with the previous or next `Compile` element), `addFolder` (`<Folder Include="rel\" />` in an SDK project, nothing when items exist under it). Paths are written project-relative with `\` as Visual Studio writes them. Edits keep the file's formatting (the construction model with `PreserveFormatting`, as brief 0049's `ProjectFileEditor`); when anything changed the host reloads the solution (generation bump) like a property write.
- `eludite/solution/reload` `{projectFiles}` → `{generation}`: `projectFiles: true` (a project, `.props`, `.targets`, `Directory.*.props`, `.sln` or `.slnx` changed) reopens the solution (the language server reloads it, generation bump); `false` (files were added or removed under a project folder) advances the generation so the next `eludite/solution/tree` re-evaluates the globs. The shell asks for the tree after either.

**The tree's items.** `eludite/solution/tree` lists `Compile`, `Content`, `None`, `EmbeddedResource`, `Folder` and every item type the project names in `AvailableItemName` (`Page`, `Resource`, `AdditionalFiles`, `Protobuf`, ...), in evaluation order, skipping items with `Visible` `false`, items under the project's `bin/` and `obj/`, and duplicates (the first item type wins). `file.itemType` is `compile`, `content`, `none`, `embeddedResource`, `folder` or `other`, with `msbuildItemType` naming the item type for `other`. A `Folder` item is a folder node even when empty.

**Nesting** (Visual Studio's default `.filenesting.json`, by name in one folder, `DependentUpon` first): `X.designer.cs`/`.vb` under `X`, `X.cs` or `X.resx`; `A.b.c` under `A.b` (fileSuffixToExtension: `Default.aspx.cs`, `Index.razor.cs`, `Index.razor.css`, `Index.cshtml.cs`); pathSegment: `appsettings.Development.json` under `appsettings.json` (`X.*.ext` under `X.ext`, also `web.Debug.config`); extensionToExtension: `X.js` under `X.ts`, `X.js.map` under `X.js`, `X.min.js` under `X.js`, `X.css` under `X.scss`/`X.less`, `X.min.css` under `X.css`, `X.css.map` under `X.css`; fileToFile: `package-lock.json`, `.npmrc`, `yarn.lock` and `pnpm-lock.yaml` under `package.json`, `bower.json`'s `.bowerrc`, `bundleconfig.json`'s `bundleconfig.json.bindings`, `libman.json`'s nothing. The File Nesting switch (on by default) shows every file at its folder instead. Nesting applies in project trees and in plain-folder trees.

**F#.** An `.fsproj`'s files keep their evaluation (compile) order, folders where their first file is; nothing nests by name. Move Up and Move Down on an F# compile item call `eludite.workspace.fsharp_move`.

**The watcher** (`workspace_watch.rs`): one thread per open workspace that every `poll` (1 s; tests 20 ms) stats the workspace folder (and each project folder outside it) recursively, skipping `bin`, `obj`, `.git`, `.vs`, `node_modules`, `target` and hidden folders, comparing names, sizes, modification times and kinds with the last scan; a change is reported once `debounce` (300 ms; tests 20 ms) passes with no further change. It reports `{files: added/removed paths, project_files: bool, under_projects: bool}` to the UI thread, which re-lists a plain folder's files (off the UI thread), calls `eludite/solution/reload` (`projectFiles` true for project, `.props`, `.targets`, `.sln`, `.slnx` changes; false when files were added or removed under a project folder), then `eludite/solution/tree`. A content change of an ordinary file does nothing (brief 0070 reloads open documents). Writes the window itself makes are not reported twice: the commands refresh the tree themselves and mark their paths for the watcher to skip in its next report. The watcher stops at close. Expanded rows and the selection are kept across refreshes by id; a vanished selected row selects its parent.

**Show All Files** lists each project folder's files and folders, `bin` and `obj` included (hidden folders skipped), off the UI thread, merging the ones that are not items as `Excluded` nodes, drawn muted with the excluded icon; it is per workspace, off by default.

**Keys** (window focused, rows not being renamed): Up/Down move the selection, Shift extends it; Left collapses an expanded node, else goes to its parent; Right expands, else goes to the first child; Home/End; Enter opens a file (a project: expands); `*` (and `shift-8`) expands the node's subtree; F2 renames; Delete deletes (asks); Ctrl+X/C/V cut, copy, paste (into the selected folder or the selected file's folder); Shift+F10 and the Menu key open the menu; typing letters selects the next row whose label starts with them (1 s reset); Ctrl+click toggles a row, Shift+click selects a range. Ctrl+[, S (shell-wide) is Sync with Active Document. Escape cancels an inline edit or a cut.

**Inline edit.** F2 or Rename puts a `TextInput` over the row's label holding the name with the stem selected; Enter commits through `eludite.workspace.rename`, Escape or losing focus cancels (Visual Studio commits on blur; Eludite commits on blur too unless the name is unchanged). New Folder adds a row "NewFolder" (then "NewFolder1", ...) in edit mode under the target and commits through `eludite.workspace.new_folder`.

**Drag and drop.** Dragging file and folder rows (the selection when the dragged row is selected) onto a folder, project or workspace row moves them there (`eludite.workspace.move`); with Ctrl held at the drop, copies. Dropping onto a file row drops into its folder. A row cannot be dropped into itself or its own descendant (no highlight). The target row is highlighted while hovered.

**Expanded state** is kept per workspace in `.eludite/workspace-window.json` (expanded ids, Show All Files, File Nesting), written at most once a second off the UI thread and read when the workspace opens.

**Icons.** `project-vb.svg`, `project-fsharp.svg` (the C# project's shape with the language's letters), `file-vb.svg`, `file-fs.svg`, `file-excluded.svg` (a dotted outline) in the style of brief 0062; `Icon::ProjectVb`, `ProjectFSharp`, `FileVb`, `FileFs`, `FileExcluded`. `FileType` gains `VisualBasic` (`.vb`) and `FSharp` (`.fs`, `.fsi`, `.fsx`, `.fsscript`).

## Proving test

- `cargo test -p eludite-workspace explorer`: folder nodes have paths; every nesting rule; the nesting switch; F# order; `with_disk_files` marks non-items `Excluded`; the widened item types; a `Folder` item makes an empty folder node.
- `cargo test -p eludite-commands workspace_files`: parsing, validation and schema conformance for every command.
- `cargo test -p eludite explorer_tests workspace_files_tests workspace_watch`: headless against the fake host: the menu of each node kind (items and order); Shift+F10; F2 rename through the inline box with an open tab following; Delete asks and deletes (trash in a temporary `XDG_DATA_HOME`); cut and paste moves; Ctrl+C/V copies with `- Copy`; drag and drop moves and Ctrl copies (GPUI's drag simulation); arrows, Left/Right, Home/End, `*`, type-ahead; Ctrl and Shift multi-select and a multi-row delete; New Folder; the watcher reporting an external new file in a plain folder and in a project folder, and a `.csproj` edit calling `eludite/solution/reload` with `projectFiles` (20 ms poll); Show All Files' muted rows and Include In Project; Sync with Active Document; Copy Full Path; Collapse All; F# order and Move Up through the fake host; the rename-the-type question answered Yes calling `textDocument/rename`.
- `dotnet test dotnet/tests/Eludite.Host.Tests --filter "ProjectItems|SolutionTree"`: an SDK console fixture with `appsettings.json`, `appsettings.Development.json`, `Properties/launchSettings.json`, a `.resx`, an empty `<Folder>` and a `Visible="false"` item lists None, EmbeddedResource and Folder items and skips the invisible one; include and exclude in SDK and legacy projects byte-for-byte against expected files; a legacy `Compile` rename and remove; F# move up and down; `addFolder`; `eludite/solution/reload` bumps the generation and the next tree sees a new file.

## Budget

- A tree of 20,000 rows: `visible_rows` with everything expanded and the window's frame (rows drawn by the virtualized list) within the frame budget (`assert_budget`, 8 ms for the rows' model work).
- The watcher on a 10,000-file workspace: one scan's cost reported (`assert_budget` style, reported on CI); idle CPU under 1 percent at the 1 s poll (scan time / poll interval).
- No network; nothing at startup beyond reading `.eludite/workspace-window.json`; the UI thread never waits on a scan, a file operation or the host.

## Exit criterion

1. The proving tests pass; `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` on every crate touched, `cargo test -p` for each, `dotnet build dotnet/Eludite.slnx` and the host tests touched are green here.
2. The report lists what was built, the numbers, what was not run, and the Integration notes (the crate map rows, the README status line and the briefs index row).

## Out of scope

- Add > New Item, Existing Item, New Project and Project Reference (briefs 0072, 0081), Publish (0074), the root's Build items (0075), Run/Debug Tests (0076), Unload/Reload project and the Properties window (0081), Manage User Secrets (0082); this brief leaves table lines for them.
- Solution folders and solution items (0081), reloading open documents changed on disk (0070).
- Roslyn's Sync Namespace code action on a move, and moving a type to its own file; F# Add Above and Add Below (they need 0072's Add New Item dialog).
- `notify` or any file system event API (polling only, as briefs 0040 and the settings store do); new dependencies.
