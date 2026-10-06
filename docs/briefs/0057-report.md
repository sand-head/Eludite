# Brief 0057 report: Visual Basic and F# as first-class .NET languages

Status: done on Linux (2026-10-05; Windows and macOS by CI). Four agents in one tree on disjoint files (the grammars, the servers, the projects and the Roslyn halves), one integrator.

## Summary

- **Spike 2 (PLAN.md section 4.3) is answered.** The pinned Roslyn language server knows Visual Basic by name only. Its project composes `Microsoft.CodeAnalysis.CSharp.Features` as the one language in the MEF composition, the composition is built from every `Microsoft.CodeAnalysis*.dll` in the server's folder, and `HostWorkspace/LanguageServerProjectLoader.cs` (line 291 at the pin) returns `null` for a loaded project whose language has no `ICommandLineParserService`. A `.vbproj` is therefore evaluated by the MSBuild build host and then silently dropped, and its `.vb` documents get nothing. Measured through the host against a copy of the server folder with the three Visual Basic DLLs removed (what the unpatched pin produces): `Completed (re)load of 1 project(s)` is logged, `Successfully completed load of ...VisualBasic.vbproj` never is, and `textDocument/diagnostic` stays empty for three minutes. Nothing is logged about the dropped project.
- **The fix is one project reference**, `tools/roslyn-pin/vb.patch`: `Microsoft.CodeAnalysis.VisualBasic.Features.vbproj` next to the C# one, which brings `Microsoft.CodeAnalysis.VisualBasic.dll` (5.8 MB), `Microsoft.CodeAnalysis.VisualBasic.Workspaces.dll` (0.9 MB) and `Microsoft.CodeAnalysis.VisualBasic.Features.dll` (1.3 MB) into the output. `build.sh` and `build.ps1` apply it after the checkout (`git apply --check`, skipped when the reverse check says it is already there, a loud failure otherwise); `COMMIT` is unchanged. With it, and with `eludite-host` unchanged, Roslyn loads the `.vbproj` and serves diagnostics (BC30512 after one pull, 2.3 s from open), hover in Visual Basic syntax (163 ms), completion (49 items after `Console.`, 669 ms), go to definition (36 ms, to `Sub New`) and rename (4 edits, 723 ms). No host-side gap-filling was needed for these five features. The patched server built here in 217 s (about 4 minutes wall from a blob-less clone, including Roslyn's SDK download); the output folder is 137 MB.
- **`.vb` goes to Roslyn through the host** as the `roslyn` registration's second glob with `languageId` `vb` (the registration is now named `C# and Visual Basic`). **`.fs`, `.fsi`, `.fsx` and `.fsscript` go to FsAutoComplete** (MIT, 0.84.0 pinned) through the generic server path like rust-analyzer: a `process` registration with `AutomaticWorkspaceInit`, root markers by glob (`*.fsproj`, `*.sln`, `*.slnx`), and a new discovery source, the .NET global tool (beside `eludite`, `ELUDITE_FSAUTOCOMPLETE`, the pinned cache `~/.cache/eludite/fsautocomplete/<pin>/` that `tools/fsautocomplete/fetch.sh` fills with `dotnet tool install --tool-path`, `~/.dotnet/tools`, `PATH`). A located .NET tool is probed and spawned with `DOTNET_ROOT` set to the SDK root of the `dotnet` on `PATH` when the variable is unset, because the tool's apphost refuses to start otherwise on a user-local SDK (this container reproduced exactly that); every other server gets nothing added. The status bar's remedy for a missing server names the registration's own fetch script (`not found (run tools/fsautocomplete/fetch.sh)`), not the npm one.
- **`.vbproj` and `.fsproj` are MSBuild projects like `.csproj`** everywhere the host lists a solution's projects (`SolutionProjects.Read`: the tree, properties, configurations, tests, build, NuGet, the legacy evaluator), as the open target (shell and host), and in `eludite.workspace.tree`, whose `kind` is now `csproj`, `vbproj` or `fsproj` from the project file's extension (schema first). A lone `.fsproj` opens and reaches `loaded` without anything being handed to Roslyn, which cannot load F#; a solution holding F# projects goes to Roslyn whole, which skips them itself. CodeLens settings exist for `vb` and `fsharp`.
- **The Test Explorer corpus gained `Corpus.VisualBasic`** (MSTest on Microsoft.Testing.Platform) **and `Corpus.FSharp`** (NUnit on VSTest), five tests each with the usual one failure, one skip and one output writer; the bridge and the host discover and run them with the expected outcomes, on CI's Linux and Windows .NET jobs too.
- **The editor highlights Visual Basic and F#** from crates.io grammars (`tree-sitter-vb-dotnet` 0.1.0, `tree-sitter-fsharp` 0.3.12 with its signature grammar for `.fsi`), with Eludite's queries (the VB grammar ships none). The VB grammar is the weak piece: real Visual Basic has constructs it does not parse (the list below, 25 of 27 fixtures carry one each), and its keywords are hidden tokens, so only modifiers can be painted as keywords. The F# grammar parses every fixture cleanly except member signatures in `.fsi` files, but a first full parse of a 2,000-line file takes about 29 ms against the brief's 10 ms (incremental re-parses after an edit are the editor's normal path). Both are grammar properties; an in-repo grammar in the style of `grammars/razor` is the follow-up (see "Not done").

