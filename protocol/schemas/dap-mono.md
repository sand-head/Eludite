# eludite-dbg-mono: the Mono soft-debugger DAP server

`eludite-dbg-mono` debugs .NET Framework programs on Linux and macOS by running them under Mono with the soft
debugger agent (PLAN.md 4.5, option 2 of ADR-0007; brief 0022). It is a C# program on Mono.Debugging.Soft (MIT,
mono/debugger-libs) that runs under the located Mono, so it is started as:

```
mono eludite-dbg-mono.exe [--port N] [--log FILE]
```

It speaks the [Debug Adapter Protocol](https://microsoft.github.io/debug-adapter-protocol/specification) (1.65 or
later) and does not assume a local client (PLAN.md D7). The shell selects it for a .NET Framework startup project off
Windows (`crates/dap/src/launch.rs`, `debugger.monoAdapterPath`, `debugger.monoPrefix`).

## Transport

- **stdio** (default): DAP on stdin and stdout, `Content-Length`-framed as LSP base protocol messages, UTF-8 JSON.
- **TCP**: `--port N` listens on loopback (127.0.0.1) and serves the first client that connects. `--port 0` picks a
  free port and prints `eludite-dbg-mono: listening on 127.0.0.1:<port>` on stderr.
- stdout carries DAP only. The adapter's log goes to stderr, and also to `FILE` with `--log FILE`.
- The adapter exits when the client disconnects or closes the channel.

## initialize

`adapterID` is `mono`. Lines and columns are 1-based (the adapter honors `linesStartAt1` and `columnsStartAt1`), paths
are native. The answer's capabilities (each true only because it is implemented):

