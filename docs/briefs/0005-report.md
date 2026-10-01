# Brief 0005 report: Claude Code via ACP in a GPUI panel, one MCP tool

Brief: [0005-acp-claude-code-spike.md](0005-acp-claude-code-spike.md). Spike code: [`spikes/0005-acp-panel/`](../../spikes/0005-acp-panel/). Production code touched: `crates/acp`, `crates/mcp`, `crates/commands`, and two new schemas in `protocol/schemas/`.

## 1. Summary

- **It works end to end on Linux.** The panel spawned Claude Code through the official ACP adapter, using the owner's existing login. On the brief's prompt, Claude called `diagnostics.list` through Eludite's MCP server and answered correctly: OrderController.cs has the most errors (3 of 4). The tool call showed in the panel with its arguments and its result. Transcript in section 9, screenshots in `spikes/0005-acp-panel/screenshots/`.
- **Adapter: `npx -y @agentclientprotocol/claude-agent-acp@0.85.0`** (Apache-2.0). It speaks **ACP protocol version 1** and uses the existing Claude Code login; Eludite reads no key. The brief's guess, `npx claude-code-acp`, resolves to an unrelated community package (section 3). `crates/acp`'s `default_agents()` and the brief's Contract line now use the verified command.
- **MCP transport: stdio, via a relay.** The agent launches the Eludite binary in `--mcp-relay` mode. The relay pipes stdio to a token-checked `127.0.0.1` TCP endpoint inside the IDE process, where the command bus lives. The MCP server is hand-written (about 200 lines), not `rmcp` (section 6).
- **Permissions:** `diagnostics.list` is class read. The adapter still sends `session/request_permission` for it, and the panel answers it by policy with no prompt, showing "Allowed without prompt". Every other request shows Yes / No in the panel. A denial fails the tool call and the agent reports it. This was checked with the fake agent (automated) and with the real agent (a Write denied; no file was created).
- **Budgets on Linux, all pass:**
  - Panel open to agent ready, warm npx cache: `initialize` 0.50 s, session ready 0.95 s (budget 5 s).
  - Cold, with an empty npm cache and a 432 MB download: 3.75 s and 4.2 s.
  - UI-thread frame work while streaming 200 chunks/s: p50 1.6 ms, p95 2.1 to 2.3 ms, **p99 2.6 to 2.8 ms** (budget 8 ms).
  - Chunk send to present: p99 18.7 ms at 60 Hz.
  - ACP and MCP I/O run on named background threads, and tests assert it.
- **Not done:** the manual run on a second OS (none was available), and CI on three OSes (no CI run from here). The crates' tests are written to be portable but have only run on Linux.

## 2. Machine and method

| | |
|---|---|
| CPU / RAM | AMD Ryzen 9 7940HS (16 threads), 30 GB |
| GPU | Radeon RX 7600M XT (Navi 33) + Radeon 780M; GPUI picked its default adapter |
| OS | CachyOS, Linux 7.2.4, KDE Plasma / KWin 6.7.5, Wayland session |
| Toolchain | Rust 1.98.1, GPUI rev 20d29fc6, Node 26.8.1, npm 12.0.2, Claude Code CLI 2.1.287 (the adapter bundles Claude Agent SDK 0.3.286, which reports CLI 2.1.286 to MCP servers) |

The desktop session was locked for the whole run (`LockedHint=yes`). KWin sends no frame callbacks to a hidden surface (brief 0001 report, section 5, item 1), so every GUI run used brief 0001's method: a nested `kwin_wayland --virtual --xwayland --no-lockscreen` with a 60 Hz virtual output and the real GPU (`spikes/0005-acp-panel/tools/nested.sh`). Screenshots come from `spectacle` inside a nested KWin that has its own D-Bus session (`tools/shots.sh`). Nobody could click while the session was locked, so in the real-agent denial run the harness flag `--auto-answer deny` pressed "No". It calls the same `Panel::answer` as the button's click handler, 1.5 s after the prompt renders. The click path itself is covered by a headless GPUI test that clicks the "No" button with a simulated mouse.

## 3. Adapter comparison

Each adapter was driven by a small Node probe through `initialize` and `session/new` with no prompt, from an empty working directory. The times are from spawn to response on a warm npx cache.

