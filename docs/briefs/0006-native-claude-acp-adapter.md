# Brief 0006: Native Rust ACP adapter for Claude Code

Status: done (Linux). Report: [0006-report.md](0006-report.md)
Plan reference: PLAN.md sections 2 (principles 2, 5, 7), 5.2, 13 (risk 9)
Related ADR: ADR-0003
Depends on: brief 0005 (report: docs/briefs/0005-report.md)

## Goal

Remove Node and npx from Claude Code support. Ship `niello-claude-acp`, a Rust binary that speaks the Agent Client Protocol on stdin/stdout to any ACP client (Niello first, but also Zed, Neovim and others) and drives the user's installed `claude` binary directly over its headless JSON-lines protocol, using the user's existing Claude Code login. It replaces `npx -y @agentclientprotocol/claude-agent-acp` in `crates/acp::default_agents()` with a binary we build and ship beside the IDE.

## Why this is feasible

- `claude` (2.1.287 here) is a native executable with no Node dependency. In `--print` mode with `--input-format stream-json --output-format stream-json` it exchanges newline-delimited JSON messages: `system` (init, with session id, model, tools, MCP servers), `assistant` and `user` messages with content blocks, `stream_event` partial chunks when `--include-partial-messages` is set, `result`, and `control_request` / `control_response` pairs used for permission prompts (`--permission-prompt-tool stdio`), initialization and hooks.
- The Node adapter (`@agentclientprotocol/claude-agent-acp`, Apache-2.0) is a mapping layer from the Claude Agent SDK's message types to ACP. The SDK itself only spawns `claude` and parses that stream. Brief 0005's recorded, redacted session in `crates/acp` tests shows the ACP side; this brief records the `claude` side the same way.
- The Agent SDK's source license is custom (not open source). This brief does not copy or vendor SDK code. It reimplements the wire protocol by observation of the official binary and by reading its public CLI reference. The Apache-2.0 adapter may be read for the ACP mapping and credited in `NOTICE`.

## Files in scope

- `agents/claude-acp/**` (new): Cargo package `niello-claude-acp`, a standalone binary with its own `[workspace]` table during the spike, to be added to the root workspace by a follow-up once it is production-grade. License: MIT with its own LICENSE file, because it is a protocol bridge other editors will want (PLAN.md principle 7, "permissive edges"); the owner may override to GPL.
- `crates/acp/src/**`: `default_agents()` gains the native adapter as the first entry when the binary is found beside the IDE or on PATH, with the npx adapter as a fallback. Tests.
- `docs/briefs/0006-report.md` (new).
- `protocol/schemas/`: none expected. If a schema is needed, add it first in a separate commit.

Do not touch `spikes/**`, `dotnet/**`, `crates/mcp/**`.

## Contract

