# lldb-dap: debugging Cargo packages (Rust) through LLDB

Eludite debugs the executables of Cargo packages with `lldb-dap`, LLVM's LLDB debug adapter (Apache-2.0 WITH
LLVM-exception), located on the machine and never bundled (PLAN.md 4.5, brief 0029). CodeLLDB's adapter (MIT,
`adapter/codelldb`) is accepted by the same discovery as an alternative. This file records what Eludite sends and what
lldb-dap 18.1.3 (Ubuntu 24.04's `lldb-18` package) answers; `tools/lldb-dap/README.md` says how to install it.

## Selection and discovery

A startup project that is a member package of the open folder's Cargo workspace (`eludite.workspace.tree` kind
`cargo`) is debugged under lldb-dap on every platform; `session.runtime` is `native` and `capabilities.adapter` is
`lldb`. The adapter is the first of (`crates/dap/src/discovery.rs`, `LldbSearch`):

1. the setting `debugger.lldbDapPath` (`ELUDITE_LLDB_DAP`): the executable, or the folder holding it;
2. `lldb-dap` on `PATH`, then `lldb-dap-22`, `lldb-dap-21`, ... down to `lldb-dap-15` on `PATH` (Debian and Ubuntu
   install versioned names);
3. `/usr/lib/llvm-22/bin/lldb-dap` down to `/usr/lib/llvm-15/bin/lldb-dap`;
4. on macOS, `xcrun --find lldb-dap` (Xcode's toolchain);
5. CodeLLDB: `ELUDITE_CODELLDB` (its extension folder, its `adapter` folder or the `codelldb` executable), then
   `codelldb/adapter/codelldb` beside the eludite executable.

An executable named `codelldb` is CodeLLDB; anything else is lldb-dap. Without one, F5 fails with a message that lists
where it looked and names the package to install for the platform.

The version in `session.adapter` (`lldb-dap 18.1.3 (stdio)`) is read once on the launch thread: from
`lldb-dap --version`, else from `lldb --version` beside the resolved executable (lldb-dap 18 prints nothing for
`--version`, and its `initialize` answer carries no version), else `(unknown version)`.

## Transport

stdio: `lldb-dap` with no arguments speaks DAP on stdin and stdout (`Content-Length` framing). Its own diagnostics go to
stderr. (`--port N` listens on TCP; Eludite does not use it.) CodeLLDB is started the same way, `codelldb` with no
arguments (not run on this machine: its releases are on GitHub, which this machine cannot download from).

## initialize

`adapterID` is `lldb`; lines and columns are 1-based and paths native, as for every adapter.

## launch

| Argument | Value Eludite sends |
|---|---|
| `program` | The executable: the binary target's `<target dir>/debug/<name>` (`release` in the Release configuration; `.exe` on Windows), or the test executable named by the `compiler-artifact` message of `cargo test --no-run --message-format=json -p <package>` |
| `args` | `[package.metadata.eludite.run]`'s `args`, or the start's `args`; for a test executable the filter and `--nocapture` |
| `cwd` | The Cargo workspace root (what `cargo run` uses) |
| `env` | `[package.metadata.eludite.run]`'s `env` as `"NAME=value"` strings (lldb-dap 18 takes a list of strings, not an object), added to the inherited environment |
| `stopOnEntry` | false |
| `initCommands` | Below |
| `sourceMap` | `[]` |

### `[package.metadata.eludite.run]`

A package's `Cargo.toml` may say how F5 and Ctrl+F5 run its binary:

```toml
[package.metadata.eludite.run]
args = ["--verbose", "input.txt"]
env = { RUST_LOG = "debug", RUST_BACKTRACE = "1" }
```

`args` is a list of strings; `env` a table of strings. Both are optional; without the table the program gets no
arguments and the inherited environment. The table is read on the launch thread when F5 starts the package.

### initCommands

Eludite sends, in order:

1. `settings set target.process.thread.step-avoid-regexp ^<?(std|core|alloc)::`: Step Into does not enter the
   standard library (Visual Studio's Just My Code; LLDB's default regexp is `^std::`, which misses Rust's `core`,
   `alloc` and trait-impl names such as `<alloc::string::String as core::convert::From<&str>>::from`).
2. When `debugger.rustFormatters` is on (the default) and `<sysroot>/lib/rustlib/etc/lldb_lookup.py` exists, the Rust
   formatters, as `rust-lldb` loads them. `<sysroot>` is `rustc --print sysroot` run once in the workspace root on the
   launch thread (`rustc` beside the configured cargo, else on `PATH`), so the rustup proxy honors
   `rust-toolchain.toml`:
   - two compatibility lines for LLDB older than 19, which define what the formatters of Rust 1.98 call and LLDB 18's
     Python API lacks, and nothing on a newer LLDB:
     ```
     script if not hasattr(lldb.SBValue, 'GetValueAsAddress'): lldb.SBValue.GetValueAsAddress = lldb.SBValue.GetValueAsUnsigned
     script if not hasattr(lldb.SBValue, 'GetSyntheticValue'): lldb.SBValue.GetSyntheticValue = lambda self: (lambda v: (v.SetPreferSyntheticValue(True), v)[1] if v.IsValid() else v)(self.GetNonSyntheticValue())
     ```
     Without them every `String` and every enum fails in the formatters with `AttributeError` (Locals then read
     `alloc::string::String @ 0x7fff...`);
   - `command script import "<sysroot>/lib/rustlib/etc/lldb_lookup.py"` (the one line `rust-lldb` 1.98 runs);
   - `command source "<sysroot>/lib/rustlib/etc/lldb_commands"` when that file exists (older toolchains shipped it;
     1.98 does not).

lldb-dap echoes the commands as `output` events with category `console` (the shell's `adapter` output source).

## Breakpoints and exceptions

- Source breakpoints with `condition` (an LLDB expression), `hitCondition` (a number: lldb-dap sets LLDB's ignore
  count to N-1, so it breaks on the Nth hit and every one after; not exercised by Eludite's tests) and `logMessage`
  (`{expression}` interpolated; the line is an `output` event with category `console`, and the program does not stop).
  Answers are `verified` with the bound line, column and `instructionReference`.
- Function breakpoints by name. The Exception Settings window's **Rust panics** row (`break_on_rust_panic`, default on)
  is a function breakpoint on `rust_panic`, sent with `setFunctionBreakpoints` before `configurationDone`; a panic stops
  with reason `breakpoint` in `__rustc::rust_panic` (`library/std/src/panicking.rs`), the stack running through
  `core::panicking::panic_fmt` to the user's function and `main`.
- Exception filters: `cpp_catch`, `cpp_throw`, `objc_catch`, `objc_throw`, `swift_catch`, `swift_throw`, all off by
  default. The Common Language Runtime filters (`all`, `user-unhandled`) are not offered, so they are not sent.
- `exceptionInfo` answers every stop with `exceptionId: "exception"`, `breakMode: "always"` and the stop's description
  (`breakpoint 3.1`); it has no Rust panic message.

## Capabilities lldb-dap 18.1.3 answers

`supportsConfigurationDoneRequest`, `supportsConditionalBreakpoints`, `supportsHitConditionalBreakpoints`,
`supportsFunctionBreakpoints`, `supportsLogPoints`, `supportsEvaluateForHovers`, `supportsSetVariable`,
`supportsExceptionInfoRequest`, `supportsExceptionOptions`, `supportsDelayedStackTraceLoading`, `supportsRestartRequest`,
`supportsModulesRequest`, `supportsDisassembleRequest`, `supportsCompletionsRequest`, `supportsValueFormattingOptions`,
`supportsProgressReporting`, `supportsRunInTerminalRequest`, `supportTerminateDebuggee`: true.
`supportsGotoTargetsRequest`, `supportsStepBack`, `supportsRestartFrame`, `supportsStepInTargetsRequest`,
`supportsLoadedSourcesRequest`: false. Not answered (so false): `supportsTerminateRequest`,
`supportsExceptionFilterOptions`, `supportsDataBreakpoints`, `supportsReadMemoryRequest`.

## Values

- Scopes: `Locals` (`presentationHint: locals`, with `namedVariables`), `Globals`, `Registers`. Their
  `variablesReference`s are always 1, 2 and 3 and mean the frame of the **last** `scopes` request: after `scopes` of
  frame 1, reference 1 is frame 1's locals, also for `setVariable`. A client that reads another frame must ask
  `scopes` again before using the first frame's top-level references (children keep their own references until the
  program resumes).
- `variables` honors `start` and `count` (a page of a 10,000-element `Vec`), so the shell treats adapter `lldb` as
  paging (`capabilities.variable_paging`). A `Vec` or slice gives `indexedVariables`.
- With the formatters: `String` `"hello"` (children are its bytes), `Vec<i32>` `size=3` with `[0]`..., `&[i32]`
  `size=2`, `Option<i32>` `Some(5)` (child `0`) and `None`, an enum `Rect{w:2, h:3}`, `Circle(1.5)`, `Empty`; integers
  as numbers (`i32` reads type `int`). Without them every Rust aggregate reads as its type and address
  (`alloc::string::String @ 0x7fffffffbec0`, `alloc::vec::Vec<int, alloc::alloc::Global> @ 0x...`,
  `core::option::Option<i32> @ 0x...`, `lldbtest::Shape @ 0x...`) with its raw fields as children.
- `evaluate` (`watch`, `hover`, `repl`) runs LLDB's expression evaluator, which is C++-flavored: locals, fields,
  `v[1]` and arithmetic (`count + 1`) work; method calls do not (`v.len()`: "called object type 'unsigned long' is not
  a function"), nor Rust syntax (`&v[..]`, `as`, closures, macros). A `hover` answer is LLDB's whole `frame variable`
  text (`(alloc::string::String) text = "hello" {\n  [0] = 'h' ...`); a `repl` one names a `$0` result variable.
- `setVariable` sets a scalar (`count` to `40`, which the program then reads). lldb-dap 18 answers with `result` where
  DAP says `value`. A function's parameter set at a breakpoint on the function's first line may not change what the
  function computes: at `-O0` rustc may already hold it in a register.

## Stepping, pause, stack

- `next`, `stepIn`, `stepOut` stop with reason `step`; `stepIn` enters user functions and steps over the standard
  library (initCommand 1); `next` from a function's last statement may stop on its closing brace (rustc gives a tail
  expression no line entry of its own). `continue` answers `allThreadsContinued`.
- `pause` stops the program with SIGSTOP, and lldb-dap 18 reports that as reason `exception` with description
  `signal SIGSTOP`; the shell reports it as reason `pause`.
- Frames of the standard library carry the path rustc recorded, `/rustc/<commit>/library/...`, which does not exist on
  the machine; the shell maps it to `<sysroot>/lib/rustlib/src/rust/library/...` when the `rust-src` component is
  installed (they then open) and shows them as external code otherwise. Frames without source (`main`, `_start`) have
  `presentationHint: subtle`.
- `restart` relaunches the program (thread ids change); `disconnect` with `terminateDebuggee` ends it (`exited`
  with code 9, the signal that killed it, then `terminated`) and the adapter exits.

## What lldb-dap 18 lacks

- **An unknown request aborts the adapter** (SIGABRT, exit -6) instead of answering with an error: a client must send
  only what the capabilities allow. `gotoTargets` (Set Next Statement) is not offered and must not be sent.
- No `--version` output, no `terminate` request, no `exceptionFilterOptions`, no data breakpoints, no `readMemory`.
- A pause is an `exception` stop; `setVariable`'s answer names `result`; scope references follow the last `scopes`
  request (above).
- The program's output comes through a pseudo-terminal: lines end with `\r\n`, and long writes arrive in pieces.
- `env` must be a list of `NAME=value` strings; an object is ignored.
