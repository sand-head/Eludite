# ADR-0007: Debuggers are remote-capable from day one

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 3 (D7), section 4.5, section 6, section 14 (item 3)

## Context

.NET Framework debugging has no permissive adapter. `vsdbg` is license-restricted to Microsoft products, so we cannot use it. Debugging .NET Framework requires the ICorDebug COM interfaces (`mscoree`, `mscordbi`), which exist only on Windows. For .NET Core and 5+, netcoredbg (MIT) already works on all three OSes.

Linux and macOS users with .NET Framework code still expect to debug it. There is no Linux implementation of ICorDebug to wrap. The owner has accepted that for v1, legacy debugging is "edit, analyze and build anywhere; debug on Windows, locally or remotely" (PLAN.md section 14, item 3).

## Decision

- Reach every debug adapter through one transport abstraction in the `dap` crate: stdio child process, TCP, or SSH-forwarded connection. The shell does not care which one is in use.
- Write `eludite-dbg-netfx` (`debuggers/netfx`, GPL) as a DAP server over ICorDebug, in Rust with the `windows` crate. It is Windows-only at runtime and compiles on every OS so CI covers it everywhere.
- Do not let `eludite-dbg-netfx` assume its client is on the same machine: no shared paths, no local pipes by default, source and symbol mapping is explicit.
- Cross-platform .NET Framework debugging, in order of likelihood:
  1. Remote DAP from Eludite on Linux or macOS to the adapter on a Windows box, VM or CI runner. It ships with the adapter and costs only the transport.
  2. A Mono soft debugger adapter for apps that run under Mono. Real but narrow.
  3. ICorDebug under Wine with .NET Framework in the prefix. Investigate after (1) and (2) exist. Expected to be fragile.
- Debugger windows are identical for local and remote sessions.
- v1 scope for `eludite-dbg-netfx`: launch and attach, breakpoints, stepping, locals, watch and exceptions. Edit and Continue and mixed-mode debugging are deferred.
- Fixes to netcoredbg go upstream, not into a fork.

## Alternatives considered

- Use `vsdbg`: it is the obvious tool, but its license limits it to Microsoft products.
- Local-only adapter first, add remote later: simpler at first, but the assumptions of a local client (paths, pipes) are costly to remove afterward.
- Wine as the primary Linux path: avoids a Windows machine, but ICorDebug under Wine is fragile and cannot be a v1 commitment.
- Mono soft debugger only: works on Linux, but only for apps that actually run under Mono, which excludes most real .NET Framework apps.
- Do not support .NET Framework debugging off Windows: honest, but gives Linux users no path at all for the product's primary workload.

## Consequences

Positive:
- Linux and macOS users get .NET Framework debugging without a second implementation.
- The transport abstraction also serves containers, VMs and remote dev boxes for netcoredbg and future adapters.
- Agent-driven debugging (PLAN.md section 5.5) works the same on remote sessions.
- Treating the client as remote from the start keeps the adapter protocol-pure.

Negative:
- Users need a Windows machine reachable over the network for off-Windows .NET Framework debugging. That is a setup burden and a security surface (authentication, encrypted transport).
- Remote debugging needs path mapping and source transfer rules, which are easy to get wrong.
- `eludite-dbg-netfx` is the largest single piece of new systems work in the plan (PLAN.md section 4.5), and the ICorDebug COM surface is intricate.
- Latency over a network makes stepping and variable expansion feel slower.

## Revisit when

- Brief 0004 shows ICorDebug from Rust over TCP DAP is not viable. Then size an alternative, such as a thin C# shim.
- Remote-session security needs (auth, TLS) exceed what an SSH-forwarded transport covers.
- A permissive, cross-platform .NET Framework debugger appears.
- The Mono or Wine investigations produce a reliable local path on Linux or macOS.
