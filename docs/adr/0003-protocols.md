# ADR-0003: Open protocols over plugin APIs

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 2 (principles 3 and 5), section 3 (D3), section 5

## Context

Niello needs language intelligence, debugging, testing, agent hosting and tool exposure. Each has an established open protocol. We do not run VS Code extensions (PLAN.md section 1), but the ecosystem built on those protocols should still be reachable. Agents build most of this product, so each boundary must be narrow, schema-described and replayable in tests.

Visual Studio-style per-feature plugin APIs would tie us to one runtime and one vendor. A protocol lets third parties replace any first-party implementation.

## Decision

Use one protocol per concern. Niello writes the first-party implementation of each.

| Concern | Protocol | First-party implementation |
|---|---|---|
| Language intelligence | LSP 3.17, Roslyn's `roslyn/*` extensions, a small `niello/*` vocabulary (project tree, build, designer-file regeneration) | `niello-host`; tree-sitter in the shell for immediate highlighting |
| Debugging | DAP, local or remote | netcoredbg (MIT) for .NET Core/5+; `niello-dbg-netfx` for .NET Framework |
| Testing | MTP server mode (JSON-RPC) and the VSTest translation-layer protocol | Test Explorer speaks both, MTP preferred |
| Agents in the IDE | ACP | Any ACP agent plugs into the Agents window |
| IDE as a tool | MCP server exposed by the shell | The command bus re-exported with schemas |
| Extensions | WASM component model (wasmtime) | See ADR-0006 |

Rules that follow:
- The command bus is the only API. The MCP server re-exports it with the same schemas. There is no separate agent API (CLAUDE.md invariant 3).
- The shell-to-host contract (`initialize`, `niello/ping`, `niello/host/info`, `shutdown`, `exit`, then the `niello/*` vocabulary) is specified in `protocol/schemas/` before implementation.
- Fixes to netcoredbg go upstream, not into a fork.
- Protocol conformance tests replay recorded LSP, DAP, MTP and ACP sessions.
- Protocols are adapters over the command bus. When ACP or MCP changes, we change the adapter, not the bus.

## Alternatives considered

- A bespoke Niello plugin API for language, debug and test support: more control, but nobody else implements it and it duplicates mature protocols.
- Run VS Code extensions through a compatibility layer: excluded by the non-goals. It would import a Node runtime and a large moving API.
- A native first-party agent with a private interface: pinned, not in scope until ACP hosting is excellent. If built later it uses the same ACP surface so it is not privileged.
- VSTest only for testing: simpler, but MTP is the direction of the platform. We support VSTest for projects that have not migrated.

## Consequences

Positive:
- Third parties can replace any first-party component: an ACP agent, a DAP adapter, a language host.
- Each boundary is testable by replaying recorded sessions, which suits agent-written code.
- New agents (Gemini CLI, Copilot CLI, Codex, OpenCode) cost nothing marginal once ACP works.
- The same schemas serve the UI, agents and tests.

Negative:
- We inherit protocol quirks and version churn. ACP and MCP are young (PLAN.md section 13, risk 8).
- Roslyn's language server tracks VS Code and may change `roslyn/*` extensions. Our extensions live in a separate layer.
- LSP is lossy for IDE-grade features. The `niello/*` vocabulary must grow carefully and stay documented.
- Two testing protocols mean two code paths in the test bridge.

## Revisit when

- ACP or MCP breaks compatibility in a way an adapter cannot absorb.
- A required IDE feature (for example designer-file regeneration or the project tree) cannot be expressed in `niello/*` without private side channels.
- MTP fully replaces VSTest in the projects we target, so the VSTest path can be dropped.
