# Brief 0054: The workspace replaces the solution everywhere the user and agents see it

Status: open
Phase: 2 (PLAN.md section 14, decisions 12 and 14)
Plan reference: PLAN.md sections 2 (principles 3, 5, 12), 4.2 (the Workspace window), 4.5 (F5 and the startup project), 4.7 (NuGet), 7 (.NET, the web stack and Rust are equally first-class), 8, 14 (decision 12: "no menu item, dialog or command offered to the user is built around a solution or a project"; decision 14: the menu bar)
Related ADRs: ADR-0002, ADR-0004
Depends on: the menu bar change on `main` (decision 14: no Project, Analyze, Extensions or Window menu; no menu label says Solution or Project). Independent of brief 0051; whichever lands second carries its run targets into the startup project list (see Contract).

## Goal

A person opens a folder as a workspace, or opens a single file. Inside a workspace the shell finds **projects**: a .NET project file (`.csproj`, `.fsproj`, `.vbproj`), a Cargo package, and (once brief 0051 lands) a `package.json` package. "Project" is the generic word for any of them, in every language. A .NET solution file (`.sln`, `.slnx`, `.slnf`) is only a source of information: which .NET projects to load, their order, their solution folders, and the configuration mapping MSBuild builds with. It is never a thing the person opens, closes, builds, configures or is told about, and no command id, menu, dialog, status line, window title, tree label, error message or agent guide uses the word "solution", except where it names the file itself (a `.sln` row in the tree; the host's internal `eludite/solution/*` methods).

When this brief is done:

- The Workspace window's root is the workspace folder ("Eludite", with its project count), never "Solution 'X'" or "Cargo workspace 'X'". Its children are the projects of every language in one list, and the solution's folders stay as virtual folders that group .NET projects.
- F5 starts the **startup project** (or several startup projects), chosen from any project that can start: a .NET project that produces an executable or a web app, or a Cargo package with a binary target. The default, the persisted choice and the Startup Projects dialog are per workspace, not per solution.
- Ctrl+Shift+B builds the workspace (every build system that owns the active document, else all of them) through `eludite.build.workspace`. Build, Rebuild and Clean on a project node build that project in its own build system.
- The configuration (Debug or Release) belongs to the workspace and drives both MSBuild's configuration and Cargo's profile. The platform list shows only when a .NET project in the workspace targets more than Any CPU. The Configuration Manager dialog and its command are gone.
- Find in Files' default scope is "Entire Workspace". "Current Project" and "Project: X" cover projects of every language.
- Manage NuGet Packages works on one .NET project (from its node, or from the active document) or on every .NET project in the workspace (Tools > NuGet Packages..., or the workspace root's context menu), and is titled for what it covers ("NuGet - Workspace", "NuGet: App").
- A .NET project's property pages and launch profiles stay. They are per-project actions on a project node, like Build and Set as Startup Project.

## Files in scope

Schemas first, in their own commit (CLAUDE.md invariant 4):

- `protocol/schemas/`:
  - Remove `solution-open`, `solution-close`, `solution-tree`, `solution-set-configuration` (input and output).
  - Rename `solution-configurations` to `build-configurations` and `solution-select-configuration` to `build-select-configuration`. Their fields say `configuration` and `platform`, with no `solution_` prefix.
  - Rename `build-solution.input.json` to `build-workspace.input.json`.
  - `search-find.input.json` (and replace's, if it shares the enum): `scope` becomes `workspace|project|document|open_documents|folder`, and `project` is described as any project of the workspace.
  - `nuget-manage.input.json`: replace `solution` (bool) with `workspace` (bool). Fix the description that cites the solution's context menu.
  - `workspace-set-startup-project.*`: describe `project` as a project of the workspace that can start. The Cargo form is unchanged.
  - `error-list-filter.input.json`: describe `project` without the solution.
  - Regenerate bindings with the repository's generator; never by hand.

Then the code:

- `crates/commands/src/workspace.rs`, `solution.rs`, `build.rs`, `project.rs`, `project/properties.rs`, `search.rs`, `nuget.rs`, `debug.rs` (`Compound::Startup` doc), `workspace_tree.rs`:
  - Delete `SOLUTION_OPEN`, `SOLUTION_CLOSE`, `SOLUTION_TREE` (`eludite.workspace.tree` already covers it; make sure it answers the projects of every language) and `SET_CONFIGURATION`.
  - Move `CONFIGURATIONS` and `SELECT_CONFIGURATION` to `build` as `eludite.build.configurations` and `eludite.build.select_configuration`.
  - Rename `build::SOLUTION` to `build::WORKSPACE` (`eludite.build.workspace`).
  - Retitle every command whose title says Solution: "Build: Build", "Build: Rebuild", "Build: Clean", "Workspace: Set as Startup Project", "Tools: Manage NuGet Packages".
  - Delete `solution.rs` if nothing is left in it.
- `crates/mcp` and its tests: tool names follow the ids. Update `src/tests.rs`'s lists of names and classes.
- `crates/ui/src/keymap.rs`: Ctrl+Shift+B and F6 go to `eludite.build.workspace`. Shift+F6 stays on `eludite.build.project`.
- `crates/ui/src/menu.rs`: Build's items use the new id. Tools > NuGet Packages... passes `{"workspace": true}`. Remove the stale comments that cite the old Project menu.
- `crates/ui/src/startup.rs`: the Startup Projects dialog is titled with the workspace's name. Its help line says "in workspace order". It lists every project that can start, with a language glyph.
- `crates/workspace/src/explorer.rs`: the root node and its labels, as Contract describes. The `.sln` file shows as an ordinary file under the root. The empty-state text names File > Open Workspace... (Ctrl+Shift+O) and Open File....
- `crates/eludite/src/shell.rs`, `app.rs`:
  - The window title is "{workspace folder} - Eludite".
  - The status slot `solution` becomes `workspace`, with "{name}: loading projects…" and "{name}: N projects loaded in X s".
  - Every "Solution" fallback becomes the folder name.
- `crates/eludite/src/shell/startup.rs`, `debug.rs`, `debug/state.rs`, `debug/native.rs`:
  - Persist the startup set per workspace (`<config dir>/eludite/workspaces/<folder>-<hash>/startup.json`, beside brief 0047's per-workspace state). Read the old per-solution file once, keyed by the solution the folder detects, and drop it after a successful write.
  - `find_default_startup` considers .NET and Cargo projects together, in workspace order: .NET projects in solution order, then Cargo members in `cargo metadata` order.
  - Rewrite every message that says "solution" (examples in Contract).
- `crates/eludite/src/shell/build.rs`, `cargo_build.rs`: `eludite.build.workspace`, the configuration as Cargo's profile (`Debug` = `dev`, `Release` = `release`), and the Output lines ("Building: 3 of 7 projects").
- `crates/eludite/src/shell/toolbar.rs`:
  - The configuration list shows for any workspace with a build system.
  - The platform list shows only as Goal says.
  - Remove `CONFIGURATION_MANAGER`.
- `crates/eludite/src/shell/configuration_manager.rs`: delete it, with its tests.
- `crates/eludite/src/shell/project_properties.rs`: the selected launch profile and configuration move to the per-workspace state file, migrated once as above. The "Skipped Build" line names the project only.
- `crates/eludite/src/shell/search.rs`, `search/dialog.rs`, `search/results.rs`:
  - `LookIn::Workspace` ("Entire Workspace") is the default.
  - The project list comes from the workspace's projects of every language.
  - A project's folder is its project file's (or `Cargo.toml`'s) directory.
- `crates/eludite/src/shell/error_list.rs`: the Project column and filter list every language's projects. A Cargo diagnostic carries its package name. A diagnostic from a language server carries the project whose folder holds the file, nearest first.
- `crates/eludite/src/shell/nuget.rs`, `nuget/window.rs`, `explorer.rs`:
  - Rename `Scope::Solution` to `Scope::Workspace`. It covers every .NET project in the workspace.
  - The workspace root's context menu has "Manage NuGet Packages..." only when the workspace has a .NET project.
  - Errors say "this workspace has no .NET project".
- `dotnet/`: no protocol change. The host keeps `eludite/solution/*` as its internal vocabulary. If the shell needs the solution's configuration mapping without the Configuration Manager, it reads it through the existing `eludite/solution/configurations`.
- Tests that use any renamed id or label. `crates/eludite/src/shell/*_tests.rs` and `debug/tests.rs` are in scope only for those edits and the new tests below.
- `docs/agents/*.md`:
  - Replace "the solution's policy" with "the workspace's policy".
  - `debugging.md:8` names `eludite.workspace.open_folder`.
  - `debugging.md:176`: "the workspace's startup projects".
  - `nuget.md`: "the workspace's .NET projects".
- `README.md`, `CLAUDE.md` (where a row says solution in the user-facing sense), `docs/briefs/README.md`, `docs/briefs/0054-report.md` (new).

## Contract

- **Vocabulary.** "Workspace" is the opened folder. "Project" is a .NET project, a Cargo package, or (with brief 0051) an npm package. "Solution" appears only where it names a `.sln`, `.slnx` or `.slnf` file. A test walks every registered command's id and title, every menu label, the Startup Projects dialog's strings, the Find in Files scope labels, the toolbar's labels and the Workspace window's root label, and fails on "solution" (any case) outside a file name.
- **Detection.** The folder rule of brief 0019 is unchanged: an `.slnx` before an `.sln`, at the root or one folder down. Opening a solution file with Open File... opens it as a text document. It does not load anything.
- **Startup project.**
  - A project can start when it is a .NET project with `OutputType` `Exe` or `WinExe`, or a web SDK project, or a Cargo package with a `bin` target.
  - The default is the first that can start, in workspace order.
  - The person's choice (one, or several with Start or Start without debugging) persists per workspace and survives a solution file being added, renamed or removed.
  - `eludite.workspace.set_startup_project` takes either form, and without arguments opens the dialog, as today.
  - Messages:
    - "this workspace has no project that can start" (replaces "open a .NET solution first: there is no startup project" and "the .NET solution has no executable project to start").
    - "{project} is not a project of this workspace that can start" (replaces the "not a project of the open solution" text).
    - "Building before starting {name}…" (replaces "Building the solution before starting {name}…").
  - When brief 0051 has landed, a `package.json` script it lists as a run target is a project that can start.
- **Build.**
  - `eludite.build.workspace` builds the system owning the active document, else every system in turn, exactly as `eludite.build.solution` does today.
  - `rebuild` and `clean` follow it.
  - `eludite.build.project` takes any project's name or path; a name shared by two languages is refused, and the error names both paths.
  - The configuration is one value per workspace. MSBuild receives it and the platform as today, through the solution's mapping when a solution exists. Cargo receives `--profile dev|release`.
- **No aliases.** The removed and renamed ids are not kept as aliases. Nothing has been released (PLAN.md status line). The report lists every old id beside its new one, or "removed".
- **State.** The new per-workspace files are written atomically and read once at workspace open. The old per-solution files are migrated, not deleted, until the new file is written.
- The UI thread never waits (invariant 1). Results from a previous workspace generation are dropped (invariant 12).
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands` / `crates/mcp`:
  - The vocabulary test above.
  - The tool list matches the new ids.
  - No removed id is registered.
- `crates/workspace`: root labels for each case:
  - a .NET-only folder
  - a Cargo-only folder
  - a mixed folder (this repository's shape: `Eludite.slnx`'s projects and the Cargo members under one root, solution folders kept)
  - an empty folder
  - a folder whose solution fails to load (the root says "Eludite (load failed: …)", and the Cargo members still show)
- `crates/eludite` headless tests:
  - F5 in a mixed workspace starts the default startup project; set a Cargo package as startup and F5 starts it.
  - The Startup Projects dialog lists .NET and Cargo projects and starts both.
  - The choice persists across reopen, including after the `.slnx` is renamed.
  - A per-solution file from before is migrated once.
  - Ctrl+Shift+B runs `eludite.build.workspace`.
  - Release builds Cargo with `--profile release` and MSBuild with `Release`.
  - Find in Files' default scope covers a Cargo member and a .NET project, and "Project: eludite-ui" searches only that crate.
  - The Error List's project filter lists both languages.
  - NuGet from the root node and from Tools opens "NuGet - Workspace", and from a project node "NuGet: App".
  - The window title and the status slot use the folder's name.
  - The Configuration Manager command is gone, and the toolbar has no button for it.
- `cargo test --workspace` and `dotnet test dotnet/Eludite.slnx` green.

## Budget

- Cold start, workspace open to editable text, and keystroke budgets unchanged (CLAUDE.md table; the merge check's 5 percent rule).
- Reading the per-workspace state adds under 1 ms to workspace open.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` green; `dotnet build` and `dotnet test` green.
2. `grep -rniE "solution" crates/*/src --include=*.rs` shows no user-visible string or command title that uses the word outside a file name. The report lists what remains (internal names, host methods, comments) and why each stays.
3. The report has:
   - the id table (old → new or removed)
   - the vocabulary test's coverage
   - screenshots of the Workspace window on this repository and on a .NET-only folder
   - the Startup Projects dialog with a Cargo package and a .NET project
4. `docs/agents/`, the MCP tool list, README.md, CLAUDE.md and the briefs index match the repository.

## Out of scope

- The host's internal protocol names (`eludite/solution/*`, the "solution generation number" of invariant 12). They name the .NET artifact the host loads.
- Several solution files in one folder (the detection rule picks one, as today).
- npm packages as projects beyond what brief 0051 delivers.
- Renaming the Workspace window or its id.
- Cargo profiles other than `dev` and `release`.
- Any change to the property pages' content (brief 0049) beyond where their state is stored.
