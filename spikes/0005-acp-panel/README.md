# Spike 0005: Claude Code over ACP in a GPUI panel, one MCP tool

Throwaway code for [brief 0005](../../docs/briefs/0005-acp-claude-code-spike.md). The findings are in [docs/briefs/0005-report.md](../../docs/briefs/0005-report.md). The production pieces it exercises live in `crates/acp`, `crates/mcp` and `crates/commands`; this directory holds only the panel and the harness.

This directory is its own Cargo workspace (empty `[workspace]` table), so the production workspace does not build it. Run commands from this directory. It uses the GPUI pin from the root `Cargo.toml` and the toolchain from the root `rust-toolchain.toml`. Running Claude Code needs Node with `npx` and an existing Claude Code login (`claude` then `/login`); Niello reads no key or token.

```
cargo test                                  # headless GPUI tests against the scripted fake agent
cargo run --release                         # the panel, hosting Claude Code (agent starts on first Send)
cargo run --release -- --agent fake         # the panel with the scripted fake agent
cargo run --release -- --cwd DIR --prompt "List the current errors in the Error List and tell me which file has the most."
cargo run --release -- --bench-ready        # spawn the agent at window open; ready timings JSON on stdout
cargo run --release -- --bench-stream       # fake agent at 200 chunks/s for 10 s; frame-time JSON on stdout
tools/shots.sh RUN_DIR -- ARGS...           # Linux/KDE: run in a nested virtual KWin, screenshot prompt and final state
tools/nested.sh [--x11] -- COMMAND...       # Linux/KDE: run any command in a nested virtual KWin (for a locked session)
```

Other flags are documented at the top of `src/main.rs`. `SPIKE_AGENT_STDERR=1` echoes the agent's stderr; `SPIKE_DEBUG=1` traces renders.

## What it does

- One window, one panel: header with the agent's state (not started, starting, ready with adapter version and ACP version, login required with the agent's own login command, failed), a virtualized transcript, and a prompt box with Send/Stop.
- On the first Send it starts Niello's MCP endpoint (`127.0.0.1`, random port, per-run token), spawns the agent, runs `initialize` and `session/new` (passing the endpoint as a stdio MCP server whose command is this binary in `--mcp-relay` mode), then `session/prompt`.
- Streaming text, tool calls (title, kind, status, arguments, result) and permission requests render as they arrive. A permission request for `mcp__niello__diagnostics-list` (class read) is answered by policy without a prompt and shown as such; anything else shows Yes / No buttons.
- Every MCP `tools/call` and every permission decision writes an `[audit]` line to stderr.

## Licenses of the dependencies added

- `gpui`, `gpui_platform`: Apache-2.0 (git, pinned rev, same as the root workspace)
- `async-channel`, `serde_json`: Apache-2.0 OR MIT / MIT OR Apache-2.0
- `jsonschema` (dev only): MIT
