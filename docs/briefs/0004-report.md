# Brief 0004 report: ICorDebug proof over a TCP DAP transport

Status: done on Windows 11, loopback and a non-loopback address on the same machine. The second-machine run (exit
criterion 2) is still owed. Linux and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0004-icordebug-dap-spike`, based on `main` at `a8214dc`. Date: 2026-10-03.
Brief: [0004-icordebug-dap-spike.md](0004-icordebug-dap-spike.md).

## 1. Summary

- **Rust with the `windows` crate is workable for ICorDebug.** `eludite-dbg-netfx` attaches to a running .NET
  Framework 4.8 process, binds a breakpoint through the portable PDB's sequence points, stops on it, walks the stack,
  reads the `int` local `counter`, continues to the next hit, and detaches, all driven by a DAP client over TCP.
  The end-to-end test passed on the first run of the real code and on every run after.
- **Budgets** (Windows 11 Pro 10.0.26200, Intel Core Ultra 9 185H, 31 GB; .NET Framework 4.8.x, CLR
  v4.0.30319; Debug build of the adapter, as `cargo test` builds it; ten sessions from five runs of the e2e suite):

  | Budget | Result |
  |---|---|
  | Attach to first `initialized` < 2 s | **34.3 to 41.3 ms**, client side, from sending `attach` to receiving `initialized`. Of that, about 15 ms creates `ICorDebug` (mscoree, the metahost, loading mscordbi) and 28 to 36 ms is `DebugActiveProcess`. Pass |
  | Breakpoint hit to `stopped` < 200 ms | **0.45 to 1.44 ms** end to end: from the debuggee's `QueryPerformanceCounter` reading just before it calls the method with the breakpoint, to the client's reading when the `stopped` event arrives (same machine, same clock). Inside the adapter, callback arrival to `stopped` written: 0.10 to 0.63 ms. Pass |
  | Adapter resident memory < 100 MB while paused | **11.5 to 11.6 MB** working set (`GetProcessMemoryInfo`) at the first stop. Pass |
  | The DAP read loop never blocks on ICorDebug callbacks | Pass by construction (section 4): the read loop only frames and forwards; callbacks post to a channel |

- **Remote (exit criterion 2):** the same test passes with the adapter bound to this machine's Wi-Fi address
  (`10.0.1.247`) and the client connecting to that address (five runs). That proves the adapter has no loopback
  assumption, but traffic between two sockets on one host never crosses the NIC or the firewall's inbound filter.
  **The run from a second machine or VM is owed.** Windows Firewall prompted on the first non-loopback listen and
  the machine now has two inbound Block rules for this build of the adapter (section 6), so that run needs an allow
  rule first.
- **New dependencies:** `windows` 0.62 and `windows-core` 0.62 (both `MIT OR Apache-2.0`), Windows targets only;
  both were already in `Cargo.lock` through GPUI. The fixture uses the NuGet package
  `Microsoft.NETFramework.ReferenceAssemblies` 1.0.3 (MIT) at build time only.
- **Verdict:** keep Rust and the `windows` crate for the real debugger; no C# shim. The costs are a hand-declared
  ICorDebug vtable layer (no metadata for `cordebug.idl` in the `windows` crate) and a portable PDB reader, both
  done here for the subset the spike needs (section 7).

## 2. What was built

Commits, in order:

1. `979f07d` The adapter, the fixture, the build script and the tests.
2. `02f89e0` The remote-client test prints every measured field (so it compiles warning-free off Windows).
3. This report.

`debuggers/netfx` (GPL-3.0-or-later), 4,409 lines including tests:

| File | Lines | What |
|---|---|---|
| `src/main.rs` | 45 | `--listen HOST:PORT` (default `127.0.0.1:0`), prints `listening on ADDR` to stderr, serves one client. Nothing on stdout |
| `src/cli.rs` | 62 | Argument parsing and usage text (states the transport has no auth or encryption) |
| `src/framing.rs` | 119 | `Content-Length` framing, read and write |
| `src/protocol.rs` | 398 | Request parsing into typed commands for the ten required requests; response shapes. Paths are carried as given |
| `src/session.rs` | 645 | The `Debugger` trait, the read loop, the engine loop, request dispatch, event emission, and timing logs |
| `src/pdb.rs` | 856 | A portable PDB reader: documents, sequence points, local scopes and variable names. Pure Rust, every OS |
| `src/lib.rs` | 88 | Crate docs, `Unsupported` (the non-Windows debugger: answers `initialize`, refuses `attach` with the reason), `serve_listener` |
| `src/windows/cordebug.rs` | 485 | 23 ICorDebug interfaces declared with `windows_core::interface`, in vtable order |
| `src/windows/mod.rs` | 1,165 | The callback object, the engine (`CorDebugger`), breakpoint binding, stack walking, value formatting |
| `tests/e2e.rs` | 546 | The end-to-end tests (section 3) |
| `fixtures/Counter/` | | `Counter.csproj` (SDK-style, `net48`, x64, Debug, portable PDB, no central package management, no repo `Directory.Build.props`) and `Program.cs` |
| `fixtures/build.ps1` | | Builds the fixture with `dotnet build --artifacts-path`, so nothing lands in the source tree; prints the exe path |

`crates/dap` already has a TCP transport behind `AdapterTransport` / `Connection` (ADR-0007), so it was not touched.

### 2.1 How attach works

`OpenProcess` -> `CLRCreateInstance(CLSID_CLRMetaHost)` -> `ICLRMetaHost::EnumerateLoadedRuntimes(process)` -> the
loaded runtime whose version starts with `v4.` -> `ICLRRuntimeInfo::GetInterface(CLSID_CLRDebuggingLegacy)` ->
`ICorDebug` -> `Initialize` -> `SetManagedHandler` -> `DebugActiveProcess(pid, FALSE)`. A process with no v4 runtime
(.NET Core, native) gets a failed `attach` response that says so.

The brief names both the metahost route and `ICLRDebugging`. The metahost route was chosen because it is the one
that delivers managed debug events for a live v4 desktop process: `ICLRDebugging::OpenVirtualProcess` takes an
`ICorDebugDataTarget` and a library provider that the debugger must implement, and is built for inspection of a
target the debugger reads through that data target (dumps, and the .NET Core runtime's out-of-process model); it does
not replace `DebugActiveProcess` for a live desktop CLR. mscordbi is reached only through mscoree from the .NET
Framework installation (`C:\Windows\Microsoft.NET\Framework64\v4.0.30319\`). No Visual Studio binary is referenced and
`vsdbg` is not used.

### 2.2 Breakpoints and symbols

`setBreakpoints` records each line, then tries to bind it in every loaded module that has a PDB beside it. A PDB is
read when a module loads: `<module path>.pdb`, parsed by `src/pdb.rs` (portable PDB, which is what `dotnet build`
emits for `net48` by default; a Windows MSF PDB is recognized and refused with a message). Binding: the document is
found by full path (case and separators ignored), else by file name when exactly one document has it; the line maps
to the earliest sequence point that starts on it (else the next line with code inside the same method); then
`GetFunctionFromToken` -> `GetILCode` -> `ICorDebugCode::CreateBreakpoint(ilOffset)` -> `Activate(TRUE)`. A line that
cannot bind yet stays pending, unverified, and binds on a later `LoadModule`, which sends a `breakpoint` event
(reason `changed`). Stack frames get their line from the last visible sequence point at or before the frame's IL
offset; locals get their names from the LocalScope and LocalVariable tables for that offset (hidden compiler locals
are skipped).

## 3. Tests and exact commands

All from the worktree root.

| Command | Result |
|---|---|
| `cargo test -p eludite-dbg-netfx` | 19 passed, 0 failed (framing 4, protocol 4, PDB 4, session 5, CLI 1, `Unsupported` 1); `tests/e2e.rs` compiles to nothing without the feature |
| `cargo test -p eludite-dbg-netfx --features e2e -- --ignored` | 3 passed: `windows_only::loopback`, `windows_only::non_loopback_address`, and `remote_client`, which **only reports that it was skipped** because `ELUDITE_NETFX_REMOTE` is unset. Run seven times in total for this report |
| `cargo clippy -p eludite-dbg-netfx --all-targets --features e2e -- -D warnings` and without `--features e2e` | Clean |
| `cargo build --workspace` | Pass (Windows) |
| `cargo fmt --check` | Pass |
| `cargo clippy --workspace --all-targets -- -D warnings` | **Fails, outside this brief:** `eludite-browser` (`tests/opener.rs` unused imports and dead code, `src/browser/data.rs:435` unused variable) and `eludite` (`src/shell/debug/tests.rs:1451` uses `std::os::unix`; `src/shell/rust_tests.rs` unused imports). These are Windows-only breakages on `main`; branch `windows-run` fixes them in `1d140bc`. With those two crates' test targets left out (`--exclude eludite-browser --exclude eludite --all-targets`, plus `-p eludite -p eludite-browser` without test targets) clippy is clean |
| `cargo test --workspace` | **Not run.** The session's permission policy refused it. It would not compile on this base on Windows anyway, for the same two test targets as clippy |

What `windows_only::loopback` does: builds the fixture through `build.ps1` (into `target/tmp/netfx-fixture`), starts
`Counter.exe` and waits for `ready <pid>`, starts the adapter with `--listen 127.0.0.1:0`, reads the port from its
stderr, connects, and sends `initialize`, `attach {processId}`, `setBreakpoints` on the line marked `// BREAKPOINT`
(`counter = counter + 1;` in `Program.Tick`), `configurationDone`. It asserts: a `stopped` event with reason
`breakpoint`; `threads` lists the stopped thread; `stackTrace` frame 0 is `Eludite.Fixtures.Counter.Program.Tick` at
the marked line with source `Program.cs`, frame 1 is `Program.Main`; `scopes` gives `Locals`; `variables` gives
`counter` with type `int` and an even integer value (the line before it doubles the iteration). Then `continue`, a
second `stopped`, `counter` larger than before; `disconnect` with `terminateDebuggee: false`, a `terminated` event,
the adapter exits with status 0 and the debuggee is still running. It asserts the three budgets and that the adapter
wrote nothing to stdout. `non_loopback_address` is the same with the adapter bound to the machine's outbound IPv4
address (or `ELUDITE_NETFX_HOST`) and the client connecting to it.

