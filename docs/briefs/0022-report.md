# Brief 0022 report: The Mono soft-debugger adapter

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0022-mono-debug-adapter`, based on `main` at `dfdf3d0`. `main` has since gained three commits that add
briefs 0023, 0025 and 0026 (documents only); the branch was not rebased, and the only expected conflict is the briefs
index (`docs/briefs/README.md`), where both sides add rows. Date: 2026-10-03.
Brief: [0022-mono-debug-adapter.md](0022-mono-debug-adapter.md).

## 1. Summary

- **F5 on a .NET Framework project debugs it under Mono** through `eludite-dbg-mono`, a C# DAP server on
  Mono.Debugging.Soft that runs under the located Mono and launches the program as `mono --debug
  --debugger-agent=...`. The shell picks the adapter from the startup project's target framework and the platform:
  netcoredbg for .NET (Core), `eludite-dbg-mono` for .NET Framework on Linux and macOS, and on Windows the message
  that names `eludite-dbg-netfx` (brief 0004). Ctrl+F5 runs `mono <exe>`. Every debugger window, key and
  `eludite.debug.*` command of brief 0018 works unchanged; `eludite.debug.state` reads
  `session.adapter: "eludite-dbg-mono under mono 6.8.0.105 (stdio)"` and `session.runtime: "mono"`.
- **No display.** Where brief 0018 drove the IDE by hand, this brief proves the flow headless: the adapter's own
  tests (18, xunit v3) drive it over stdio and TCP against a real Mono program, `cargo test -p eludite-dap --test mono`
  drives it through Eludite's DAP client with the launch configuration the shell computes for an SDK-style net472
  project, and the shell's headless tests check the plan the shell sends and the state it shows.
- **Budgets** (Ubuntu 24.04 container, 4 cores, Mono 6.8.0.105 from the distribution, Debug build of the adapter;
  load average 0.9 to 1.7 while another agent built in a second worktree). Each number is from the integration tests'
  output (section 7), three runs each:

  | Budget | Result |
  |---|---|
  | Launch to the first `stopped` at a breakpoint in the TestApp < 3 s warm (cold reported) | `launch` request to the `stopped` event: **warm 124 to 145 ms** (stdio and TCP sessions of three full runs), **cold 136 to 168 ms** (first adapter after evicting Mono's runtime, its class libraries, the adapter and the TestApp from the page cache with `posix_fadvise(DONTNEED)`). Including Mono's start and the adapter's JIT (from spawning `mono eludite-dbg-mono.exe`): warm 221 to 250 ms, cold 275 to 309 ms; through `eludite-dap` (`crates/dap/tests/mono.rs`, adapter spawn to `stopped`): 225 to 251 ms. Pass |
  | `next` round trip < 150 ms p95 over 50 steps | **p95 12.4 to 12.9 ms** (p50 2.2 to 11.8 ms, max 12.5 to 22.3 ms; six runs of 50 steps). Pass |
  | `stackTrace` + `scopes` + `variables` for a frame with 200 locals < 100 ms | **5.8 to 7.2 ms** for the 201 locals of a static method (`l000` to `l199` and one more), six runs. Pass |
  | Adapter resident memory at a break < 80 MB | **55.8 to 57.6 MB** (`VmRSS` of the adapter process at the 200-locals break). Pass |
  | Cold start of Eludite unchanged: nothing of Mono touched before F5 | Pass by construction: `MonoSearch::from_env` and `MonoAdapterSearch::from_env` read environment variables only; the file system search (`find_mono`, `find`) and `mono --version` run on the `debug-launch` thread after F5 (`crates/eludite/src/shell/debug.rs`). Not separately benchmarked (no display) |

- **Tests:** `cargo test --workspace` 468 passed, 0 failed, 1 ignored (a doc example), with `crates/dap/tests/mono.rs`
  running against the real adapter. `dotnet test dotnet/Eludite.slnx` 171 tests: 164 passed, 0 failed, 7 skipped; all
  18 of `Eludite.Debugger.Mono.Tests` ran and passed (the 7 skips are existing `Eludite.Host.Tests` that need the
  Roslyn language server build, the legacy corpus or Mono's MSBuild, none of which this machine has). fmt, clippy
  (`-D warnings`) and `dotnet build` (0 warnings, also with `--no-incremental`) clean.
- **New dependencies** (all MIT, in `dotnet/Directory.Packages.props` with a comment giving the SPDX ids):
  `Mono.Debugging.Soft` 1.0.20170212.42 with its dependencies `Mono.Debugging` and `Mono.Debugger.Soft`
  1.0.20170212.42, `Mono.Cecil` 0.9.6 and `ICSharpCode.NRefactory` 5.5.1; `Newtonsoft.Json` 13.0.3. Section 3 has the
  details. Mono itself is located at run time, never bundled.

## 2. What was built

Commits, in order:

1. `6f737e1` The brief's Status line.
2. `1bee5dc` `protocol/schemas/` alone: `dap-mono.md` (the adapter's transport, `launch` and `attach` arguments,
   capabilities, requests, events, values and what it lacks), `session.runtime` in `debug-state.output.json`, the
   adapters named in `debug-start.input.json`, and `debugger.monoPrefix` (`ELUDITE_MONO_PREFIX`) and
   `debugger.monoAdapterPath` (`ELUDITE_DBG_MONO`) in `settings.json` under Debugging > General.
3. `c89f08e` `debuggers/mono/`: the protocol library, the adapter, the TestApp and the tests (section 2.1), the four
   projects in `dotnet/Eludite.slnx` under `/debuggers/`, the package versions.
4. `312e05e` `crates/dap`: target-framework classification, the .NET Framework program path, Mono and adapter
   discovery, adapter selection, the Mono `launch` arguments, `connect_with_env`, and `tests/mono.rs` (section 2.2).
5. `ca899a8` The shell: the launch thread's adapter choice, Ctrl+F5 under Mono, the Windows message,
   `session.runtime`, the adapter description, the two settings (section 2.3).
6. `d537350` A long array's range rows carry no `evaluateName`; `dap-mono.md` documents values, expressions and
   breakpoint behavior.
7. `48e681f` CI installs `mono-devel` in the Linux .NET job; `tools/run-dev.sh` sets `ELUDITE_DBG_MONO` when the
   adapter is built.
8. `6787552` Three fixes from checking the adapter against the contract: Just My Code `stepIn` on a line that calls
   only external code now lands on the next line (Mono stopped back on the same line), type names the debuggee has
   not loaded yet (`Math`, `Environment`, `DateTime`) and namespace-qualified names (`System.Math.Max(a, b)`) now
   evaluate; a test for `stopAtEntry` (reason `entry`) and that step; the timing lines also give the time from the
   adapter's spawn.
9. `f4a8f81` Interpolated strings are refused with an error naming `string.Format` (the 2017 parser answered `$"{x}"`
   as the literal text `"{x}"`, a wrong value rather than an error).
10. This report, the brief's Status line, the briefs index, `CLAUDE.md` and `README.md`.

### 2.1 `debuggers/mono` (GPL-3.0-or-later)

- `Eludite.Debugger.Mono.Protocol` (netstandard2.0, no Mono.Debugging dependency, so its tests run on .NET 10):
  `Content-Length` framing, the dispatcher (one thread answers every request and runs everything Mono.Debugging posts
  to it; an unknown request or a throwing handler gets a failed response naming the command), the handle table for
  variables references and frame ids (paged with `start` and `count`, cleared on resume), `launch`, `attach`,
  hit-condition, log-message and exception-filter parsing, the command line (`--port N`, `--log FILE`, `--help`), the
  `initialize` capabilities.
- `Eludite.Debugger.Mono` (net472, `AssemblyName` `eludite-dbg-mono`): `Program.cs` (stdio or one TCP client on
  loopback; `Console.Out` is redirected to stderr so only DAP reaches stdout; `DebuggerLoggingService.CustomLogger`
  set before anything runs), `MonoAdapter.cs` (the requests over one `SoftDebuggerSession`), `MonoEvaluator.cs`
  (Mono.Debugging's C# evaluator with the gaps of section 5 closed).
- `Eludite.Debugger.Mono.TestApp` (net472 console program with `// MARK: name` comments the tests locate): a method
  with `this`, parameters and locals and a static callee, a loop, a thrown and caught exception, a `string[25]`, an
  `int[1000]`, an object with fields and properties, a method that never returns (for the evaluation timeout), a
  frame with 200 locals, and a `sleep` mode for Pause.
