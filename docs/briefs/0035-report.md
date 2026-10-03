# Brief 0035 report: Test Explorer over MTP, VSTest and cargo test

Status: done on Linux. Windows and macOS: not run (no machines; the code paths for Windows are written and
cfg-clean, and the corpus has `build.ps1`). CI: not run (nothing pushed). netcoredbg: not available here, so Debug Test
of a .NET (CoreCLR) test, MTP or VSTest, was not run against a real adapter (section 9).
Branch: `brief/0035-test-explorer`, based on `main` at `8919bd3`. Main moved to `3d62248` (a Windows run) during the
work; the branch is not rebased (the coordinator merges). Date: 2026-10-03.
Brief: [0035-test-explorer.md](0035-test-explorer.md).

## 1. Summary

- **Test > Test Explorer (Ctrl+E, T)** opens Visual Studio's Test Explorer, docked left: a toolbar (Run All, Run,
  Debug, Run Failed Tests, Repeat Last Run, Cancel), the outcome counts, a search box (`FullName:`, `Outcome:`,
  `Trait:` and plain text), the tree (project > namespace > class > test for .NET, one node per target framework of a
  multi-targeted project; package > target > module > test for Rust) with the outcome glyphs (green check, red cross,
  blue skip, grey not run) and durations, a details pane (the selected test's message, stack trace and output), and the
  summary line. Failures become Error List rows (source `Test`) at the first stack frame in the project, with
  click-through; the status bar reads `Tests: 21 passed, 5 failed, 5 skipped (5.4 s)` and, while a run goes,
  `Tests: running 31 tests…` or `Tests: debugging 1 test…`; the Output window has a `Tests` source.
- **The keys and menus are Visual Studio's**: Run All Tests (Ctrl+R, A), Debug All Tests (Ctrl+R, Ctrl+A), Run Tests
  (Ctrl+R, T) and Debug Tests (Ctrl+R, Ctrl+T) on the Test Explorer's selection or, without one, the test at the
  editor's caret, Repeat Last Run (Ctrl+R, L), Test > Run Failed Tests.
- **Agents call the same commands**: `eludite.test.discover` and `eludite.test.results` (read), `eludite.test.run`,
  `eludite.test.debug` and `eludite.test.cancel` (execute), and the view command `eludite.test.explorer`, with schemas
  in `protocol/schemas/test-*.json`. `run` and `debug` take `ids`, `filter`, `project`, `selection`, `failed_only`,
  `repeat_last` and `wait_ms`; `debug` answers brief 0025's stop summary, so the session continues with
  `eludite.debug.*`.
- **.NET tests run out of process in `eludite-host`** (`dotnet/src/Eludite.TestBridge`, `Eludite.Host/Testing`), over
  **Microsoft.Testing.Platform's server mode** (xunit.v3, MSTest) and **VSTest's translation-layer protocol** (xunit 2,
  NUnit), both hand-written (no new dependency; `Microsoft.TestPlatform.TranslationLayer` was not needed), mapped to
  one model and streamed to the shell as `eludite/test/update` notifications with the generation rule and a status
  replay after a host restart (`protocol/schemas/host/test-*.json`, host-rpc.md "Tests").
- **Rust tests run through the shell's Cargo path**: `cargo test --no-run` as the build, `cargo test -- --list
  --format terse` per target, `cargo test -- --exact <names> --nocapture --test-threads=1` with libtest's output
  parsed (section 6).
- **Debug Test** builds (the F5 build gate), sets a temporary function breakpoint on the first test and starts a brief
  0028 session: an MTP test application launched under the adapter with `--server --client-port` (the host connects
  back, so results flow while the person steps), a VSTest testhost that vstest.console starts paused with the adapter
  attached (brief 0027), a Cargo test executable under lldb-dap (brief 0029's `test: true`). It ran here under the real
  lldb-dap (Rust) and the real `eludite-dbg-mono` (the xunit.v3 project built for net472, through the real
  `eludite-host`). Stopping the session before its tests finished cancels the run.
- **The corpus** (`corpus/tests`, MIT): Corpus.XunitV3 (net10.0 and net472), Corpus.MSTest, Corpus.Xunit2,
  Corpus.NUnit and the Cargo package `corpus-tests`, each with passing, failing, skipped, output-writing and data-row
  tests; `build.sh` and `build.ps1`; CI builds it in both jobs.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, other agents' builds running beside):

  | Budget | Result |
  |---|---|
  | Discovery of a 100-test MTP project under 3 s warm | **555 to 659 ms** (xunit.v3, `Corpus.Many`, 2 runs of 3 warm discoveries; the test asserts 3 s). Pass |
  | The tree rendered in under 50 ms | 100 tests from the model to a drawn window: **3.3 to 3.9 ms** (5 times; the test asserts the median under 50 ms). Pass |
  | Run All of the corpus under 20 s warm | .NET (five containers, 31 tests, through the host): **4.5 to 5.4 s** (Xvfb runs 4454, 5022, 5358 ms; host test 5242 ms). Cargo package (7 tests): **0.24 to 0.31 s**. They run side by side, so the corpus is about 5.5 s. Pass |
  | First result row under 100 ms after the runner reports it | **0.16 to 0.18 ms** from the update's arrival on the UI thread to the row's model (the test asserts 100 ms). Pass |
  | Debug Test to the first stop under 3 s warm | Rust under lldb-dap 18: **1.41 to 1.85 s**; .NET Framework (MTP xunit.v3 net472) under eludite-dbg-mono through the real host: **1.86 to 2.07 s**; the fake host and adapter: 34 to 64 ms. CoreCLR under netcoredbg: not run. Pass where run |
  | No new Rust dependency; .NET deps with SPDX ids | `Cargo.lock` unchanged; no package added to `dotnet/`. The corpus's test frameworks (corpus only, not shipped): xunit.v3 4.0.1, xunit 2.9.3, xunit.runner.visualstudio 3.1.5, Microsoft.NET.Test.Sdk 18.10.1, MSTest 4.4.1, NUnit 4.6.1, NUnit3TestAdapter 6.3.0, all `MIT`. Pass |

- **Tests:** `cargo test --workspace`: 735 passed, 0 failed, 1 ignored; `dotnet test`: 201, 194 passed, 7 skipped. fmt and clippy (`-D warnings`) clean; `dotnet build`: 0 warnings.

## 2. What was built

Commits on top of `8919bd3`, in order:

1. `241eff5` Mark brief 0035 in progress.
2. `6b5990c` The schemas first: `eludite.test.*` command schemas, `host/test-*.json`, the settings
   (`test.runSettings`, `test.parallel`, `test.vstestConsolePath`), the Tests output source and the Test Error List
   source.
3. `1c18813` Work in progress (the bridge's first half), saved at a stopping point.
4. `b2b75cd` The bridge and the host: `MtpRunner`/`MtpConnection`, `VsTestRunner`/`VsTestConnection`/
   `VsTestConsoleLocator`, `TestProcess`, `TestProjectInspector.Inspect`, the model; `TestService` with the
   `eludite/test/discover|run|cancel|attached|status` RPC; the bridge and host tests against the corpus.
5. `4fb7d8d` The typed messages (`protocol/rust/src/host.rs`, schema conformance tests), the `eludite.test.*` commands
   (`crates/commands/src/test.rs`), the fake host's test service (`crates/lsp/src/fake.rs`) and the client tests,
   including the real host on the MTP corpus project.
6. `6e49e41` The corpus's Cargo package, `build.sh`/`build.ps1` and the CI steps.
7. `9528e46` The shell: `test_runs.rs` (the model, runs, Debug Test, the Error List rows, the status bar),
   `tests_window.rs` (the window), `cargo_tests.rs` (listing, running, the libtest parser), the Test menu and keys
   (`crates/ui`), the docked tool window (`crates/docking`), the debugger's start seam, and the headless tests.
8. `a3652f5` A discovery that outlives its solution generation starts over; a debugged run stopped before its tests
   finished is canceled; the status bar shows the going run. (All three found by the Xvfb run and the real-host test.)
9. `16773d4` Debug Test of the net472 xunit.v3 test under the real eludite-dbg-mono and the real host, in a shell test.
10. `9979798` The real host's corpus test skips when `dotnet` is not on PATH.
11. `ff17a96` The Xvfb run script and its screenshots.
12. `769a468` The 100-test tree timed from the model to a drawn window.
13. `e1c62b4` The schema wording (the restarted discovery; the debug session's name).
14. This report, the brief's status, the briefs index, `CLAUDE.md` and `README.md`.

## 3. Microsoft.Testing.Platform: protocol findings

Versions: Microsoft.Testing.Platform 2.4 (as xunit.v3 4.0.1 and MSTest 4.4.1 bring it), .NET SDK 10.0.302.

- **Server mode** is `--server --client-port <port>`: the host listens on loopback, the application connects. JSON-RPC
  2.0 with `Content-Length` framing. Messages used: `initialize` (`processId`, `clientInfo`,
  `capabilities.testing.debuggerProvider: false`), `testing/discoverTests` and `testing/runTests` (`runId`, and for a
  run the discovered nodes as `tests`), the notifications `testing/testUpdates/tests` (`changes: [{ node, parent }]`,
  then `changes: null` when the request's updates are done) and `client/log`, `$/cancelRequest`, and `exit`.
  `telemetry/update` arrives too and is ignored.
- **Nodes**: `uid`, `display-name`, `location.file`, `location.line-start` (the attribute's line, `[Fact]`, not the
  method's), `location.type`, `location.method` (a theory's includes its parameter types,
  `AddsPairs(System.Int32,System.Int32,System.Int32)`, stripped for the fully qualified name), `traits`,
  `execution-state` (`discovered`, `in-progress`, `passed`, `failed`, `timed-out`, `error`, `skipped`, `cancelled`),
  `time.duration-ms`, `error.message`, `error.stacktrace`, `standardOutput`, `standardError`.
- **Quirks**: there is no `testing/cancel`: cancel is `$/cancelRequest`, and frameworks stop only between tests, so
  the host kills the process tree after 2 s. xunit.v3 refuses `testing/runTests` with a tree-node `filter`, so runs
  always name the nodes. xunit.v3 sends no `in-progress` state (tests go from nothing to their outcome), so a running row shows only for
  frameworks that send it.
  One application process could serve discovery and a run, but the host starts one per request, since a build in
  between replaces the executable. **MTP 2.4 sends usage telemetry to Microsoft unless `TESTINGPLATFORM_TELEMETRY_OPTOUT=1`**;
  the host sets it and `DOTNET_CLI_TELEMETRY_OPTOUT=1` on every test application, vstest.console and testhost (no
  telemetry, CLAUDE.md).
- **Debugging**: the application is launched by the shell under the adapter with `--server --client-port`; the host
  waits up to 60 s for the connection, then runs as usual, so results flow while the person steps. MTP's own
  `debuggerProvider` capability (the client attaches) was not needed.

## 4. VSTest translation layer: protocol findings

Version: vstest.console 18.6.0 from the .NET SDK 10.0.302 (`sdk/<version>/vstest.console.dll`, located from `dotnet
--list-sdks` or `test.vstestConsolePath`).

- **Transport**: `dotnet vstest.console.dll --port:<port> --parentprocessid:<pid>` connects to the host's loopback
  listener; messages are `{ MessageType, Version, Payload }` JSON with a 7-bit-encoded length prefix (BinaryWriter
  strings). `TestSession.Connected`, then `ProtocolVersion` (7).
- **Messages used**: `TestDiscovery.Start`, `TestDiscovery.TestFound`, `TestDiscovery.Completed`, `TestDiscovery.Cancel`;
  `TestExecution.RunAllWithDefaultHost` (Sources) and `TestExecution.RunSelectedWithDefaultHost` (the TestCase objects
  as discovery sent them), `TestExecution.StatsChange`, `TestExecution.Completed`, `TestExecution.Cancel`;
  `TestSession.Message`, `TestSession.Terminate`; for debugging `TestExecution.GetTestRunnerProcessStartInfoForRunAll`
  and `...ForRunSelected` with `DebuggingEnabled`, answered by `TestExecution.EditorAttachDebugger2` (`ProcessID`) and
  our `TestExecution.EditorAttachDebuggerCallback`; `TestExecution.CustomTestHostLaunch` and its callback are handled
  but were never sent by 18.6 (it starts the testhost itself and asks for an attach).
- **Quirk**: vstest.console 18.6 aborts a selected run (ArgumentNullException in `TestRequestManager.RunTests`) when the
  payload's JSON has no space after the colons; it finds the run's sources by text. The host writes indented JSON.
- **Results**: `Outcome` 1 passed, 2 failed, 3 skipped, 0 and 4 not run; `Duration` a TimeSpan string;
  `ErrorMessage`, `ErrorStackTrace`; output from `Messages` (`StdOutMsgs`, `StdErrMsgs`, `AdditionalInfo`); a test in
  `ActiveTests` of a `StatsChange` is running.
- `RunSettings` default: `<MaxCpuCount>` 0 or 1 by `test.parallel`, `<DesignMode>True</DesignMode>`; a configured
  `.runsettings` file replaces it.

## 5. What each framework needed

- **xunit.v3 4.0.1** (MTP): `OutputType Exe` and `UseMicrosoftTestingPlatformRunner`; data rows have their own
  display names (`AddsPairs(a: 1, b: 2, sum: 3)`) and the method's parameter types in `location.method`; `[Trait]` is
  `{ name: value }`. The net472 build runs under Mono 6.8 (`mono --debug Corpus.XunitV3.exe --server ...`) and debugs
  under eludite-dbg-mono; Mono stops a function breakpoint on the method's opening brace (line 16), a line before the
  first statement.
- **MSTest 4.4.1** (MTP): `EnableMSTestRunner`; `[TestCategory("X")]` arrives as the trait `{ X: "" }`, mapped to
  `Category=X`; `[Ignore]` gives `skipped` with the reason; `TestContext` output and `Console` output both arrive.
- **xunit 2.9.3** (VSTest, xunit.runner.visualstudio 3.1.5, Microsoft.NET.Test.Sdk 18.10.1): `ManagedType` and
  `ManagedMethod` give the class and method; traits from `TestObject.Traits`; output from `ITestOutputHelper` in
  `StdOutMsgs`.
- **NUnit 4.6.1** (VSTest, NUnit3TestAdapter 6.3.0): the class and method come from the `NUnit.ClassName` and
  `NUnit.MethodName` properties, categories from `NUnit.TestCategory` (an array); `[Ignore]` is `skipped`; test
  cases have display names with their arguments.
- `TestProjectInspector` tells them apart from the project file alone: MTP for an MTP runner property
  (`UseMicrosoftTestingPlatformRunner`, `EnableMSTestRunner`, `EnableNUnitRunner`, ...), `MSTest.Sdk`, or a
  `Microsoft.Testing.Platform*` or `xunit.v3*` package; VSTest for `Microsoft.NET.Test.Sdk` alone or `UseVSTest`.

## 6. The Rust path and its parsing limits

`cargo test` runs in the shell (the host never sees Rust), as brief 0019 runs `cargo build`:

- **Listing** is per target (`--lib`, `--bin N`, `--test N`), `-- --list --format terse`. Doc tests, examples and
  benches are not listed (doc tests have no `--exact` name that can be run alone cheaply; they are a later brief).
- **Locations**: libtest reports none. The shell looks for `fn <last segment>` after a `#[test]` under the target's
  source folder (at most 500 files, files named after the test's modules first). Tests generated by macros (`rstest`,
  `test-case`, a `macro_rules!` that writes `#[test]`) and two same-named test functions in different files of one
  module path can get no line or the wrong file.
- **Results** come from libtest's human output (`test <name> ... ok|FAILED|ignored[, reason]`); the JSON format needs
  nightly. With `--nocapture --test-threads=1` (the default; `test.parallel` drops both) a test's output is what is
  printed between its `... ` and its outcome; in parallel runs it comes from the `---- <name> stdout ----` sections.
  Panic messages and locations come from stderr (`thread '<name>' (<tid>) panicked at <file>:<line>:<col>:`), so a
  failure's Error List row is the panic's location. A test that prints a line looking like `test x ... ok` confuses
  the parser; a custom test harness (`harness = false`) is listed only if it implements `--list --format terse`.
- **Durations** are measured between a test's start and outcome lines (stable libtest prints none), so they include
  output time and are only meaningful one test at a time.
- **Debug Test** runs the test executable under lldb-dap with `--exact <names> --test-threads=1`; its results are the
  libtest lines in the Debug pane of the Output window, parsed when the session ends.

## 7. Screenshots (Xvfb run)

`crates/eludite/tools/tests-linux.sh OUT_DIR` drives the real `eludite` binary on an Xvfb screen with xdotool (keys
only: Ctrl+F to the line, F9, Ctrl+E, T, Ctrl+R, A, Ctrl+R, Ctrl+T, Shift+F5). Screenshots in
[0035-run/screenshots](0035-run/screenshots/):

- `rust-explorer.png`: `--folder corpus/tests/rust`; the tree after Run All: 5 passed, 1 failed (`subtracts`, Error
  List row at `lib.rs:25`), 1 skipped; the summary `Tests: 5 passed, 1 failed, 1 skipped (0.4 s)`.
- `rust-debug.png`: Ctrl+R, Ctrl+T on `adds` with a breakpoint at line 18: stopped at line 18 under lldb-dap, the Call
  Stack through libtest, the status bar `Tests: debugging 1 test…`.
- `dotnet-explorer.png`: `--solution` of the four corpus projects with the real host: 31 tests in five containers,
  21 passed, 5 failed, 5 skipped (5.4 s), the Error List with the failures.
- `dotnet-debug.png`: Ctrl+R, Ctrl+T in `CalculatorTests.Adds`: the net472 build debugged under eludite-dbg-mono,
  stopped at the function breakpoint on line 16 with `this` and `sum` in Locals.

## 8. Tests

What each proves:

- **Bridge** (`dotnet/tests/Eludite.TestBridge.Tests`, against the corpus copied to a temp folder and built):
  `RunnerTests`: MTP discovery and run of xunit.v3 and MSTest with outcomes, messages, stack traces, output, durations
  and traits; VSTest discovery and run of xunit 2; NUnit selected runs with its names and categories; cancel of a
  waiting theory mid-run; the MTP debug launch (command line, then results over the connection); the VSTest debug
  attach (`EditorAttachDebugger2` answered, results after); xunit.v3 net472 under Mono; 100-test discovery under 3 s;
  `Inspect` on the corpus. `MappingTests`: both protocols' JSON to the model (trait shapes, outcomes, data rows,
  NUnit properties, durations).