| Capability | Value |
|---|---|
| `supportsConfigurationDoneRequest` | true |
| `supportsConditionalBreakpoints` | true (C# expression, Mono's evaluator) |
| `supportsHitConditionalBreakpoints` | true: `N` (break on the Nth hit), `>=N`, `%N` (every Nth hit); also `==N`, `>N`, `<N`, `<=N` |
| `supportsFunctionBreakpoints` | true (`Namespace.Type.Method`, optionally with `(paramType, ...)`) |
| `supportsLogPoints` | true (`logMessage` with `{expression}` interpolation; `{{` is a literal brace) |
| `supportsEvaluateForHovers` | true |
| `supportsSetVariable` | true |
| `supportsExceptionInfoRequest` | true |
| `supportsExceptionFilterOptions` | true |
| `exceptionBreakpointFilters` | `all` ("All Exceptions": first chance), `user-unhandled` ("User-Unhandled Exceptions", default on); both accept a `condition` |
| `supportsTerminateRequest` | true |
| `supportsDelayedStackTraceLoading` | true (`stackTrace` honors `startFrame` and `levels` and answers `totalFrames`) |
| `supportTerminateDebuggee` | true |

The adapter sends no `capabilities` event: nothing changes after `initialize`.

## launch

| Argument | Type | Default | Meaning |
|---|---|---|---|
| `program` | string | required | The built `.exe` (absolute, or relative to `cwd`) |
| `args` | string[] | `[]` | The program's arguments |
| `cwd` | string | the program's folder | Working directory |
| `env` | object | `{}` | Variables added to the inherited environment |
| `runtimeExecutable` | string | the Mono running the adapter (read from the current process) | The `mono` that runs the program |
| `runtimeArgs` | string[] | `[]` | Extra `mono` options, before the program (the adapter adds `--debug --debugger-agent=...`) |
| `stopAtEntry` | bool | false | Stop at the program's entry point (`stopped` with reason `entry`) |
| `justMyCode` | bool | true | Frames of assemblies without symbols are external code; steps do not enter them |

The program runs as `<runtimeExecutable> --debug --debugger-agent=transport=dt_socket,address=127.0.0.1:<port>
<runtimeArgs> <program> <args>`. Its stdout and stderr become `output` events with categories `stdout` and `stderr`;
its stdin is closed (no `runInTerminal`). The adapter's own messages use `console`.

## attach

| Argument | Type | Default | Meaning |
|---|---|---|---|
| `address` | string | `127.0.0.1` | Where the program's debugger agent listens |
| `port` | integer | required | Its port |

The program was started with `--debug --debugger-agent=transport=dt_socket,server=y,address=<address>:<port>` (with
`suspend=y` to stop before `Main` until the adapter connects). Its output stays where it was started. `disconnect`
detaches unless `terminateDebuggee` is true.

## Requests

`initialize`, `launch`, `attach`, `setBreakpoints`, `setFunctionBreakpoints`, `setExceptionBreakpoints`,
`configurationDone`, `threads`, `stackTrace`, `scopes`, `variables`, `evaluate`, `setVariable`, `continue`, `next`,
`stepIn`, `stepOut`, `pause`, `exceptionInfo`, `disconnect`, `terminate`. Any other request gets a failed response
naming the command.

- Breakpoints set before the program loads their assembly answer `verified: false`; a `breakpoint` event with reason
  `changed`, `verified: true` and the bound line follows when Mono binds them. The `message` of an unbound breakpoint
  says whether it is pending (`The breakpoint will not currently be hit`, `The breakpoint could not yet be bound to a
  valid location`) or failed (`The breakpoint location is invalid...`, `The breakpoint could not be bound`, the
  condition's error).
- `setBreakpoints` keeps a breakpoint the request repeats unchanged (same line asked for, condition, hit condition and
  log message): same `id`, its binding and hit count kept (brief 0036). The others of the file are replaced.
- `scopes` answers one scope, `Locals`, holding `this`, the parameters and the locals, in that order.
- `variables` honors `start` and `count`. A variables reference names one expandable value in one stop; every
  reference (and frame id) is dropped when the debuggee resumes.
- `evaluate` takes the contexts `watch`, `hover` and `repl`, and an optional `timeout` in milliseconds (default
  3000). An evaluation that does not finish in time is aborted and answers an error naming the timeout. A failed
  evaluation is an error answer whose message is the evaluator's; it writes nothing to the output.
- `setVariable` assigns a C# expression's value to a local, parameter, field or property and answers the new value.
- `exceptionInfo` answers the exception's type (`exceptionId`), message (`description`), `breakMode` (`always` for a
  first-chance stop, `userUnhandled`) and `details` (type, message, stack trace, inner exception).
- `pause` interrupts every thread (`stopped` with reason `pause`).
- `disconnect` ends the debuggee when `terminateDebuggee` is true (the default after `launch`) and detaches
  otherwise; `terminate` ends it. Both are followed by `terminated`.
- A detach (`disconnect` after `attach` without `terminateDebuggee`, or with it false) is followed by `terminated`
  without `exited`; the program runs on and the adapter exits with code 0, as it does when the client closes the
  channel of an attached session.

## Events

`initialized` (after `launch` or `attach` is accepted), `stopped` (`reason`: `breakpoint`, `step`, `exception`,
`pause`, `entry`, `function breakpoint`; `threadId`; `allThreadsStopped: true`; `description` and `text` for
exceptions; `hitBreakpointIds`), `continued`, `thread` (`started`, `exited`), `breakpoint` (`changed`), `output`,
`process` (`systemProcessId` of a launched program), `exited` (`exitCode`), `terminated`.

## Values

`value` is Mono.Debugging's display string: strings quoted (`"hello"`), characters quoted, numbers and booleans as C#
writes them, objects with a `ToString` override as its result, other objects as `{Namespace.Type}`, arrays as
`{string[3]}`, `null`. `type` is the C# type name. An expandable value has a `variablesReference`. A one-dimensional
array of up to 150 elements gives `indexedVariables` (its length) and its elements page with `start` and `count`; a
longer one lists the ranges Mono.Debugging groups it in (`[0..99]`, ...), each expandable. Objects give no
`namedVariables` (Mono.Debugging knows the count only by reading the members); `start` and `count` page their members
too.

## Expressions

C# as Mono.Debugging's evaluator (NRefactory 5.5) reads it, in the stopped frame: locals, parameters, `this`, members,
indexers, method and property calls (which run debuggee code), casts, literals, operators. Integer and floating
arithmetic and comparisons (`i == 5`, `a + b * 2`), which the 2017 build's numeric unboxing fails, are retried with
explicit `long` or `double` casts. Lambdas, array creation, `default(T)`, `checked`, `nameof` and `++`/`--` are not
supported by the evaluator. NRefactory 5.5 reads an interpolated string (`$"{x}"`) as its literal text, so the adapter
refuses one with an error naming `string.Format`.

### Type names (brief 0036)

`evaluate` (every context), breakpoint conditions and the `{expressions}` of log points resolve a type name the way
Visual Studio's C# expression evaluator does, so `Coin.Quarter` works at a break in `MissingCase.Program.Main` without
`MissingCase.`. A name that is not a local, a parameter or a member of `this` or of the enclosing types (those come
first, as in C#) is looked up as a type:

1. a nested type of the stopped method's type, then of each type enclosing it;
2. each namespace level of the method, innermost first, ending with the global namespace: the level's own types (the
   method's namespace, then its parents: `A.B.Coin`, `A.Coin`, `Coin`), then the `using` aliases and the namespaces the
   `using` directives declared at that level import (inside `namespace A.B { ... }`, or at the top of the file for
   the global level; `global using` counts as top level, `using static` is ignored). The directives are read from the
   source file the debug information names for the frame (cached per file and its time stamp; file-scoped
   namespaces, records and raw strings are understood). Two imports of one level that both have the name make it
   ambiguous;
3. `System` (for a file without `using System;`);
4. a unique simple name among the types of the loaded assemblies: all of the method's own assembly, the public ones of
   the debuggee's other assemblies (those outside Mono's `lib/mono`) and the types Mono.Debugging has seen loaded; two
   or more make it ambiguous;
5. the first segment of a namespace (`System` in `System.Math.Max(a, b)`), which then evaluates as `global::System`.

Namespace-qualified names (`MissingCase.Coin.Quarter`, `System.Math.Max(a, b)`) and `global::` names always work. An
ambiguous name fails with `'Kind' is ambiguous between A.Kind and B.Kind: qualify it`. A type of an assembly the
debuggee has not loaded is not found. Each resolution is written to the adapter's log with its time
(`type name `Coin` in MissingCase.Coins: MissingCase.Coin (0.6 ms, pre-pass)`).

A condition that fails to evaluate (an unknown or ambiguous name, a non-boolean result) is not inserted: the adapter
sends a `breakpoint` event with `reason: changed`, `verified: false` and the evaluator's message (for example
`Unknown identifier: Coin`), and the breakpoint does not stop until it is set again with another condition.

