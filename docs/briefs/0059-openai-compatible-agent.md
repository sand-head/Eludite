# Brief 0059: An ACP agent over OpenAI-compatible servers: llama.cpp, Ollama, vLLM, OpenRouter and the rest

Status: open
Phase: 2
Plan reference: PLAN.md sections 2 (principles 2, 3, 5), 5.1, 5.2 (the pinned first-party agent: the owner lifted the
pin for this adapter on 2026-10-05; this brief adds the dated note), 5.3, 5.7, 11
Related ADR: ADR-0012 (new, in this brief)
Depends on: brief 0016 (the Agents window and the MCP endpoint), brief 0006 (the native adapter's shape), brief 0057
(the model picker over ACP config options), brief 0056 (the slash-command menu), brief 0046 (the credential store
and the loopback fixture server pattern)
Reference: the owner's harness [slopcoder](https://git.sand.town/sand_head/slopcoder),
`src/SlopCoder.Agent/Providers/OpenAIChatCompletionsProvider.cs` and
`src/SlopCoder.Web/Services/Providers/OpenAICompatibleProviderCatalog.cs`, whose rules for the wire format this brief
restates; nothing is copied from it (C#, a different license), the rules are re-implemented in Rust

## Goal

A person points Eludite at any server that speaks the OpenAI Chat Completions API (a llama.cpp `llama-server` on
their own machine, Ollama, vLLM, LM Studio, OpenRouter, Groq, Together, DeepSeek, Mistral, or OpenAI itself),
gives it a name and, when the server wants one, a key, and that server appears in the Agents window's agent list
beside Claude Code. Starting it lists the server's models through `GET {base}/models` into brief 0057's model
picker, and a prompt runs an agent loop over the chosen model with Eludite's own tools (the MCP endpoint from brief
0016) as its only tools, so every read, edit, build, test, debug and browser action goes through the same commands,
the same permission classes and the same pending-change review as every other agent. The loop is a new MIT adapter,
`agents/openai-acp/` (`eludite-openai-acp`), built like `agents/claude-acp/`: an ACP agent on stdio that Eludite,
Zed or any ACP client can run. PLAN.md 5.2 pins a first-party agent "until ACP hosting is excellent"; the owner
lifted that pin for this adapter because the local-model case has no agent CLI to wrap, and ACP keeps it
unprivileged: it reaches the IDE only through the endpoint any agent gets.

## Files in scope

- `docs/adr/0012-openai-compatible-agent.md` (new, 40 to 90 lines: why a first-party loop now, why Eludite's tools
  only, why a separate MIT process, what is not built: a sandbox, an agent's own file or shell tools, a router),
  `docs/adr/README.md` (the row), `docs/PLAN.md` section 5.2 only: the dated note under "Native first-party agent:
  pinned" naming this brief and the ADR.
- `protocol/schemas/` first and alone: `agents-settings.json` (the `providers` array), `agents-provider-set.input.json`,
  `agents-provider-remove.input.json`, `agents-provider-models.input.json` and `.output.json`, `agents-provider.output.json`,
  `agents-state.output.json` (`source: "provider"`), `file-read.input.json` and `.output.json`, `file-edit.input.json` and
  `.output.json`; then `crates/commands/src/agents.rs` and `crates/commands/src/files.rs` (or where `eludite.file.open`
  lives) for the four commands.
- `agents/openai-acp/` (new; its own Cargo workspace, MIT, `LICENSE`, `NOTICE`, `README.md`, `Cargo.toml` with
  `rust-toolchain.toml` as the root's): `src/main.rs`, `src/lib.rs`, `src/agent.rs` (ACP), `src/provider.rs` (the HTTP
  client, SSE), `src/models.rs` (listing and the catalog), `src/loop.rs` (the turn), `src/mcp.rs` (the MCP client over
  the stdio relay and HTTP), `src/compact.rs`, `src/prompt.rs` (the system prompt), `src/log.rs`, `tests/` with a
  loopback fake server and recorded fixtures, `examples/bench.rs`.
- `crates/acp/src/lib.rs` (`find_native_openai_adapter`, `provider_agent`), `crates/acp/src/settings.rs`
  (`ProviderSettings`, `AgentSource::Provider`), `crates/acp/src/protocol.rs` (`Usage::turn` also from `_meta.eludite.usage`).
- `crates/eludite/src/shell/agents.rs` (the provider commands, the key from the credential store into the agent's
  environment, the registry entries), `crates/eludite/src/shell/agents/window.rs` (the "Add server…" item in the
  agent picker and the dialog), `crates/eludite/src/shell/agents/providers.rs` (new: the dialog view),
  `crates/eludite/src/shell/options.rs` (the Agents page lists the servers with Edit and Remove),
  `crates/eludite/src/shell/agents/tests.rs`, `crates/eludite/src/shell/settings.rs` (the table row).
- `crates/forge/src/credentials.rs` only to make `Credentials` usable with a key that is not a forge host (it already
  is a string; the change is documentation and a test), nothing else in `crates/forge`.
- `tools/package/companions.sh` and `linux.sh` (`eludite-openai-acp` beside the shell where found), `CLAUDE.md` (the
  crate map row), `README.md` (the agents paragraph), `docs/briefs/README.md`, this file, `docs/briefs/0059-report.md`
  (new).

## Contract

### Servers in settings and the credential store

- `agents.json` (`agents-settings.json`) gains `providers: [{name, baseUrl, defaultModel?, headers?: {name: value},
  models?: [{id, name?, contextWindow?}], tools?: "core" | "all"}]`. `name` is unique, shown as the agent's name in the
  window and used as the credential key `provider:<name>`. `baseUrl` is the API root ending in the version segment
  (`http://localhost:8080/v1`, `https://openrouter.ai/api/v1`); the adapter appends `/models` and `/chat/completions`
  and strips one trailing slash. `headers` are extra request headers (OpenRouter's `HTTP-Referer` and `X-Title`), never
  the authorization. `models` is the hand-written catalog for a server that will not list its own (see Models).
  The key is never in the file: `eludite_forge::credentials::Credentials::system()` keeps it under `provider:<name>`
  (keyring, else the consented 0600 file, brief 0046's rule and prompt). A provider with no key sends no
  `Authorization` header at all (llama.cpp and Ollama accept that; a bare `Bearer` with nothing after it is refused by
  some gateways).
- Presets the dialog offers, prefilling `baseUrl` and the key hint: llama.cpp (`http://localhost:8080/v1`, no key),
  Ollama (`http://localhost:11434/v1`, no key), LM Studio (`http://localhost:1234/v1`, no key), vLLM
  (`http://localhost:8000/v1`), OpenAI (`https://api.openai.com/v1`), OpenRouter (`https://openrouter.ai/api/v1`),
  Groq (`https://api.groq.com/openai/v1`), Together (`https://api.together.xyz/v1`), DeepSeek
  (`https://api.deepseek.com/v1`), Mistral (`https://api.mistral.ai/v1`), and Custom. No xAI preset, by the owner's
  decision; a person can still enter its URL as Custom.
- Commands, all on the bus and agent-visible (PLAN.md 5.1): `eludite.agents.provider_set` (`{name, baseUrl, apiKey?,
  defaultModel?, headers?, models?, tools?}`; class `dangerous`: it stores a credential; the audit record and every log
  line replace `apiKey` with `"<redacted>"`; an absent `apiKey` keeps the stored one, `""` deletes it),
  `eludite.agents.provider_remove` (`{name}`; class `execute`; deletes the credential too), `eludite.agents.provider_models`
  (`{name?, baseUrl?, apiKey?}`: lists the server's models through the adapter's own code path, off the UI thread,
  with the stored key when `name` is given; answers `{models: [{id, name?, contextWindow?}], listing: "server" |
  "catalog" | "none", message?}`; class `execute`: it makes a network call). The window's dialog ("Add server…" in
  the agent picker; Tools > Options > Agents lists them with Edit and Remove) is a form over these three: name, preset,
  base URL, key (masked), a Test button that runs `provider_models` and shows "12 models" or the message, Save.
- The registry (`crates/acp/src/settings.rs`): one `RegisteredAgent` per provider, `AgentSource::Provider`, after the
  built-in Claude entries and before `agents` from settings, launching the adapter with `--base-url URL [--model
  defaultModel] [--header NAME=VALUE]... [--tools core|all] [--catalog PATH]` and the key in the environment as
  `ELUDITE_OPENAI_API_KEY` (set per launch from the store, never written to a file; `env_remove` the inherited
  `OPENAI_API_KEY` so the person's shell key is not picked up silently). The adapter is found like the Claude one:
  `ELUDITE_OPENAI_ACP`, beside the executable (`eludite-openai-acp`, `.exe` on Windows), then `PATH`; without it the
  provider entries show in the picker with the state `error` and the message "eludite-openai-acp was not found".

### Models

- `GET {base}/models` with the authorization and headers; a 30 s timeout. Statuses mean, as in slopcoder's catalog:
  401 and 403 "the key was rejected"; 400, 404, 405, 410 and 501 "this server does not list models" (`listing: "none"`
  unless the provider has a `models` catalog, then `"catalog"`); any other non-2xx an error with the status; a 200
  whose body is not JSON (a proxy's landing page) is "no listing"; `data[].id` are the models, sorted by id, case
  folded. A provider with a `models` catalog never asks the server. When `data[].meta.n_ctx_train` is present
  (llama.cpp) it is the context window; when `data[].context_length` is present (OpenRouter) likewise; else the
  catalog's `contextWindow`; else 0 (unknown; the strip shows no bar).
- The listed models become brief 0057's `model` config option (category `model`, `name` from the catalog's `name` or
  the id); the current one is `--model`, else the first listed. `session/set_config_option` changes the model for the
  next request with no restart. No `effort` option; no modes (the shell's policy is the permission model).
- `available_commands_update` after `session/new`: `compact` ("Summarize the conversation so far to free context") and
  `clear` ("Forget the conversation; the next prompt starts fresh"), handled by the adapter without a model call for
  `clear` and with one for `compact`.

### The turn

- The system prompt (`src/prompt.rs`): who it is (an agent inside Eludite, the IDE), the workspace root (`cwd`), the
  OS, the date, the rules (use the tools, never claim to have run something it did not, prefer `eludite-file-edit`
  for small changes and `eludite-workspace-apply-edit` for many, read before editing, build and run tests through
  the tools), and the MCP resources `eludite://guides/debugging`, `eludite://guides/git` and `eludite://guides/terminal`
  read once at session start and appended, each under its heading. Under 4,000 tokens by `tiktoken`-free estimate
  (bytes / 4); the report gives the real count on one model.
- Tools: the MCP `tools/list` of Eludite's endpoint (the stdio relay `eludite --mcp-relay ADDR` with `ELUDITE_MCP_TOKEN`
  from `session/new`'s `mcpServers`, or an HTTP server when given; the adapter's own MCP client, `src/mcp.rs`, is the
  client side of `crates/mcp`'s protocol: `initialize`, `tools/list`, `tools/call`, `resources/read`), each mapped to a
  Chat Completions `function` tool (`name`, `description`, `parameters` = `inputSchema`). Local models have small
  windows and 187 schemas are too many, so `--tools core` (the default) sends a core set and one meta tool:
  `eludite-tools` (`{prefix?}`: lists the other tools' names and one-line descriptions, and adds the ones named in
  `enable: [..]` to this session's tool list from the next request). The core set, by command id: `file.read`,
  `file.edit`, `file.open`, `workspace.tree`, `workspace.apply_edit`, `search.find`, `search.replace`, `diagnostics.list`,
  `build.workspace` (or `build.solution` until brief 0054 lands), `build.project`, `output.show`, `test.discover`,
  `test.run`, `test.results`, `terminal.open`, `terminal.send`, `terminal.read`, `terminal.wait`, `git.status`, `git.diff`,
  `git.stage`, `git.commit`, `git.log`, `editor.go_to_definition`, `editor.find_references`, `editor.hover`, `debug.start`,
  `debug.snapshot`, `debug.toggle_breakpoint`, `debug.wait`; `--tools all` sends everything. `tools/list` is read at
  session start and again after a `tools/list_changed` notification (brief 0016: a command registered at runtime
  appears on the next list).
- The request: `POST {base}/chat/completions` with `model`, `messages` (the system prompt, the history, the prompt),
  `tools`, `stream: true`, `stream_options: {include_usage: true}`, `max_tokens` 8,192 (or `max_completion_tokens` when
  `--max-completion-tokens` is set, for OpenAI's reasoning models), no `temperature`, no `tool_choice`, nothing else
  unless configured (`--chat-template-kwargs JSON` passes llama.cpp's `chat_template_kwargs` verbatim, for
  `enable_thinking`). Tool results are `role: "tool"` messages with `tool_call_id` and the result text; an image in a
  result (a screenshot) rides the next `role: "user"` message as an `image_url` data URI with a note in the tool
  message, since `role: tool` content is a plain string on most servers. Assistant turns are replayed with their
  `tool_calls` and never with reasoning.
- The stream (SSE, `data:` lines, `[DONE]`): `delta.content` fragments become `agent_message_chunk`;
  `delta.reasoning_content` or `delta.reasoning` become `agent_thought_chunk`; `delta.tool_calls[]` are accumulated by
  `index` (id and name on the first fragment, `function.arguments` concatenated across the rest) and each becomes an
  ACP `tool_call` (pending, title from the tool's name and its first argument, kind from the command's class as
  `mapping.rs` does for Claude) as soon as its name is known, then `tool_call_update` to `in_progress` when it runs and
  `completed` or `failed` with the result text; `usage` may ride a content chunk or arrive alone with an empty
  `choices` array; `finish_reason` is remembered; a response with `Content-Type: application/json` is a server that
  ignored `stream: true` and is parsed as one body; a stream that ends without `[DONE]` but with a `finish_reason` is
  complete; one that ends with neither is a dropped connection, retried once, then the turn fails with
  "The server closed the stream before the answer was complete". A non-2xx status is the turn's error with the
  server's `error.message` when the body has one (a context-length error is reported as such and triggers one
  compaction and retry, below). Malformed `arguments` JSON is handed to the model as the tool's failure, not run.
- The loop: after a response with tool calls, run them (`tools/call`, one at a time, in order; the MCP gate in the
  shell prompts or refuses by the policy, and a refusal's error text is the tool result), append the results, request
  again; stop when the response has no tool calls (`stop_reason: end_turn`), after 50 requests in one turn
  (`max_turn_requests`), on `session/cancel` (the HTTP request is dropped, the running MCP call is left to finish, the
  answer is `cancelled`), or on an error. The adapter never raises `session/request_permission`: it has no tools of
  its own, and the shell's gate already decides every call (brief 0016, ADR-0009).
- `usage_update` after each response: `used` = that response's `prompt_tokens` + `completion_tokens` (what the next
  request will carry), `size` = the model's context window or 0, no `cost`, and in `_meta.eludite.usage` the turn's
  totals so far under ACP's `Usage` names (`inputTokens`, `cachedReadTokens` when the server reports
  `prompt_tokens_details.cached_tokens`, `outputTokens`, `thoughtTokens` when it reports
  `completion_tokens_details.reasoning_tokens`, `totalTokens`, `model`); `crates/acp`'s `Usage::turn` reads
  `_meta.eludite.usage` and `_meta.claudeCode.usage` alike.
- Compaction (`src/compact.rs`): when `prompt_tokens` passes 85 percent of a known window, or the server answers a
  context-length error, the adapter asks the model (one request, no tools) for a summary of everything but the last
  four messages under a fixed instruction, replaces them with one `role: "user"` message holding it, notifies the
  shell with an `agent_message_chunk` line "Compacted the conversation (N tokens to M)", and continues; `/compact`
  does the same on demand. Tool results older than the last ten are trimmed to their first 2,000 characters before
  that point is reached.
- Logging: `$ELUDITE_OPENAI_ACP_LOG=stderr` or a path; request bodies are logged without the authorization header and
  with the key, if it appears anywhere, replaced; stdout carries ACP only (invariant 10).

### Two commands the loop needs

- `eludite.file.read` (`{path, startLine?, endLine?}`; class `read`): the file's text with line numbers, at most 2,000
  lines or 256 KB per call (`truncated: true` with the next line to ask for), from the open buffer when the file is
  open (unsaved text included, `source: "buffer"`), else from disk; a binary file is refused with its size; a path
  outside the workspace is refused unless absolute and readable (audited either way).
- `eludite.file.edit` (`{path, oldText, newText, replaceAll?}`; class `edit-in-buffer`): `oldText` must occur exactly
  once (or `replaceAll`), else the error says how many times; the edit becomes one `eludite.workspace.apply_edit` with
  the matching range, so it is a pending change the person reviews like any other, and the answer is
  `workspace-apply-edit.output.json`'s. Both are agent-visible and show in every agent's tool list, Claude Code's too.

### Packaging and discovery

- `tools/package/companions.sh` adds `eludite-openai-acp` beside the shell where it finds a release build (the same
  rule as `eludite-claude-acp`); `linux.sh --with-companions` carries it; the smoke test in
  `browsers/chromium/tests/package.rs` is not touched (the package test for companions is `tools/package/`'s own, if
  any; else the report says it was checked by hand).

## Proving test

- `agents/openai-acp` tests against a loopback fake server (`tests/fake_server.rs`: `std::net::TcpListener`, hand-written
  HTTP/1.1 as `crates/forge`'s fixture server does, scripted per test): `GET /models` listing, a 404 listing, a 401
  key, a non-JSON 200; a turn that streams text; a turn with two tool calls accumulated across fragments (one with
  arguments split mid-token) that calls a fake MCP server (`tests/fake_mcp.rs`, the client side proven against
  `crates/mcp`'s real server in the shell test below) and sends the results back in order; the usage-only final
  chunk; a JSON body instead of SSE; a stream ending without `[DONE]`; a dropped stream retried once then failed;
  `reasoning_content` as thoughts; cancel mid-stream answers `cancelled` within 100 ms; 51 tool rounds stop at
  `max_turn_requests`; compaction at 85 percent of a 16k window and on a context-length error; `/compact` and `/clear`;
  `set_config_option` changing the model for the next request; the key never appearing in the log with
  `ELUDITE_OPENAI_ACP_LOG` set; the system prompt under 4,000 estimated tokens; `--tools core` sending the core set
  plus `eludite-tools`, and `eludite-tools {enable: [...]}` adding to the next request.
- A real-server test, skipped without `ELUDITE_LLAMA_SERVER=URL` (the owner's `llama-server`; the report names the
  model and its size): list models, one turn that must call `eludite-file-read` on a file the test wrote and quote a
  word from it; the test asserts the tool was called and the word appears, not the prose.
- `crates/eludite` `agents::tests`: a provider added through `eludite.agents.provider_set` with the fake server's URL
  appears in the registry with source `provider`, its key is in the test credential store (the `MemoryStore`) and not
  in `agents.json`; starting it runs the adapter built by the test (`CARGO_BIN_EXE_` of the adapter is in another
  workspace, so the test builds `agents/openai-acp` once with `cargo build` as the Claude adapter's tests do, or skips
  with a message when `cargo` is absent); the model picker lists the fake's two models; a prompt produces a
  `diagnostics-list` call answered without a prompt, an `eludite-terminal-send` call that prompts and is denied (the
  model gets the refusal), an `eludite-file-edit` call held as a pending change and accepted; the usage strip reads
  the fake's counts; `provider_models` on a 404 server with a catalog answers `listing: "catalog"`; `provider_remove`
  deletes the credential; the audit record of `provider_set` has `"<redacted>"` for the key.
- `crates/commands`: `file.read` on an open buffer with unsaved text, a line range, the 2,000-line cap, a binary file;
  `file.edit` with zero, one and two occurrences, and `replaceAll`.
- Manual, recorded in the report with two screenshots: the Add server dialog with the llama.cpp preset and Test
  showing the model count; the window mid-turn against the owner's `llama-server` with a tool call and the usage
  strip (`linux-agents-openai-llama.png`).

## Budget

- From the server's first SSE byte to the first `agent_message_chunk` on stdout under 5 ms; the hop from a response's
  last byte to the next request's first byte (ACP updates, the MCP call excluded) under 20 ms.
- A 187-tool `tools/list` mapped and the request body built under 10 ms; `--tools core`'s request under 12,000
  estimated tokens before the history.
- The adapter under 6 MB stripped; resident memory under 60 MB after a 100-request turn; no thread per request
  beyond the HTTP read.
- The shell's frame p99 during a streamed turn within 5 percent of brief 0058's number.
- Dependencies for the adapter (SPDX in the PR): `agent-client-protocol` 2.2.0 (Apache-2.0, as the Claude adapter),
  `ureq` 3 (MIT OR Apache-2.0) with `rustls` (Apache-2.0 OR ISC OR MIT), `serde`, `serde_json`, `futures`, `uuid`.
  Nothing new in the root workspace.

## Exit criterion

1. Every test above is green; `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`,
   `cargo test --workspace` in the root and in `agents/openai-acp` pass; the real-server test passed once on the
   owner's machine and the report says against which model.
2. ADR-0012 is written and Accepted; PLAN.md 5.2 carries the dated note; CLAUDE.md's crate map and README name the
   adapter and the provider settings.
3. `docs/briefs/0059-report.md` has the budget numbers, the model and quantization the manual run used, how many
   tool rounds the local model needed for the file-read task, which presets were tried for real (llama.cpp at least),
   and what a small model got wrong (malformed arguments, invented tool names) and how the adapter answered it.
4. `docs/briefs/README.md` has this brief's row.

## Out of scope

- Anthropic's Messages API, Google's, or any non-OpenAI-compatible dialect; a "Responses API" client; OAuth or
  device-code sign-in (ChatGPT subscriptions, Codex); any subscription proxy.
- Tools of the adapter's own: no shell, no file writes outside `eludite.*`, no web fetch, no subagents.
- A sandbox or container; a model router ("Auto"); per-model pricing and cost estimates; a model catalog shipped in
  code beyond `contextWindow` from the server or the person's catalog.
- Thinking controls beyond `--chat-template-kwargs`; an `effort` option.
- Prompt caching, parallel tool execution, structured outputs, images in prompts (an image in a tool result is
  forwarded as above).
- Changing the Claude adapter, the Node adapter, the MCP server's tool mapping, or the permission classes.