## Versions and licenses

| Piece | Version | License | Where |
|---|---|---|---|
| `tree-sitter-vb-dotnet` | 0.1.0 | MIT | crates.io; `crates/editor/Cargo.toml` |
| `tree-sitter-fsharp` | 0.3.12 | MIT | crates.io; `crates/editor/Cargo.toml` (exports `LANGUAGE_FSHARP` and `LANGUAGE_SIGNATURE`) |
| FsAutoComplete | 0.84.0 | MIT | external, located, never bundled; `tools/fsautocomplete/PIN` and the `dotnetTools` entry of `servers.json` agree (tested) |
| FSharp.Core | 10.1.302 | MIT | `corpus/tests/Directory.Packages.props`: an explicit reference is needed because central package management sets `DisableImplicitFSharpCoreReference`, and without it the test assembly has no `FSharp.Core.dll` beside it and NUnit finds zero tests |
| Roslyn | unchanged pin `7c238e7c` | MIT | `tools/roslyn-pin/vb.patch` adds one `ProjectReference` |

No other new dependency.

## What was built

**Editor (`crates/editor`).** `VISUAL_BASIC` (id `vb`, `.vb`), `FSHARP` (id `fsharp`, `.fs`, `.fsx`, `.fsscript`) and `FSHARP_SIGNATURE` (id `fsharp-signature`, `.fsi`) in the builtin table; `queries/vb/highlights.scm` written from the grammar's node types; `queries/fsharp/highlights.scm` adapted from tree-sitter-fsharp's to the editor's capture names and precedence rule, with `(access_modifier) @keyword` ahead of the declaration rules so `let private f x` keeps `private` as a keyword and highlights `f`; `queries/fsharp/signature-highlights.scm`, a reduced query, because the signature grammar has no expression layer and the main query does not compile against it. Fixtures in `corpus/languages/` (27 VB files, 6 F# files) shaped like real code; `corpus/languages/README.md` lists what each exercises.

**Servers (`crates/lsp`, the shell's `servers`).** `CommandSpec::dotnet_tool`; `ServerRegistry::dotnet_tools` (the pin and fetch script per tool, from `servers.json`), `dotnet_tool_cache`, `fetch_command_for`; `Located::envs` and `dotnet_tool_envs` (the `DOTNET_ROOT` rule, symlinks resolved, Windows' verbatim prefix stripped); `Environment::home` for the global tools folder (`DOTNET_CLI_HOME` first); `has_marker` matching a `*` marker against the folder's file names. The shell's `GenericLaunch` takes its cache folder and fetch script from the registration, and the spawn applies the located environment. `tools/fsautocomplete/{PIN,fetch.sh,fetch.ps1}`.

