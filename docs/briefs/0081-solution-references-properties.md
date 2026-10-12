# Brief 0081: Solution structure, references and properties

Status: in progress
Phase: 2 (PLAN.md section 10: on par with Visual Studio and Rider for .NET work)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 5, 12), 4.2 (the Workspace window, Solution Explorer's semantics), 4.6 (the project system), 8 (the default layout: Properties below the Workspace window)
Related ADRs: ADR-0002 (process topology: every MSBuild and solution file edit runs in `eludite-host`), ADR-0003 (protocols), ADR-0004 (project system), ADR-0014 (deterministic tests)
Depends on: brief 0012 (the tree), brief 0048 (the Dependencies node), brief 0049 (the formatting-preserving project editor, `Microsoft.Build.Construction`, and the solution file editor `SolutionConfigurationFile.cs`), brief 0063 (`.vbproj` and `.fsproj`). Vocabulary from brief 0054: user-facing labels say "workspace" for the opened directory; "Solution" only where the `.sln`/`.slnx` file itself is meant.

## Goal

A person who opens a solution sees it as Visual Studio shows it and can change its shape without leaving Eludite:

1. **Solution folders and solution items.** The Workspace window draws a `.slnx`'s `<Folder>`s and a `.sln`'s solution folders (nested by `NestedProjects`), each holding its projects, its sub-folders and its solution items (`<File>`, `ProjectSection(SolutionItems)`), with the solution folder icons.
2. **Editing the solution.** The solution node and a solution folder have Add > Existing Project..., Add > New Solution Folder and (a folder) Add > Existing Item...; a project has Remove (from the solution); a folder has Rename and Remove; a solution item has Remove. The host edits the solution file's text, keeping every other byte.
3. **The Properties window (F4).** A generic property grid (`crates/ui`) follows the Workspace selection: a file shows Visual Studio's File Properties (Build Action, Copy to Output Directory, Custom Tool, Custom Tool Namespace, File Name, Full Path), a project its name, path, target frameworks and kind, a solution folder its name (editable: Rename), the solution its path. Editable values are written by the host (`Update=` items for SDK default items, in place for explicit items) and the tree refreshes. Properties joins the default layout below the Workspace window.
4. **The Reference Manager.** Add > Project Reference... on a project and on its Dependencies node opens Visual Studio's dialog: a Projects tab of check boxes, a Browse tab for assemblies, and a COM tab saying COM references are not supported yet. OK sends the differences; a cycle is refused with its path. A project or assembly reference row has Remove.
5. **Dependencies depth.** Analyzers (Analyzer items and package analyzers, source generators among them) and Assemblies (direct `Reference` items) under Dependencies; a group per target framework for a multi-targeted project; a References node for legacy projects (assembly references, project references, `packages.config` packages).
6. **Unload Project / Reload Project.** A per-workspace unloaded set: an unloaded project is out of Roslyn's workspace and shows as `Name (unloaded)` with Reload Project and Edit Project File; Reload Project also re-evaluates a project that failed to load.
7. **Project Dependencies** (Build Order) through `eludite/solution/edit`'s `setDependencies` (`.sln` `ProjectDependencies`, `.slnx` `<BuildDependency>`), only after the rest.

## Files in scope

**Protocol (schemas first).**
- `protocol/schemas/host/solution-tree.json` (`folders`, `unloaded`, a project's `solutionFolder` and `unloaded`, the Dependencies node's `analyzers`, `assemblies`, `frameworkGroups`), new `solution-edit.json`, `project-item-properties.json`, `project-set-item-properties.json`, `project-edit-references.json`, `project-set-unloaded.json`; `protocol/schemas/host-rpc.md` (rows and a section); `protocol/rust/src/host.rs` (the mirror types, additive and marked), `protocol/rust/src/schema_tests.rs` (conformance tests).
- Command schemas `protocol/schemas/solution-*.input.json`/`.output.json` and `project-*.input.json`/`.output.json` for the commands below.

**Host (.NET).**
- `dotnet/src/Eludite.Host/Legacy/SolutionProjects.cs`: `ReadStructure` (folders, items, project folders) in functions of its own (brief 0072 edits `Read`).
- New `dotnet/src/Eludite.Host/Projects/SolutionEditor.cs` (text edits of `.sln` and `.slnx`), `ItemProperties.cs`, `ReferenceEditor.cs`, `UnloadedProjects.cs`; the Dependencies part of `MsBuildProjectTreeEvaluator.cs`; `SolutionTreeProvider.cs` (folders, unloaded projects); `Rpc/HostRpcTarget.cs` and `Rpc/Messages.cs` (additive); `Lsp/LspProxy.cs` only for the marked hook that opens the loaded projects instead of the solution when some are unloaded.
- Tests: `dotnet/tests/Eludite.Host.Tests/SolutionStructureTests.cs`, `SolutionEditorTests.cs`, `ItemPropertiesTests.cs`, `ReferenceEditorTests.cs`, `DependenciesTests.cs`, `UnloadTests.cs` (new).

**Rust.**
- `crates/commands/src/solution.rs` and `project.rs` (additive: the command ids, parse, specs and the target trait of this brief, or new modules `solution_edit.rs`/`project/references.rs` registered from them), `crates/commands/src/lib.rs` (a `mod` line) if a module is added.
- `crates/lsp/src/host.rs` re-exports if needed; `crates/lsp/src/fake.rs` and a new `crates/lsp/src/fake/structure.rs` (the fake's model of these methods, additive).
- `crates/workspace/src/explorer.rs`: model additions only (marked): `NodeKind::SolutionFolder`, `SolutionItem`, `Analyzer`, `Assembly`, `DependencyGroup::{Analyzers, Assemblies, Framework}`, the References label for legacy projects, unloaded projects, folders in `from_tree`.
- `crates/ui/src/property_grid.rs` (new, generic) and its `mod`/re-export line in `crates/ui/src/lib.rs`.
- `crates/eludite/src/shell/solution_edit.rs`, `properties_window.rs`, `reference_manager.rs` (new) and their `_tests.rs`; marked blocks in `crates/eludite/src/shell.rs` (mod lines, registration, the Properties tool body) and `crates/eludite/src/shell/explorer.rs` (menu entries, icons for the new kinds).
- `crates/docking/src/model.rs`/`persist.rs`: Properties below the Workspace window in the default layout (marked).

## Contract

**Host methods** (schemas are the contract; host-rpc.md summarizes):
- `eludite/solution/tree`: `folders` in solution order, each `{ path, name, files }`; a project's `solutionFolder`; `unloaded` projects listed with `unloaded: true`, no files and no dependencies. Dependencies gain `analyzers` and `assemblies` (`{ name, path?, include?, package? }`) and, for a project with two or more target frameworks, `frameworkGroups` (one per framework, from that framework's inner evaluation). Legacy projects get `dependencies` too (assemblies from `Reference` items with their `HintPath`, project references, `packages.config` packages as packages with `requested`).
- `eludite/solution/edit` (`addProject`, `removeProject`, `addFolder`, `renameFolder`, `removeFolder`, `addFile`, `removeFile`, `setDependencies`): text edits that keep every other byte; `.sln` project type GUIDs: `{9A19103F-16F7-4668-BE54-9A1E7A4F7556}` (SDK C#), `{FAE04EC0-301F-11D3-BF4B-00C04F79EFBC}` (legacy C#), `{778DAE3C-4631-46EA-AA77-85C1314464D9}` (VB SDK), `{F184B08F-C81C-45F6-A57F-5ABD9991F28F}` (VB legacy), `{6EC3EE1D-3C4E-46DD-8F32-0CC8E7565705}` (F#); solution folders `{2150E333-8FDC-42A3-9474-1A3956D46DE8}`. A new `.sln` project's configuration lines map every solution configuration to the project's configuration of the same name if it is Debug or Release (else Debug) with `Any CPU`. A no-op answers `written: false` with `message`.
- `eludite/project/itemProperties` / `setItemProperties`: Visual Studio's Build Action list (`None`, `Compile`, `Content`, `EmbeddedResource`, `AdditionalFiles`, `Page`, `Resource`, `ApplicationDefinition`, `CodeAnalysisDictionary`, `PRIResource`, `Analyzer`, plus the project's own item types), Copy to Output Directory as `never`/`always`/`preserveNewest` (`Never`, `Always`, `PreserveNewest` metadata; `never` removes the element). Writes keep formatting (brief 0049's editor rules).
- `eludite/project/editReferences`: add/remove `ProjectReference` (relative Include with `\` separators, Visual Studio's spelling) and `Reference` (by name, or by path with `HintPath`); cycles refused with `reason` naming the chain (`cycle: A -> B -> A`).
- `eludite/project/setUnloaded`: the unloaded set; Roslyn gets `project/open` with the loaded C#/VB projects when the set is non-empty.
- Every write: generation check first (-32801), serialized under the evaluation lock, the solution reloaded after a write (`LspProxy.AdvanceGeneration` or a reopen), stale answers dropped by the shell.

**Commands** (schemas in `protocol/schemas/`, agent-visible, typed results):
- `eludite.solution.add_project` `{ project, folder? }`, `eludite.solution.remove_project` `{ project }`, `eludite.solution.add_folder` `{ name?, parent? }` (no name: Visual Studio's `NewFolder1`, `NewFolder2`...), `eludite.solution.rename_folder` `{ folder, name }`, `eludite.solution.remove_folder` `{ folder }`, `eludite.solution.add_item` `{ folder, path }`, `eludite.solution.remove_item` `{ folder, path }`, `eludite.solution.set_project_dependencies` `{ project, depends_on }`: execute; output `{ path, written, folder?, message?, generation }`. From the UI without `project`/`path`, Add Existing Project... and Add Existing Item... open the file dialog first.
- `eludite.project.item_properties` `{ project?, path }` (read) and `eludite.project.set_item_properties` `{ project?, path, build_action?, copy_to_output?, custom_tool?, custom_tool_namespace? }` (execute).
- `eludite.project.add_reference` `{ project, reference, kind? }`, `eludite.project.remove_reference` `{ project, reference, kind? }` (execute; output lists `refused`), `eludite.project.reference_manager` `{ project? }` (opens the dialog; UI).
- `eludite.project.unload` and `eludite.project.reload` `{ project }` (execute).
- `eludite.view.show` `properties` (F4) shows the Properties window.

**Shell.**
- Workspace draws solution folders (`folder-solution` icons, open and closed) before projects at each level, sorted case-insensitively, and solution items as files under their folder. Node ids: `<solution>|sf|/src/tests/`, `<solution>|sf|/src/|item|<path>`.
- The Properties window: the property grid (categories, name and value columns, a read-only value greyed, an enum value as a drop-down, a text value as an inline text box committed on Enter or focus loss, a description pane at the bottom). It follows the Workspace selection; a file's values come from `eludite/project/itemProperties` off the UI thread, keyed by (project, path, generation): an answer for an older selection or generation is dropped. An edit sends `eludite.project.set_item_properties` and the grid shows the new generation's values.
- Reference Manager: a modal dialog over the window with Projects (every other project of the solution with a check box, checked when referenced), Browse (the assemblies referenced by path, with Browse... opening the file dialog), COM (Windows only, "COM references are not supported yet."); OK sends one `eludite/project/editReferences` with the differences; refused references go to the status bar.
- Unloaded projects persist per workspace beside the configuration selection and are sent to the host after the solution opens.

## Proving test

- `dotnet test dotnet/tests/Eludite.Host.Tests` with the real SDK: `SolutionStructureTests` (folders, nested folders and items from a `.sln` and a `.slnx`; projects' folders), `SolutionEditorTests` (add/remove project, add/rename/remove folder, add/remove item, set dependencies on `.sln` and `.slnx` fixtures: the result is the original text with exactly the expected lines added or removed; CRLF and a BOM kept), `ItemPropertiesTests` (Build Action and Copy to Output read and written on an SDK project (an `Update` item for a default item) and a legacy project (in place), byte-preserving outside the edited lines), `ReferenceEditorTests` (a project reference added with Visual Studio's relative spelling; a cycle refused; an assembly reference with `HintPath`; removal), `DependenciesTests` (Analyzers and Assemblies listed; per-TFM groups; a legacy project's references and `packages.config`), `UnloadTests` (an unloaded project is listed unloaded; with the fake language server, `project/open` names the other projects and not the unloaded one).
- `cargo test -p eludite-protocol` (the new messages conform), `cargo test -p eludite-commands` (parse and specs), `cargo test -p eludite-workspace` (folders, items, unloaded, the new dependency groups and the legacy References label), `cargo test -p eludite-ui property_grid`, `cargo test -p eludite-docking` (the default layout and its migration), `cargo test -p eludite solution_edit_tests properties_window_tests reference_manager_tests` (headless with the fake host: folders drawn; the menus run the commands and the host gets the edits; the Properties grid shows a file's properties and an edit sends `setItemProperties`; a stale answer is dropped; Reference Manager check boxes send the differences; unload and reload).

## Budget

- The Properties grid redraws within one frame of a selection change: values are fetched off the UI thread, the grid shows what it has (the tree's name and path at once) and stale answers are dropped (asserted with `assert_budget`).
- No regression of the tree's load time (brief 0012's 0.5 s for `Eludite.slnx`): reading the structure is one more pass over the solution file's text.

## Exit criterion

The proving tests pass here and on CI; a `.slnx` with folders shows them; Add Existing Project, New Solution Folder, Remove, Unload/Reload, Reference Manager and the Properties window's Build Action edit work against the real host.

## Out of scope

COM references (the tab says so), shared projects (`.shproj`) and `.projitems`, solution filters (brief 0072), Add New Project and Add New Item (brief 0072), Workspace operations such as rename, delete, drag and drop of files and Edit Project File's implementation (brief 0073; this brief only adds the menu entry where 0073's command exists), the Properties window for non-.NET nodes beyond name and path, Build Order's Configuration Manager integration beyond writing the dependencies.
