# Brief 0076: The Test Explorer at Visual Studio's daily depth, and code coverage

Status: in progress (2026-10-12)
Phase: 2 (PLAN.md section 10: on par with Visual Studio and Rider for daily .NET work)
Plan reference: PLAN.md sections 2 (principles 1, 2, 3, 4, 5, 6, 12), 4.6 (tests: "Test Explorer ... code coverage with editor highlighting"), 9 (budgets)
Related ADRs: ADR-0002 (process topology: runners and coverage collectors stay out of the shell), ADR-0003 (protocols: MTP, VSTest), ADR-0014 (tests fail on behavior only)
Depends on: brief 0035 (the Test Explorer), brief 0049 (the host's MSBuild evaluation), brief 0052 (CodeLens test lenses), brief 0048 (NuGet service, for the coverage package offer), brief 0063 (VB and F# test projects).

## Goal

The Test Explorer becomes the window a person keeps open all day, as in Visual Studio and Rider: new tests appear after any build (Ctrl+Shift+B included) without the tree emptying; test projects are found the way MSBuild sees them (Directory.Build.props, `IsTestProject`, `Platform`, `RuntimeIdentifier`, `OutputPath`, the artifacts layout); Ctrl+R, T runs what the caret is in; the details pane's stack frames are links; Debug Test stops only where the person asked. The tree groups like Visual Studio's (Group By), shows data-driven cases under their method, has a context menu and keyboard navigation, and the Workspace window runs a project's tests. Test > Analyze Code Coverage collects Cobertura coverage out of process, shows it in the Code Coverage Results window and colors the editor's lines.

## Files in scope

- Shell: `crates/eludite/src/shell/test_runs.rs`, `tests_window.rs`, `test_runs_tests.rs`, new `coverage.rs` (model, the overlay computation off the UI thread, the command answers), `coverage_window.rs` (the Code Coverage Results window), `coverage_tests.rs`; one marked block each in `crates/eludite/src/shell.rs` (mod lines, the command handlers, the coverage decorations call), `explorer.rs` (Run Tests / Debug Tests on test projects and the root), `codelens.rs` only if the caret rule needs it (marked).
- `crates/ui/src/test_explorer.rs` (the context menu row and the coverage glyphs), theme tokens additive in `crates/ui` (`coverage_covered`, `coverage_partial`, `coverage_uncovered`), additive entries in `crates/ui/src/menu.rs` and `keymap.rs`.
- `crates/commands/src/test.rs` (the new commands), one line in the builtins/registration list if needed.
- `crates/docking/src/model.rs`: the window id `code_coverage_results` (additive).
- `protocol/schemas/test-*.json` (edits), new `test-coverage.input.json`, `test-coverage.output.json`, `test-coverage-results.input.json`, `test-coverage-results.output.json`, `test-coverage-window.input.json`, `test-coverage-coloring.input.json`; `protocol/schemas/settings.json` (keys `test.groupBy`, `test.runAfterBuild`, `test.showCoverageColoring`, additive); `protocol/schemas/host/test-discover.json`, `test-run.json`, `test-update.json` (additive members), `host-rpc.md` ("Tests", additive), `protocol/rust/src/host.rs` (the mirror, additive block), `crates/lsp/src/fake.rs` (the fake host's coverage update and the new params, one marked block).
- `dotnet/src/Eludite.TestBridge/**`, `dotnet/src/Eludite.Host/Testing/**` (new `TestProjectEvaluator.cs`, `Cobertura.cs`, `CoverageMessages` in `TestMessages.cs`), their tests in `dotnet/tests/Eludite.TestBridge.Tests` and `Eludite.Host.Tests`.
- `corpus/tests/**`: new variants under `corpus/tests/Variants/` (keep every existing project's expected outcomes): `Corpus.PropsDriven` (MSTest on MTP whose test packages and `IsTestProject` come from a `Directory.Build.props`), `Corpus.Platform64` (xunit 2 on VSTest, `<Platforms>x64</Platforms>`, `<PlatformTarget>x64</PlatformTarget>`), `Corpus.Artifacts` (MSTest on MTP with `UseArtifactsOutput`), `Corpus.Coverage` (MSTest on MTP referencing `Microsoft.Testing.Extensions.CodeCoverage`), `Corpus.Coverlet` (xunit 2 on VSTest referencing `coverlet.collector`); `build.sh`, `build.ps1`, `README.md`, `Directory.Packages.props` (the two coverage packages).
- `docs/briefs/0076-report.md`, this file.

Not in scope: `README.md`, `CLAUDE.md`, `docs/briefs/README.md`, `docs/PLAN.md` (the report's Integration notes carry their lines), other briefs' files.

## Contract

**P0: rediscovery after any build.**
- A build the Test Explorer did not start, once finished successfully, rediscovers the outputs (no new build) when the tree was discovered before or the window is visible; the old tree stays shown until the new discovery's containers answer, then the .NET part is replaced container by container (no empty frame), outcomes of tests that keep their id are kept.
- A new solution generation keeps the shown tree, marked stale (`stale: true` in the window and in `eludite.test.discover`'s answer), until the next discovery under the new generation replaces it. Results computed under the old generation are still dropped (invariant 12).

**P0: test projects as MSBuild sees them.** The host evaluates each project of the solution with the brief 0049 MSBuild path (`Configuration` and `Platform` as global properties; `TargetFramework` per framework) and decides from the evaluation: `IsTestProject`, `TestingPlatformDotnetTestSupport`, `EnableMSTestRunner`, `UseMicrosoftTestingPlatformRunner`, `EnableNUnitRunner`, `UseVSTest`, the `PackageReference` items (wherever they are declared: Directory.Build.props, Directory.Packages.props, the project), `MSTest.Sdk`, TUnit (`TUnit` or `TUnit.Engine`: MTP); the program is the evaluated `TargetPath` (the apphost `.exe` for a .NET Framework executable), so `OutputPath`, `Platform`, `RuntimeIdentifier`, `AppendTargetFrameworkToOutputPath` and the artifacts layout hold. The coverage tool comes from the same packages (`Microsoft.Testing.Extensions.CodeCoverage`: `mtp`; `coverlet.collector`: `coverlet`; `Microsoft.CodeCoverage` through `Microsoft.NET.Test.Sdk`: `microsoft` for VSTest). When MSBuild is not available the file-only `TestProjectInspector` answers, as before. `eludite/test/discover` and `eludite/test/run` take the solution `platform` (Any CPU is MSBuild's `AnyCPU`).

**P0: Ctrl+R, T and Debug at the caret.** With the editor focused (the last focused pane is a document), Run Tests / Debug Tests take the caret's scope even when the Test Explorer has a selection: the test whose method contains the caret; the caret on a test class's declaration line or in its body outside any member: the class's tests; anywhere else in a file with tests: the file's tests; the caret in a non-test member: that member is not a test, so the enclosing class's tests run only when the caret is on the class line, else the file's tests. Member and class extents come from the test lines and a brace/indent scan of the document (C#, VB `End Sub`/`End Function`, F# indentation). From the Test Explorer (it has focus, its buttons, its context menu) the selection is used.

**P0: details pane.** Each stack frame line with a file and line (`at ... in <file>:line <n>`, and Rust's `<file>:<line>:<col>`) is a link that opens the file at the line (`eludite.file.open`); Copy (button and Ctrl+C on the pane) copies the details as text; the pane's height is resizable by dragging its top border (80 to 600 px), kept for the session.

**P0: Debug Test stops where the person asks.** Debug Test from the window, the menu, the keys and the lenses sets no breakpoint: the session stops at the person's breakpoints and exceptions. `eludite.test.debug` from an agent (another thread) keeps the temporary function breakpoint on the first test unless `break_at_test: false`; `break_at_test: true` from the UI asks for it. Debugging tests of several projects is refused with a message naming them (`the tests to debug are in N projects (A, B): debug one project at a time`), except Debug All Tests without a selection, which debugs the first project and says so in the Output window's Tests source and the answer's `message`.

**P1: the tree.**
- Group By (the toolbar's Group By menu, setting `test.groupBy`, one of `project` (default: project > namespace > class), `namespace` (namespace > class), `class`, `outcome` (outcome > project > namespace > class), `duration` (Fast < 100 ms, Medium < 1 s, Slow, Not Run > project > ...), `traits` (trait > project > ...; a test with no trait under `No Traits`), `target_framework` (framework > project > ...)).
- A method with data-driven cases (several tests sharing a fully qualified name, or a display name with arguments) is a method node aggregating glyph, count and duration; run-time expanded cases take the method's source and line from a sibling.
- A Duration column (each row's duration, nodes summed) and Sort By (name, duration) in the toolbar menu, persisted for the session.
- Row context menu: Run, Debug, Analyze Code Coverage, Go To Test (F12), Copy (Ctrl+C: the test's full name, or the node's tests' names); keyboard: Up, Down, Left (collapse or go to parent), Right (expand or first child), Enter (run the selection; a test row with Ctrl: go to it), F12, Ctrl+C, Home, End.
- Run Tests and Debug Tests on a test project and on the solution root in the Workspace window's context menu (one marked block in `explorer.rs`).
- Ctrl+R, F: Run Failed Tests (keymap and menu).
- Requested tests show the running glyph from the run's start until their result arrives (xunit.v3 sends no in-progress state).
- The search box: `Outcome:"Not Run"` and `Outcome:NotRun` work (quoted values and `Not Run` as two words), paste (Ctrl+V), a caret moved by Left/Right/Home/End, Delete.
- Test > Run Tests After Build toggles `test.runAfterBuild`: after a successful build the Test Explorer did not start, the rediscovered tests that were affected (every test of a rebuilt container) run.
- The tree and the last outcomes are cached per workspace under `.eludite/tests.json` and shown at startup marked stale until a discovery replaces them.
- `Test > Configure Run Settings > Select Solution Wide runsettings File` sets `test.runSettings` (an open-file dialog through the existing settings command).

**P1: code coverage.**
- `eludite.test.coverage` (execute; schema `test-coverage.input.json`/`.output.json`): the run's selection members (`ids`, `filter`, `project`, `selection`, `failed_only`) and `wait_ms`; runs the tests with coverage, answers the run and, once done, the coverage summary (line and branch rate, per assembly). Test > Analyze Code Coverage for All Tests, and for Selected Tests (the Test Explorer's context menu and toolbar).
- The host collects per container: MTP with `Microsoft.Testing.Extensions.CodeCoverage`: `--coverage --coverage-output-format cobertura --coverage-output <file>`; VSTest with `coverlet.collector`: run settings with the `XPlat Code Coverage` data collector, `Format` `cobertura`; VSTest with only `Microsoft.CodeCoverage`: the `Code Coverage` collector with `Format` `Cobertura`. A container without a collector reports `coverage: none` with the message `add the Microsoft.Testing.Extensions.CodeCoverage package (MTP) or coverlet.collector (VSTest) to collect coverage`. The reports are parsed in the host (`Cobertura.cs`) into one merged typed result (assembly > class > method with covered and coverable lines, and per file the lines' hits and branch counts), sent as the update kind `coverage` before `finished` (`host/test-update.json`).
- `eludite.test.coverage_results` (read): the last coverage's rows (assembly, namespace, class, method; covered, not covered, %), and for `file` its line states; paged.
- The Code Coverage Results window (`code_coverage_results`, Test > Windows > Code Coverage Results): assembly > namespace > class > method with Covered (Lines), Not Covered (Lines), Covered (% Lines); double-click opens the source; a toolbar with Show Code Coverage Coloring and the run's summary.
- The editor colors covered, partially covered and uncovered lines in the gutter (a 3 px bar, theme tokens `coverage_covered`, `coverage_partial`, `coverage_uncovered`) of every open document of a covered file; the overlay is computed off the UI thread; Show Code Coverage Coloring (`eludite.test.coverage_coloring`, setting `test.showCoverageColoring`) toggles it; a document edited after the coverage ran shows it as stale (muted) until the next coverage.

Every new user-visible action is a command with a schema checked in first; agents reach them over MCP (`eludite.test.coverage`, `eludite.test.coverage_results`); the windows' commands are view commands.

## Proving test

- `cargo test -p eludite test_runs_tests coverage_tests tests_window`: headless against the fake host: a build the Test Explorer did not start rediscovers and the tree never empties (the rows are checked between the build's end and the discovery's answer); a new generation keeps the tree stale; Ctrl+R, T in the editor runs the caret's test, the class at the class line, the file's tests outside a test, and never the test above a non-test member, even with a Test Explorer selection; a stack frame row in the details is a link that opens the file at its line; Debug Test from the window sets no breakpoint, from an agent sets one; debugging two projects is refused with a message; Group By outcome and namespace produce the expected rows; a data row sits under its method node with the aggregate; the context menu's Run, Go To Test and Copy; keyboard navigation; `Outcome:"Not Run"`; Run Tests After Build; the cache shown stale at startup; Workspace's Run Tests; a coverage run passes `coverage: true`, the fake host's `coverage` update fills the Code Coverage Results window and colors an open document's lines, Show Code Coverage Coloring toggles it, an edit marks it stale; 10,000-test regroup under 50 ms (`assert_budget`).
- `dotnet test dotnet/Eludite.slnx`: the evaluator finds `Corpus.PropsDriven`, `Corpus.Platform64` (with `platform: x64`, `bin/x64/Debug/...`) and `Corpus.Artifacts` (`artifacts/bin/...`), discovers and runs them; TUnit by package; the Cobertura parser on a checked-in sample; coverage collected from `Corpus.Coverage` (MTP) and `Corpus.Coverlet` (VSTest) through the real host, with the expected covered and uncovered lines of their `Calculator` class.
- Debug Test against the real netcoredbg (`ELUDITE_NETCOREDBG`; skips otherwise): the corpus's MSTest test stops only at a user breakpoint.
- `cargo fmt --check`; `cargo clippy --all-targets -- -D warnings` and `cargo test` for each touched crate; `dotnet build` and `dotnet test` of the touched projects.

## Budget

- Rediscovery after a build runs off the UI thread (the host) and replaces the tree without an empty frame.
- Regrouping a 10,000-test tree under 50 ms.
- The coverage overlay of a 5,000-line file computed off the UI thread; applying it costs one decoration set.
- No new Rust dependency. Corpus-only NuGet packages: `Microsoft.Testing.Extensions.CodeCoverage` (MIT), `coverlet.collector` (MIT).

## Exit criterion

1. The proving tests pass here; fmt and clippy clean on the touched crates; `dotnet build` with no warning.
2. The report lists what ran, the numbers, what is owed, and the Integration notes (the crate map rows, the README status line, the briefs index row).

## Out of scope

Live Unit Testing; Playlists; test impact analysis; `.coverage` binary files and the Visual Studio coverage format (Cobertura only); coverage of Rust tests (cargo-llvm-cov is a later brief); merging coverage across runs; a coverage gutter in the scrollbar (brief 0067 exposes markers; optional); Hot Reload of tests; the editor's right-click Run Tests (brief 0068 owns that menu: Integration notes).