**Host and projects (`dotnet/`, `protocol/`, `crates/commands`, the shell).** `SolutionProjects.ProjectExtensions`, `IsProjectFile`, `Read` over the three kinds (the `.sln` regex and the `.slnx` filter); `LspProxy` accepts `.fsproj` and reports `loaded` for it through `MarkLoadedWithoutLanguageServerAsync` (after the Roslyn session exists, so restarts, replays and the missing-server failure keep their semantics); `workspace-tree.output.json`'s `kind` enum and the `editor.languages.vb.codeLens` and `editor.languages.fsharp.codeLens` settings; `workspace_tree::MSBUILD_KINDS`, `msbuild_kind`, `WorkspaceProject::is_msbuild`; the shell's `publish_workspace_tree` derives the kind from the extension and the startup filter covers every MSBuild kind; `target::solution_path` accepts `.fsproj`; the Debug slot strips any of the three extensions from the project name; `codelens::LANGUAGES` gains `vb` and `fsharp`.

**Roslyn (`tools/roslyn-pin`).** `vb.patch`, the patch step in both scripts, `README.md` with the Spike 2 write-up; `corpus/projects/VisualBasic/` (a `net10.0` console project with `Option Strict On` and one deliberate BC30512) and `dotnet/tests/Eludite.Host.Tests/VisualBasicTests.cs`.

## Tests (what each proves)

