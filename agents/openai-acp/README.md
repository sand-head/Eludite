# eludite-openai-acp

An [Agent Client Protocol](https://agentclientprotocol.com) agent, written in Rust, over any server that speaks the OpenAI Chat Completions API: a llama.cpp `llama-server` on your own machine, Ollama, vLLM, LM Studio, OpenRouter, Groq, Together, DeepSeek, Mistral, or OpenAI itself. It speaks ACP (protocol version 1) on stdin/stdout to any ACP client (Eludite, Zed and others). Its only tools are the IDE's, from the MCP servers the client passes in `session/new` (Eludite's endpoint: every read, edit, build, test, debug and browser action goes through the IDE's commands, permission classes and pending-change review). It has no shell, no file access and no tools of its own, and it never asks for permission itself: the IDE's gate decides every call.

Built for [brief 0060](../../docs/briefs/0060-openai-compatible-agent.md) and [ADR-0013](../../docs/adr/0013-openai-compatible-agent.md). License: MIT (see `LICENSE` and `NOTICE`). This directory is its own Cargo workspace, like `agents/claude-acp/`.

## Use

```
cargo build --release      # target/release/eludite-openai-acp (about 3.5 MB, stripped)
eludite-openai-acp --base-url URL [--model M] [--header NAME=VALUE]... [--tools core|all]
                   [--catalog PATH|JSON] [--max-completion-tokens N] [--chat-template-kwargs JSON] [--name NAME]
eludite-openai-acp models --base-url URL [--header NAME=VALUE]... [--catalog PATH|JSON]
```

- `--base-url` is the API root ending in the version segment: `http://localhost:8080/v1` (llama.cpp), `http://localhost:11434/v1` (Ollama), `http://localhost:1234/v1` (LM Studio), `http://localhost:8000/v1` (vLLM), `https://api.openai.com/v1`, `https://openrouter.ai/api/v1`, `https://api.groq.com/openai/v1`, `https://api.together.xyz/v1`, `https://api.deepseek.com/v1`, `https://api.mistral.ai/v1`. One trailing slash is stripped; `/models` and `/chat/completions` are appended.
- The key comes from `$ELUDITE_OPENAI_API_KEY` only (never `$OPENAI_API_KEY`). Without one no `Authorization` header is sent at all, which llama.cpp and Ollama accept. Eludite sets it per launch from its credential store (`provider:<name>`); it is never written to a file and never logged.
- `--header` adds request headers (OpenRouter's `HTTP-Referer` and `X-Title`); `Authorization` is never taken from there.
- Models come from `GET {base}/models` (30 s timeout): 401 and 403 mean the key was rejected; 400, 404, 405, 410 and 501 mean the server does not list models; a 200 that is not JSON is no listing. The context window is llama.cpp's `meta.n_ctx_train` or OpenRouter's `context_length`, else the catalog's `contextWindow`, else unknown. `--catalog` (a file, or the JSON itself: `[{"id", "name"?, "contextWindow"?}]`) replaces the listing and the server is not asked. `session/new` offers them as the `model` config option (brief 0058's shape); `session/set_config_option` changes the model for the next request. `models` prints the listing as JSON (`{models, listing: "server" | "catalog" | "none", message?}`) and exits; Eludite's `eludite.agents.provider_models` runs it.
- `--tools core` (the default) sends the core tools (file read and edit, the workspace tree and edits, search, diagnostics, build, output, tests, the terminal, git, navigation, the debugger's start, snapshot, breakpoints and wait) and the meta tool `eludite-tools`, which lists the others (by `prefix`) and adds the ones the model names (`enable`) from the next request. `--tools all` sends every tool. `tools/list` is read again after `notifications/tools/list_changed`.
- The request carries `model`, `messages`, `tools`, `stream: true`, `stream_options.include_usage`, `max_tokens: 8192` (or `max_completion_tokens` with `--max-completion-tokens`, for OpenAI's reasoning models) and, with `--chat-template-kwargs`, llama.cpp's `chat_template_kwargs` verbatim (`{"enable_thinking": false}`). Nothing else: no temperature, no `tool_choice`.
- A turn runs up to 50 requests (`max_turn_requests`). Tool calls run one at a time, in order; a malformed `arguments` or an unknown tool name goes back to the model as the tool's failure. `session/cancel` answers `cancelled` at once and drops the request. A stream cut before its end is retried once. `/compact` summarizes the conversation (also done on its own past 85 percent of a known window, or after a context-length error) and `/clear` forgets it.
- After each response, a `usage_update`: `used` (the response's prompt and completion tokens), `size` (the window, 0 when unknown), and in `_meta.eludite.usage` the turn's totals under ACP's `Usage` names (`inputTokens`, `cachedReadTokens`, `outputTokens`, `thoughtTokens`, `totalTokens`, `model`).
- `$ELUDITE_OPENAI_ACP_LOG=stderr` (or a file path) turns on the verbose log, request bodies included, with the key replaced wherever it appears. stdout carries ACP only.

Eludite finds the adapter through `$ELUDITE_OPENAI_ACP`, beside its own executable, then on `PATH`, and runs one per server in `agents.json`'s `providers`. Other ACP clients launch it like any stdio agent, with `--base-url` and the key in the environment.

## Test

```
cargo test                                   # unit tests, the turns against a fake server and a fake MCP endpoint,
                                             # and the MCP client against crates/mcp's real server
ELUDITE_LLAMA_SERVER=http://localhost:8080/v1 cargo test --test real_server -- --nocapture
cargo build --release
cargo run --release --example bench -- target/release/eludite-openai-acp target/release/eludite-openai-fake-relay
```

`tests/fake_server.rs` is a loopback OpenAI-compatible server in hand-written HTTP/1.1, scripted per test (listings, statuses, streams cut short or stalled, JSON bodies); `tests/fake_mcp.rs` fakes Eludite's MCP endpoint behind `eludite-openai-fake-relay`, a test-only binary that does what `eludite --mcp-relay ADDR` does; `tests/mcp_real.rs` runs the adapter against `crates/mcp`'s real server and command bus (dev-dependencies only, not linked into the binary). `tests/fixtures/*.sse` are stream bodies replayed by `stream_fixtures_replay`: the `synth-*` ones are synthesized from llama.cpp's, OpenAI's and OpenRouter's documented shapes, not recorded; `tools/record.sh NAME BASE_URL MODEL` records a real server's listing and one streamed tool call beside them (the key never written). The real-server test is skipped without `ELUDITE_LLAMA_SERVER` (`ELUDITE_LLAMA_MODEL` picks the model).

## Dependencies

Linked into the binary (SPDX): `agent-client-protocol` 2.2.0 (Apache-2.0), `ureq` 3.4 (MIT OR Apache-2.0) with `rustls` 0.23 (Apache-2.0 OR ISC OR MIT), `ring` 0.17 (Apache-2.0 AND ISC), `rustls-webpki` and `untrusted` (ISC), `webpki-roots` 1.0 (CDLA-Permissive-2.0), `subtle` (BSD-3-Clause), `serde` and `serde_json` (MIT OR Apache-2.0), `futures` (MIT OR Apache-2.0), `uuid` (Apache-2.0 OR MIT), and their dependencies under MIT, Apache-2.0 or both (`NOTICE` has the rest). Tests only: `eludite-commands` and `eludite-mcp` (GPL-3.0-or-later, from this repository).

## Layout

- `src/agent.rs`: the ACP handlers and sessions.
- `src/provider.rs`: the HTTP client, SSE and the chunk rules.
- `src/models.rs`: the listing, the catalog and the `model` config option.
- `src/loop.rs` (module `turn`): the turn.
- `src/mcp.rs`: the MCP client over stdio and HTTP, the tool mapping, the core set and `eludite-tools`.
- `src/compact.rs`: trimming old tool results and summarizing.
- `src/prompt.rs`: the system prompt, with the IDE's guides within 4,000 estimated tokens.
- `src/log.rs`: the log, with the key redacted.