Sample output line (loopback):

```
RESULT listen=127.0.0.1:0 connect=127.0.0.1:57169 attach->initialized (client) 34.6 ms, (adapter) [34.5] ms;
breakpoint hit->stopped at the client [0.45, 1.34] ms, callback->stopped in the adapter [0.118, 0.249] ms;
adapter working set while paused 11.6 MB; counter 4 then 6
```

## 4. ICorDebug surface and threading model (exit criterion 4)

### 4.1 Interfaces used

| Interface | Methods called |
|---|---|
| `ICLRMetaHost` (`windows` crate) | `CLRCreateInstance(CLSID_CLRMetaHost)`, `EnumerateLoadedRuntimes` |
| `IEnumUnknown` (`windows` crate) | `Next` |
| `ICLRRuntimeInfo` (`windows` crate) | `GetVersionString`, `GetInterface(CLSID_CLRDebuggingLegacy)` |
| `ICorDebug` | `Initialize`, `SetManagedHandler`, `DebugActiveProcess`, `Terminate` |
| `ICorDebugManagedCallback`, `ICorDebugManagedCallback2` | Implemented (all 26 + 8 methods); v4 refuses a handler without `Callback2` |
| `ICorDebugController` (via `ICorDebugProcess`) | `Stop`, `Continue`, `Detach`, `Terminate` |
| `ICorDebugProcess` | `GetThread` |
| `ICorDebugAppDomain` | `Attach` (on `CreateAppDomain`) |
| `ICorDebugThread` | `GetID`, `EnumerateChains` |
| `ICorDebugChainEnum`, `ICorDebugFrameEnum` | `Next` |
| `ICorDebugChain` | `IsManaged`, `EnumerateFrames` |
| `ICorDebugFrame` | `GetFunction` |
| `ICorDebugILFrame` | `GetFunctionToken`, `GetIP`, `GetLocalVariable` |
| `ICorDebugFunction` | `GetModule`, `GetILCode` |
| `ICorDebugModule` | `GetName`, `GetBaseAddress`, `GetFunctionFromToken`, `GetMetaDataInterface` |
| `ICorDebugCode` | `CreateBreakpoint` |
| `ICorDebugBreakpoint`, `ICorDebugFunctionBreakpoint` | `Activate` |
| `ICorDebugValue` | `GetType` |
| `ICorDebugGenericValue` | `GetSize`, `GetValue` |
| `ICorDebugReferenceValue` | `IsNull` |
| `IMetaDataImport` (`windows` crate) | `GetMethodProps`, `GetTypeDefProps` |