- `crates/editor/src/syntax/dotnet_tests.rs` (8 tests, 1 ignored timing test): the three ids resolve from their suffixes; VB and F# fixtures highlight the expected kinds at known positions (modifiers as keywords in VB; `let rec`, `private`, types, functions, strings, numbers, `///` comments, `true` in F#); every fixture parses with no error node except the ones listed below, each asserted at its exact count so a grammar bump that fixes or worsens one is noticed; the ignored `a_2000_line_file_parses_within_the_budget` enforces a 100 ms ceiling on a release build.
- `crates/lsp/src/registry.rs`: `.vb` resolves to `roslyn` with `vb`; `.fs`, `.fsi`, `.fsx` and `.fsscript` to `fsautocomplete` with `fsharp`; a glob root marker finds the folder holding an `.fsproj`; the .NET tool discovery order (beside, the variable, the pinned cache, the global tools folder, `PATH`); `DOTNET_ROOT` added for a .NET tool and not for anything else; the pin in `PIN` matches `servers.json`.
- `crates/lsp/tests/real_fsautocomplete.rs` (skips unless `ELUDITE_FSAUTOCOMPLETE` names a server and `dotnet` is on `PATH`): a temp `net10.0` F# project restored with `dotnet`, the real server started through the registration, a type error diagnosed and hover text on a function. Passed here against FsAutoComplete 0.84.0 installed by `tools/fsautocomplete/fetch.sh`.
- `crates/eludite/src/shell/dotnet_languages_tests.rs` (headless GPUI, the fake host and a `FakeServer`): a `.vb` document is the host's, opened with `languageId` `vb`, with its diagnostics as squiggles and no generic server started; a `.fs` under a folder with an `.fsproj` starts the `fsautocomplete` registration rooted at that folder with `AutomaticWorkspaceInit` and gets completion and a diagnostic through the shared paths; an `.fsx` outside any project is rooted at the solution's folder; a missing server says `not found (run tools/fsautocomplete/fetch.sh)` while highlighting stays.
- `crates/commands/src/workspace_tree.rs`: the kinds from the extension, every MSBuild kind in the schema's enum, startup on a `.vbproj`; `settings.rs`: the two CodeLens keys.
- `dotnet/tests/Eludite.Host.Tests/LegacyEvaluatorTests.cs`: `.sln` and `.slnx` with a legacy `.vbproj` (WebForms markup), an SDK `.fsproj` and a `.vdproj` (ignored), in order; single-file reads; `IsLegacy` and `MentionsMarkup` on the new kinds. `LspProxyTests.cs`: a lone `.fsproj` reaches `loaded` with one project and nothing sent upstream, a `.vbproj` is handed to Roslyn with `project/open`, a solution with an `.fsproj` goes whole with the right count.
- `dotnet/tests/Eludite.TestBridge.Tests/RunnerTests.cs`: `Corpus.VisualBasic` over MTP and `Corpus.FSharp` over VSTest discover 5 tests each with the expected outcomes, messages, line numbers, skip reasons and output; `TestServiceTests.cs`: eight containers through the host with the new totals.
- `dotnet/tests/Eludite.Host.Tests/VisualBasicTests.cs` (skips with `CodeLensTests`' message when the server is not built): the five Roslyn features above on `corpus/projects/VisualBasic` through the host.

## Grammar gaps (the fixtures assert these counts)

**Visual Basic** (`tree-sitter-vb-dotnet` 0.1.0; `Program.vb` and `Shapes.vb` are clean). One construct per file, with the `ERROR` plus `MISSING` node count today:

| Construct | File | Count |
|---|---|---|
| A comment before `Option`, a blank line between `Option` and `Imports` | `Header.vb` | 2 |
| Generic types and declarations (`Dictionary(Of String, T)`, `Class R(Of T As {...})`, `Function F(Of T)(...)`) | `Generics.vb` | 9 |
| `As New T(...)` in a field, a `Dim` and a `Using` | `AsNew.vb` | 5 |
| `Inherits` and `Implements` on their own lines | `InheritsLine.vb` | 2 |
| `Event` declarations with `RaiseEvent` | `RaiseEvent.vb` | 2 |
| `AddHandler` and `RemoveHandler` statements | `AddHandler.vb` | 5 |
| `WithEvents` and `Handles` | `Handles.vb` | 2 |
| `Custom Event` | `CustomEvent.vb` | 12 |
| `Await` in `Async` methods | `AsyncAwait.vb` | 3 |
| `Yield` in an `Iterator Function` | `Iterator.vb` | 2 |
| A LINQ query | `Linq.vb` | 4 |
| An XML literal | `XmlLiteral.vb` | 9 |
| `#Const`, `#Region`, `#If` between declarations | `Regions.vb` | 8 |
| `Operator +`, `Widening Operator CType` | `Operators.vb` | 9 |
| `Implements I.Member` on members | `ImplementsMember.vb` | 2 |
| `Enum E As Byte` | `EnumBase.vb` | 1 |
| `Dim a() As Integer = {...}`, `Dim a(2) As String` | `ArrayDeclarations.vb` | 3 |
| `For Each x As T In xs` | `ForEachTyped.vb` | 1 |
| `With` block statements starting with `.Member` | `WithBlock.vb` | 3 |
| `TypeOf x Is T` | `TypeOfIs.vb` | 1 |
| Two-argument `If(a, b)` | `IfCoalesce.vb` | 1 |
| `x?.Member` | `NullConditional.vb` | 2 |
| `New T From {...}` | `CollectionInitializer.vb` | 1 |
| An attribute on the declaration's line | `AttributeInline.vb` | 2 |
| Identifiers starting with a keyword (`Document`, `Format`, `Subtotal`) | `KeywordPrefixes.vb` | 5 |

Also: the grammar's keywords (`Sub`, `End Sub`, `If`, `Dim`, ...) are hidden tokens that a query cannot capture, so only the modifiers (`Public`, `Shared`, `Overrides`, ...) are painted as keywords; types, members, strings, numbers, comments, attributes and literals are.

**F#** (`tree-sitter-fsharp` 0.3.12; `Domain.fs`, `Library.fs`, `Program.fs`, `Script.fsx` and `Domain.fsi` are clean): member signatures in a signature file (`new:`, `member`, `static member`, `override`, `with get, set`, `interface` in a type, type extensions), `Members.fsi`, 3; the signature grammar has only `abstract` members and `val`.

## Budget numbers

Full non-incremental parse, release build of a standalone probe on the same tree-sitter 0.27 and grammar crates, best of 10, three runs, this 4-core VM at load 1.6 to 2.9, on the samples the ignored test builds (header once, body repeated):

| Sample | Lines | Size | Parse |
|---|---|---|---|
| Visual Basic (`Program.vb` repeated) | 2,092 | 70 KB | 9.6 ms (budget 10 ms) |
| F# (`Library.fs` repeated) | 2,099 | 57 KB | 29.0 to 30.7 ms (over the 10 ms budget; the test's ceiling is 100 ms) |
| F# signature (`Domain.fsi` repeated) | 2,019 | 50 KB | 11.5 to 12.3 ms (slightly over) |

All three parse with no error node. The F# number is a property of the grammar (a 56 MB generated parser); the editor's keystroke path re-parses incrementally, which the brief's budget does not measure, so the product impact is the first paint of a large F# file. Roslyn, Visual Basic through the host: see the summary (diagnostics 2.3 s from open on a cold server, hover 163 ms, completion 669 ms, definition 36 ms, rename 723 ms). The Roslyn server build: 217 s, 137 MB output.

## Checks run here

- `cargo fmt --check`: clean. `cargo clippy --workspace --all-targets -- -D warnings`: clean.
- `cargo test -p eludite-editor`: 83 passed, 1 ignored; `cargo test -p eludite-lsp` (with the real FsAutoComplete test): passed; `cargo test -p eludite-commands`: 121 passed.
- `cargo test -p eludite` (403 tests, `CARGO_INCREMENTAL=0` to fit the disk): 401 passed, 2 failed in the full parallel run on this loaded 4-core VM, both timing-sensitive and untouched by this brief: `git_tests::a_thousand_changed_files_draw_in_a_frame` (the slowest frame 8.83 ms against 8 ms) and `debug::tests::the_end_of_session_summary_lists_what_failed_in_the_session` (a debuggee driven with `wait_ms` deadlines); both pass when run alone (1.2 s). The new `dotnet_languages_tests` passed in the full run.
- `corpus/tests/build.sh --no-cargo`: both new projects build, 0 warnings. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.
- `dotnet test` prints "Zero tests ran" in this container (the SDK here picks Microsoft.Testing.Platform's runner for the xunit.v3 projects while they are built for the in-process one; CI's SDK does not), so the test assemblies were run directly: `Eludite.TestBridge.Tests` 27 passed, 1 skipped (Mono); `Eludite.Host.Tests` 181 passed, 7 skipped (Mono, the legacy corpus, the benchmark solution), including `TestServiceTests` (8 containers) and `VisualBasicTests` against the patched server (16 s); `Eludite.Web.Tests` 22 and `Eludite.Wcf.Tests` 8 passed.

## Not done, and why

- **A Visual Basic grammar worth the name.** `tree-sitter-vb-dotnet` 0.1.0 is the only VB.NET grammar on crates.io and it misses generics, events, `Await`, LINQ, XML literals, operators, directives between declarations and more (the table above), with keywords uncapturable. The brief put grammar fixes out of scope. The right follow-up is a brief like 0056: fork it into `grammars/vb` under ADR-0012 (generate from the checked-in grammar JSON at build time), make the keywords visible nodes, add the missing constructs with corpus cases, and measure on real projects. Until then VB highlighting is partial on real code; the semantic features all come from Roslyn and are complete.
- **F# first-parse time** (29 ms on 2,000 lines) is over the brief's 10 ms. It would take grammar work upstream (ionide/tree-sitter-fsharp) or a trimmed in-repo fork; not attempted.
- **A headless shell test that opens a `.vbproj` or `.fsproj` as the workspace and reads `eludite.workspace.tree`'s kinds**: the kind derivation is unit-tested in `crates/commands`; the shell's `tests.rs` was not in the projects agent's files.
- **Windows and macOS**: `build.ps1` and `fetch.ps1` are written in their siblings' style and untested here (no PowerShell); CI runs the Rust tests on all three and the .NET tests on Linux and Windows.
- **Formatting F# (Fantomas), F# Interactive, FsAutoComplete's own `fsharp/*` requests, Expecto's adapter, VB WebForms code-behind through the legacy designer**: out of scope by the brief.
- `SolutionConfigurationFile.cs` keeps its own three-extension checks rather than `SolutionProjects.IsProjectFile`; both agree.
