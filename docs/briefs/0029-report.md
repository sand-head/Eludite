# Brief 0029 report: Debug Rust with lldb-dap

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). No display: every test
is headless.
Branch: `brief/0029-rust-debug-adapter`, based on `main` at `6be6af5` (briefs 0022 to 0025 merged). Brief 0026 (run
control) was in progress in another worktree; section 9 lists where the two meet. Date: 2026-10-03.
Brief: [0029-rust-debug-adapter.md](0029-rust-debug-adapter.md).

## 1. Summary

- **F5 on a Cargo package debugs its binary under lldb-dap.** A member package of the open folder's Cargo workspace can
  be the startup project (Set as Startup Project on the package, persisted, bold in Workspace, `startup: true` in
  `eludite.workspace.tree`). F5 builds it through brief 0019's Cargo build path (`cargo build -p <package>`, the build
  gate of brief 0020; a failed build does not launch), then starts `lldb-dap` (located on the machine:
  `/usr/bin/lldb-dap-18` here) with `adapterID` `lldb`, the executable `<target dir>/debug/<name>`, the workspace root
  as working directory, the arguments and environment of `[package.metadata.eludite.run]`, and the Rust toolchain's
  LLDB formatters in `initCommands`. `eludite.debug.state` reads `session.runtime: "native"` and
  `session.adapter: "lldb-dap 18.1.3 (stdio)"`. Ctrl+F5 runs the executable. `eludite.debug.start` with `test: true`
  and `args: ["my_test"]` builds the package's test executable (`cargo test --no-run`), takes it from cargo's
  `compiler-artifact` message and debugs it with the filter and `--nocapture`. The debugger windows, keys and the
  `eludite.debug.*` commands of briefs 0018 and 0025 are unchanged.
- **Rust values read as Rust**: `"hello"` for a `String`, `size=3` with `[0]`, `[1]`, `[2]` for a `Vec<i32>`,
  `Some(5)` and `None`, `Rect{w:2, h:3}`, `Circle(1.5)` and `Empty` for an enum, `size=10000` for a long `Vec` paged by
  `start` and `count`. This needed two compatibility lines before the formatters: Rust 1.98's formatters call two
  `SBValue` methods LLDB 18's Python API lacks, and without them every `String` and enum failed (section 5).
- **Exception Settings gains a Rust panics row** (`break_on_rust_panic`, on by default, persisted): a function
  breakpoint on `rust_panic`, sent before `configurationDone` and again when the row changes during a session. A panic
  stops in `__rustc::rust_panic` with the stack running through `core::panicking::panic_fmt` to the user's `main`.
