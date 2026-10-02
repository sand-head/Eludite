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
  `changed`, `verified: true` and the bound line follows when Mono binds them.
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

## Events

`initialized` (after `launch` or `attach` is accepted), `stopped` (`reason`: `breakpoint`, `step`, `exception`,
`pause`, `entry`, `function breakpoint`; `threadId`; `allThreadsStopped: true`; `description` and `text` for
exceptions; `hitBreakpointIds`), `continued`, `thread` (`started`, `exited`), `breakpoint` (`changed`), `output`,
`process` (`systemProcessId` of a launched program), `exited` (`exitCode`), `terminated`.

## Values

`value` is Mono.Debugging's display string: strings quoted (`"hello"`), characters quoted, numbers and booleans as C#
writes them, objects with a `ToString` override as its result, other objects as `{Namespace.Type}`, arrays as
`{string[3]}`, `null`. `type` is the C# type name. An expandable value has a `variablesReference`; an array gives
`indexedVariables` (its length) and an object `namedVariables` when Mono knows the count without evaluating, so a
client can page big collections with `start` and `count`.

## What the adapter lacks

| DAP feature | Status |
|---|---|
| `restart`, `gotoTargets` and `goto` (Set Next Statement), `stepInTargets`, `stepBack`, `reverseContinue` | Not implemented. Mono.Debugging has `SetNextStatement` (left to proposal 0001 brief B) |
| `completions`, `modules`, `loadedSources`, `source`, `readMemory`, `writeMemory`, `disassemble` | Not implemented |
| Data breakpoints, instruction breakpoints, `breakpointLocations` | Not implemented (the soft debugger has no watchpoints) |
| `exceptionOptions` (`supportsExceptionOptions`) | Not offered; per-type exception settings go through the filters' `condition` (comma-separated type names) |
| `valueFormattingOptions` (hex display), `setExpression`, `cancel`, `terminateThreads` | Not implemented |
| `runInTerminal` | Not requested: the program's stdin is closed |
