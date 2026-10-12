# Brief 0072: The start experience and templates

Status: in progress
Phase: 2 (PLAN.md section 8: "New Project backed by dotnet new templates"; the owner's request of 2026-10-11 to bring Eludite on par with Visual Studio and Rider for .NET work, which accepts this brief)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 5, 12), 4.2 (the Workspace window, the project system), 4.6, 8 (the shell: Welcome, File menu, New Project), 9 (budgets: no network at startup, cold start), 14 (decision 12: the workspace is a folder)
Related ADRs: ADR-0002 (process topology: `dotnet new` and `dotnet sln` run out of process in `eludite-host`'s children), ADR-0003 (no bespoke hooks)
Depends on: brief 0019 (folders as workspaces, `eludite.workspace.open_folder`), brief 0049 (the formatting-preserving project edit), brief 0063 (`.csproj`, `.vbproj`, `.fsproj` alike). Runs beside briefs 0065 to 0082; 0073 owns the Workspace window's context menu structure, 0070 owns Open File (Ctrl+O), New File (Ctrl+N) and `app.rs`, 0078 owns the menu mechanics (submenus) and `main.rs`, 0080 adds an info bar on the Welcome page through the hook this brief leaves, 0081 adds solution folders to `SolutionProjects.cs`. Brief 0054's vocabulary holds: the person opens a folder as a workspace; "solution" names only a .NET file.

## Goal

A person starting Eludite can reach any .NET code the way Visual Studio and Rider allow, and create new code from the SDK's templates:

1. **Any .NET entry point opens.** A folder with no solution file but with project files (the `dotnet new console` layout) loads them: one project directly, several through a solution file Eludite generates for the workspace. File > Open Project/Solution... opens a `.sln`, `.slnx`, `.slnf` or project file as the workspace of its folder with that file chosen. `.slnf` (solution filter) is understood by the host. `eludite <path>` opens a folder, a solution or project file, or a file at `file:line:col`; `eludite --version` prints the version. The Welcome page and the empty tree stop promising a menu item that does not exist.
2. **Recent workspaces.** A per-user list of recent workspaces (folders, and solution or project files) and recent files, written on open, read in under 5 ms. The Welcome page takes Visual Studio 2022's Start window shape: "Open recent" with search, pin and remove, and "Get started" (Open a local folder, Open a project or solution, Clone a repository, Create a new project, Continue without code). The setting `workspace.reopenLast` (off) reopens the last workspace at startup after the first frame.
3. **New Project.** File > New Project... (Ctrl+Shift+N) opens Visual Studio's two-page dialog over the installed `dotnet new` templates: search, Language / Platform / Project type filters, recent templates, the list; then "Configure your new project" (name, location, solution: create new / add to the current one, framework and the template's main options). The host runs `dotnet new` and `dotnet sln add` out of process; the shell opens the result and the new project's main file.
4. **Add New Item / Add Existing Item** on project nodes (and folder nodes once brief 0073's folder menus exist; Ctrl+Shift+A / Shift+Alt+A on the Workspace window's selection): the item templates of the SDK plus Eludite's built-in Class, Interface, Record, Enum and Struct (C#) and Class, Interface, Module, Enum, Structure (Visual Basic), with the namespace Visual Studio would give (RootNamespace plus the folder path); Add Existing Item copies or links files into the folder, and writes `Compile`/`Content` items into legacy (explicit-include) projects with the formatting-preserving editor.

Every action is a command an agent can call: `eludite.project.templates`, `eludite.project.new`, `eludite.project.add_item`, `eludite.workspace.recent`, `eludite.workspace.recent_edit`, `eludite.workspace.open_project`.

## Files in scope

Schemas first (CLAUDE.md invariant 4), in the same commit as or before the code on either side:

- `protocol/schemas/host/templates-list.json`, `templates-create.json`, `project-add-item.json`, `project-add-existing.json` (new); `protocol/schemas/host/solution-open.json` (`path` names `.slnf`); `protocol/schemas/host-rpc.md` (a "Templates and items (brief 0072)" section, the `.slnf` line).
- `protocol/schemas/project-templates.{input,output}.json`, `project-new.{input,output}.json`, `project-add-item.{input,output}.json`, `workspace-recent.{input,output}.json`, `workspace-recent-edit.{input,output}.json`, `workspace-open-project.{input,output}.json` (new); `workspace-open-folder.input.json` (`path` may be a solution or project file); `solution-open.input.json` (`.slnf`).
- `protocol/rust/src/host.rs` (additive block: the four methods' types), `protocol/rust/src/schema_tests.rs` (additive test).

Host (`dotnet/`):

- `dotnet/src/Eludite.Host/Templates/` (new): `TemplateCatalog.cs` (reads the template packages), `TemplateService.cs` (`eludite/templates/list`, `eludite/templates/create`), `ItemService.cs` (`eludite/project/addItem`, `eludite/project/addExisting`), `BuiltInItems.cs` (Eludite's own item templates), `ProjectNamespace.cs` (RootNamespace plus folders).
- `dotnet/src/Eludite.Host/Legacy/SolutionProjects.cs`: `.slnf` in its own functions (`ReadFilter`), the projects of a solution filter.
- `dotnet/src/Eludite.Host/Lsp/LspProxy.cs`: `.slnf` accepted; Roslyn gets `project/open` with the filter's projects (one line in `SupportedExtensions`, one branch in the load).
- `dotnet/src/Eludite.Host/Projects/`: the item-write path for legacy projects (a `ProjectItemWriter.cs`, new, on the existing construction-model editor).
- `dotnet/src/Eludite.Host/Rpc/HostRpcTarget.cs` (additive: the four methods).
- `dotnet/tests/Eludite.Host.Tests/TemplatesTests.cs`, `ItemTests.cs`, `SolutionFilterTests.cs` (new).

Shell and crates:

- `crates/workspace/src/folder.rs`: `find_projects` (the project-file fallback) and `projects_slnx` (the generated solution's text).
- `crates/eludite/src/shell/folder.rs`: the fallback, a solution or project file as `open_folder`'s path, the recent list written on open.
- `crates/eludite/src/shell/target.rs`: `.slnf`.
- `crates/eludite/src/shell/welcome.rs`: the Start window shape; one marked hook for brief 0080's info bar.
- `crates/eludite/src/shell/recent.rs` (new): the recent list (load, save, pin, remove, search), its commands.
- `crates/eludite/src/shell/new_project.rs`, `crates/eludite/src/shell/new_item.rs` (new): the dialogs and the commands' shell side.
- `crates/eludite/src/shell/start_tests.rs` (new): the headless tests.
- `crates/eludite/src/shell/session.rs`: one marked addition, `ServerSession::start` (starts the host without a solution, for the template list).
- `crates/eludite/src/shell.rs`: `mod` lines, registration, dispatch and the dialogs' overlay, each in a block marked "brief 0072".
- `crates/eludite/src/shell/explorer.rs`: the project node's "Add >" items as one marked block (0073 owns the menu structure).
- `crates/eludite/src/args.rs`: the positional path and `--version`; `crates/eludite/src/main.rs` (one marked arm for `--version`); `crates/eludite/src/app.rs` (one marked change: the positional path's line and column passed to `eludite.file.open`).
- `crates/commands/src/start.rs` (new; its `mod` line in `lib.rs`): the commands' ids, parsing, typed outputs and registration.
- `crates/lsp/src/fake.rs` (additive): the fake host answers the four new methods.
- `crates/ui/src/menu.rs` (`vs_menus`, additive: File > New Project..., Open Project/Solution..., Recent Workspaces; Workspace menu block), `crates/ui/src/keymap.rs` (`vs_keymap`, additive: Ctrl+Shift+N, Ctrl+Shift+A, Shift+Alt+A).
- `crates/commands/src/settings.rs` (or wherever keys are listed): `workspace.reopenLast`.
- `docs/briefs/0072-report.md`, this file.

Nothing in `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, `docs/PLAN.md` (the integrator applies the report's Integration notes).

## Contract

**Opening .NET entry points.**
- `find_solution` is unchanged (an `.slnx` before an `.sln`, at the root then one folder down). When it finds none, `find_projects(root)` lists the `.csproj`, `.vbproj` and `.fsproj` files at the root, else in the folders directly below it (hidden folders and `SKIPPED_FOLDERS` skipped), sorted by path. One project: it is the .NET part, opened in the host as a project file is today. Several: the shell writes `<root>/.eludite/workspace.slnx` (an `.slnx` with a `<Project Path="../<relative>" />` per project, rewritten only when its text changes; `.eludite/` already holds the workspace's settings and forge cache) and opens that. The Workspace window, builds, F5 and tests then work as for a solution.
- `eludite.workspace.open_folder`'s `path` may also be a `.sln`, `.slnx`, `.slnf`, `.csproj`, `.vbproj` or `.fsproj`: its folder opens as the workspace with that file as the .NET part (no search). Its output's `solution` names the file.
- `eludite.workspace.open_project` (File > Open Project/Solution..., no shortcut: Ctrl+Shift+O stays Open Workspace per brief 0019) shows a file dialog from the UI and runs `open_folder` with the chosen file; with `path` it validates the extension and does the same.
- The host's `SolutionProjects.Read` reads a `.slnf` (`{"solution": {"path": "<relative .sln/.slnx>", "projects": ["<relative to the solution's folder>"]}}`): the filter's projects that the solution lists, in the filter's order. `eludite/solution/open` accepts `.slnf`; Roslyn gets `project/open` with those projects. MSBuild builds a `.slnf` itself.
- `eludite <path>`: a folder or a solution/project file becomes `--folder`; any other existing file becomes `--open-file`, with `:line` and `:line:col` suffixes split off when the full name does not exist; a missing path is an error. `--version` prints `eludite <version>` and exits.
- `welcome::HINT` and the empty-tree text name File > Open Workspace... (Ctrl+Shift+Alt+O) and File > Open Project/Solution....

**Recent workspaces.**
- `recent.json` in the config directory (`ELUDITE_CONFIG_DIR` replaces it): `{"version": 1, "workspaces": [{"path", "kind": "folder"|"solution"|"project", "pinned", "opened"}], "files": [{"path", "opened"}]}`, at most 30 workspaces and 30 files (pinned ones never dropped), most recent first. Written off the UI thread after each successful open (folder, solution or project file; a file opened in an editor). Read once at startup on the UI thread (a few kilobytes; under 5 ms asserted with `assert_budget`). A missing or bad file is an empty list.
- `eludite.workspace.recent` (Read) returns the list (`query` filters by name or path, case-insensitive). `eludite.workspace.recent_edit` (Read: the user's own list) pins, unpins or removes an entry.
- The Welcome page: the wordmark and the mark on top, then two columns as Visual Studio 2022's Start window: "Open recent" (a search box, the pinned entries first, each row the name in bold with its path under it, a pin button and a remove button on hover; a click opens it with `eludite.workspace.open_folder`; missing paths are drawn muted) and "Get started" (five buttons: Open a local folder → `eludite.workspace.open_folder`; Open a project or solution → `eludite.workspace.open_project`; Clone a repository → a "not yet" message in the Output window since `crates/git` has no clone; Create a new project → `eludite.project.new`; Continue without code → closes the Welcome tab). A marked hook (`WelcomePage::set_banner`) for brief 0080's info bar.
- File > Recent Workspaces shows the Welcome page (dynamic submenus wait for brief 0078's menu mechanics).
- `workspace.reopenLast` (bool, default false): at startup with no path argument, the most recent workspace that still exists opens after the first frame.

**Templates.**
- `eludite/templates/list` (host): reads the template packages (`*.nupkg`, zip files) of the SDK in use (`<dotnet root>/templates/<highest version folder>/`) and the user's installed ones (`~/.templateengine/packages/`), each `content/**/.template.config/template.json`: identity, groupIdentity, name, shortName (string or array), `tags.language`, `tags.type`, classifications, author, description, defaultName, and the `parameter` symbols (name, displayName, description, datatype, defaultValue, choices, isRequired). Cached per package set (paths and sizes). Why not `dotnet new list`: its table carries no parameters, is localized, and `dotnet new <t> --help` per template costs a process each (about a second); the packages are the templates' own source. Templates from packages that fail to read are skipped and logged.
- `eludite/templates/create` (host): runs `dotnet new <shortName> --name <name> --output <output> [--language L] [--<param> <value>]... --no-update-check` with `DOTNET_CLI_TELEMETRY_OPTOUT=1`, `DOTNET_NOLOGO=1`; with `newSolution` first `dotnet new sln --name <n> --output <dir>`; with `solution` (existing or new) then `dotnet sln <solution> add <project>`. Returns the files created, the project file, the primary file, the solution, the combined output and exit code; the shell appends the output to the Output window's "Templates" source. Cancelable; a failure returns the output with `exitCode` not 0.
- `eludite/project/addItem` (host): the namespace is `RootNamespace` (else the project's file name) plus the folder path below the project, each segment made an identifier (invalid characters to `_`, a leading digit prefixed with `_`); Eludite's built-in items (`eludite.class`, `eludite.interface`, `eludite.record`, `eludite.enum`, `eludite.struct`; the VB set) write the file themselves with that namespace (file-scoped `namespace X;` for C#); any other item template runs `dotnet new <shortName> --name <name> --output <folder> --project <project>`. A legacy project gets a `Compile` item (`.cs`, `.vb`) or `Content` item (anything else) per new file through the construction model, preserving formatting; an SDK project is not written (globs). An existing target file is refused.
- `eludite/project/addExisting` (host): copies each file into the folder (refusing to overwrite) or, with `link`, writes `<Compile Include="<relative>" Link="<folder path>\<name>" />` (`None` for non-code files); a legacy project gets an item for each copy. Returns the resulting paths and whether the project file changed.
- `eludite.project.templates` (Read): the host's list filtered by `type`, `language`, `query`. `eludite.project.new` (Execute): `template`, `name`, `location`, `solution` (`new`, `add`, `none`; default `add` with a workspace solution, else `new`), `framework`, `parameters`, `open` (default true); from the UI without arguments opens the dialog. `eludite.project.add_item` (EditBuffer): `project`, `folder`, and either `template` + `name` or `existing` (+ `link`); from the UI without arguments opens the Add New Item dialog for the Workspace window's selected project or folder; with `existing: []` from the UI shows the file picker.
- The New Project dialog: page one lists the templates of type `project` (search over name, short name, description and tags; Language, Platform (classifications), Project type (classifications) filters; up to five recent templates first from `recent.json`'s `templates`); page two shows name (default from the template's `defaultName`, else `<ShortName>1`), location (default: the workspace folder, else the user's home `source/repos` as Visual Studio), the solution choice, the framework (the template's `Framework` or `framework` choice parameter) and the template's other choice and bool parameters with their defaults; Create sends `eludite.project.new`. The host starts if no workspace is open (`ServerSession::start`): templates never need a solution.
- The Add New Item dialog: the item templates (type `item`) for the project's language plus the built-ins first, a search box, the name box (default `Class1.cs` and so on), Add sends `eludite.project.add_item`; the new file opens.

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-workspace folder`: `find_projects` at the root and one level down, skipped folders, the generated `.slnx` text.
- `cargo test -p eludite-commands start`: parsing and the schemas of the six commands.
- `cargo test -p eludite args`: the positional path forms and `--version`.
- `cargo test -p eludite start_tests` (headless, the fake host): a project-only folder opens its project; a folder with two projects opens `.eludite/workspace.slnx`; `eludite.workspace.open_project` with a `.slnf`; the recent list written on open, drawn on the Welcome page, pinned and removed; `eludite.project.templates` and `eludite.project.new` with the request the host gets; the New Project dialog filters by language and search and sends the request; `eludite.project.add_item` sends the request and the created file opens; recent read under budget.
- `dotnet test --filter Templates|Item|SolutionFilter` with the real SDK: the list holds `console`, `classlib`, `xunit` and `web` with parameters (`Framework` choices); `dotnet new console` into a temp folder and `dotnet sln add` into a temp `.slnx`; the namespace of `Folder/Sub/Class1.cs`; a `.slnf`'s projects; a legacy project's `Compile` item written byte-preservingly; `addExisting` copy and link.
- `cargo fmt --check`, `cargo clippy` on the touched crates with `-D warnings`, `dotnet build dotnet/Eludite.slnx`.

## Budget

- The Welcome page draws with no host and no network at startup; the recent list is read in under 5 ms; the host is not started until a workspace opens or a template dialog opens.
- The template list is read once per package set (about 10 packages; under 500 ms cold on a developer machine, reported) and answered from the cache afterwards.
- No UI-thread waits: every host call answers on a spawned task.
- No new dependency.

## Exit criterion

The proving tests pass here; the report lists what was run, what was not, the numbers, and the Integration notes (the crate map rows, README status line, briefs index row).

## Out of scope

Dynamic submenus for File > Recent (brief 0078), the folder nodes' context menu (brief 0073), cloning a repository (no clone in `crates/git` yet), installing or uninstalling template packages, template post actions beyond `dotnet sln add` (restore runs on the first build), item templates' `--project` binding for built-ins, the Rider-style template parameter editor for every parameter type (text parameters other than the name are not shown), Windows and macOS runs beyond CI.
