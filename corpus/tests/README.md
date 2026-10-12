# corpus/tests: the Test Explorer corpus

Small test projects for brief 0035's Test Explorer, one per protocol and framework, each with passing, failing, skipped
and output-writing tests (MIT, this repository's; `LICENSE`); brief 0063 adds the same in Visual Basic (`.vbproj`) and
F# (`.fsproj`), so the project list, the Workspace tree and the Test Explorer are proven on all three .NET languages. Build them in place with `build.sh` (or `build.ps1`);
the bridge's and the host's tests copy them to a temp folder and build them there.

| Project | Framework | Protocol | Target frameworks | Expected outcomes |
|---|---|---|---|---|
| `Corpus.XunitV3` | xunit.v3 4.0.1 | Microsoft.Testing.Platform 2.4 (server mode) | net10.0, net472 (run by Mono off Windows) | 7 tests: `Subtracts` fails (`CalculatorTests.cs:line 25`), `Divides` skipped (`Division is not written yet`), `WritesOutput` writes `Hello from xunit.v3`, `AddsPairs` is a theory of two rows with trait `Category=Math`, `Waits` sleeps `CORPUS_SLOW_MS`; the `Calculator` they test is in `Calculator.cs`, so its CodeLens counts references from another file (brief 0052) |
| `Corpus.MSTest` | MSTest 4.4.1 | Microsoft.Testing.Platform 2.4 | net10.0 | 6 tests: `ParsesNegatives` fails (`ParserTests.cs:line 21`), `ParsesHex` skipped (`Hexadecimal comes later`), `WritesOutput` writes `Hello from MSTest` and `Console from MSTest`, `ParsesRows` is two data rows with category `Rows` |
| `Corpus.Xunit2` | xunit 2.9.3, xunit.runner.visualstudio 3.1.5 | VSTest (translation layer) | net10.0 | 5 tests: `GreetsNobody` fails (`GreeterTests.cs:line 26`), `SaysGoodbye` skipped (`Farewells come later`), `WritesOutput` writes `Hello from xunit 2`, `Waits` has trait `Category=Slow` and sleeps `CORPUS_SLOW_MS` |
| `Corpus.NUnit` | NUnit 4.6.1, NUnit3TestAdapter 6.3.0 | VSTest | net10.0 | 6 tests: `Pops` fails, `Peeks` ignored (`Peeking is not decided yet`), `WritesOutput` writes `Hello from NUnit`, `PushesPairs` is two cases with category `Pairs` |
| `Corpus.VisualBasic` | MSTest 4.4.1, Visual Basic (`Corpus.VisualBasic.vbproj`) | Microsoft.Testing.Platform 2.4 | net10.0 | 5 tests: `ParsesNegatives` fails (`Negatives keep their sign`, `ParserTests.vb:line 18`), `ParsesHex` skipped (`Hexadecimal comes later`), `WritesOutput` writes `Hello from Visual Basic` and `Console from Visual Basic`, `ParsesNumbers` and `TrimsBeforeParsing` pass |
| `Corpus.FSharp` | NUnit 4.6.1, NUnit3TestAdapter 6.3.0, F# (`Corpus.FSharp.fsproj`) | VSTest | net10.0 | 5 tests: `Dequeues` fails (`the queue is empty after its one item is taken`, `Tests.fs:line 21`), `Peeks` ignored (`Peeking is not decided yet`), `WritesOutput` writes `Hello from F#` and `Console from F#`, `Enqueues` and `KeepsOrder` pass |
| `Variants/Corpus.PropsDriven` | MSTest 4.4.1 | Microsoft.Testing.Platform 2.4 | net10.0 | Brief 0076: `IsTestProject`, `OutputType`, `EnableMSTestRunner` and the MSTest package come from its `Directory.Build.props` (the project file has only `TargetFramework`); 2 tests: `ReadsProps` passes, `FailsFromProps` fails (`Four is not five`) |
| `Variants/Corpus.Platform64` | xunit 2.9.3, xunit.runner.visualstudio 3.1.5 | VSTest | net10.0, `Platforms` and `PlatformTarget` x64 (built with `-p:Platform=x64`: `bin/x64/Debug/net10.0/`) | Brief 0076: 2 tests pass, `RunsIn64Bits` and `Adds` |
| `Variants/Corpus.Artifacts` | MSTest 4.4.1 | Microsoft.Testing.Platform 2.4 | net10.0, `UseArtifactsOutput` with `ArtifactsPath` in its folder (`artifacts/bin/Corpus.Artifacts/debug/`) | Brief 0076: 2 tests pass, `Builds` and `Multiplies` |
| `Variants/Corpus.Coverage` | MSTest 4.4.1, Microsoft.Testing.Extensions.CodeCoverage 18.11.0 | Microsoft.Testing.Platform 2.4 (`--coverage`) | net10.0 | Brief 0076: 2 tests pass, `Adds` and `Subtracts`, calling `Variants/Corpus.Calculator` (a library, not a test project): `Add` covered but for its `b == 0` branch (the `if` is one branch of two, `return a;` not hit), `Subtract` covered, `Divide` never called. In a library of its own because both collectors leave the test assembly out by default |
| `Variants/Corpus.Coverlet` | xunit 2.9.3, coverlet.collector 6.0.4 | VSTest (`XPlat Code Coverage`) | net10.0 | Brief 0076: the same 2 tests on the same `Corpus.Calculator`, with the same covered and uncovered lines |
| `rust/` (`corpus-tests`) | libtest | `cargo test` | the host's | 7 tests: `tests::subtracts` fails (`src/lib.rs:25`), `tests::divides` ignored (`division is not written yet`), `tests::writes_output` prints `Hello from Rust`, `tests::nested::adds_negatives` in a nested module, `integration::adds_from_outside` in an integration test, `tests::waits` sleeps `CORPUS_SLOW_MS` |

Used by `dotnet/tests/Eludite.TestBridge.Tests` and `dotnet/tests/Eludite.Host.Tests` (copied and built in a temp folder,
with a generated 100-test `Corpus.Many` for the discovery budget), `crates/lsp/tests/real_host.rs` (the built
`Corpus.XunitV3` through the real `eludite-host`), `dotnet/tests/Eludite.Host.Tests/CodeLensTests.cs` (the real Roslyn's lenses on `Corpus.XunitV3`, built in place), the shell's headless tests (`crates/eludite/src/shell/test_runs_tests.rs`:
the Rust package copied into a temp folder) and `crates/eludite/tools/tests-linux.sh` (the Xvfb run). The variants (brief
0076) are copied and built in a folder of their own by the same tests (`Corpus.VariantsAsync`), with a `Variants.slnx`.