## Stepping

- `next`, `stepIn` and `stepOut` step by source line on the thread named (Mono.Debugging steps its active thread).
- Just My Code (`justMyCode`, default true): steps do not stop in assemblies without symbols. `stepIn` on a line that
  calls only such code (`Console.WriteLine("x" + n)`) lands on the next line, as in Visual Studio; Mono itself would
  stop back on the calling line after the callee returns, so the adapter steps in again when a step in comes back to
  the frame and line it started on.

## Breakpoint behavior

- Hit conditions count hits before the condition is checked (Mono.Debugging's order) for source breakpoints.
- Breakpoints set while the program runs bind even while it loads types (brief 0036). Mono.Debugging 2017 inserts a
  breakpoint on its own operation thread from the tables of loaded types its event thread fills, without a lock: an
  insertion during a type load could fail ("Could not set breakpoint at location ... (Collection was modified; ...)")
  or miss the type and stay pending forever. The adapter changes the breakpoint list only under the lock the library's
  start-up enumeration takes, inserts again at once a breakpoint whose insertion failed (the library's message is
  logged, not shown), and re-inserts a breakpoint still pending although a loaded type of its file has code on its
  line, checked soon after the insertion, at every assembly load and at every stop.
- Function breakpoints: Mono.Debugging 2017 binds a function breakpoint to every method of the named type, so the
  adapter lets only the named method stop and applies the function breakpoint's condition, hit condition (counted
  after the condition) and log message itself. Its `breakpoint` event has no line.
- `all` with Just My Code (the default) stops at first-chance exceptions thrown in user code only; an exception the
  runtime throws and handles inside its own code does not stop.

## What the adapter lacks

| DAP feature | Status |
|---|---|
| `restart`, `gotoTargets` and `goto` (Set Next Statement), `stepInTargets`, `stepBack`, `reverseContinue` | Not implemented. Mono.Debugging has `SetNextStatement` (left to proposal 0001 brief B) |
| `completions`, `modules`, `loadedSources`, `source`, `readMemory`, `writeMemory`, `disassemble` | Not implemented |
| Data breakpoints, instruction breakpoints, `breakpointLocations` | Not implemented (the soft debugger has no watchpoints) |
| `exceptionOptions` (`supportsExceptionOptions`) | Not offered; per-type exception settings go through the filters' `condition` (comma-separated type names) |
| `valueFormattingOptions` (hex display), `setExpression`, `cancel`, `terminateThreads` | Not implemented |
| `runInTerminal` | Not requested: the program's stdin is closed |
