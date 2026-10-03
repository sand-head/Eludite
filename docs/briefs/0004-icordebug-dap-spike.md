# Brief 0004: ICorDebug proof over a TCP DAP transport

Status: done on Windows; the second-machine run (exit criterion 2) is owed
Plan reference: PLAN.md sections 3 (D3, D7), 4.5, 10 (Phase 0 item 4), 13 (risk 2)
Related ADRs: ADR-0003, ADR-0007

## Goal

Prove that `eludite-dbg-netfx` can be a DAP server written in Rust over the ICorDebug COM interfaces: on Windows, attach to a running .NET Framework process, set a breakpoint, hit it, and read a local variable, with the DAP client connected over TCP. The result sizes the real debugger brief.

## Files in scope

- `debuggers/netfx/**` (crate `eludite-dbg-netfx`, GPL-3.0-or-later)
- `debuggers/netfx/tests/**` and `debuggers/netfx/fixtures/**` (a tiny .NET Framework 4.8 console app, source only, plus a build script)
- `crates/dap/**`: only if the transport abstraction does not yet exist, and only to add a TCP transport behind a trait. Coordinate through the PR description.
- `docs/briefs/0004-report.md` (new)

The crate must compile on Linux and macOS (CI builds it everywhere). Windows-only code sits behind `#[cfg(windows)]`, with stubs elsewhere.

## Contract

- Rust `windows` crate for COM. ICorDebug via `mscoree` (`CLRCreateInstance`, `ICLRMetaHost`, `ICLRDebugging`) and `mscordbi`. Do not use `vsdbg`, and do not reference any Visual Studio binary.
- DAP over TCP: the server listens on a configurable `host:port` (default `127.0.0.1:0`, prints the chosen port to stderr). Content-Length framed JSON per the DAP specification. The server must not assume the client is on the same machine (ADR-0007): no shared pipes, and paths are carried as given.
- Required DAP requests: `initialize`, `attach` (by process id), `setBreakpoints` (source and line), `configurationDone`, `threads`, `stackTrace`, `scopes`, `variables`, `continue`, `disconnect`. Required events: `initialized`, `stopped` (reason `breakpoint`), `terminated`.
- Breakpoint resolution uses the PDB sequence points (portable or Windows PDB) to map source line to IL offset.
- Fixture program: a console app targeting .NET Framework 4.8 that loops, calls a method with an `int` local named `counter`, and waits between iterations.

## Proving test

- `cargo test -p eludite-dbg-netfx` runs on every OS. On non-Windows it runs unit tests for framing and request parsing only.
- On Windows, `cargo test -p eludite-dbg-netfx --features e2e -- --ignored` runs the end-to-end test: build the fixture, start it, start the adapter with `--listen 127.0.0.1:0`, connect a minimal DAP client over TCP, then send `initialize`, `attach`, `setBreakpoints` on the line that updates `counter`, `configurationDone`. It asserts a `stopped` event arrives, `stackTrace` returns the fixture method at that line, and `variables` for the locals scope returns `counter` with an integer value.
- The same test run twice from a second machine (or VM) with the adapter bound to a non-loopback address proves the remote claim. Record the evidence in the report.

## Budget

- Attach to first `initialized` event under 2 s on the reference Windows machine.
- Breakpoint hit to `stopped` event under 200 ms.
- Adapter resident memory under 100 MB while paused.
- The adapter never blocks its DAP read loop on ICorDebug callbacks. Callbacks marshal to the loop through a channel.

## Exit criterion

1. The end-to-end test passes on Windows 10 or 11 against a .NET Framework 4.8 process.
2. The same test passes with the DAP client on a different machine, connected over TCP.
3. `cargo build --workspace` still passes on Linux and macOS with the crate present.
4. The report lists every ICorDebug interface used, the threading model (apartment, callbacks, stop-the-world handling) and what broke or surprised the agent.
5. The report ends with a sized brief for the real debugger: the v1 scope from PLAN.md section 13 (launch and attach, breakpoints, stepping, locals, watch, exceptions), the ordered work items, and a verdict on whether Rust plus the `windows` crate is workable or a C# shim is needed.

## Out of scope

- Launch (only attach), stepping, watch expressions, expression evaluation, exception settings.
- Edit and Continue, mixed-mode debugging, Hot Reload, symbol servers, Source Link.
- Authentication or encryption on the TCP transport. Bind to loopback by default and note the gap.
- Debugging .NET Core or 5+ (netcoredbg covers it).
- Any GPUI or shell integration of debugger windows.
- Mono soft debugger and Wine investigations.