- `Eludite.Debugger.Mono.Tests` (net10.0, xunit v3): section 7.

### 2.2 `crates/dap`

- `launch.rs`: `FrameworkKind` (`netcoreapp*`, `netstandard*`, `netN.M` with N >= 5 are CoreCLR; `net2*` to `net4*`
  and a legacy project's `TargetFrameworkVersion` v2.0 to v4.8.1 are .NET Framework); a legacy project's program is
  `<OutputPath>` (default `bin\Debug\`, backslashes normalized) plus `<AssemblyName>.exe`, an SDK-style .NET Framework
  project's is `bin/Debug/<tfm>/<AssemblyName>.exe`; the missing-program error names the first candidate.
  `select_adapter(kind, platform)` with the Windows message of the brief; `runtime_name`; `mono_arguments` (program,
  args, cwd, env, `runtimeExecutable`, `justMyCode`); `run_command` for Ctrl+F5 (`mono <exe>`, `<exe>` on Windows,
  `dotnet <dll>`). `Platform` is a parameter everywhere, so the Windows path is tested on Linux.
- `discovery.rs`: `MonoSearch` (the setting, `mono` on `PATH` with the prefix two directories above the resolved
  executable, `~/.local/opt/mono-root/usr`, `/usr`, `/usr/local`, `/Library/Frameworks/Mono.framework/Versions/Current`;
  a candidate counts when `<prefix>/bin/mono` exists; brief 0003's environment for a relocated prefix: `PATH`,
  `LD_LIBRARY_PATH`, `MONO_CFG_DIR`, `MONO_GAC_PREFIX`), `mono --version`'s first line parsed for the version, and
  `MonoAdapterSearch` (beside the executable, `<exe dir>/eludite-dbg-mono/eludite-dbg-mono.exe` then
  `<exe dir>/eludite-dbg-mono.exe`, then the setting, a file or its folder). The errors name the places searched,
  `dotnet build dotnet/Eludite.slnx` and its output path, and the distribution packages.
- `transport.rs`: `connect_with_env` adds Mono's environment to a spawned adapter.

### 2.3 The shell (`crates/eludite`, `crates/commands`)

- The `debug-launch` thread classifies the startup project, locates Mono for .NET Framework off Windows (with or
  without the debugger), reads `mono --version` once, finds `eludite-dbg-mono` and starts it as `mono
  eludite-dbg-mono.exe` with the relocated-prefix environment, and sends `adapterID` `mono` with the Mono `launch`
  arguments. On Windows a .NET Framework F5 fails with the brief's message and starts nothing.
- `SessionRow.runtime` (`coreclr`, `mono`, `netfx`) in `eludite.debug.state`; the status bar slot is unchanged in shape.
- The settings `debugger.monoPrefix` and `debugger.monoAdapterPath` reach `MonoSearch` and `MonoAdapterSearch`
  (the Options dialog shows them under Debugging > General; `--help` lists their environment variables).
- Breakpoints, exception settings, watches, Run To Cursor, data tips and the two-driver rules are unchanged; the
  adapter advertises hit conditions, so the shell hands them to it instead of counting (brief 0018's rule).

## 3. Mono and the libraries

| What | Version | SPDX | How it was obtained |
|---|---|---|---|
| Mono runtime and class libraries | 6.8.0.105 (`6.8.0.105+dfsg-3.6ubuntu2`) | `MIT` (the runtime and class libraries since 2016; the Debian packaging is the distribution's) | Ubuntu 24.04's `mono-devel` package (already installed on this machine for brief 0003). Located at run time (section 2.2), never bundled or redistributed. CI installs it with `sudo apt-get install -y --no-install-recommends mono-devel` |
| `Mono.Debugging.Soft` | 1.0.20170212.42 | `MIT` | NuGet (nuget.org), the last package mono/debugger-libs published; the soft-debugger backend of Mono.Debugging |
| `Mono.Debugging` | 1.0.20170212.42 | `MIT` | NuGet, dependency of the above: sessions, breakpoints, the expression evaluator |
| `Mono.Debugger.Soft` | 1.0.20170212.42 | `MIT` | NuGet, dependency: the wire protocol client of Mono's debugger agent |
| `Mono.Cecil` | 0.9.6 | `MIT` | NuGet, dependency (pinned exactly by the packages) |
| `ICSharpCode.NRefactory` | 5.5.1 | `MIT` | NuGet, dependency: the C# parser the evaluator uses |
| `Newtonsoft.Json` | 13.0.3 | `MIT` | NuGet: DAP's JSON in the protocol library (netstandard2.0, so it runs under Mono 6.8 and on .NET 10 for the tests; `System.Text.Json` is not part of Mono 6.8) |
| `Microsoft.NETFramework.ReferenceAssemblies.net472` | 1.0.3 | `MIT` | Referenced implicitly by the .NET SDK for a `net472` target off Windows; compile-time reference assemblies only, never shipped |

All are compatible with GPL-3.0-or-later. `xunit.v3` was already a dependency of `dotnet/`.

## 4. The adapter's DAP features

As brief 0018's section 5, for `eludite-dbg-mono` (the full contract is `protocol/schemas/dap-mono.md`):

| Feature | eludite-dbg-mono | Notes |
|---|---|---|
| stdio, TCP (`--port N`, `--port 0` prints the port on stderr), `--log FILE` | Yes | stdout carries DAP only; logs on stderr |
| `initialize` capabilities | `supportsConfigurationDoneRequest`, `supportsConditionalBreakpoints`, `supportsHitConditionalBreakpoints`, `supportsFunctionBreakpoints`, `supportsLogPoints`, `supportsEvaluateForHovers`, `supportsSetVariable`, `supportsExceptionInfoRequest`, `supportsExceptionFilterOptions`, `supportsTerminateRequest`, `supportTerminateDebuggee`, `supportsDelayedStackTraceLoading`; filters `all` and `user-unhandled` (default on), both with a type-name `condition` | No `capabilities` event (nothing changes) |
| `launch` (`program`, `args`, `cwd`, `env`, `runtimeExecutable`, `runtimeArgs`, `stopAtEntry`, `justMyCode`) | Yes | Program output becomes `output` events (`stdout`, `stderr`); the adapter's own messages use `console`; a `process` event gives the pid |
| `attach` (`address`, `port`) | Yes | To a program started with `--debugger-agent=transport=dt_socket,server=y,...` |
| Source breakpoints with `condition`, `hitCondition` (`N`, `>=N`, `%N`, also `==N`, `>N`, `<N`, `<=N`), `logMessage` (`{expression}`, `{{` literal) | Yes | Pending breakpoints answer `verified: false` and bind with a `breakpoint` `changed` event with the bound line |
| Function breakpoints (`Namespace.Type.Method`, optional parameter types) | Emulated in part | Mono.Debugging 2017 binds every method of the type; the adapter stops only in the named method and applies the condition, hit condition and log message itself (section 6) |
| Exception breakpoints | Yes | `all`: a catchpoint on `System.Exception` and subclasses (or the condition's type names), first chance; with Just My Code it does not stop for exceptions thrown and handled in external code. `user-unhandled`: Mono's unhandled-exception event |
| `threads`, `stackTrace` (`startFrame`, `levels`, `totalFrames`), `scopes` (one `Locals`: `this`, parameters, locals) | Yes | Frames without source get `presentationHint: subtle`, `moduleId` the assembly file |
| `variables` (`start`, `count`) | Yes | `indexedVariables` for arrays up to 150 elements; longer arrays come as Mono.Debugging's ranges (`[0..99]`, ...); objects give no `namedVariables` (section 6). DAP's `filter` (`indexed`, `named`) is ignored |
| `evaluate` (`watch`, `hover`, `repl`; `timeout`, default 3 s) | Yes | A failed `hover` is an error answer that writes nothing; a call that does not return is aborted and the answer names the timeout |
| `setVariable` | Yes | Locals, parameters, fields, properties |
| `continue`, `next`, `stepIn`, `stepOut`, `pause` | Yes | `continued` event after each resume; `stopped` with `allThreadsStopped: true` |
| `exceptionInfo` | Yes | `exceptionId`, `description`, `breakMode`, `details` with stack trace and inner exceptions |
| `disconnect` (`terminateDebuggee`), `terminate` | Yes | Default after `launch`: terminate; after `attach`: detach |
| Events: `initialized`, `stopped` (`breakpoint`, `step`, `exception`, `pause`, `entry`, `function breakpoint`), `continued`, `thread`, `breakpoint`, `output`, `process`, `exited`, `terminated` | Yes | |
| `restart`, `goto`/`gotoTargets` (Set Next Statement), `stepInTargets`, `stepBack`, `reverseContinue` | No | Mono.Debugging has `SetNextStatement`: left to proposal 0001 brief B |
| `completions`, `modules`, `loadedSources`, `source`, `readMemory`, `writeMemory`, `disassemble`, data and instruction breakpoints, `breakpointLocations` | No | The soft debugger has no watchpoints or native memory |
| `exceptionOptions`, `valueFormattingOptions` (hex), `setExpression`, `cancel`, `terminateThreads`, `runInTerminal` | No | The program's stdin is closed |

## 5. The expression evaluator

Mono.Debugging's C# evaluator (NRefactory 5.5.1 parser) was probed in two frames of the TestApp (an instance method
with `int` parameters and the static `Main`). With the adapter's fixes:

- **Works:** locals, parameters, `this`, fields and properties (private too), member chains, indexers
  (`names[4]`, `big[999]`), method calls on objects and statics (`order.Describe()`, `a.ToString()`,
  `Calculator.Twice(3)`), `new` of a class (`new System.Text.StringBuilder("q").Append(a).ToString()`), casts
  (`(long)a << 3`, `(byte)300`), `typeof`, `is`, `as`, `?:`, `?.`, string concatenation, `string.Format`, assignment
  (`sum = 9`), comparisons and logic (`a < b && b == 3`), and with the adapter's numeric retry all integer and
  floating arithmetic (`a + b * 2`, `total / 7`, `total / 7.0`, `(float)total / 3`, `'c' + 1`, `-a`, `~a`).
  Type names resolve from the frame's namespace outwards and then `System`, loaded or not (`Program.Hang()`,
  `Math.Max(a, b)`, `DateTime.Now.Year`, `Environment.NewLine`), and namespace-qualified and `global::` names work
  (`System.Math.Max(a, b)`, `Eludite.Debugger.Mono.TestApp.Calculator.Twice(5)`).
- **Fails with an error:** lambdas and LINQ (`names.Where(n => ...)`: "Expression not supported"), array creation
  (`new int[] { 1, 2 }`), `default(T)`, `checked(...)`, `nameof`, `++` and `--` (the numeric-cast message), division
  by a constant zero, and types of assemblies the debuggee has not loaded (`System.Linq.Enumerable` in a program that
  never touched `System.Core`). A method group without a call (`Program.Hang`) is "Unknown member".
- **Refused by the adapter:** interpolated strings (`$"{total}"`): the 2017 parser reads them as their literal text,
  so the evaluator answered `"{total}"`; the adapter now answers an error naming `string.Format`.
- **Side effects:** method and property evaluation runs debuggee code (as in Visual Studio). The numeric retry
  evaluates each operand once more to learn its type, so an operand with side effects runs twice in that case only.

## 6. Mono.Debugging gaps found

1. **Runs on Mono only.** `Mono.Debugger.Soft` uses delegate `BeginInvoke` (`PlatformNotSupportedException` on .NET
   10), so the adapter is net472 under the located Mono. Upstream debugger-libs targets `net6.0;net472` but publishes
   no packages (out of scope).
2. **`DebuggerLoggingService.CustomLogger` must be set** before a session starts (the library dereferences it); the
   adapter sets its own logger, which writes to stderr and `--log`.
3. **Stops arrive as `TargetEvent`** variants (`TargetHitBreakpoint`, `TargetStopped`, `TargetExceptionThrown`,
   `TargetUnhandledException`, `TargetInterrupted`), not only `TargetStopped`.
4. **Numeric unboxing bug:** binary operators unbox both operands as `long` or `double`, so `i == 5` on an `int` fails
   with "Value '0' of type System.Int32 cannot be casted to System.Int64" (fixed upstream since with
   `Convert.ToInt64`). The adapter re-evaluates with explicit casts and casts the result back to C#'s type.
5. **Type names** need an IDE's type system (MonoDevelop resolves them before evaluation), and
   `SoftDebuggerSession.GetType` knows only types the debuggee has loaded. The adapter's resolver asks the debuggee's
   VM (`VirtualMachine.GetTypes`) for capitalized names and answers a namespace's first segment as itself, so the
   evaluator's `global::` path handles qualified names.
6. **Function breakpoints bind every method of the type** (the name filter does not work in this build); the adapter
   filters hits by method and applies condition, hit condition and log message itself (hits counted after the
   condition, unlike source breakpoints, whose hits Mono counts before the condition).
7. **Slow value creation:** values are created on an evaluator thread and waited for with `WaitHandle.WaitAny`, which
   wakes 3 to 6 ms late under Mono 6.8; a frame with 200 locals took about 2 s. The adapter swaps the library's timed
   evaluator for its synchronous mode (by reflection on this pinned build), and evaluates on its dispatcher thread;
   invocations into the debuggee still end at the evaluation timeout. 200 locals now read in 6 to 7 ms.
8. **Just My Code step in** into a method of an assembly without symbols steps out of it and stops back on the
   calling line, part way through it; Visual Studio goes on to the next line. The adapter steps in again when a step in
   returns to the frame and line it started on (bounded to 32 times).
9. **Long arrays** (over 150 elements) are listed as ranges (`[0..99]`, ...) rather than paged by index, and an
   object's member count is unknown until its members are read, so the adapter gives `indexedVariables` only for
   short arrays and never `namedVariables` (the shell pages by `start` and `count` anyway).
10. **NRefactory 5.5 predates C# 6:** interpolated strings parse as plain strings (refused by the adapter), `nameof`
    is looked up as a method; lambdas, array creation, `default` and `checked` are unsupported.
11. **Step timing is bimodal:** `next` takes about 2 ms or about 12 ms (one of Mono.Debugging's waits); p95 stays near
    12.5 ms, far under the budget, so it was not chased.
12. **Detach left the adapter spinning (found by brief 0027, fixed since):** `DebuggerSession.Detach` only queues the
    work on the thread pool, the adapter detached twice (on `disconnect`, then on shutdown), and the second
    `VM_Dispose` could wait forever; Mono's `Environment.Exit`, which suspends every other thread first, then spun at
    about 90% of a core. The adapter now detaches once, waits for it (at most 1 s), sends `terminated` without
    `exited`, and exits through libc's `_exit`: 10 to 45 ms from the detach to its exit, the program running on.

## 7. Tests

| Where | Tests | What they prove |
|---|---|---|
| `Eludite.Debugger.Mono.Tests/ProtocolTests.cs` (no Mono) | 10 | Framing round trips (multibyte text, extra headers) and rejects a frame without `Content-Length`; the dispatcher answers unknown requests and throwing handlers with failures and runs posted work and requests on one thread in order; `launch` arguments with defaults and path resolution; hit conditions as the shell sends them; `{expression}` interpolation with `{{` escapes; the variables table pages `start` and `count` and is cleared on resume; exception filters and their type conditions; options, capabilities and command lines |
| `Eludite.Debugger.Mono.Tests/MonoAdapterTests.cs` (Mono; skips with a message naming the search order and packages when no Mono is found) | 8 | Over stdio: `initialize` capabilities, `launch` then `initialized`, verified or bound breakpoints, `configurationDone`, `stopped` `breakpoint` on the right line with `allThreadsStopped`, `threads`, `stackTrace` with `totalFrames` and `startFrame` 1, one `Locals` scope (`this`, `a`, `b`, `sum`, `doubled`), a `string[25]` paged with `start` 2 and `count` 3, an `int[1000]`'s ranges, an object expanded one level, `evaluate` of an identifier, a member chain, method calls, arithmetic, statics, unloaded and qualified type names, a refused interpolated string, a failing `hover` that writes nothing, a hanging call that times out and leaves the session usable, `setVariable` then `variables` and the program's result, `next`, `stepIn` (to the callee's opening brace), `stepOut` each a `step` stop on the expected line, references dropped on resume, 50 timed `next`s, the 200-locals frame timed, the adapter's RSS, `exited` 3 and `terminated`; `stopAtEntry` (`entry` in `Main`) and a Just My Code `stepIn` over `Console.WriteLine`; a condition that skips five hits, hit condition `%2` (hits 2 and 4), a log point writing 100 lines without stopping; a function breakpoint by `Namespace.Type.Method`; `all` stops at the caught throw with `exceptionInfo` (type, message, `always`, stack trace) and without it the throw does not stop; `pause` of the sleeping program with reason `pause`, `disconnect` ends it and `terminated` arrives; TCP with `--port 0` and an unknown request (`restart`) failing with its name; `attach` to a TestApp started with `server=y,suspend=y` |
| `crates/dap/src/launch.rs` | 3 new | The classification table; the legacy (`OutputPath` default and backslashes) and SDK-style `.exe` paths in temp projects and the missing-program error; the selection table with the Windows message, `runtime_name`, the Mono `launch` arguments and the Ctrl+F5 command lines per platform |
| `crates/dap/src/discovery.rs` | 2 new | Mono's search order (setting, `PATH` prefix, user space, system prefixes) and the relocated-prefix environment, in temp directories; the adapter's search (beside the executable, then the setting as file or folder) and both errors |
| `crates/dap/tests/mono.rs` | 1 | Eludite's DAP client against the real adapter debugging the built TestApp with the launch configuration `launch_config` computes for its net472 project: launch, breakpoint, stack, locals, `evaluate` (`a + b * 2`), `next`, `continue`, exit code and output, `terminated`, `disconnect`. Skips with a message unless Mono, the adapter and the TestApp are found |
| `crates/eludite/src/shell/debug/tests.rs` (headless, fake adapter) | 4 new | A net472 startup project sends a `mono` plan (`adapterID` `mono`, `program` ending in `.exe`, `runtimeExecutable`) and the state shows `runtime` `mono` and the adapter description; Ctrl+F5 runs `mono <exe>`; the Windows message with the platform forced through `DebugSetup.platform`, no adapter started; the two settings reach the searches |
| `crates/commands/src/settings.rs` | 1 updated | The settings registry lists the two new settings with their environment variables |
| `dotnet/tests/Eludite.Host.Tests/SolutionTreeTests.cs` | 1 updated | `Eludite.slnx` now lists twelve projects |

How the real-adapter Rust test ran: `ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace` after `dotnet build dotnet/Eludite.slnx` (the test also finds the repository's build output
when the variable is unset). It printed `eludite-dbg-mono under mono 6.8.0.105 (/usr/bin/mono)`. `crates/dap/tests/netcoredbg.rs` returned early (counted
as passed) because netcoredbg is not fetched on this machine; brief 0018's path is unchanged.

## 8. Deviations and findings

1. **Files outside the brief's list, each needed by a listed change:** `crates/commands/src/settings.rs` (only its
   test, which enumerates the settings `settings.json` defines) and `dotnet/tests/Eludite.Host.Tests/SolutionTreeTests.cs`
   (it asserts the number of projects in `Eludite.slnx`, which the brief grows from eight to twelve).
2. **`protocol/schemas/dap-mono.md` changed after the code** in commits 6, 8 and 9 to document behavior found while
   testing (range rows, stepping, type names, interpolated strings). It is documentation of the adapter, not a
   schema with generated bindings.
3. **"Cold" is cold for Mono's files only:** the page cache of Mono's runtime, class libraries, the adapter and the
   TestApp was evicted with `posix_fadvise(DONTNEED)` (a full drop needs root and would disturb the other worktree's
   build). The cold and warm numbers differ little: the adapter is JIT-compiled at every start either way.
4. **The launch budget is measured from the adapter**, not from a key press: there is no display. The time from the
   adapter's spawn (Mono start, JIT, handshake, debuggee start, binding, stop) is 221 to 309 ms; brief 0018 measured
   F5 to a shown break at 167 to 221 ms with netcoredbg, so F5 under Mono should land near 0.3 s.
5. **`continued` events** follow every resume, as DAP allows; brief 0018's shell decodes and ignores them (it moves to
   running when it sends the request).
6. **Not run:** Windows (where the adapter is not used: the shell refuses with the brief 0004 message, tested on Linux
   through the platform parameter) and macOS (the Mono framework prefix is in the search order and tested in temp
   directories only). CI was not run; its Linux .NET job installs `mono-devel`, and on Windows the adapter's Mono tests
   skip with their message.
7. **Not rebased** (see the header); `main`'s three new commits are briefs only.

## 9. What brief A of proposal 0001 (brief 0025, inspection depth) needs from this adapter

Brief 0025 can use the adapter as it is for `pause` (reason `pause`), `exceptionInfo` (with `details.innerException`
and `stackTrace`), `stackTrace` paging (`supportsDelayedStackTraceLoading` is true; `startFrame`, `levels`,
`totalFrames` tested), `variables` paging (`start` and `count` on every reference) and external frames
(`presentationHint: subtle`). Four things to plan for:

1. **Long arrays are ranges.** An array over 150 elements expands to range rows (`[0..99]`, ...) whose children are the
   elements; there is no `indexedVariables` on it. 0025's `variables` paging of a 10,000-element array works on the
   fake adapter; against Mono it pages the range rows, or the adapter must learn to page elements by index (a change
   in `MonoAdapter.Children`, about a day).
2. **No `namedVariables`** on objects: `total` for an object's members is known only after reading them.
3. **No parameter marking:** the one `Locals` scope holds `this`, the parameters and the locals in that order, with no
   `presentationHint` that tells parameters from locals (DAP has no such hint kind). 0025's `scope: arguments` would
   need the adapter to add an `Arguments` scope or a non-standard hint; `this` is recognizable by name.
4. **`supportsVariablePaging` is not advertised** (it is not a DAP capability); the adapter honors `start` and `count`
   on every reference, so 0025's `capabilities.variable_paging` should be true for adapter id `mono`. DAP's
   `variables` `filter` (`indexed`, `named`) is ignored.

## 10. How to reproduce

```
dotnet build dotnet/Eludite.slnx                     # the adapter, the TestApp and the tests
dotnet test dotnet/Eludite.slnx                      # Mono tests run when Mono is found
dotnet test --project debuggers/mono/Eludite.Debugger.Mono.Tests/Eludite.Debugger.Mono.Tests.csproj --output Detailed
                                                     # prints the timing and memory lines
ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe \
  cargo test -p eludite-dap --test mono -- --nocapture
cargo test --workspace
mono debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe --port 0 --log /tmp/dbg.log
                                                     # a TCP adapter for a remote client
tools/run-dev.sh                                     # the IDE with ELUDITE_DBG_MONO set (needs a display)
```
