# corpus/debugging: the seeded-bug corpus

Three small C# console programs, each with one seeded bug, for brief 0030's agent debugging proving scenario (proposal
0001 section 1, PLAN.md section 10's Phase 4 exit in its measurable form). Each program's `Main` is its self-check: it
prints `PASS ...` and exits with 0 when the code is right, and prints `FAIL <Type.Method>: expected <value>, actual
<value>` and exits with 1 when it is not, which with the seeded bug it always does. Until the Test Explorer exists
(PLAN.md 4.6) this is the "failing test"; the Test Explorer brief will point `eludite.test.debug` at the same programs.

The table below is the expected answer: the statement that produces the wrong value, its line, and the locals whose
values, at a stop on that statement, show the bug. `crates/eludite/src/shell/agents/tests.rs` reads it: the scripted
fake agent's scenario must stop on that line with those locals in its answer, and the corpus test checks each program's
message and that the line holds that statement. Values are as the Locals window shows them; a value here is matched as
a part of the shown one (`Quarter` matches `MissingCase.Coin.Quarter`).

| Program | Seeded bug | Check message (run directly; exit code 1) | Faulting statement | Line | Locals that reveal it |
|---|---|---|---|---|---|
| `OffByOne` | A loop over a list stops one element early, so the sum skips the last price | `FAIL Basket.Total: expected 75, actual 54` | `for (var i = 0; i < count - 1; i++)` | `Program.cs:13` | `count` = `5`; `total` = `0` |
| `MissingCase` | A switch over an enum has no case for `Quarter`, which falls through to `default` | `FAIL Coins.Cents(Quarter): expected 25, actual 0` | `return 0;` | `Program.cs:28` | `coin` = `Quarter` |
| `NullField` | The constructor sets a folder's `Path` only when it has a parent, so the root's stays null and the check's dereference throws `NullReferenceException` | `FAIL Tree.Describe: expected 21,10,5, actual NullReferenceException` | `if (parent != null)` | `Program.cs:16` | `parent` = `null`; `name` = `root`; `this.Path` = `null` |

How each is found, in the scripted scenario (`crates/eludite/src/shell/agents/scenario.rs`):

- **OffByOne** and **MissingCase** run to their end: `eludite.debug.start`, then `eludite.debug.wait`, answers
  `mode: design`, `exit_code: 1` and the `FAIL` line in `output`, which names the function. A breakpoint on the
  function's first statement, a second start and wait, `snapshot`, and one step: to the loop's header with `count`
  known (`OffByOne`), or from the `switch` to the `default` branch it takes for `Quarter` (`MissingCase`).
- **NullField** stops on the unhandled exception (`stopped.reason: exception`) in `Tree.Describe` at
  `lengths.Add(f.Path.Length.ToString());`, where the locals two levels deep show `f.Path` is null. A breakpoint on
  `Folder`'s constructor, `restart` and wait, `snapshot`, and one step reach the `if` that leaves the root's `Path`
  unset (`parent` is null there). Run directly, an `UnhandledException` handler prints the `FAIL` line and exits with 1;
  under a debugger the exception stops first.

## Adapters

Each program multi-targets `net10.0;net472`:

- **.NET 10** runs under netcoredbg (`tools/netcoredbg/fetch.sh`), on Linux, Windows and macOS: CI's Rust job.
- **.NET Framework 4.7.2** runs under `eludite-dbg-mono` (debuggers/mono) with Mono 6.8 on Linux and macOS. Eludite
  starts a multi-targeted project's first target framework, or Visual Studio's `ActiveDebugFramework` from the
  project's `.user` file; the tests and `crates/eludite/tools/debug-agent-linux.sh` write `<Program>.csproj.user` with
  `<ActiveDebugFramework>net472</ActiveDebugFramework>` where netcoredbg is missing, and remove it afterwards.

## Building

```
corpus/debugging/build.sh        # Linux and macOS: dotnet build of the three, Debug configuration
corpus\debugging\build.ps1       # Windows
```

`dotnet build` needs the .NET SDK pinned in `global.json`. There are no package references; off Windows the SDK's
implicit `Microsoft.NETFramework.ReferenceAssemblies` package supplies the `net472` reference assemblies (restored once
from NuGet). `Directory.Build.props` stops MSBuild's search for one further up and keeps the builds debuggable
(`Optimize` off, real source paths in the PDB, C# 7.3 for both frameworks).

The corpus is MIT (`LICENSE`). Run directly, each program is deterministic and needs no network.
