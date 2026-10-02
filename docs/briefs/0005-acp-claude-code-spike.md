# Brief 0005: Claude Code via ACP in a GPUI panel, one MCP tool

Status: done on Linux; second OS not run ([report](0005-report.md))
Plan reference: PLAN.md sections 3 (D3), 5.1, 5.2, 10 (Phase 0 item 5), 13 (risks 8, 9)
Related ADR: ADR-0003

## Goal

Host Claude Code inside a GPUI panel through the Agent Client Protocol, and let it call one Eludite MCP tool that reads the Error List. This proves the ACP client, the MCP server over the command bus, and the agent-visible-state path end to end.

## Files in scope

- `crates/acp/**` (ACP client: spawn adapter, JSON-RPC over stdio, session, streaming updates, permission requests)
- `crates/mcp/**` (MCP server exposing registered commands)
- `crates/commands/**`: only to add one command, `diagnostics.list`, and the minimal registry the MCP server reads
- `spikes/0005-acp-panel/**` (new throwaway GPUI app with a single panel)
- `docs/briefs/0005-report.md` (new)

Do not edit `protocol/**` without adding the schema first. The command's input and output schemas go in `protocol/schemas/` before code (CLAUDE.md invariant 4), in a separate commit.

## Contract

- Agent process: `npx -y @agentclientprotocol/claude-agent-acp@0.85.0` (the official adapter, verified by this brief; the originally guessed `npx claude-code-acp` resolves to an unrelated community package). It uses the user's existing Claude Code subscription login, so no API key is read or stored by Eludite. If the user is not logged in, the panel shows that state and the agent's own login instructions.
- ACP: implement the client side of the Agent Client Protocol: `initialize`, `session/new`, `session/prompt`, streaming `session/update` notifications (message chunks, tool calls, tool call updates), and `session/request_permission`. Use the protocol version the adapter reports and record it.
- MCP: the panel passes a Eludite MCP server to the agent in `session/new` (`mcpServers`). The server listens on a local transport (stdio or localhost HTTP, document which) and exposes one tool, `diagnostics.list`, mapped to the command bus command of the same name. Input schema: optional `severity` filter. Output schema: array of `{path, line, column, severity, code, message}`.
- The Error List for the spike is a fixed in-memory fixture of at least 5 diagnostics. No host or compiler is involved.
- Permission model (PLAN.md section 5.3): `diagnostics.list` is class read and runs without a prompt. Any other tool call the agent attempts triggers a visible permission prompt in the panel, which the user can deny.
- No telemetry and no network call at startup. The agent is spawned only when the user opens the panel and sends a prompt.

## Proving test

- Automated: `cargo test -p eludite-acp -p eludite-mcp` runs against a scripted fake ACP agent (a recorded session replayed by a test binary) and asserts the client sequence, the permission flow, and that `diagnostics.list` returns the fixture and matches its schema.
- Manual, recorded in the report with a transcript and a screenshot: run the spike, send the prompt "List the current errors in the Error List and tell me which file has the most." The agent must call `diagnostics.list` and answer correctly. The tool call must be visible in the panel with its arguments and result.
- Run the manual test on Linux and one other OS.

## Budget

- Panel open to ready state (agent initialized) under 5 s on a warm `npx` cache. Record the cold time.
- Streaming text renders as it arrives. UI thread frame time stays under 8 ms at p99 while streaming 200 chunks per second from the fake agent.
- The ACP and MCP I/O never run on the UI thread.

## Exit criterion

1. The automated tests pass in CI on all three OSes.
2. The manual run shows Claude Code calling `diagnostics.list` through Eludite's MCP server and answering correctly, on two OSes.
3. A denied permission request stops the tool call and the agent reports the denial.
4. The report records the ACP protocol version, the adapter version, the MCP transport chosen and why, and the gaps found in either protocol.
5. The report states what the subscription terms question needs answered (PLAN.md section 13, risk 9) before documenting this path for anyone other than the owner. It does not answer it.

## Out of scope

- A production Agents window, multiple concurrent agents, worktree-per-agent.
- Inline diff proposals, edit-in-buffer, review changesets.
- Any other command or MCP tool, any real Error List or build.
- Other agents (Gemini CLI, Codex, Copilot CLI, OpenCode).
- Storing credentials, handling the login flow beyond showing its state, or any API-key path.
- Persisted permission policies and audit log storage (the audit record is a log line for the spike).