- ACP side: implement the agent half of the Agent Client Protocol as the official `agent-client-protocol` Rust crate defines it (record its version and SPDX id; MIT or Apache-2.0 are acceptable). Must pass the same client sequence brief 0005 exercised: `initialize` (report the protocol version the crate implements and `agentCapabilities` including `mcpCapabilities.http` only if implemented, `promptCapabilities` for text; declare `loadSession` if `--resume` is wired), `session/new` (spawns `claude`; passes the client's `mcpServers` through `--mcp-config` as a temp JSON file plus `--strict-mcp-config`), `session/prompt` (writes a `user` message to the child's stdin, streams `session/update` notifications: `agent_message_chunk` from partial text, `agent_thought_chunk` from thinking, `tool_call` and `tool_call_update` from tool_use/tool_result blocks with `kind`, `status`, `locations` and `content` mapped the way the Node adapter does), `session/cancel` (sends the interrupt control request or SIGINT; the prompt returns `stopReason: cancelled`), `session/request_permission` (raised from the child's `control_request` `can_use_tool`, answered with the matching `control_response`; map ACP's allow-once, allow-always, reject-once, reject-always to the child's `behavior` and `updatedPermissions`), `authenticate` (not needed when logged in; when the child reports a login requirement, surface `authMethods` with the `claude login` instruction exactly as brief 0005 did).
- Child side: launch `claude --print --input-format stream-json --output-format stream-json --include-partial-messages --replay-user-messages --verbose --permission-prompt-tool stdio --session-id <uuid> --permission-mode default`, in the session's `cwd`, with the environment stripped of Claude Code's own session variables (`CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT` and friends, as brief 0005 found necessary). Discover the binary via an explicit path argument, then `$PATH`, then `~/.local/bin/claude`. Record the `claude --version` and refuse to run below the version this brief validates against, with a clear message, rather than guessing at protocol drift.
- Model selection: pass through `--model` from an ACP session setting or an environment variable; never hard-code one.
- Logging: stderr or a file only. stdout is ACP.
- No Node, npm or npx anywhere in the runtime path. The binary must run on a machine with no Node installed. Prove it by running under an environment with `PATH` restricted to a directory containing only `claude`.
- No credentials are read, stored, logged or forwarded. The child inherits the login.
- Windows: use `claude.exe` discovery and CreateProcess-safe argument quoting; untested here, said so in the report.

## Proving test

- `cargo test` in `agents/claude-acp`: a recorded-session conformance test for the `claude` side (a redacted real headless session captured as JSON lines, replayed by a fake `claude` binary built as a test executable), asserting the full mapping to ACP messages, cancellation, and both permission outcomes. Plus the brief 0005 fake ACP client driving the real adapter against the fake `claude`.
- `cargo test -p niello-acp`: `default_agents()` ordering and discovery.
- Manual, recorded in the report with the transcript: run the brief 0005 spike panel (`spikes/0005-acp-panel`, unmodified, pointed at the native adapter via its agent descriptor), send "List the current errors in the Error List and tell me which file has the most." The agent calls `diagnostics-list` through Niello's MCP server and answers correctly. Then one prompt that triggers a Write permission request; deny it; the agent reports the denial and no file exists. Three or four real prompts total.
- A no-Node proof: the same manual run with `PATH` reduced so `node`, `npm` and `npx` cannot be found.

## Budget

- Session ready (ACP `initialize` to `session/new` response) under 1.5 s warm, measured over 5 runs. Record cold.
- Adapter resident memory under 30 MB. No per-chunk allocations on the hot path that would show in a 200 chunks/s stream (reuse brief 0005's measurement harness if useful).
- Binary size under 10 MB release, stripped.

## Exit criterion

1. The adapter builds on Linux with `cargo build --release`, runs with no Node on PATH, and passes the conformance tests.
2. The brief 0005 panel works unchanged against the native adapter: tool call rendered, correct answer, permission prompt and denial honored.
3. `crates/acp::default_agents()` prefers the native adapter when present and falls back to npx, with tests.
4. The report documents every `claude` stream-json message type and control request subtype observed, which fields the adapter depends on, the `claude` version validated, and a watch list of fields likely to drift.
5. The report states whether ACP session resume (`loadSession` via `--resume`) was implemented or deferred, and sizes it.
6. The report addresses PLAN.md risk 9 (subscription terms) factually: what the official Node adapter does, what this adapter does differently (nothing at the protocol level), and what remains for the owner to confirm with Anthropic's terms.

## Out of scope

- Any provider other than Claude Code. The adapter design should not preclude a `niello-codex-acp` later, but do not build abstractions for it now.
- Hosting the adapter inside the IDE process. It is a separate process by PLAN.md principle 2.
- Changing the panel, `crates/mcp`, or the `diagnostics.list` command.
- Session persistence UI, history browsing, multi-session management.
- Windows and macOS runs. Write the discovery code; report them as not run.
- Replacing or wrapping the Claude Agent SDK's other features (hooks, subagents configuration, skills). Only what ACP needs.