Declared but not called: `ICorDebugEnum` (base of the enumerators), `ICorDebugValueEnum`. `cordebug.rs` lists every
method of each declared interface in vtable order, transcribed from `cordebug.idl` (the same layout as dotnet/runtime
`src/coreclr/inc/cordebug.idl`, MIT); methods the spike never calls keep their slot with raw-pointer parameters.

### 4.2 Threading

- **Read loop** (the connection's thread): reads frames, parses them into requests, sends them into a channel. It
  never calls the debugger. It polls the socket with a 100 ms read timeout so it can end when the session ends
  (section 5, item 4); a timeout is retried below the framing, never surfaced.
- **Engine thread** (`dap-engine`): enters the multithreaded apartment (`CoInitializeEx(COINIT_MULTITHREADED)`;
  ICorDebug requires MTA callers), creates `ICorDebug` and owns every ICorDebug object. It drains one channel that
  carries both DAP requests and marshalled callbacks, so requests and callbacks are handled one at a time in arrival
  order.
- **Callback thread** (mscordbi's own event thread): each `ICorDebugManagedCallback` method AddRefs its arguments,
  wraps them (`Agile<T>`, which is `Send` because ICorDebug's right-side objects are free-threaded), posts them with a
  timestamp, and returns `S_OK` **without** calling `Continue`.
- **Stop-the-world:** while a callback is outstanding every managed thread in the debuggee is held. The engine handles
  the event (records a thread, loads a module's PDB and binds pending breakpoints, attaches an app domain) and then
  calls `ICorDebugController::Continue(FALSE)`. On a breakpoint it does not continue: the debuggee stays synchronized,
  so `stackTrace`, `scopes` and `variables` read a frozen process, and DAP `continue` calls `Continue`. mscordbi does
  not dispatch the next callback until the previous one is continued, so the engine never sees two at once.
  `ExitProcess` is not continued.
- **Requests that need a synchronized debuggee while it runs** (`setBreakpoints`, `disconnect`): the engine calls
  `Stop(0)`, does the work, and `Continue`s. `Stop` and `Continue` nest, so this composes with a callback that is
  queued but not yet handled. This path ran in every e2e session: `setBreakpoints` arrived while the attach's
  `LoadModule` for `Counter.exe` was still queued.
- **Frame and variable references** are valid for one stop: frame ids index the frames handed out by `stackTrace`,
  the locals reference is the frame id, and both are dropped on `continue`.

## 5. What broke or surprised

1. **The `windows` crate has no ICorDebug.** Its metadata covers mscoree (`ICLRMetaHost`, `ICLRRuntimeInfo`,
   `CLSID_CLRDebuggingLegacy`) and `IMetaDataImport`, but not `cordebug.idl`. The 23 interfaces were declared by hand
   with `#[windows_core::interface]` and the callback object written with `#[implement]`. It worked first time, but a
   slot out of order would be silent undefined behavior. The real debugger needs a generator from the IDL (section 7).
2. **The `interface` macro has two quirks:** its expansion names `IUnknown_Vtbl` unqualified, so it must be imported,
   and each method needs `pub fn` to be callable from another module. `windows-core` must be a direct dependency
   because the macros expand to `::windows_core` paths.
3. **Breakpoints arrive before the module is known.** `DebugActiveProcess` returned in 28 to 36 ms and the attach's
   synthesized `CreateProcess`/`LoadModule`/`CreateThread` callbacks were delivered after it, so in every session
   `setBreakpoints` came back unverified and the breakpoint bound a few milliseconds later on `LoadModule`. Pending
   breakpoints with a later `breakpoint` event are needed from day one, also for attach.
4. **On Windows, `shutdown` does not wake a `recv` blocked in another thread.** The first version ended the read
   loop by shutting the socket down from the engine thread after `disconnect`; a unit test whose client kept its
   socket open waited 120 s for `WSAETIMEDOUT`. Fixed with the read-timeout poll in section 4.2. Clients that close
   after `disconnect` (the e2e test) never hit it.
5. **No deadlock between `DebugActiveProcess` and the callback thread**, which was the main risk of doing attach on
   the same thread that later continues callbacks: `DebugActiveProcess` does not wait for any callback to be
   continued.
6. **The firewall.** `NotifyOnListen` is on for all three profiles and the Wi-Fi network is Public. The first listen on
   `10.0.1.247` popped Windows Firewall's prompt, and two inbound Block rules named `eludite-dbg-netfx.exe` (Public
   profile) now exist for `...\agent-a84cd546b8c4e08c8\target\debug\eludite-dbg-netfx.exe`. The same-machine runs
   still passed because traffic between two local sockets is not filtered. I did not change any firewall setting.
7. **A VM on this machine is not a usable second machine as configured.** From the Ubuntu WSL2 VM, a TCP probe of
   an adapter on `0.0.0.0:47110` timed out on the host's WSL interface address (`172.26.48.1`, consistent with the
   Block rule) and succeeded on `10.0.1.247`, but the adapter logged the peer as `10.0.1.247`: the VM's default route
   is podman's user-mode network, which proxies the connection through a Windows process, so it is not a remote
   client either. The VM has no Rust toolchain, so the remote test could not run there anyway.
8. **The repo root has no `nuget.config` on this base** (it arrives with branch `windows-run`, commit `253eb18`). The
   fixture restored from the machine's NuGet configuration and cache.
9. **Two e2e tests building the fixture at the same time collide** in obj/ (a Source Link file lock). The tests now\n   share one build and run one at a time.\n10. **Nothing else broke.** ICorDebug from Rust needed no workaround: method names, IL offsets, sequence points and
   `int` values came out right on the first run.

## 6. Exit criteria

| # | Criterion | State |
|---|---|---|
| 1 | The end-to-end test passes on Windows 10 or 11 against a .NET Framework 4.8 process | **Done.** Windows 11 Pro 10.0.26200, .NET Framework 4.8.x (CLR v4.0.30319); seven runs of the whole e2e suite plus single-test runs during development. Every session that got as far as attaching passed; one early run failed before that, because two tests built the fixture at once (now serialized) |
| 2 | The same test passes with the DAP client on a different machine, connected over TCP | **Owed.** Passed with the adapter on a non-loopback address and the client on the same machine (section 1). For the real run: an administrator must allow inbound TCP for the adapter (the prompt created Block rules for the current path), then on the Windows box run `Counter.exe` (from `build.ps1`) and `eludite-dbg-netfx --listen 0.0.0.0:4711`, and on the other machine run `ELUDITE_NETFX_REMOTE=winbox:4711 ELUDITE_NETFX_REMOTE_PID=<pid> cargo test -p eludite-dbg-netfx --features e2e --test e2e remote_client -- --ignored --nocapture`, twice. The client sends its own path for `Program.cs`; the adapter matches it to the PDB's document by file name |
| 3 | `cargo build --workspace` still passes on Linux and macOS with the crate present | **Not run** (no Linux or macOS machine, and no Linux target installed here). The crate is written for it: ICorDebug code is `#[cfg(windows)]`, the `windows` crates are Windows-only dependencies, everything else (framing, protocol, session, PDB reader, CLI, `Unsupported`) is portable and unit-tested. CI will be the first real check |
| 4 | The report lists every ICorDebug interface used, the threading model and what broke or surprised | Sections 4 and 5 |
| 5 | A sized brief for the real debugger, with a verdict | Section 7 |

Gaps the spike leaves on purpose (brief "Out of scope"): no launch, stepping, watch, evaluation or exception settings;
no authentication or encryption on TCP (loopback by default; the usage text says so); no source map beyond the
file-name fallback; the PDB is not checked against the module's debug directory (PDB id and age); one client per
adapter process.

## 7. Sized brief for the real debugger

### 7.1 Verdict

Rust plus the `windows` crate is workable; a C# shim is not needed. Everything the spike needed from ICorDebug was
reachable, the threading model is simple, and the numbers leave large margins (35 ms attach, about 1 ms from a hit
to `stopped`, 12 MB). A C# shim would put a second runtime and a second process boundary between DAP and ICorDebug to
buy the managed `ICorDebug` wrappers (`Microsoft.Samples.Debugging.CorDebug`) and `System.Reflection.Metadata`; the
wrappers are the part that is cheap to write, and the PDB reader is already here. The real costs in Rust are the
hand-maintained COM layer and an expression evaluator, and the evaluator is equally hard in either language.

### 7.2 Scope (PLAN.md section 13, risk 2; ADR-0007)

v1: launch and attach, breakpoints, stepping, locals, watch and exceptions, over stdio and TCP, remote-capable.
Deferred: Edit and Continue, mixed-mode, Hot Reload, symbol servers, Source Link, Memory and Disassembly.

### 7.3 Ordered work items

Sizes are agent-days of implementation and tests on a Windows machine, from this spike's pace.

| # | Item | Size | Notes |
|---|---|---|---|
| 1 | `protocol/schemas/dap-netfx.md` (transport, `launch`/`attach` arguments, capabilities, requests, events, values, what is missing), as brief 0022 did for Mono | 0.5 | Invariant 4: before the code |
| 2 | Generate the ICorDebug bindings from `cordebug.idl` (dotnet/runtime, MIT) instead of hand declarations: a small generator in `tools/` or a pinned winmd, checked-in output, a test that compares slot counts with the IDL | 2 | Removes the silent-UB risk of item 1 in section 5; covers `ICorDebug*` 2 to 4 variants, steppers, evals, exceptions |
| 3 | Production crate layout: the engine split into process, modules/symbols, breakpoints, stack, values; stdio transport beside TCP; one client at a time but many sessions per process life | 2 | Reuse `session.rs`, `framing.rs`, `pdb.rs` |
| 4 | Symbols: PDB id/age check against the module's CodeView entry, Windows (MSF) PDB support through `diasymreader` from the .NET Framework directory or a Rust MSF reader, `sourceFileMap` and path mapping for remote clients, embedded PDBs | 3 | Legacy projects often emit Windows PDBs (`DebugType=full`) |
| 5 | Launch: `ICorDebug::CreateProcess` with `stopAtEntry`, arguments, environment, working directory, console choice, `terminateDebuggee`; `exited`; stdout/stderr as `output` events | 2 | Check the 32-bit case: a 32-bit debuggee needs a 32-bit adapter (Framework\v4.0.30319 mscordbi) |
| 6 | Breakpoints: conditions and hit counts (shell already does hit counts, brief 0018), function breakpoints, rebinding on module load and unload, `BreakpointSetError` reporting, many app domains | 2 | |
| 7 | Stepping: `ICorDebugStepper` with step ranges from sequence points (into, over, out), Just My Code (`ICorDebugFunction2::SetJMCStatus`, non-user code by PDB presence and `DebuggerNonUserCode`), `pause` via `Stop` + `Break` semantics | 3 | |
| 8 | Values: objects, fields, properties via func-eval (`ICorDebugEval`), strings, arrays, generics (`ICorDebugType`), statics, `this`, arguments with names from metadata, paging | 4 | The largest item after evaluation |
| 9 | Watch and evaluate: an expression evaluator for a C# subset (members, indexers, literals, operators, method calls by func-eval), with timeouts and abort | 5 | Reuse brief 0022's experience; consider Roslyn in `eludite-host` for parsing |
| 10 | Exceptions: `ICorDebugManagedCallback2::Exception` first-chance, user-unhandled and unhandled, exception filters in `setExceptionBreakpoints`, `exceptionInfo` | 2 | |
| 11 | Threads and modules: names (`ICorDebugThread` object `Thread.Name`), `modules` request and events, `loadedSources` | 1 | |
| 12 | Remote: the second-machine test of this brief turned into CI-runnable form (a Windows runner as adapter, a Linux runner as client), documentation of SSH forwarding as the secure path, a firewall note | 1.5 | The owed exit criterion 2 lands here at the latest |
| 13 | Shell integration: adapter selection on Windows for .NET Framework projects (today it names `eludite-dbg-netfx` as missing), the launch configuration, `eludite.debug.*` commands unchanged | 1.5 | Mirrors brief 0022's shell work |
| 14 | Budgets in CI on the reference Windows machine: attach < 2 s, launch to first stop, step round trip, locals of a 200-local frame, memory at a break | 1 | Brief 0022's budget set is the template |

Total: about 30 agent-days, split into four briefs: (A) items 1 to 4, the foundation; (B) items 5 to 7, launch,
breakpoints and stepping; (C) items 8 to 11, values, evaluation and exceptions; (D) items 12 to 14, remote, shell and
budgets. A depends on nothing; B and C depend on A; D depends on B.

### 7.4 Risks carried forward

- Func-eval (items 8 and 9) is where ICorDebug is hardest: it must run with all other threads suspended and can
  deadlock the debuggee. Budget a spike day inside brief C.
- Bitness: one adapter build per debuggee bitness (x64 and x86), chosen by the shell or by a launcher.
- Remote security: TCP has no auth or encryption; SSH forwarding (already in `crates/dap`) should be the documented
  remote path, with a direct TCP listener opt-in.