| Package | Version, license | ACP version | Existing login | Warm: init / session | Verdict |
|---|---|---|---|---|---|
| `@agentclientprotocol/claude-agent-acp` | 0.85.0, Apache-2.0; SDK `@anthropic-ai/claude-agent-sdk` 0.3.286 | 1 | **Yes.** `_auth/status_update` reports the subscription account; prompts run on it. | 0.45 s / 0.90 s (in the panel: 0.50 / 0.95 s) | **Chosen.** Works end to end. |
| `@zed-industries/claude-code-acp` | 0.16.2, Apache-2.0; **deprecated on npm** ("renamed to @agentclientprotocol/claude-agent-acp") | 1 | Yes, but only with `CLAUDECODE` unset. Inside a Claude Code session its bundled CLI refuses: "Claude Code cannot be launched inside another Claude Code session". | 0.44 s / 2.8 s | Works, but superseded; pins SDK 0.2.44 (older models). |
| `claude-code-acp` (the brief's guess) | 0.1.1, MIT, bin `cc-acp`, last published 2025-09 | 1 | Claims to. It advertises a `claude-code-subscription` auth method and creates sessions without checking. | 0.70 s / 0.75 s | **Rejected.** It writes log lines to **stdout** (`[ACP] No CLAUDE_API_KEY found...`), which corrupts the JSON-RPC stream. It advertises no `mcpCapabilities` and depends on `@anthropic-ai/claude-code` ^1.0.96. |

Notes on the chosen adapter:

- **Protocol version.** It answered `protocolVersion: 1` to requests for 1, 2 and 99. Its own SDK (`@agentclientprotocol/sdk` 1.5.1) exports `PROTOCOL_VERSION = 2`, so v2 is defined but this adapter negotiates 1. Eludite requests and implements 1.
- **Version pin.** `default_agents()` pins `@0.85.0` so behaviour is reproducible. Bumping the pin should re-run the conformance test against a new recording (section 11).
- **Environment.** Eludite strips Claude Code's session variables (`CLAUDECODE`, `CLAUDE_CODE_SESSION_ID` and 8 more) from the agent's environment. The deprecated adapter shows why: launched from a Claude Code terminal, it refuses to start.
- **Login when logged out.** With an empty `CLAUDE_CONFIG_DIR` (no credential files touched), `initialize` lists **no** auth methods unless the client declares `clientCapabilities.auth.terminal: true`. `session/new` still succeeds, and the first `session/prompt` fails with `-32000 Authentication required`. A non-standard `_auth/status_update` notification says `{"kind":"none","label":"Not logged in"}`. Eludite declares `auth.terminal`, so the adapter returns its own methods. The panel shows them with the full command to run in a terminal, e.g. `npx -y @agentclientprotocol/claude-agent-acp@0.85.0 --cli auth login --claudeai`. It does not run the login flow (out of scope).

## 4. What was built

Production crates. These are not throwaway, but they are spike quality where noted in the follow-up brief.

- `protocol/schemas/diagnostics-list.input.json` and `diagnostics-list.output.json`. These were the first commit, before any code (CLAUDE.md invariant 4). Input: optional `severity` (`error`, `warning`, `message`, the VS Error List names). Output: an array of `{path, line, column, severity, code, message}`, with 1-based line and column.
- `crates/commands/src/diagnostics.rs`: the `diagnostics.list` command (class read). It includes the schema files with `include_str!`, so the bus, the MCP server and the tests share one copy. It reads rows from a caller-supplied source. `fixture()` is the in-memory Error List: 7 rows across 3 files (OrderController.cs has 3 errors, PricingService.cs has 1 error and 1 warning, Customer.cs has 1 warning and 1 message).
- `crates/mcp`:
  - `McpServer` implements `initialize` (version negotiation), `ping`, `tools/list` and `tools/call` over an allow-list of registry commands.
  - Class read always runs. Other classes go through a permission gate that denies by default.
  - An observer gets one record per call, used for the audit line.
  - `transport` has newline-delimited stdio serving, `listen_local` (the 127.0.0.1 endpoint with a token) and `relay_stdio`. There is also an example stdio server over the fixture (`cargo run -p eludite-mcp --example fixture_stdio_server`).
- `crates/acp`:
  - `protocol`: a typed, tolerant subset of ACP v1. Unknown update kinds decode to `Other`.
  - `AcpClient`: spawn or connect; `initialize`, `session/new`, `session/prompt`, `session/cancel`; `session/update` and `session/request_permission` arrive as events; unknown agent→client requests get `-32601`.
  - `fake_agent` (also built as `eludite-fake-acp-agent`): a scripted agent that replays the shapes of a recorded real session. It honours permission answers and really calls the MCP server it is given.
  - `default_agents()` returns the verified command (with a test).

The spike (`spikes/0005-acp-panel`, throwaway) has three parts:

- `session`: the driver thread, the MCP endpoint and the permission policy.
- `transcript`: rows, one per agent text line, so the virtualized GPUI `list` re-measures only the tail while text streams.
- `panel`: the GPUI view and the harness modes described in its README.

New dependencies, all in the spike only: `async-channel` 2 (Apache-2.0 OR MIT) and `jsonschema` 0.58.4 (MIT, dev). The production crates gained no dependencies, and the root `Cargo.lock` is unchanged. Official crates considered but not used:

- `agent-client-protocol` 2.2.0 (Apache-2.0). It pulls in `async-io`, `async-process`, `futures`, `schemars` and `tracing`. The shell has no async runtime elsewhere, and a tolerant hand-written subset was smaller than adapting to it. `agent-client-protocol-schema` 1.10.2 (Apache-2.0) is the types-only option to revisit for production.
- `rmcp` 3.5.0 (Apache-2.0); see section 6.

## 5. ACP: the client sequence and the gaps found

The sequence is the same with the real adapter and the fake one:

1. Client → `initialize` with `{protocolVersion: 1, clientCapabilities: {fs: {readTextFile: false, writeTextFile: false}, terminal: false, auth: {terminal: true}}, clientInfo}`.
2. Client → `session/new` with `{cwd, mcpServers: [{name: "eludite", command: <eludite exe>, args: ["--mcp-relay", "127.0.0.1:PORT"], env: [{name: "ELUDITE_MCP_TOKEN", ...}]}]}`. The adapter then starts the MCP server and connects to it.
3. Client → `session/prompt`. The agent → client traffic is a stream of `session/update` messages:
   - `agent_message_chunk`;
   - `tool_call` and `tool_call_update` for `ToolSearch` (Claude Code defers MCP tools behind a search step);
   - `tool_call` for `mcp__eludite__diagnostics-list`, then `tool_call_update` with `rawInput {"severity":"error"}`;
   - `session/request_permission` for it, which the client answers with `allow-once` by policy;
   - `tool_call_update` `completed` with `rawOutput` and `content`;
   - more `agent_message_chunk`s, plus `usage_update` and `available_commands_update` (extensions).
4. Turn end: `{stopReason: "end_turn"}`.

The fake agent also covers the shell or edit tool that needs a prompt, a denial, `session/cancel` during a pending prompt (`stopReason: cancelled`), login required, and 200 chunks/s streaming.

Gaps and quirks in ACP and in this adapter:

1. **Tool identity is adapter-specific.**
   - The tool name travels in `_meta.claudeCode.toolName`. `tool_call` also carries a non-standard `name` field.
   - In `session/request_permission`, the adapter includes `toolName` (and `_meta.claudeCode.mcpServer {name, source}`) only for MCP tools and subagents. For built-ins the request has just a display title such as `Write notes.txt`.
   - The spike's policy keys on `mcp__eludite__<tool>` plus the server name when present. Production should correlate by `toolCallId` with the preceding `tool_call` and trust `mcpServer.source`, not titles.
2. **Read-class tools still produce permission requests.** The adapter asks for every MCP tool call (it has no annotation-based auto-allow). So "runs without a prompt" is the client answering by policy, which costs one round trip. It still shows in the transcript as "Allowed without prompt".
3. **The client sees only what the agent asks about.** The adapter loads the user's Claude Code settings (`settingSources: user, project, local`). Tools Claude Code allows by itself never reach `session/request_permission`: Read, Glob, Grep, allow-listed Bash, and anything in the user's allow rules. So "any other tool call triggers a prompt" holds only for calls the agent asks about. Closing this needs the agent's permission mode set (the adapter offers `default`, `acceptEdits`, `plan`, `auto`, `bypassPermissions`), or `_meta.claudeCode.options` overrides (seen in the source, not tested).
4. **Login state is not in the standard flow.**
   - Auth methods are only listed if the client declares `auth.terminal`.
   - A missing login is not reported until `session/prompt` fails with `-32000`.
   - The status itself arrives in `_auth/status_update`, an extension notification that includes the account's e-mail. The panel discards the e-mail; the recorded fixture is redacted.
5. **Unrequested context reaches the session.** `available_commands_update` lists the user's personal skills and commands, and the user's global CLAUDE.md applies. For a hosted agent inside an IDE this may or may not be wanted, and it is a setting to expose.
6. **Protocol version 2** is defined in the SDK but not negotiated by this adapter. Watch it when bumping the pin (PLAN.md risk 8).
7. **Cancellation.** Per spec, the client must answer pending permission requests with `cancelled` after `session/cancel`. Eludite does this. The fake agent checks it, but the real adapter's handling of it was not exercised.
8. **Extensions decode safely.** `usage_update` (with cost and rate-limit data), `available_commands_update` and other unknown kinds decode to `SessionUpdate::Other` and are ignored. The adapter version contains `ToolSearch` and several `_claude/*` meta keys. The client must stay tolerant, and the conformance fixture guards this.

## 6. MCP: transport, server and gaps

**Transport chosen: stdio, through a relay to a local TCP endpoint.**

- ACP requires every agent to support stdio MCP servers. HTTP and SSE are optional; this adapter advertises both, the community one neither. Stdio therefore works with every ACP agent Eludite will host.
- But a stdio MCP server is a child process of the agent, while the command bus must stay in the IDE process (the MCP server re-exports the bus; ADR-0003). So the stdio server is a thin relay: the Eludite binary itself in `--mcp-relay ADDR` mode, connecting to `127.0.0.1:<random port>`. On that port the IDE serves MCP on a per-connection thread.
- The relay's first line must be a per-run 128-bit token, passed in the server's `env`. Another local process cannot drive the bus by guessing the port. The token comes from std's OS-seeded `RandomState`, which is fine for a spike; production should use the OS RNG.
- Zed solves the same problem the same way (`zed --nc`). Localhost HTTP would avoid the relay process but only for agents that advertise `mcpCapabilities.http`, so it was not chosen. TCP was used instead of a Unix socket so the code is identical on Windows.

**Server: hand-written, not `rmcp`.** `rmcp` 3.5.0 (Apache-2.0) requires tokio, and its idiomatic path derives tool schemas from Rust types with `schemars`. Eludite's schemas come from `protocol/` first, and the server needs three methods over the existing `eludite-protocol` JSON-RPC types. The hand-written server is about 200 lines plus transports, with no new dependencies.

**What Claude Code did on the MCP side.** Captured by teeing the stdio server:

1. It first sent `server/discover` with `io.modelcontextprotocol/protocolVersion: "2026-07-28"`, a newer MCP revision with a discovery handshake.
2. On our `-32601` it fell back to `initialize` with `protocolVersion: "2025-11-25"`, which the server accepted.
3. Then `notifications/initialized`, `tools/list`, and later `tools/call` with `{"name": "diagnostics-list", "arguments": {"severity": "error"}}`.

MCP gaps found:

1. **Tool names cannot contain dots.** The command `diagnostics.list` is exposed as `diagnostics-list` (the existing `tool_name` mapping: `.` becomes `-`, which is reversible because ids never contain `-`). Claude then calls it `mcp__eludite__diagnostics-list`. The brief asked for a tool named `diagnostics.list`; that is not possible verbatim.
2. **`outputSchema` must describe an object** (`structuredContent` is always an object), but the brief's output is an array. The MCP side wraps non-object outputs as `{"result": <output>}`, in both the schema and the content. The bus keeps the `protocol/` schema unchanged. The text content block carries the same JSON. Claude Code's `rawOutput` was the wrapped JSON string.
3. **`$schema` and `$id` are stripped** from schemas sent over MCP (2020-12 is MCP's default dialect, and some client validators reject an explicit 2020-12 `$schema` or duplicate ids).
4. **A newer MCP revision is already in use** by the client (`2026-07-28`, `server/discover`). The server should learn it before clients drop the fallback.
5. **Tool discovery is deferred.** Claude Code does not put MCP tools in context up front; the model calls `ToolSearch` first (`select:mcp__eludite__diagnostics-list`). It costs about one model round trip. The tool's `description` (taken from the input schema's root `description`) is what makes it findable, so schema descriptions are product surface.

## 7. Permission flow

Policy (PLAN.md 5.3, brief Contract). It runs on the ACP reader thread, never the UI thread:

- **Auto-allow.** A permission request whose tool is `mcp__eludite__<tool>` qualifies when all of these hold:
  - the adapter-reported server, if present, is `eludite`;
  - `<tool>` maps back to an **exposed** command;
  - that command is class **read**.

  The client then answers with the first `allow_once` option. The panel shows the request with "Allowed without prompt: `diagnostics.list` is class read", and an `[audit]` line goes to stderr.
- **Ask.** Everything else, including built-in tools, other MCP servers, and Eludite commands that are registered but not exposed, becomes a pending card with the agent's options. In the real run those were "Yes", "Yes, allow all edits during this session" and "No". Answering writes the response on a background thread. Stop (or Esc) sends `session/cancel` and answers every pending card with `cancelled`.
- **Server-side check.** The MCP server checks again: a non-read command is denied unless a gate allows it. This spike exposes only the read command, so the gate is never consulted.
- **Audit.** The audit record is a log line (out of scope: storage). The bus's in-memory `AuditLog` also records each invocation, and the MCP test asserts it.

Evidence:

- Automated: `crates/acp/tests/client.rs` (`client_sequence_read_runs_and_shell_is_denied`, `denied_read_stops_the_call_and_agent_reports_it`, `cancel_while_permission_pending`) and `spikes/0005-acp-panel/tests/panel.rs` (`diagnostics_read_runs_without_prompt_and_shell_is_denied` clicks "No" on the shell prompt; `policy_only_auto_allows_eludite_read_tools`).
- Manual: the real agent asked to `Write notes.txt` and the prompt was denied. The tool call ended `failed` with "User refused permission to run tool", no file was created, and the agent replied: "The write to notes.txt was declined by the permission prompt, so the file was not created." (Screenshots `linux-claude-permission-prompt.png` and `linux-claude-permission-denied.png`.)

## 8. Budgets

**Panel open to agent ready.** Measured with `--bench-ready`: the window opens, the panel starts the session at once, and the run ends at the `session/new` response. These times include spawning `npx`, `initialize`, and `session/new` (which starts the MCP relay and connects to it). No prompt was sent.

| Run | `initialize` | session ready |
|---|---|---|
| Warm npx cache (5 runs) | 492 to 516 ms, median 496 | 942 to 997 ms, median 952 |
| Cold: empty `npm_config_cache` each time, 432 MB download (3 runs) | 3743 to 3787 ms | 4180 to 4293 ms |

Budget under 5 s warm: **pass** (0.95 s). The cold time also met it here, but it depends on the network. Panel RSS at ready was about 76 MB. The agent itself (Node plus the Claude Code binary) is a separate process and is not counted.

**Streaming.** Measured with `--bench-stream`: the fake agent, as a real child process, sends 2000 `agent_message_chunk`s at 200/s, about 40 characters each, with a newline every 8th. That makes a 250-line message in a virtualized list. The run used nested KWin, Wayland backend, 60 Hz virtual output, release build, 3 runs. Frame work is the time from the panel's `render` to a `cx.defer` after the frame is presented, the same method as brief 0001. Apply is the UI-thread time to fold one batch of events into the transcript.

| | p50 | p95 | p99 | max |
|---|---|---|---|---|
| Frame work (601 to 602 frames per 10 s run; 60 fps) | 1.60 to 1.67 ms | 2.10 to 2.29 ms | **2.56 to 2.80 ms** | 8.96 to 10.2 ms |
| UI apply per batch (about 2000 batches of 1.003 events) | 0.009 ms | 0.020 ms | 0.030 ms | 0.31 ms |
| Chunk send (agent process) to end of present | 10.0 to 10.2 ms | 17.5 to 17.6 ms | 18.6 to 18.7 ms | 21 ms |

Budget, UI thread under 8 ms at p99: **pass** with a wide margin. Apply plus frame work stays under 3 ms at p99. The worst single frame per run was 9.0 to 10.2 ms; the cause was not investigated. Text renders as it arrives: chunk to present is at most one 16.7 ms refresh interval plus the frame. 1999 of 2000 chunks are counted because the report prints before the final frame presents. Informational: at 1000 chunks/s (10,000 chunks), frame work p99 was 2.56 ms and chunk to present p99 18.6 ms.

**I/O off the UI thread.**

- ACP reads run on `acp-reader` and `acp-stderr` threads. The blocking `initialize`, `session/new` and `session/prompt` calls run on the `acp-driver` thread. Permission answers and cancels are written from short-lived threads.
- MCP is served on `mcp-accept` and `mcp-conn` threads.
- The UI thread only drains an `async_channel` and updates the model.
- Tests assert two of these: `spawned_process_and_close` asserts that every ACP event is delivered on `acp-reader` or `acp-stderr`, and the panel test asserts that the MCP call was served on `mcp-conn`.
- The panel drops events from a replaced session by generation number (invariant 12).

## 9. Manual test transcript (Linux)

The run used `tools/shots.sh RUN -- --cwd <empty dir> --prompt "List the current errors in the Error List and tell me which file has the most."`, with the real agent and a warm cache. Panel transcript (from `--transcript-out`; the fixture's file paths are relative):

- **User:** List the current errors in the Error List and tell me which file has the most.
- **Agent:** I'll pull the IDE's diagnostics list and tally errors per file.
- **Tool call** `ToolSearch` (other), completed. Arguments `{"query": "select:mcp__eludite__diagnostics-list", "max_results": 1}`. Result `Tool: mcp__eludite__diagnostics-list`.
- **Tool call** `mcp__eludite__diagnostics-list` (other), completed. Arguments `{"severity": "error"}`. Result:
  ```
  {"result":[{"path":"src/Contoso.Web/Controllers/OrderController.cs","line":42,"column":17,"severity":"error","code":"CS0103","message":"The name 'orderTotal' does not exist in the current context"},{"path":"src/Contoso.Web/Controllers/OrderController.cs","line":57,"column":34,"severity":"error","code":"CS1002","message":"; expected"},{"path":"src/Contoso.Web/Controllers/OrderController.cs","line":88,"column":24,"severity":"error","code":"CS0029","message":"Cannot implicitly convert type 'string' to 'int'"},{"path":"src/Contoso.Core/Services/PricingService.cs","line":15,"column":9,"severity":"error","code":"CS0246","message":"The type or namespace name 'DiscountPolicy' could not be found (are you missing a using directive or an assembly reference?)"}]}
  ```
- **Permission** `mcp__eludite__diagnostics-list` `{"severity": "error"}`. Options: Yes / Yes, and don't ask again for Diagnostics-List commands / No. Allowed without prompt: `diagnostics.list` is class read.
- **Agent:**
  > The Error List currently shows 4 errors across 2 files.
  >
  > **src/Contoso.Web/Controllers/OrderController.cs** has the most, with 3 errors:
  >
  > - `src/Contoso.Web/Controllers/OrderController.cs:42` CS0103: The name 'orderTotal' does not exist in the current context
  > - `src/Contoso.Web/Controllers/OrderController.cs:57` CS1002: ; expected
  > - `src/Contoso.Web/Controllers/OrderController.cs:88` CS0029: Cannot implicitly convert type 'string' to 'int'
  >
  > **src/Contoso.Core/Services/PricingService.cs** has 1 error:
  >
  > - `src/Contoso.Core/Services/PricingService.cs:15` CS0246: The type or namespace name 'DiscountPolicy' could not be found (missing using directive or assembly reference)
  >
  > The missing semicolon on line 57 of OrderController.cs may be causing the other errors in that file, so fixing it first is worth trying before chasing the rest.
- Turn ended: `end_turn`. The stderr audit lines were `[audit] permission auto-allowed: diagnostics.list is class read` and `[audit] mcp tools/call diagnostics-list -> Some("diagnostics.list") class=Some(Read) args={"severity":"error"} ok=true 0.02 ms`.

The answer is correct for the fixture. Screenshots in `spikes/0005-acp-panel/screenshots/`:

- `linux-claude-diagnostics.png`: the end of this run, showing the result, the auto-allowed permission and the answer.
- `linux-claude-permission-prompt.png`: a pending prompt with the real agent.
- `linux-claude-permission-denied.png`: after the denial.
- `linux-fake-agent-denied.png`: the scripted flow.

The first real session, recorded through a Node probe before the panel existed, made the same tool call and gave the same answer. Redacted, it is the conformance fixture `crates/acp/tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl`. In total, three prompts used the subscription: two diagnostics runs and one denial run. A fourth prompt, sent while logged out to observe the auth error, failed before reaching the model.

**Second OS: not run.** Only this Linux machine was available.

## 10. Exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Automated tests pass in CI on all three OSes | **Partly met.** They pass on Linux; CI was not run. Run here: `cargo test -p eludite-acp -p eludite-mcp -p eludite-commands` (36 tests: acp 9, mcp 12, commands 15), the workspace gate (`cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace`, all clean) and the spike's `cargo test` (7 tests, 4 of them headless GPUI tests with real child processes). The tests use only `std::io::pipe`, localhost TCP and `CARGO_BIN_EXE`, and spawning maps `npx` to `npx.cmd` on Windows, but Windows and macOS are untested. |
| 2 | Manual run: Claude Code calls `diagnostics.list` through Eludite's MCP server and answers correctly, on two OSes | **Linux: pass** (section 9). Second OS: **not run.** |
| 3 | A denied permission request stops the tool call and the agent reports the denial | **Pass**, automated (fake agent) and manual (real agent, section 7). |
| 4 | Report records the ACP protocol version, adapter version, MCP transport and why, and protocol gaps | **Done:** ACP v1; adapter 0.85.0; stdio relay to local TCP (section 6); gaps in sections 5 and 6. |
| 5 | State what the subscription-terms question needs answered (risk 9), without answering it | **Done:** section 12. |

## 11. Conformance and test inventory

- `crates/commands` (4 new tests): spec and schemas, fixture shape, filtering, invalid input and audit.
- `crates/mcp` (10 new; 12 in total):
  - `initialize` negotiation;
  - `tools/list` exposes exactly one tool, with the wrapped output schema;
  - `tools/call` returns the fixture, validated against the `protocol/` output schema and the MCP-wrapped one by a JSON Schema subset validator (which is itself tested);
  - bad calls; the permission gate; read never reaching the gate; audit;
  - stdio framing; TCP endpoint plus relay; wrong token rejected.
- `crates/acp` (2 unit, 7 integration):
  - client sequence, read auto-allowed, shell denied;
  - denied read;
  - cancel during a pending permission;
  - login required;
  - 300-chunk ordering;
  - real child process and close;
  - **every message of the recorded real session decodes** with the crate's types.
- Spike (3 unit, 4 integration):
  - the full GUI flow (type the prompt, click Send, read auto-allowed, MCP through the relay process, result validated with the `jsonschema` crate, click "No", denial reported);
  - login state;
  - Stop cancels;
  - the policy table.

## 12. Subscription terms (PLAN.md risk 9): what needs answering

This brief does not answer these questions. Before Eludite documents "Claude Code via ACP with your subscription" for anyone but the owner, someone with authority over the terms needs to confirm:

1. Whether the consumer and organization subscription terms (Pro, Max, Team, Enterprise) allow a third-party application, Eludite, to launch Claude Code through an ACP adapter and drive it on the user's behalf. The adapter does this through the Claude Agent SDK and the user's own CLI login.
2. Whether it matters that Eludite neither sees nor stores credentials (the agent reads its own login), or whether any programmatic driving of a subscription login counts as automated or third-party use.
3. Whether the Agent SDK's terms (as opposed to the Claude Code CLI's) apply when the adapter, not the user, starts sessions, and whether subscription use through the SDK is permitted or reserved for API-key billing.
4. Whether usage through Eludite may count toward, or be restricted by, the plan's rate limits differently. The adapter reports `rateLimit` and overage status in `usage_update`.
5. Whether Eludite may show the account data the adapter emits (plan, e-mail, organization in `_auth/status_update`) and log or redact it.
6. Whether the answer differs for an organization's members on the org's plan, which is the owner's case, versus individuals.
7. Whether Eludite may recommend the `@agentclientprotocol/claude-agent-acp` package by name, and pin it, in its documentation.

## 13. How to reproduce

From `spikes/0005-acp-panel/` (see its README), with `CARGO_TARGET_DIR` set to the shared target dir if wanted:

```
cargo test
cargo build --release
tools/shots.sh /tmp/run-diag -- --cwd /tmp/empty --prompt "List the current errors in the Error List and tell me which file has the most."
tools/shots.sh /tmp/run-deny -- --cwd /tmp/empty2 --auto-answer deny --prompt "Create a file named notes.txt in the current directory containing the word hello."
tools/nested.sh -- target/release/spike-acp-panel --bench-ready --cwd /tmp/empty                 # warm
tools/nested.sh -- target/release/spike-acp-panel --bench-ready --cwd /tmp/empty --npm-cache /tmp/npm-cold-N  # cold
tools/nested.sh -- target/release/spike-acp-panel --bench-stream --linger-ms 300                 # output in $OUT_DIR/nested.out
```

On an unlocked desktop, run the binary directly and type the prompt. On Windows and macOS, `cargo run --release` and the `--bench-*` modes work without the nested-compositor scripts. RSS is reported on Linux only.

One more observation, not investigated: with GPUI's X11 backend on the nested Xwayland, an explicit `cx.notify()` after the turn ended produced no further render. Screenshots taken there with `import` lag one presented frame. The Wayland backend in the same nested compositor behaved normally, so all reported numbers and screenshots use Wayland.

## 14. Follow-up brief (sized): production ACP and MCP in `crates/`

**Proposed brief 00NN: Agents tool window over ACP and the command-bus MCP server.** It takes this spike's code into production shape and adds a second OS. Estimated at roughly 2,500 lines including tests. It fits two agent-weeks, or splits into two briefs (A: crates; B: window) that can run in parallel once the crate API is fixed.

- **Files in scope:** `crates/acp/**`, `crates/mcp/**`, `crates/commands/**` (registry policy hooks only), `crates/eludite/**` (the `--mcp-relay` mode and the Agents tool window), `crates/ui/**` (a text input widget), and `protocol/schemas/` (an `acp-permission-policy` schema, added first).
- **Work:**
  1. Move the spike's session driver into `eludite-acp` as a `Session` type: per-session generation, cancellation tokens, and a single writer thread instead of a thread per answer. Replace the hand-written types with `agent-client-protocol-schema` if its dependency set is acceptable (license Apache-2.0); otherwise keep the tolerant subset and grow it from recordings.
  2. Make the permission policy production-grade. Correlate permission requests to `tool_call`s by `toolCallId`, and key MCP trust on `_meta.claudeCode.mcpServer.source`. Add per-solution committable policy files (PLAN.md 5.3), and audit to the `AuditLog` with persistence left to the audit brief.
  3. Set the agent's permission mode at session start so that tools the agent would self-approve are surfaced or governed (gap 5.3). Decide whether user settings, skills and CLAUDE.md flow into IDE-hosted sessions.
  4. MCP:
     - generic exposure of every registry command, gated by policy;
     - the OS RNG for the token, and a check that the relay's parent is the agent;
     - the `2026-07-28` `server/discover` handshake;
     - `notifications/tools/list_changed` when commands register;
     - generated Rust bindings from the `protocol/` schemas (needs the generator from the protocol brief).
  5. The Agents tool window in the real shell: docking, the virtualized transcript, markdown rendering, a GPUI `EntityInputHandler` text input (IME), the login state with the agent's own terminal command, and Stop.
  6. CI: run the crates' tests on Linux, Windows and macOS. Run the manual test on Windows. Add a recorded-session conformance test per adapter pin, and a documented procedure to bump the pin.
- **Exit:**
  - The tests pass in CI on three OSes.
  - The manual diagnostics and denial runs pass on Linux and Windows.
  - Frame work stays under 8 ms at p99 while streaming at 200 chunks/s in the real shell window.
  - Ready time is under 5 s warm.
  - The audit entries are queryable.
- **Out of scope:** multiple concurrent agents, worktree-per-agent, inline diffs and review changesets, and agents other than Claude Code (a separate, small brief once this lands).

## 15. Deviations and notes

- **Commit trailers.** `git commit -s` always appends the sign-off as the last trailer unless it is already last, so it cannot produce a message that ends with the two required trailer lines. The commits therefore carry the same `Signed-off-by` line written explicitly, placed before `Co-Authored-By` and `Claude-Session`.
- **`git fetch origin` before starting.** `origin` has no `main` ref; the branch started at the local `main` (6925e91).
- **Briefs index not updated.** `docs/briefs/README.md`'s index row is outside this brief's files. The brief's own Status and Contract lines are updated.
- **Spike harness flags.** `--auto-answer` and `--prompt` exist because the session was locked; they call the same code paths as the buttons and the prompt box.
