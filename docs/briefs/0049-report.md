# Brief 0049 report: project property pages, launch profiles, configurations and platforms

Status: done on Linux, against the real `eludite-host` (MSBuild in process) and the fake host. Windows and macOS: not
built here; nothing added is platform-specific except that Win32 resource properties and IIS Express profiles are
read-only off Windows. CI: not run (nothing pushed).
Branch: `brief/0049-project-properties`, based on `main` at `d53af80`; not rebased (the coordinator merges).
Date: 2026-10-04. Brief: [0049-project-properties-and-launch-profiles.md](0049-project-properties-and-launch-profiles.md).

## 1. Summary

- **Schemas first** (`86f318a`, alone): six host messages under `protocol/schemas/host/` (`eludite/project/properties`,
  `eludite/project/setProperty`, `eludite/project/launchProfiles`, `eludite/project/setLaunchProfile`,
  `eludite/solution/configurations`, `eludite/solution/setConfiguration`), the "Project properties" section of
  `host-rpc.md` (catalog, evaluation, condition rule, inherited rule, generation rule, launch profiles, solution
  configurations), seven command schema pairs, and `framework` on `debug-start`, `test-run` and `test-debug`,
  `project_properties` on `view-show`, the note on `settings.json` that the selection is state, not settings.
- **Host** (`fdde6fd`, `dotnet/src/Eludite.Host/Projects/`): `PropertyCatalog` (section 3), `ProjectPropertyEvaluator`
  (a second evaluation with `Configuration`, `Platform` and `TargetFramework` as global properties in its own
  `ProjectCollection`, each value's source found by walking the property's predecessors: the project file and line, a
  conditioned group, an imported file under the project's tree (inherited), or the SDK and MSBuild defaults),
  `ProjectFileEditor` (the edit with `Microsoft.Build.Construction` and `preserveFormatting`; `TextFileFormat` restores
  the BOM, line endings, XML declaration and trailing newline the writer would change), `LaunchSettingsFile`
  (System.Text.Json nodes, the file's own indentation and newline), `SolutionConfigurationFile` (`.sln` line edits in
  `ProjectConfigurationPlatforms`, `.slnx` through `XDocument` with whitespace preserved), `ProjectPropertiesService`
  (values cached per generation, writes serialized, the solution reloaded after a write: the generation moves on and
  the tree, IntelliSense and builds see the change; the active selection and its mapping). Six `[JsonRpcMethod]`s in
  `HostRpcTarget`. Logs go to stderr only.
- **Typed messages and commands** (`92971a0`): `protocol/rust/src/host.rs` types and request markers, schema
  conformance tests; `crates/commands/src/project/properties.rs` (`eludite.project.properties` read,
  `eludite.project.set_property` execute, `eludite.project.launch_profiles` read, `eludite.project.set_launch_profile`
  execute) and `solution.rs` (`eludite.solution.configurations` read, `eludite.solution.select_configuration`
  edit-buffer class (it changes only the shell's selection), `eludite.solution.set_configuration` execute);
  `framework` on `eludite.debug.start`, `eludite.test.run` and `eludite.test.debug`.
- **Fake host** (`c3abb55`): `crates/lsp/src/fake/projects.rs` serves the six messages with the generation rule and a
  reload that bumps the generation.
- **Shell** (`e0aac1b`): the property pages as a document tab (`project_properties:<path>`), the left page list and the
  right form generated from the catalog (text boxes, check boxes, one button per enum choice, Browse... for paths), the
  Configuration and Platform lists on pages with per-configuration properties ("All Configurations" last), "Differs per
  configuration", dirty state with Save (Ctrl+S or the button) as one `setProperty` per project, the close prompt
  (Save, Don't Save, Cancel), the inherited banner with Override, the Debug page as the launch profiles editor (list,
  New, Delete, Rename, the fields per kind, the environment variables table, applied at once as Visual Studio's dialog
  does). Opened by a double-click on a project, Project > Properties, Alt+Enter and the Workspace context menu's
  Properties, and `eludite.view.show` with `project_properties`. The toolbar (`toolbar.rs`, at the right of the menu
  bar row): Solution Configurations, Solution Platforms (each ending with Configuration Manager...), the Target
  Framework list when the startup project is multi-targeted, the Debug toolbar's Start with the launch profile list.
  Build > Configuration Manager... (`configuration_manager.rs`): the active selection lists and the per-project grid
  (configuration, platform, Build). The selection is persisted per solution in
  `<state dir>/solutions/<stem>-<hash>.configuration.json` and restored on open.
- **Build, debug, tests use the selection.** A solution build passes the selection; a project the mapping does not
  build gets Visual Studio's Output lines (`------ Skipped Build: Project: Lib, Configuration: Release Any CPU ------`,
  `Project not selected to build for this solution configuration`); a project build uses the project's mapped
  configuration and platform; a Cargo workspace lists Debug and Release, Release being `--release`. F5 uses the
  mapped configuration, the selected framework (`-f`, and the output path under it) and the selected profile (only when
  the file has it). Test runs filter to the selected framework's containers.
- **Real host in the client's tests** (`e57029b`): a property edit and a launch profile edit on a copy of
  `corpus/projects`. **MCP** (`535d338`): the seven tools listed with their classes and answering.
- **Xvfb run** (`b9a32a7`, see section 6): `crates/eludite/tools/project-properties-linux.sh` drives the real binary
  against the real host with real X input; the pages, the toolbar lists and Configuration Manager record their bounds
  while `--bounds-out` probes.

## 2. Commits

| Commit | Message |
|---|---|
| `f0fedef` | Mark brief 0049 in progress |
| `86f318a` | Specify the project properties, launch profiles and solution configuration schemas |
| `fdde6fd` | Serve the project property catalog, the condition-preserving project edit, launch profiles and solution configurations from the host |
| `92971a0` | Type the project properties messages and add the project property and solution configuration commands |
| `c3abb55` | Serve project properties, launch profiles and solution configurations from the fake host |
| `e0aac1b` | Add the project property pages, the launch profiles page, the toolbar lists and Configuration Manager to the shell |
| `e57029b` | Edit a property and a launch profile through the real host in the client's tests |
| `535d338` | List the project property and solution configuration tools over MCP with their classes |
| `b9a32a7` | Record the property pages', toolbar lists' and Configuration Manager's bounds and add the Xvfb run with its screenshots |
| (last) | Report brief 0049 and update the index, CLAUDE.md and corpus/README.md |

## 3. The catalog as shipped

40 properties on five pages; "per config" means the Configuration and Platform lists apply and a value for one
configuration goes to its conditioned group.

| Page | Section | Property (MSBuild) | Type | Per config |
|---|---|---|---|---|
| Application | General | `AssemblyName`, `RootNamespace`, `StartupObject` | string | no |
| | | `TargetFramework` | string | no |
| | | `TargetFrameworks` | list | no |
| | | `OutputType` (Exe, WinExe, Library) | enum | no |
| | | `Nullable` (disable, enable, warnings, annotations) | enum | no |
| | | `ImplicitUsings` (enable/disable) | bool | no |
| | | `LangVersion` (default, latest, latestMajor, preview, 7.3 to 14.0) | enum | no |
| | Win32 resources | `ApplicationIcon`, `ApplicationManifest`, `Win32Resource` (read-only off Windows) | path | no |
| Build | General | `DefineConstants` | list | yes |
| | | `Optimize` | bool | yes |
| | Errors and warnings | `WarningLevel` (0 to 9, 9999) | enum | yes |
| | | `TreatWarningsAsErrors` | bool | yes |
| | | `WarningsAsErrors`, `NoWarn` | list | yes |
| | Output | `GenerateDocumentationFile` | bool | yes |
| | | `DocumentationFile`, `OutputPath` | path | yes |
| | | `BaseOutputPath` | path | no |
| | Events | `PreBuildEvent`, `PostBuildEvent` (written as Visual Studio's `PreBuild`/`PostBuild` targets with an `Exec`) | multiline | no |
| | Strong naming | `SignAssembly` | bool | no |
| | | `AssemblyOriginatorKeyFile` | path | no |
| | Advanced | `Deterministic` | bool | no |
| | | `DebugType` (portable, embedded, full, pdbonly, none) | enum | yes |
| Package | General | `GeneratePackageOnBuild` | bool | no |
| | | `PackageId`, `Version`, `Authors`, `Description`, `PackageProjectUrl`, `RepositoryUrl`, `PackageTags` | string | no |
| | License | `PackageLicenseExpression` | string | no |
| | | `PackageReadmeFile` | path | no |
| Debug | | the launch profiles editor (section 5) | | |
| Code Analysis | General | `EnforceCodeStyleInBuild` | bool | no |
| | | `AnalysisLevel` (latest and its `-minimum`, `-recommended`, `-all`, preview, none, 5.0 to 10.0) | enum | no |
| Resources, Settings, Signing | | "Not yet" with a note (section 8) | | |

Legacy (non-SDK) project files are shown read-only with a note.

## 4. The rules

- **Condition rule.** A value for one configuration goes to the property group whose condition matches Visual Studio's
  form `'$(Configuration)|$(Platform)'=='Debug|AnyCPU'` (matched whatever its spacing, quoting and order, also a
  configuration alone, `'$(Configuration)'=='Debug'`, and a group naming the framework), or to a new group with that
  condition placed after the last unconditioned group. An unconditioned value goes to the first unconditioned property
  group (created if none), indented like its siblings. A value equal to the default (the value the project evaluates to
  without the element) removes the element, and a group left empty is removed. "All Configurations" writes the
  unconditioned value and removes the property from the conditioned groups; the shell confirms first when they
  differ. Every edit changes one element and leaves the rest byte-for-byte (the eight corpus files round-trip exactly).
- **Inherited rule.** A property whose winning definition is in an imported file under the project's tree
  (`Directory.Build.props`, `Directory.Build.targets`, other imports) is answered `source: inherited` with
  `inheritedFrom`; a write without `override: true` answers `status: inherited` and changes nothing. The pages show a
  banner with Override; with it the value is written into the project file, which then wins.
- **Generation rule.** Every call carries the solution generation; a stale one answers `-32801` (ContentModified) and
  the client maps it to `Error::Stale`; the shell drops answers of an older generation and never renders them. A write
  reloads the solution in the host and the generation moves on; cached evaluations are keyed by generation.

## 5. Launch profiles

`Properties/launchSettings.json` is read and written through System.Text.Json nodes: profile order, unknown members
(`x-corpus-note` in the corpus) and the file's indentation and newlines survive; a set changes only its members
(the rest byte-for-byte, tested). New creates a Project profile with the project's defaults; Rename keeps the
position; Delete removes one; the environment variables table keeps its order, appends new ones and drops removed ones.
`Executable` profiles show the executable path; IIS Express profiles are listed read-only off Windows. The Debug
toolbar's profile list runs `eludite.project.set_launch_profile` with `select`, and F5 (`eludite.debug.start` without
`profile`) uses it: the toolbar and `debug.start`'s `profile` agree. JSON comments in the file are not kept (System.Text
.Json nodes drop them; none of Visual Studio's templates have any).

## 6. Proving tests

| Test | What it proves |
|---|---|
| `dotnet/tests/Eludite.Host.Tests/ProjectPropertiesTests.cs` | The corpus round-trips byte-for-byte (8 files: comments, tabs, CRLF, BOM, declaration, odd indentation); an edit changes exactly one element; unconditioned appends with the group's indentation; a group is created when none; the condition rule with a new group, an existing group of any spacing, a default removing the element, a configuration alone and a framework group; All Configurations; the inherited rule; every catalog property read, written and removed back to the original file; build events as targets; legacy read-only; unknown and Windows-only refusals; the service's per-generation cache, reload after a write and stale refusal; the selection's mapping and cancellation (-32800); the wire methods' generation rule |
| `LaunchProfilesTests.cs` | Order, unknown members and IIS Express marking; a set changes only its member; the environment table's order; New, Rename, Delete keep order; the file is created when missing; the file's own indentation |
| `SolutionConfigurationsTests.cs` | `.sln` and `.slnx` configurations, platforms and mapping; a project file maps to itself; `.sln` edits change only their lines keeping tabs, CRLF and BOM; `.slnx` rules added and removed restore the file; the 100-project configuration change budget |
| `protocol/rust/src/schema_tests.rs` `project_property_messages_conform_to_their_schemas` | The Rust types serialize to what the schemas accept |
| `crates/commands/src/project/properties.rs`, `solution.rs`, `debug.rs`, `test.rs` unit tests | Parsing, classes, `framework` refused with a compound launch |
| `crates/lsp/tests/in_process.rs` `project_property_messages_are_typed_and_writes_follow_the_generation` | The six messages round-trip through the client against the fake host; a stale write maps to `Error::Stale` |
| `crates/lsp/tests/real_host.rs` `real_host_edits_a_property_and_a_launch_profile` | A property edit and a launch profile edit on a copy of `corpus/projects` through the real host: one element written, the generation moved on, the profile's member changed and the rest kept |
| `crates/dap/src/launch.rs` `launch_config_in_follows_the_configuration_and_the_framework` | The launch's configuration, framework and output path |
| `crates/mcp/src/tests.rs` `the_project_property_tools_are_listed_with_their_classes_and_answer` | The seven tools over MCP with their classes |
| `crates/ui/src/menu.rs` `project_properties_and_configuration_manager_are_menu_items` | Project > Properties and Build > Configuration Manager... |
| `crates/eludite/src/shell/project_properties_tests.rs` (9 headless tests, fake host) | Pages open from Workspace (double-click), the menu and `eludite.view.show` (under 150 ms to values), show sources, an edit marks dirty and Ctrl+S writes through the host and moves the generation, close prompts; the Configuration and Platform lists change the values and route the write, All Configurations confirms, Override writes an inherited value; the Debug page edits a profile and F5 uses the toolbar's profile; the toolbar lists select and persist (restored on reopen), the build uses the selection, a skipped project writes the Output lines, a project build uses its mapping; Configuration Manager and the agent edit the mapping; the Target Framework list drives a start and a test run; an agent's `properties` and `set_property` match the pages; the controls record their bounds while probed (the Xvfb run) |

**Xvfb run** (`crates/eludite/tools/project-properties-linux.sh`, real binary, real host, `Corpus.slnx` copied):
[screenshots](0049-run/screenshots/). `solution.png` (the toolbar lists), `pages-application.png`,
`pages-build-dirty.png` (Debug, Treat warnings as errors checked, "Changed (not saved)", the tab marked `Console*`),
`pages-build-saved.png`, `pages-debug.png` (the Console profile, " --trace" appended and applied, the environment table
in file order), `configuration-list.png`, `configuration-release.png`, `configuration-manager.png` (Release: Lib not
built). The files the run wrote: [Console.csproj.diff](0049-run/Console.csproj.diff) (one new group with Visual
Studio's condition, nothing else) and [launchSettings.json.diff](0049-run/launchSettings.json.diff) (one member). The
Error List's "No language server is configured" is because the run does not locate the Roslyn pin; the project system
is the host's own.

## 7. Budget numbers

Measured on this machine (not the reference machine).

| Metric | Budget | Measured |
|---|---|---|
| Pages open to values shown (fake host, shell side) | < 150 ms | 22.8 ms (asserted < 150 ms) |
| Properties, real host, cached evaluation | < 150 ms | 0.3 ms in process; 2.8 ms over the wire |
| Properties, real host, another project warm | | 48.7 ms |
| Properties, real host, the first evaluation of a fresh process | | 130 to 576 ms (466 ms in the last run): MSBuild's first evaluation, once per process; the pages show "Evaluating..." and the UI never waits |
| Save (the write, before the reload) | < 300 ms + reload | 76 to 125 ms in tests (106 ms last), 197 to 231 ms in the Xvfb run; the solution reload after it took 907 ms there (5 projects, cold) |
| Configuration list change, 100-project solution | < 500 ms | 39.4 ms the first, 0.6 and 0.3 ms after (host side; the shell sends it off-thread) |
| New dependencies | none | none |

## 8. Not done, and why

- **The editor's navigation bar Target Framework list.** The editor has no navigation bar yet, so the list is on the
  toolbar only (next to the configuration lists, shown when the startup project is multi-targeted). It changes the
  launch and test target only (`debug.start` and `test.run` with `framework`); it does not switch the Roslyn semantic
  context (the host's language server opens one context per project).
- **IntelliSense does not follow the configuration.** A configuration change re-evaluates properties but the language
  server keeps the project's default evaluation.
- **The selection is not written to `.user` files** (`ActiveDebugProfile`, `ActiveDebugFramework`); it lives in
  Eludite's per-solution state.
- **Configuration Manager edits mappings but cannot add or remove solution configurations or platforms** (the `<New...>`
  and `<Edit...>` entries).
- **The Platform list shows the solution's own platforms** (and the project's for a project file), not a fixed
  Any CPU/x64/x86/ARM64 list; this follows the contract's "the lists show the solution's own entries".
- **A value defined in `Directory.Build.targets`** (imported after the project body) still wins over a value written in
  the project with Override; the host answers it and the note says where it comes from.
- **Resources, Settings, Signing.** Resources needs the `.resx` editor (a grid of name, value, comment over the XML,
  preserving the schema header), Settings the `.settings` designer (and its generated `Settings.Designer.cs`), Signing
  ClickOnce manifest signing (certificate selection from the store, a Windows concern). Each is "Not yet" with its note;
  strong naming is on the Build page.
- Legacy projects are read-only, as the brief's out-of-scope says.

## 9. Files outside the listed scope

- `crates/dap/src/launch.rs`: `launch_config_in(project, profile, configuration, framework)` (the launch needs the
  configuration and the framework for the output path); `launch_config` delegates to it.
- `crates/commands/src/debug.rs` and `test.rs`: the `framework` field the schemas add.
- `crates/eludite/src/shell/debug/state.rs` (two test constructors gain `framework: None`), `explorer.rs` (the context
  menu's Properties, Alt+Enter, double-click on a project), `session.rs` (`refresh_tree` after a write),
  `crates/eludite/src/tests.rs` (Build's enabled labels gain Configuration Manager...).

## 10. Merging with brief 0048

- **The project file edit.** `ProjectFileEditor` (`dotnet/src/Eludite.Host/Projects/ProjectFileEditor.cs`) is this
  brief's own `Microsoft.Build.Construction` helper with `TextFileFormat`; 0048 has its own for package references.
  They should be unified into one helper (one place for the formatting rules); its remarks say so.
- **Likely conflicts:** `HostRpcTarget`'s constructor (an optional `ProjectPropertiesService` parameter here),
  `crates/lsp/src/fake.rs`'s answer arm (six methods routed to `fake/projects.rs`), `explorer.rs` (context menu items
  and Alt+Enter; 0048 adds the Dependencies node), `crates/docking/src/model.rs` (`DOCUMENT_WINDOWS` gains Properties),
  `view-show.input.json`'s enum and description, `crates/ui/src/menu.rs` (Project > Properties gets `open: true`; 0048
  adds Manage NuGet Packages), `protocol/rust/src/host.rs` (both add method constants to `ELUDITE_ACCEPTED`), the
  briefs index rows and `CLAUDE.md`'s `dotnet/` row. All are additive.

## 11. Verification

`cargo fmt --check`, `cargo clippy --workspace --all-targets --features eludite-chromium/cef -- -D warnings`,
`dotnet build dotnet/Eludite.slnx` (0 warnings), `dotnet test dotnet/Eludite.slnx --no-build` (236 tests: 229 passed,
7 skipped), and `cargo test --workspace --no-fail-fast --features eludite-chromium/cef` with Mono, the test corpus,
Chrome, CEF, js-debug and sshd: see the coordinator hand-off for the run's numbers.