- **Host** (`Eludite.Host.Tests/TestServiceTests`): discover and Run All of the corpus through the RPC with the
  expected counts; ids before discovery are discovered silently first; `status` before `initialize` and the errors;
  scripted ordering (seq, one `finished`), -32012 for a second run of a container, status replay and cancel; the
  generation rule; the debug launch and attach handshake; a not-built container.
- **Client** (`crates/lsp`): the fake host's test service (in-process round trips, held runs, the error code) and the
  real host discovering and running the MTP corpus project (`real_host.rs`).
- **Protocol**: every `eludite/test/*` message conforms to its schema (`schema_tests.rs`).
- **Commands**: request parsing and validation, specs (read/execute), cutting long messages.
- **Shell** (`crates/eludite/src/shell/test_runs_tests.rs`, headless GPUI):
  - the window opens with the tree from Ctrl+E, T; Run All (Ctrl+R, A) streams results into rows, the Error List
    (click-through to the stack frame), the status bar and the Output window; Run Failed and Repeat Last Run rerun the
    right tests; the filter box; the first result row under 100 ms;
  - an agent discovers, runs with `wait_ms` and reads `results` with the same data the window shows; refused input;
  - cancel of a held run (the window's button), stale updates dropped, a restarted host's status replayed, the status
    bar while a run goes;
  - a discovery that outlives its generation starts over under the new one (fails without the fix);
  - a 100-test tree built and drawn in under 50 ms;
  - Debug Test over the fake host and adapter breaks at the first line, results still flow, the launch carried
    `--client-port`;
  - the real `cargo` on the corpus package: listed, run, libtest parsed (the failure's message and line, the ignored
    test's reason, output, a nested module, an integration test);
  - Debug Test of a Rust test under the real lldb-dap stops at line 18, twice; Stop cancels the run;
  - Debug Test of the net472 xunit.v3 test under the real eludite-dbg-mono with the real eludite-host stops in
    `Adds`; the session's arguments carry `--client-port`; Stop cancels the run.
- Keymap, menu, docking and settings tests adjusted for the new window, menu and keys.

Counts (this machine, `DISPLAY=:99`, `ELUDITE_DBG_MONO` set):

- `cargo test --workspace --no-fail-fast`: **735 passed, 0 failed, 1 ignored** (exit 0); the lldb-dap, eludite-dbg-mono
  and real-host tests ran, not skipped.
- `dotnet test dotnet/Eludite.slnx --no-build`: **201 tests, 194 passed, 0 failed, 7 skipped** (the skips are the
  Windows-only and netcoredbg tests of earlier briefs).
- `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`: clean. `dotnet build
  dotnet/Eludite.slnx`: 0 warnings, 0 errors. `bash corpus/tests/build.sh`: builds.

## 9. Deviations, gaps and findings

1. **netcoredbg was not run.** Debug Test of a CoreCLR test (MTP under netcoredbg, VSTest's attach) is proven only
   against the fake adapter and, on the host side, the real handshake (`RunnerTests`: the MTP launch connects back;
   VSTest's testhost waits for `EditorAttachDebuggerCallback`). CI's Linux job or a machine with netcoredbg should
   run the corpus once; `tests-linux.sh` does it by keys.
2. **The debug session is not marked `test`.** The brief asks for a brief 0028 session "named after the project and
   marked `test`". It is named after the project, but brief 0028's session model has no kind field, and adding one
   is a debugger change beyond the start seam; the schema text was corrected. A later debugger brief can add it.
3. **Files outside the brief's list**, each a small hook: `crates/docking` (the Test Explorer tool window id and
   default dock), `crates/eludite/src/shell/debug.rs` (a `test_launch` override at the start seam and the prelaunch
   build hook, 20 lines), `shell/build.rs` and `shell/cargo_build.rs` (`cargo test --no-run` as a build),
   `shell/session.rs` (the test update and status events), `shell/output.rs` (the Tests pane), `shell/settings.rs`
   and `settings_tests.rs` (the test settings section), `crates/commands/src/{build,diagnostics,settings}.rs` (the
   Tests source, the Test row source, the settings keys), `crates/eludite/src/tests.rs` (the menu test). No change to
   `crates/dap` or `shell/debug/tests.rs`.
4. **Found by the real runs and fixed** (commit 8): the real host's solution can still be loading when the Test
   Explorer first discovers, and the new generation used to end that discovery as an empty, fresh tree; now it starts
   over. Stopping a debugged test used to end its run `passed` (MTP: the host saw the application leave and reported
   `failed` with no results); now the run is `canceled` and the host's run is canceled. The status bar said
   `discovering` for the whole run.
5. **The host's finished state for a stopped debug run** is still `failed` (the application went away before the
   cancel arrived); the shell shows `canceled`. Harmless; noted for the host's next change.
6. **VSTest's `CustomTestHostLaunch`** is handled but untested: vstest.console 18.6 never sent it (it starts the
   testhost and asks for an attach).
7. **One window per kind of workspace**: a folder whose root has a Cargo workspace and a .NET solution shows both in
   one tree, but the corpus keeps them in separate folders, so the Run All budget was measured per part (both are far
   under 20 s).
8. **Durations and Rust parsing limits**: section 6.
9. **The Debug status text for a Cargo session** reads `Debugging: Cargo.toml` (brief 0029's naming from the manifest);
   not changed here.

## 10. What proposal 0001's proving scenario needs to use `eludite.test.debug`

- Call `eludite.test.discover` (or skip it: `run` and `debug` discover first when the tree is stale), then
  `eludite.test.debug` with `filter` (`CalculatorTests.Adds`, `tests::adds`) or `ids` from `discover`/`results`, and
  `project` when the corpus has more than one test project (the tests to debug must be of one project; a
  multi-targeted project is named per framework, `Corpus.XunitV3 (net472)`). `wait_ms` up to 300,000.
- The answer is brief 0025's stop summary at the test's first line (a temporary function breakpoint that is removed
  at that stop) with `run`, `tests`, `project`, `breakpoint` and the `session`; the agent continues with
  `eludite.debug.*` (breakpoints on the faulting line, `continue`, `snapshot`, `evaluate`).
- The run's results keep flowing while the agent steps; `eludite.test.results` with the `run` reads them, and
  `eludite.debug.stop` (or `eludite.test.cancel`) ends the run as canceled.
- For the seeded-bug corpus (`corpus/debugging`, brief 0030) to be debugged through `eludite.test.debug`, its program
  needs a test project: an MTP xunit.v3 project referencing it (net10.0 for netcoredbg; net472 runs under
  eludite-dbg-mono off Windows, which works today), or a Cargo test. Agent policy applies unchanged: `test.debug` is
  `execute`, like `debug.start`.

## 11. How to reproduce

```
corpus/tests/build.sh                       # the corpus, in place (build.ps1 on Windows)
dotnet build dotnet/Eludite.slnx            # the host and eludite-dbg-mono
dotnet test dotnet/Eludite.slnx --no-build
ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe \
  cargo test -p eludite --bins -- test_runs --nocapture     # prints the timing lines
crates/eludite/tools/tests-linux.sh OUT_DIR  # Xvfb, xdotool, ImageMagick; the screenshots of section 7
dotnet/tests/Eludite.TestBridge.Tests/bin/Debug/net10.0/Eludite.TestBridge.Tests -method "*Discovers100*" -diagnostics
```
