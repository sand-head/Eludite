# eludite-claude-acp

An [Agent Client Protocol](https://agentclientprotocol.com) agent for Claude Code, written in Rust. It speaks ACP (protocol version 1) on stdin/stdout to any ACP client (Eludite, Zed, Neovim and others) and drives the `claude` executable you already have installed, over its headless stream-json protocol. It needs no Node, npm or npx, and reads no credentials: `claude` uses your existing login.

Built for [brief 0006](../../docs/briefs/0006-native-claude-acp-adapter.md); the findings are in [docs/briefs/0006-report.md](../../docs/briefs/0006-report.md). License: MIT (see `LICENSE` and `NOTICE`). This directory is its own Cargo workspace for now.

## Use

```
cargo build --release                       # target/release/eludite-claude-acp (about 2.3 MB, stripped)
eludite-claude-acp [--claude PATH] [--model MODEL]
eludite-claude-acp auth login                # runs `claude auth login` (the ACP terminal login method)
```

- `claude` is found through `--claude`, then `$ELUDITE_CLAUDE_PATH`, then `$PATH`, then `~/.local/bin`. Versions older than 2.1.287 (the version validated) are refused with a message.
- The model comes from `--model`, `$ELUDITE_CLAUDE_MODEL`, or the session's `_meta.claudeCode.options.model`. None is hard-coded.
- `$ELUDITE_CLAUDE_ACP_LOG=stderr` (or a file path) turns on the verbose log. stdout carries ACP only.

Eludite's `crates/acp::default_agents()` prefers this binary when it is beside the IDE's executable, on `PATH`, or named by `$ELUDITE_CLAUDE_ACP`, and falls back to `npx -y @agentclientprotocol/claude-agent-acp`.

Other ACP clients launch it like any stdio agent: the command is the binary's path, with no arguments. Only Eludite has been tested.

## Test

```
cargo test                                   # unit, golden mapping and conformance tests (fake claude)
cargo run --release --example bench -- ready target/release/eludite-claude-acp 5
cargo run --release --example bench -- stream target/release/eludite-claude-acp target/release/eludite-fake-claude 2000 200
```

`eludite-fake-claude` is a test-only binary that replays the recorded sessions in `tests/fixtures/`. To re-record against a new `claude` version, use `tools/record.py` (real model calls, keep them few) and then `tools/redact.py`, and regenerate the golden file with `UPDATE_GOLDEN=1 cargo test --test golden`.

## Layout

- `src/agent.rs`: the ACP handlers and sessions.
- `src/process.rs`: one `claude --print` child per session.
- `src/translate.rs`: stream-json messages to `session/update`.
- `src/mapping.rs`: tool calls, permissions, the MCP config and prompt content.
- `src/discovery.rs`: finding `claude` and checking its version.
- `screenshots/`: the brief 0005 panel running against this adapter on Linux, with no Node on `PATH`.
