# Brief 0057: Visual Basic and F# as first-class .NET languages

Status: in progress
Phase: 2 (PLAN.md section 10: "The other first-class families from section 7: F# and VB.NET"; section 7, the .NET row)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 6, 12), 4.1 (syntax highlighting from tree-sitter), 4.2 (the Workspace window), 4.3 ("F# via FsAutoComplete (MIT) and VB.NET via the same Roslyn server are Phase 2 work. Spike 2 must confirm which VB features the Roslyn language server exposes outside Visual Studio; gaps are filled in eludite-host"), 4.5, 4.6, 7 (the .NET row: `.fsproj` and `.vbproj`, Expecto and NUnit for F#), 9, 10 (Phase 2)
Related ADRs: ADR-0002 (process topology: FsAutoComplete is a process the shell speaks LSP to, like rust-analyzer), ADR-0003 (protocols: LSP, no bespoke hooks), ADR-0005 (licensing: every new dependency MIT)
Depends on: brief 0019 (the generic server path and `servers.json`), brief 0050 (several servers per file, the language table, `languageIds` per glob, npm discovery), brief 0056 (the grammar registration pattern), brief 0035 (the Test Explorer corpus), brief 0049 (project files of the open solution), brief 0007 (the host bridge).

## Goal

A person opens a workspace holding Visual Basic or F# projects and gets what C# gets today, through the same paths: the Workspace window lists `.vbproj` and `.fsproj` projects with their files, the editor highlights `.vb`, `.fs`, `.fsi` and `.fsx` at once from tree-sitter grammars, `.vb` documents go to Roslyn through `eludite-host` with the `vb` language id, `.fs` documents go to FsAutoComplete (MIT) through the generic server path with the same discovery, status bar slot and fetch script pattern as rust-analyzer and the web servers, builds and the Error List work unchanged (MSBuild is language-neutral), tests in VB and F# projects are discovered and run by the Test Explorer over MTP and VSTest, and F5 debugs them with netcoredbg (the launch reader already reads `.vbproj` and `.fsproj`). Spike 2 of PLAN.md section 4.3 is answered in writing: the pinned Roslyn language server composes only `Microsoft.CodeAnalysis.CSharp.Features`, so it skips VB projects (`LanguageServerProjectLoader` drops a project whose language has no `ICommandLineParserService`); `tools/roslyn-pin` therefore adds the VB feature assemblies to the server's build, and the report records what the server then exposes for VB.

## Files in scope

**Editor (owner: the grammars agent).**
- `crates/editor/Cargo.toml` (`tree-sitter-vb-dotnet` 0.1, `tree-sitter-fsharp` 0.3; both MIT), `Cargo.lock`.
- `crates/editor/src/syntax/language.rs`: `VISUAL_BASIC` (id `vb`, name `Visual Basic`, suffixes `vb`), `FSHARP` (id `fsharp`, name `F#`, suffixes `fs`, `fsx`, `fsscript`) and `FSHARP_SIGNATURE` (id `fsharp-signature`, name `F# signature`, suffix `fsi`, the `fsharp_signature` grammar) in `BUILTINS`; crate docs.
- `crates/editor/queries/vb/highlights.scm` (written for this brief: the VB grammar ships none), `crates/editor/queries/fsharp/highlights.scm` (adapted from tree-sitter-fsharp's, with its license line, to the editor's capture names and precedence rule).
- `crates/editor/src/syntax/dotnet_tests.rs` (new, registered in `syntax/mod.rs` under `#[cfg(test)]`).
- `corpus/languages/vb/` and `corpus/languages/fsharp/` (new, MIT): fixtures covering the constructs below, `corpus/languages/README.md`, the row in `corpus/README.md`.

**Language servers (owner: the servers agent).**
- `crates/lsp/src/servers.json`: the `roslyn` entry gains `*.vb` with `languageIds` `{"*.vb": "vb"}` and the name `C# and Visual Basic`; a new `fsautocomplete` entry (`languageId` `fsharp`, globs `*.fs`, `*.fsi`, `*.fsx`, `*.fsscript`, `via` `process`, root markers `*.sln`, `*.slnx`, `*.fsproj` by glob, command `fsautocomplete` with `envOverride` `ELUDITE_FSAUTOCOMPLETE` and `dotnetTool` `fsautocomplete`, `initializationOptions` `{"AutomaticWorkspaceInit": true}`).
- `crates/lsp/src/registry.rs`: `CommandSpec::dotnet_tool` (a .NET global tool: searched beside eludite, at the override variable, in `tools/<tool>/PIN`'s cache folder `~/.cache/eludite/<tool>/<pin>/`, in the .NET global tools folder `~/.dotnet/tools` (`%USERPROFILE%\.dotnet\tools`), then `PATH`), root markers with `*` matched by glob, `Located::envs` (a .NET tool's apphost needs `DOTNET_ROOT` when `dotnet` is not at the default location: the resolved `dotnet`'s folder when the variable is unset), tests.
- `crates/lsp/src/connection.rs` and `crates/eludite/src/shell/servers.rs` only as far as passing those environment variables to the spawned server.
- `crates/lsp/tests/real_fsautocomplete.rs` (new; skips unless `ELUDITE_FSAUTOCOMPLETE` names a server), `crates/eludite/src/shell/dotnet_languages_tests.rs` (new, headless: a `.vb` document is served by the host with `languageId` `vb`; a `.fs` document starts the `fsautocomplete` registration against a `FakeServer`, with completion and diagnostics through the shared paths; a server that is not found says `not found (run tools/fsautocomplete/fetch.sh)`), its `mod` line in `crates/eludite/src/shell.rs`.
- `tools/fsautocomplete/PIN`, `tools/fsautocomplete/fetch.sh`, `tools/fsautocomplete/fetch.ps1`: `dotnet tool install fsautocomplete --version <pin> --tool-path ~/.cache/eludite/fsautocomplete/<pin>` and the printed path.

**Projects, host, tests (owner: the projects agent).**
- `dotnet/src/Eludite.Host/Legacy/SolutionProjects.cs` (`.csproj`, `.vbproj` and `.fsproj` from `.sln`, `.slnx` and a single project file), `dotnet/src/Eludite.Host/Lsp/LspProxy.cs` (`.fsproj` accepted as the open target; no `project/open` to Roslyn for an `.fsproj`, which it cannot load), `dotnet/src/Eludite.Host/Build/BinlogReader.cs` and `Projects/SolutionConfigurationFile.cs` only if they still exclude a kind, their tests in `dotnet/tests/Eludite.Host.Tests/`.
- `protocol/schemas/host/solution-open.json`, `protocol/schemas/solution-open.input.json`, `protocol/schemas/host-rpc.md` (the descriptions name `.fsproj`), `protocol/schemas/workspace-tree.output.json` (`kind` gains `vbproj` and `fsproj`), `crates/commands/src/workspace_tree.rs` (the docs and tests of `kind`), `crates/eludite/src/shell/folder.rs` (the kind from the project file's extension; nothing filters on `"csproj"` that should mean any MSBuild project), `crates/eludite/src/shell/target.rs` (`.fsproj` as an open target), `crates/eludite/src/shell/codelens.rs` (`LANGUAGES` gains `vb` and `fsharp`), `crates/commands/src/settings.rs` or wherever `editor.languages.<id>.codeLens` keys are enumerated.
- `corpus/tests/Corpus.VisualBasic/` (MSTest on Microsoft.Testing.Platform, `.vbproj`) and `corpus/tests/Corpus.FSharp/` (NUnit on VSTest, `.fsproj`), each with a passing, a failing, a skipped and an output-writing test, `corpus/tests/build.sh`, `build.ps1`, `README.md`, `Directory.Packages.props` only if a version is missing; `dotnet/tests/Eludite.TestBridge.Tests/Corpus.cs` (`Projects`, the project file extension per project, the generated `.slnx`), `dotnet/tests/Eludite.Host.Tests/TestServiceTests.cs` and `dotnet/tests/Eludite.TestBridge.Tests/RunnerTests.cs` (the expected containers and counts).

**Roslyn (owner: the Roslyn agent).**
- `tools/roslyn-pin/build.sh`, `tools/roslyn-pin/build.ps1`, `tools/roslyn-pin/vb.patch` (new): after the checkout, apply the patch adding `<ProjectReference Include="..\..\Features\VisualBasic\Portable\Microsoft.CodeAnalysis.VisualBasic.Features.vbproj" />` to `src/LanguageServer/Microsoft.CodeAnalysis.LanguageServer/Microsoft.CodeAnalysis.LanguageServer.csproj` (idempotent: skipped when already applied), `tools/roslyn-pin/README.md` (new or extended: why, and what the server exposes for VB).
- `dotnet/tests/Eludite.Host.Tests/VisualBasicTests.cs` (new; skips when the pinned server is not built): opens `corpus/projects/VisualBasic/VisualBasic.vbproj` through the host, opens its `.vb` with `languageId` `vb`, and asserts completion, hover and a diagnostic from Roslyn; `corpus/projects/VisualBasic/` (new, MIT), the row in `corpus/projects/README.md`.

**Docs (owner: the integrator).** `CLAUDE.md` (the build paragraph's optional tools, the crate map rows for `crates/editor`, `crates/lsp`, `dotnet/`, `tools/`), `README.md` (status and the optional tools list), `docs/briefs/README.md` (the row), `docs/briefs/0057-report.md`, this file.

Nothing in `vendor/`, `docs/PLAN.md`, `corpus/dap/` or the generated `protocol/rust/src/cdp/`.

## Contract

**Editor.**
- One `LanguageConfig` per language; grammars from crates.io (`tree-sitter-vb-dotnet` 0.1.0, `tree-sitter-fsharp` 0.3.12, both MIT, ABI compatible with `tree-sitter` 0.27). `for_path` maps `.vb` to `vb`, `.fs`/`.fsx`/`.fsscript` to `fsharp`, `.fsi` to `fsharp-signature`. No injections, no Emmet.
- The queries follow the editor's precedence rule (an earlier pattern wins on the same node) and the C# query's conventions: comments, strings and their escapes, numbers, `@constant.builtin` for `True`/`False`/`Nothing` and `true`/`false`/`()`, keywords (VB's are case-insensitive: the query matches the grammar's anonymous nodes, which the grammar defines case-insensitively), types, type parameters, functions and methods at their declarations and calls, parameters, properties and members, attributes, preprocessor lines, VB's XML doc comments (`'''`) as comments, F#'s `///` as comments, operators left in the default color like C#'s punctuation.
- Fixtures parse with no `ERROR` or `MISSING` node, except where a construct is unsupported by the grammar: the report lists each such construct (the VB grammar's README says LINQ and XML literals are planned); the fixtures keep one file per unsupported construct, marked in `corpus/languages/README.md`, and the test asserts the known error count so a grammar bump that fixes it is noticed.
- VB fixtures cover: `Option Strict`, `Imports`, `Namespace`, `Module` with `Sub Main`, `Class` with `Inherits` and `Implements`, `Structure`, `Interface`, `Enum`, `Delegate`, `Event` and `RaiseEvent`, `AddHandler`, `Handles`, auto and full `Property`, `Shared`, `Overrides`, `Overloads`, `Async`/`Await`, generics with constraints, lambdas (`Function(x) x * 2`, multi-line `Sub()`), `If`/`ElseIf`, `Select Case`, `For`, `For Each`, `Do While`, `While`, `Try`/`Catch When`/`Finally`, `Using`, `With`, `SyncLock`, string interpolation `$"..."`, date literals, type characters, line continuations, `'''` doc comments, `#Region`, `#If`, attributes, `Dim ... As New`, `ReDim`, LINQ (`From x In xs Where ... Select`), an XML literal.
- F# fixtures cover: a module and a namespace, `open`, `let` and `let rec`, `let mutable`, functions with tupled and curried parameters, type annotations, records with `{ x with }`, discriminated unions, `match` with guards and active patterns, `option`, lists, arrays, sequences, computation expressions (`async { }`, `task { }`, `seq { }`), pipelines, lambdas, classes with members and constructors, interfaces and object expressions, `[<Attribute>]`, exceptions with `try ... with` and `try ... finally`, string interpolation `$"..."`, triple-quoted strings, `///` doc comments, `(* *)` comments, units of measure, `#if INTERACTIVE`, a `.fsx` script with `#r "nuget: ..."`, a `.fsi` signature.

**Language servers.**
- `.vb` goes to the host as the `roslyn` registration's second glob with `languageId` `vb`; the host forwards `textDocument/didOpen` unchanged (it already carries the client's `languageId`), and Roslyn chooses the language from the extension. Nothing C#-specific is added to the host.
- `fsautocomplete` is a `process` registration like `rust-analyzer`: the generic client, `rootMarkers` naming `*.fsproj`, `*.sln` and `*.slnx` (the registry's `find_root` matches a marker with `*` against the folder's file names; markers without `*` stay exact), `initializationOptions` `{"AutomaticWorkspaceInit": true}` so the server loads the workspace's projects on its own. Unknown server notifications (`fsharp/notifyWorkspace`, `fsharp/notifyWorkspacePeek`, `fsharp/documentAnalyzed`, `fsharp/testDetected`) are logged and ignored like today's.
- Discovery order for a .NET tool: beside `eludite`, `ELUDITE_FSAUTOCOMPLETE`, the pinned cache folder, `~/.dotnet/tools`, `PATH`. Each candidate proves it runs with `--version`. The spawned process gets `DOTNET_ROOT` set to the folder of the `dotnet` the shell resolved from `PATH` when the variable is unset and the tool is a .NET tool (the apphost refuses to start otherwise on a user-local SDK); the fake-server tests pass an executable that is not a .NET tool and see no such variable.
- `tools/fsautocomplete/fetch.sh` installs the pinned version with `dotnet tool install --tool-path` into `~/.cache/eludite/fsautocomplete/<pin>/`, prints the path, and is idempotent. `PIN` holds `version 0.84.0`.
- The status bar slot, Output source and `languageServers.*` settings follow the generic path unchanged.

**Projects and tests.**
- `SolutionProjects.Read` returns `.csproj`, `.vbproj` and `.fsproj` paths in solution order; `IsLegacy` and `MentionsMarkup` keep working on any of them; `LegacyDesignTime` keeps its WebForms rule (markup lives in `.csproj` and `.vbproj` projects alike; the evaluator is language-neutral).
- `eludite/solution/open` accepts a `.fsproj` and does not hand it to Roslyn (`project/open` is for C# and VB); a solution with F# projects is handed to Roslyn whole, which skips them itself; the tree, properties, configurations and tests see them through `SolutionProjects.Read`.
- `eludite.workspace.tree` reports `kind` `csproj`, `vbproj` or `fsproj` from the project file's extension; schema first (`workspace-tree.output.json`), then `WorkspaceProject`'s docs and the shell. The Workspace window, startup project list and NuGet node treat the three alike.
- `Corpus.VisualBasic` (MSTest on MTP, `net10.0`) has 5 tests: one fails, one is skipped with a reason, one writes output, two pass; `Corpus.FSharp` (NUnit on VSTest, `net10.0`) has 5 tests: one fails, one is ignored with a reason, one writes output, two pass. `corpus/tests/README.md` lists both with their expected outcomes. The bridge's and host's tests build the corpus solution with both and assert the containers and counts. The corpus builds on CI's Linux and Windows .NET jobs (`corpus/tests/build.sh` and `build.ps1` include both).

**Roslyn.**
- `tools/roslyn-pin/vb.patch` adds the one `ProjectReference`; `build.sh` and `build.ps1` apply it with `git apply --check` first and skip when already applied; the pin's `COMMIT` is unchanged. The report records whether the patched server was built here, its build time, and the VB features seen through the host (completion, hover, diagnostics, go to definition, rename if time permits).
- `VisualBasicTests` skips with the same message as `CodeLensTests` when the server is not built.

Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `cargo test -p eludite-editor dotnet_tests`: the three ids resolve from their suffixes; VB and F# fixtures highlight the expected kinds at known positions (`Keyword` for `Public Class`/`End Class` and `let rec`, `Type` for a type name, `Function` for a method name and an F# function, `String`, `Number`, `Comment` for `'''` and `///`, `ConstantBuiltin` for `Nothing` and `true`); every fixture parses with no error nodes except the listed constructs.
- `cargo test -p eludite-lsp`: `.vb` resolves to `roslyn` with `vb`; `.fs`, `.fsi`, `.fsx` resolve to `fsautocomplete` with `fsharp`; a glob root marker finds the folder holding an `.fsproj`; the .NET tool discovery order; `DOTNET_ROOT` added for a .NET tool and not otherwise.
- `cargo test -p eludite dotnet_languages_tests`: headless, against the fake host and a `FakeServer` for FsAutoComplete.
- `cargo test -p eludite-lsp --test real_fsautocomplete` with `ELUDITE_FSAUTOCOMPLETE` set: the real server on a small F# project (hover and completion on a `.fs` with a diagnostic).
- `dotnet test dotnet/Eludite.slnx`: `SolutionProjects` lists all three kinds; the bridge discovers and runs `Corpus.VisualBasic` and `Corpus.FSharp` with the expected outcomes; `VisualBasicTests` against the patched server when built.
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `dotnet build` and `dotnet test` green here and on CI.

## Budget

- The editor's keystroke and highlighting budgets (PLAN.md section 9) unchanged on VB and F# documents; a 2,000-line VB file and a 2,000-line F# file each parse in under 10 ms in release.
- The cold `cargo build` grows by the two grammars' C compile time (the F# grammar is 56 MB of generated C plus 20 MB for signatures; the VB one 10 MB): the report records the time and the release binary's size delta.
- No network at startup: FsAutoComplete is located, never fetched, at run time.
- New dependencies: `tree-sitter-vb-dotnet` 0.1.0 (MIT), `tree-sitter-fsharp` 0.3.12 (MIT). FsAutoComplete 0.84.0 (MIT) is an external tool, located, never bundled. No other.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, `dotnet build dotnet/Eludite.slnx` and `dotnet test dotnet/Eludite.slnx` green on Linux here and on CI's platforms.
2. The report answers Spike 2 (what the pinned Roslyn server does with VB before and after the patch) and lists every grammar gap found by the fixtures.
3. `CLAUDE.md`'s crate map, the build paragraph, `README.md`, the briefs index and `corpus/README.md` match the repository.

## Out of scope

- A Razor-style in-repo grammar for VB or F# (crates.io grammars are used as they are; gaps are reported, not fixed here); LINQ and XML literal highlighting beyond what the grammar gives; semantic tokens; F# formatting with Fantomas; FsAutoComplete's own commands (`fsharp/*` requests: signature data, documentation, FSI); F# Interactive; Paket; Expecto's own protocol (Expecto runs through its VSTest adapter only if a later brief adds it); Visual Basic WebForms code-behind through the legacy designer; the Windows and macOS runs beyond CI; publishing anything; changing the Roslyn pin's `COMMIT`.