- **lldb-dap 18's differences are absorbed at the adapter seam** (`crates/eludite/src/shell/debug/native.rs`): its
  pause is an `exception` stop described `signal SIGSTOP` (reported as `pause`); standard library frames carry
  `/rustc/<commit>/...` paths (mapped under the `rust-src` component when it is installed, so they open, otherwise
  shown as external code); its `hitCondition` is LLDB's ignore count rather than Visual Studio's semantics (the shell
  counts hits itself). It aborts on any request it does not know (section 4).
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, lldb-dap 18.1.3 from Ubuntu's `lldb-18`, Rust 1.98.1;
  load average 0.7 to 1.6; three runs of `cargo test -p eludite-dap --test lldb`, each after evicting LLVM, LLDB,
  Python and the formatters from the page cache):

  | Budget | Result |
  |---|---|
  | Launch to the first `stopped` at a breakpoint in the temp program < 3 s warm (report cold) | From spawning lldb-dap to the `stopped` event through `eludite-dap`: **cold 571, 780, 701 ms** (the run's first lldb-dap, after the eviction); **warm 619, 656, 684 ms** (the second launch of the run, which also binds the `rust_panic` breakpoint). Earlier runs without eviction: 527 to 628 ms. Pass |
  | `next` round trip < 150 ms p95 over 50 steps | **p95 7.0, 7.7, 8.4 ms** (p50 5.0 to 5.2 ms, max 7.0 to 9.8 ms). Pass |
  | Locals of a frame with a `Vec` of 10,000 elements paged by brief 0025's `variables` < 100 ms per page | A `variables` request with `start` and `count: 50` on the `Vec`'s reference (what 0025's `variables` sends when `capabilities.variable_paging`, now true for adapter `lldb`), 20 pages spread over the 10,000: **p95 17.7, 10.0, 11.5 ms** (max 18.7, 10.2, 12.1 ms). Measured at the DAP request; the shell's part was measured against the fake by brief 0025. Pass |
  | Cold start of Eludite unchanged; no new dependency | Pass by construction: `NativeSetup::from_env` reads `PATH` and two environment variables; the lldb-dap search, `lldb-dap --version` (and `lldb --version`), `rustc --print sysroot` and `cargo test --no-run` run on the `debug-launch` thread after F5. No new crate: `toml` 0.8 (`MIT OR Apache-2.0`) was already a workspace dependency (eludite-extensions) and is now also one of `eludite-dap`'s, for `[package.metadata.eludite.run]` (one line in `Cargo.lock`) |

  Also measured: `pause` to `stopped` 115 to 122 ms; `disconnect` to the adapter's exit 172 to 311 ms; the temp
  program's `cargo build` 326 to 445 ms; `rustc --print sysroot` 48 to 79 ms; `cargo test --no-run` (already built)
  210 to 247 ms.
- **Tests:** `cargo test --workspace` **578 passed, 0 failed, 1 ignored** (a doc example), with lldb-dap, Mono
  (`ELUDITE_DBG_MONO`) and Chrome (`ELUDITE_CHROME`) present so their real-adapter tests ran. `dotnet build` 0 warnings,
  0 errors; `dotnet test` 171 tests: 164 passed, 0 failed, 7 skipped (the same skips as brief 0022). fmt and clippy
  (`-D warnings`) clean. Section 7 lists the intermittent failures seen on the way.

## 2. What was built

Commits, in order:

1. `0832496` The brief's Status line.
2. `eae4592` `protocol/schemas/` alone: `debug-start.input.json` (Cargo packages and lldb-dap in the description;
   `target`, `test`, `args`), `debug-state.output.json` (`session.runtime` `native`, the `lldb-dap 18.1.3 (stdio)`
   adapter text, `capabilities.adapter` `lldb`, `exceptions.break_on_rust_panic`),
   `debug-exception-settings.input.json` (`break_on_rust_panic`), `settings.json` (`debugger.lldbDapPath` with
   `ELUDITE_LLDB_DAP`, `debugger.rustFormatters`, both under Debugging > General),
   `workspace-set-startup-project.input.json` (Cargo packages allowed), and the new `dap-lldb.md`.
3. `9fac2e0` `crates/dap`: lldb-dap discovery and version (`discovery.rs`), the Cargo launch configuration
   (`cargo.rs`), `FrameworkKind::Native` and `AdapterKind::Lldb` (`launch.rs`), function breakpoints in the start
   handshake (`session.rs`, `types.rs`), and `tests/lldb.rs` (section 2.1).
4. `e154478` The shell: `shell/debug/native.rs` (section 2.2), the launch thread's lldb branch, F5's Cargo build gate,
   Set as Startup Project on packages, the Rust panics row, the settings; `crates/commands` (`CargoOptions` on
   `eludite.debug.start`, `break_on_rust_panic`), and the headless tests in `shell/debug/native_tests.rs`.
5. `97478c6` `dap-lldb.md`: lldb-dap 18's pause, scope references and `setVariable` answer, found by the real-adapter
   test.
6. `1858c1d` `tools/lldb-dap/README.md`, `tools/run-dev.sh` (passes `ELUDITE_LLDB_DAP` and `ELUDITE_CODELLDB`
   through), `CLAUDE.md` (the tools row, the optional `lldb-18`), `README.md` (the Linux packages note, the layout).
7. `9a8c199` The tests use the platform's executable suffix.
8. `1f31adb` `dap-lldb.md`: lldb-dap 18's hit condition semantics.
9. `043becf` The shell counts hits for lldb-dap; the real-adapter test checks a condition and a hit count.
10. This report, the brief's Status line and the briefs index.

### 2.1 `crates/dap`

- `discovery.rs`, `LldbSearch`: the setting `debugger.lldbDapPath` (`ELUDITE_LLDB_DAP`; a file or the folder holding
  `lldb-dap` or `codelldb`; a wrong one is an error, not a fallback), `lldb-dap` then `lldb-dap-22` down to
  `lldb-dap-15` on `PATH`, `/usr/lib/llvm-22/bin/lldb-dap` down to `llvm-15`, `xcrun --find lldb-dap` on macOS, then
  CodeLLDB (`ELUDITE_CODELLDB`: the extension folder, its `adapter` folder or the executable; then
  `codelldb/adapter/codelldb` beside the eludite executable). `LldbAdapter::version`: `lldb-dap --version`, else
  `lldb --version` beside the resolved executable. The error lists every place and the package for the platform (from
  `tools/lldb-dap/README.md`).
- `cargo.rs`: `CargoPackageInfo::binary` (the only `bin`, or the named one, else refused listing them),
  `binary_path` (`<target dir>/<debug|release>/<name>`), `parse_artifacts`, `binary_executable` and
  `test_executable` (from `compiler-artifact` messages: `target`'s test executable, else the library's unit tests,
  else the only one), `RunMetadata` (`[package.metadata.eludite.run]`, `args` a list, `env` a table, with the file
  named in errors), `test_build_command` and `test_arguments` (`--nocapture` added), `sysroot` (`rustc --print sysroot`
  in the workspace root, `rustc` beside a configured cargo), `rust_init_commands`, `STEP_AVOID`, `rust_src` and
  `map_rustc_path`, `CargoStart` (`binary_launch`, `test_launch`, `build_tests` streaming cargo's lines),
  `CargoLaunch::lldb_arguments` (section 3).
- `launch.rs`: `FrameworkKind::Native` (runtime `native`, adapter `Lldb` on every platform, Ctrl+F5 runs the
  executable), `LaunchConfig::from_cargo`.
- `session.rs`: `StartPlan::function_breakpoints`, sent with `setFunctionBreakpoints` after the source breakpoints and
  before `configurationDone` when the adapter has function breakpoints; a refused one does not fail the start.

### 2.2 The shell

- `shell/debug/native.rs`: `NativeSetup` (the search, the formatters switch), `CargoContext` (a snapshot of the Cargo
  model for the launch thread: members with their binaries, target directory, the toolbar's configuration, the cargo
  of `build.cargoPath`), `resolve_cargo` (the hint's package, else the startup project's, else, when the solution has
  no executable project, the root package with a binary or the first member with one), `prepare` and `connect` (the
  launch thread), `adapt` (on the client's reader thread: the pause stop and the standard library frames),
  `adapt_capabilities` (hit counts in the shell), `function_breakpoints`, `Shell::set_cargo_startup` and
  `Shell::debug_folder_opened`.
- `shell/debug.rs` (kept to the adapter-selection and launch seams): the launch thread resolves a Cargo package first
  and otherwise runs brief 0022's .NET path unchanged; `target`, `test` and `args` on a .NET project are refused with a
  message; the lldb adapter's description, client sink, function breakpoints and capabilities; F5's build gate also
  applies to an open Cargo workspace; the Rust panics row sends `setFunctionBreakpoints` during a native session;
  `capabilities.variable_paging` is true for adapter `lldb`.
- Set as Startup Project (`shell/startup.rs`, `shell/explorer.rs`, `shell/folder.rs`, `shell.rs`): enabled on Cargo
  packages in the context menu; kept as the package's `Cargo.toml` in the same `startup_project` field; bold in
  Workspace; `startup: true` on the `cargo` row of `eludite.workspace.tree`. A folder with a Cargo workspace and no
  solution now keeps its debugger state (breakpoints, watches, exception settings, startup project) in the store keyed
  by the folder's `Cargo.toml`; before, such a folder persisted nothing.
- Settings (`shell/settings.rs`): `debugger.lldbDapPath` and `debugger.rustFormatters` reach the next session.
- Exception Settings window (`shell/debug/windows.rs`): the "Rust panics" row.

## 3. The launch arguments and the Rust formatters

What Eludite sends (`protocol/schemas/dap-lldb.md` has the whole contract):

```json
{"type": "lldb", "request": "launch", "program": "<target dir>/debug/app", "args": ["--fast"], "cwd": "<workspace root>",
 "env": ["APP_MODE=dev"], "stopOnEntry": false, "sourceMap": [],
 "initCommands": [
   "settings set target.process.thread.step-avoid-regexp ^<?(std|core|alloc)::",
   "script if not hasattr(lldb.SBValue, 'GetValueAsAddress'): lldb.SBValue.GetValueAsAddress = lldb.SBValue.GetValueAsUnsigned",
   "script if not hasattr(lldb.SBValue, 'GetSyntheticValue'): lldb.SBValue.GetSyntheticValue = lambda self: (lambda v: (v.SetPreferSyntheticValue(True), v)[1] if v.IsValid() else v)(self.GetNonSyntheticValue())",
   "command script import \"<sysroot>/lib/rustlib/etc/lldb_lookup.py\""]}
```

- **What `rust-lldb` runs.** Rust 1.98.1's `rust-lldb` (`<sysroot>/bin/rust-lldb`) runs exactly one command,
  `command script import "<sysroot>/lib/rustlib/etc/lldb_lookup.py"`, through `--one-line-before-file`; the toolchain
  has no `lldb_commands` file any more (older ones did, and Eludite sources it after the import when it exists).
  `lldb_lookup.py`'s `__lldb_init_module` registers the formatters in a `Rust` category.
- **What the formatters format** (lldb-dap 18.1.3, Rust 1.98.1, with the compatibility lines): `String` (`"hello"`,
  children its bytes as `'h'`...), `&str`, `Vec<T>` and `VecDeque<T>` (`size=N`, children `[i]`), slices (`size=N`),
  `Option<T>` (`Some(5)` with child `0`, `None`), any enum by its variant (`Rect{w:2, h:3}`, `Circle(1.5)`, `Empty`),
  and tuples; `OsString`, `Path`, `PathBuf`, `HashMap`, `HashSet`, `Rc`, `Arc`, `Cell`, `RefCell` and `NonZero` have
  providers too (registered by `lldb_lookup.py`; not exercised by the tests). Integers read as numbers with C type names (`int` for `i32`, `unsigned int` for `u32`).
- **What they do not do on LLDB 18.** Without the two compatibility lines, `StdStringSummaryProvider` fails with
  `AttributeError: 'SBValue' object has no attribute 'GetValueAsAddress'` and `ClangEncodedEnumProvider` with
  `... 'GetSyntheticValue'` (both LLDB 19 APIs; `GetStaticFieldWithName` and `GetConstantValue` are missing too but
  only the MSVC enum path uses them, behind a feature check). `lldb_lookup.py` checks LLDB's features for the static
  fields and type recognizers, but not for these two. The lines define each method only when LLDB lacks it, so a newer
  LLDB is untouched.
- **Without the formatters** (`debugger.rustFormatters` off, or no `lldb_lookup.py`): every aggregate reads as its type
  and address, `alloc::string::String @ 0x7fffffffbec0`, `alloc::vec::Vec<int, alloc::alloc::Global> @ 0x...`,
  `core::option::Option<i32> @ 0x...`, `lldbtest::Shape @ 0x...`, with the raw fields (`vec`, `buf`, `len`,
  `$variants$`) as children.
- **Standard library sources.** Frames in `std`, `core` and `alloc` carry `/rustc/<commit>/library/...`. The shell maps
  them to `<sysroot>/lib/rustlib/src/rust/library/...` when the `rust-src` component is installed (it is on this
  machine), so they open; otherwise it removes the path and marks the frame `subtle`, so it is external code. This is
  done on the client's reader thread rather than with lldb-dap's `sourceMap`, which would need the commit hash (the
  stable toolchain's frames on this machine carried a different hash from 1.98.1's).

## 4. lldb-dap 18.1.3's DAP features

As brief 0018's section 5, for lldb-dap from Ubuntu 24.04's `lldb-18` (the real-adapter test's findings):

| Feature | lldb-dap 18.1.3 | Notes |
|---|---|---|
| stdio, `--port N` | Yes | Eludite uses stdio. `--version` prints nothing (exit 0); the version comes from `lldb --version` beside it |
| `initialize` capabilities | `supportsConfigurationDoneRequest`, `supportsConditionalBreakpoints`, `supportsHitConditionalBreakpoints`, `supportsFunctionBreakpoints`, `supportsLogPoints`, `supportsEvaluateForHovers`, `supportsSetVariable`, `supportsExceptionInfoRequest`, `supportsExceptionOptions`, `supportsDelayedStackTraceLoading`, `supportsRestartRequest`, `supportsModulesRequest`, `supportsDisassembleRequest`, `supportsCompletionsRequest`, `supportsValueFormattingOptions`, `supportsProgressReporting`, `supportsRunInTerminalRequest`, `supportTerminateDebuggee`; filters `cpp_catch`, `cpp_throw`, `objc_catch`, `objc_throw`, `swift_catch`, `swift_throw` (all off by default) | False: `supportsGotoTargetsRequest`, `supportsStepBack`, `supportsRestartFrame`, `supportsStepInTargetsRequest`, `supportsLoadedSourcesRequest`. Not answered: `supportsTerminateRequest`, `supportsExceptionFilterOptions`, `supportsDataBreakpoints`, `supportsReadMemoryRequest`. No version anywhere |
| `launch` (`program`, `args`, `cwd`, `env` as a list of `NAME=value`, `stopOnEntry`, `initCommands`, `sourceMap`) | Yes | An `env` object is silently ignored. Program output comes through a pseudo-terminal: `\r\n` line ends, long writes in pieces. The `initCommands` are echoed as `console` output |
| Source breakpoints, `condition`, log points (`logMessage` with `{expr}`) | Yes | Tested: a condition `i == 7` in a loop stops at `i = 7`; a log point wrote a line per call (76 lines) without stopping |
| `hitCondition` | A bare number only, as LLDB's ignore count | `3` set at `i = 7` stopped at `i = 10` (the third hit after it was set) and would stop on every hit after; `>=N` and `%N` do not parse and break on every hit. The shell counts hits itself for lldb-dap |
| Function breakpoints | Yes | `rust_panic` binds to `__rustc::rust_panic` in `library/std/src/panicking.rs`, `verified` |
| `threads`, `stackTrace` (`startFrame`, `levels`), `scopes`, `variables` (`start`, `count`) | Yes | Scopes `Locals` (with `namedVariables`), `Globals`, `Registers`, references always 1, 2, 3 for **the frame of the last `scopes` request** (a `setVariable` with reference 1 after `scopes` of another frame targets that frame). `Vec`s give `indexedVariables` |
| `evaluate` (`watch`, `hover`, `repl`) | Yes, LLDB's C++-flavored evaluator | Section 6. A `hover` answer is LLDB's whole `frame variable` text with children; a `repl` one names `$0` |
| `setVariable` | Yes | Answers `result` where DAP says `value`. `count` set to 40 was read by the program; a parameter set at a function's first line was not (rustc held it in a register at `-O0`) |
| `next`, `stepIn`, `stepOut`, `continue` | Yes | `step` stops; Step Into enters `add` and steps over `String::len` and `Vec::len` with Eludite's step-avoid regexp; `next` from a tail expression may stop on the closing brace |
| `pause` | Yes, as an `exception` stop | Reason `exception`, description `signal SIGSTOP`; the shell reports reason `pause` |
| `exceptionInfo` | Answers every stop | `exceptionId: "exception"`, `breakMode: "always"`, the stop's description (`breakpoint 2.1`); no panic message |
| `restart` | Yes | Relaunches; thread ids change (the shell does not use it yet: brief 0027) |
| `disconnect` (`terminateDebuggee`) | Yes | `exited` 9 then `terminated`; the adapter exits |
| `terminate`, `gotoTargets`/`goto`, `stepInTargets`, data breakpoints, `readMemory` | No | |
| **An unknown request** | **Aborts the adapter** (SIGABRT, exit -6) | Probed with `gotoTargets` and an invented command. Every request must be gated on the capabilities |

## 5. What the Rust formatters do and do not format

Section 3 has the detail. In short: with the compatibility lines, everything the tests check reads as Rust (`String`,
`Vec`, slices, `Option`, user enums with tuple, struct and unit variants); without them on LLDB 18, `Vec` and slices
still work but `String` and every enum (`Option` included) fail inside the formatters and read as type and address.
Without the formatters at all, every Rust aggregate reads as type and address. What the tests saw unformatted even
with them: type names stay LLDB's C view (`int` for `i32`, `unsigned int` for `u32`, `unsigned char` for a `String`'s
bytes, `alloc::vec::Vec<int, alloc::alloc::Global>` as the type of a `Vec<i32>`), and a `String`'s children are its
bytes rather than its characters. Closures, trait objects and types without a provider show their raw fields (not
tested).

## 6. The evaluator's limits

LLDB's expression evaluator parses C++ against the Rust program's DWARF:

- **Works:** locals and fields (`count`, `text`, `o`), indexing a `Vec` or slice (`v[1]` is `2`, through the
  formatter's synthetic children), integer arithmetic and comparison (`count + 1`), assignment through `setVariable`.
- **Fails with an error:** method calls (`v.len()`: "called object type 'unsigned long' is not a function or function
  pointer"; `text.len()`: "no member named 'len' in 'alloc::string::String'"; `o.is_some()`), Rust syntax (`&v[..2]`:
  "expected expression"; `count as i64`: "unknown type name 'as'"), macros, closures, paths with generics. `v` alone
  works and shows `size=3`.
- **So Watch and data tips** show variables and fields well and expressions poorly; agents should read values with
  `eludite.debug.variables` rather than compute them with `evaluate`.

## 7. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/dap/src/discovery.rs` | 3 new | The search order in temp directories (the setting as file or folder and its error, `lldb-dap` before `lldb-dap-22`..`-15` on `PATH`, newest first, `/usr/lib/llvm-NN` newest first, CodeLLDB under `ELUDITE_CODELLDB` and beside the executable, the error listing every place with the package per platform); version parsing and descriptions; the installed lldb-dap's version (18.1.3, from `lldb --version`) |
| `crates/dap/src/cargo.rs` | 7 new | `[package.metadata.eludite.run]` parsing (inline and section tables, missing table, wrong types named); binary choice; executables from recorded `cargo test --no-run` artifact messages (lib, bin, integration test; the computed and the message's path); the test command line and arguments; the launch arguments with `initCommands` from a fake sysroot (with and without `lldb_commands`); `rust-src` mapping; `rustc` beside a configured cargo; a start resolving the binary and the test executable; this repository's toolchain has the formatters |
| `crates/dap/src/launch.rs`, `types.rs` | 2 extended | `Native` selects lldb and runtime `native` on every platform, Ctrl+F5 runs the executable; lldb-dap 18's recorded `initialize` answer decodes |
| `crates/dap/tests/lldb.rs` (skips without lldb-dap or cargo) | 2 | Section 1's real-adapter checks: the executable from the build's artifact message, the run table, launch, breakpoints, `setVariable` reaching the program, the stack with the caller, Rust-formatted locals, paging the 10,000-element `Vec`, `evaluate` and its failures, `next`, `stepOut`, `stepIn` over the standard library into a user function, 50 timed `next`s, a log point, the program's output with the arguments and environment, a condition, a hit count, the panic at `rust_panic` with the stack through `main`, `exceptionInfo`, `pause`, `disconnect`, the values without formatters; the test executable built with `cargo test --no-run` and run with `my_test` breaking in `lldbtest::tests::my_test`, `1 passed; ... 1 filtered out` |
| `crates/eludite/src/shell/debug/native.rs` | 2 | Package resolution (hint by name, `Cargo.toml` or folder; the startup project; the default when no .NET project is executable); the pause and frame adaptation with and without `rust-src`; hit counts in the shell; the panic function breakpoint |
| `crates/eludite/src/shell/debug/native_tests.rs` (headless, fake adapter in lldb-dap's place) | 3 | Set as Startup Project on the package (command answer, bold in Workspace, `startup` in the tree, persisted beside the folder's `Cargo.toml`); F5's plan: `adapterID` `lldb`, `program` under `target/debug`, `cwd` the workspace root, `args` and `env` from the run table, `initCommands` with the step filter and the formatters from the fake sysroot, `setFunctionBreakpoints` `rust_panic` before `configurationDone`; `runtime` `native`, the adapter text, `hit_conditions` `shell`; the Rust panics row in the Exception Settings window sends `setFunctionBreakpoints` `[]` and the next session sends none; an unknown target refused naming the binaries; the settings reach the search, the formatters off leave only the step filter, and without an adapter F5 names where it looked and `tools/lldb-dap/README.md`; (Unix, a shell-script `cargo`) F5 builds through `cargo build -p app` and does not launch on a failed build ("Not started: the build failed (1 error, 0 warnings)"), launches on success; Ctrl+F5 runs the executable in the workspace root with the run table; `test: true` with `args: ["my_test"]` runs `cargo test --no-run`, shows cargo's lines in the Debug source and launches the named test executable with `my_test --nocapture` |
| `crates/commands/src/debug.rs`, `settings.rs` | 2 extended | `start`'s `target`, `test`, `args` and their validation; `break_on_rust_panic`, and a settings file from before brief 0029 reads it as on; the settings registry lists the two new settings |

Runs: `cargo test --workspace` (with `ELUDITE_DBG_MONO` after `dotnet build`, `ELUDITE_CHROME` and
`ELUDITE_CHROME_NO_SANDBOX`): 578 passed, 0 failed, 1 ignored. Intermittent failures seen on the way, none in code
this brief touched, each passing when rerun:

- `shell::debug::tests::the_context_menu_sets_the_startup_project_and_builds_and_it_persists` (brief 0020) timed out
  waiting for the default startup project once in a full run and once in three runs alone; it then passed 18 times in a
  row alone. The default startup project is found off the UI thread and dropped when the shell's generation moved
  meanwhile; a race between the tree and the generation update would explain it. Not chased (outside this brief).
- `crates/browser/tests/chrome.rs` (brief 0023): two tests failed once in a full run with "Chrome did not report its
  DevTools endpoint within 10 s (exit status: 0)" while the machine ran the whole suite; they passed alone and in the
  next full run.
- `dotnet test` reported 1 failure in `Eludite.Host.Tests` in one of four runs (the run right after the Rust suite; the
  failing test's name was not captured); the other three runs passed 164, skipped 7. No .NET code changed.

## 8. Deviations and findings

1. **Files outside the brief's list**, each needed by a listed change: `crates/commands/src/debug.rs` (the start's
   `target`, `test`, `args` and the Rust panics row are command inputs: invariant 3), `crates/commands/src/settings.rs`
   (its test lists the settings), `protocol/schemas/debug-exception-settings.input.json` (the row's input),
   `crates/eludite/src/shell/startup.rs` (where Set as Startup Project is implemented; the brief names `shell.rs`),
   `shell/folder.rs` (one line: the tree's `startup` mark on `cargo` rows), `shell/debug/windows.rs` (the row),
   `shell/debug/state.rs` (two test constructions of the start request), and the existing `crates/dap/tests/*.rs` (the
   start plan's new field). New logic went into new files, `shell/debug/native.rs` and `shell/debug/native_tests.rs`,
   rather than into `debug.rs` and `debug/tests.rs`, to keep the merge with brief 0026 small.
2. **The binary's executable is not taken from the artifact message of the build F5 ran.** The Cargo build path
   (`shell/cargo_build.rs`, `shell/build.rs`) parses and drops the artifact messages, and changing it was outside the
   brief's files. The launch uses `<target_directory>/<debug|release>/<name>` from `cargo metadata`, which is where
   cargo puts (uplifts) a host binary and what the artifact message names for it; `binary_executable` takes the
   message's path when one is at hand (tested with recorded messages and in `tests/lldb.rs`). The **test executable**
   does come from artifact messages: the launch thread runs `cargo test --no-run --message-format=json... -p
   <package>` itself after F5's build, streaming its lines to the Debug source; a compile error there ends the start
   with its message but is not an Error List row. Wiring both through the build path (an artifacts slot on
   `CargoBuildSpec`, a `test` build kind) is about half a day.
3. **`initCommands` are not exactly `rust-lldb`'s**: the step filter comes first and two LLDB 18 compatibility lines
   precede the import (section 3). Without them the brief's `"hello"`, `Some(5)` and enum checks fail on this machine's
   LLDB.
4. **The version** comes from `lldb --version` beside lldb-dap: lldb-dap 18 prints nothing for `--version` and its
   `initialize` answer has no version.
5. **`sourceMap` stays empty** and the `rust-src` mapping is done in the shell (section 3).
6. **Pause and hit counts** are adapted in the shell (section 4).
7. **CodeLLDB**: discovered and started on stdio with lldb-dap's `launch` arguments, never run (no download from
   GitHub here). CodeLLDB takes `env` as an object and loads its own Rust formatters (`sourceLanguages: ["rust"]`), so
   its launch arguments likely need a flavor of their own; untested.
8. **`lldb-dap-15` to `-17`** are searched as the brief says, but LLVM 15 to 17 install `lldb-vscode-NN`; the README
   says to point the setting at it.
9. **Dogfooding** used the temporary program, not a member of this repository: this machine's local cargo
   configuration builds the repository with `debug = "line-tables-only"` (no locals), and a full-debug build of a
   member would add gigabytes to the worktree's `target/` on a tight disk. The temporary program's package sets
   `debug = 2` and builds in 0.3 to 0.4 s.
10. **The Debug menu** needed no change: F5 and Ctrl+F5 were already enabled; with an open Cargo workspace and no
    startup project they start the root package (or the first member with a binary).
11. **Not run:** Windows and macOS (the search's `.exe` names, `xcrun`, LLVM's Windows paths are tested through temp
    directories only); a display (everything is headless).

## 9. What the next briefs need

**Brief 0026 (run control)** meets this one in `shell/debug.rs`: the launch thread's lldb branch, the
`ExceptionSettings` arm (the Rust panics row sends `setFunctionBreakpoints` with `native::function_breakpoints`; 0026's
user function breakpoints must be sent in the same list, since DAP's `setFunctionBreakpoints` replaces them all), and
`capabilities_row`. Against lldb-dap: Set Next Statement must stay disabled (`supportsGotoTargetsRequest` false), and
no request outside the capabilities may be sent (lldb-dap 18 aborts on it); `setVariable` answers `result`; set a
variable of a frame only after `scopes` of that frame; tracepoints can use `logMessage`.

**The Test Explorer brief, for `cargo test` debugging:**

1. **Debug a test** with `eludite.debug.start { project: <package or Cargo.toml>, test: true, target: <test target>,
   args: [<test path>, "--exact"] }`: the launch builds with `cargo test --no-run`, picks the executable of `target`
   (the lib's unit tests by default; an integration test by its file name; a bin's unit tests by the bin's name) and
   adds `--nocapture`. Add `--test-threads=1` when debugging one test, so a breakpoint in shared code stops in that
   test's thread.
2. **Listing**: `<test executable> --list --format terse` prints `path::to::test: test` lines on stable Rust; JSON
   events (`--format json`) need `-Z unstable-options` on nightly. Run it per test executable from the same artifact
   messages (`eludite_dap::cargo::parse_artifacts`, `profile.test`).
3. **Results** without the debugger: `cargo test -p <package> -- --format terse` per executable, parsing `test <name>
   ... ok|FAILED|ignored`; exit code 101 on failures. Doc tests are not debuggable (rustdoc compiles them on the fly).
4. **Build output**: route `cargo test --no-run` through the Cargo build path (deviation 2) so test compile errors
   become Error List rows.

## 10. How to reproduce

```
sudo apt install lldb-18                                   # lldb-dap-18, python3-lldb-18
cargo test -p eludite-dap --test lldb -- --nocapture       # the real adapter; prints the timing: lines
cargo test -p eludite native                               # the shell's headless tests and native.rs
dotnet build dotnet/Eludite.slnx
ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe \
  ELUDITE_CHROME=/opt/pw-browsers/chromium-1194/chrome-linux/chrome ELUDITE_CHROME_NO_SANDBOX=1 \
  cargo test --workspace
dotnet test dotnet/Eludite.slnx
tools/run-dev.sh --folder <a Cargo workspace>              # F5 on a package (needs a display)
```

The cold numbers evict LLVM, LLDB, Python and the formatters from the page cache first, with
`posix_fadvise(POSIX_FADV_DONTNEED)` on each file (a full drop needs root and would disturb the other worktree's
build).
