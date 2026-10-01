# Brief 0006 report: native Rust ACP adapter for Claude Code

Brief: [0006-native-claude-acp-adapter.md](0006-native-claude-acp-adapter.md). Code: [`agents/claude-acp/`](../../agents/claude-acp/) (package `niello-claude-acp`, MIT) and `crates/acp/src/lib.rs` (`default_agents()`).

## 1. Summary

- **It works on Linux with no Node.** `niello-claude-acp` speaks ACP v1 on stdio and drives the installed `claude` 2.1.287 over its headless stream-json protocol. It uses the owner's existing login and reads no credential.
- **The brief 0005 panel works unchanged against it.** The spike's source and lock file are untouched. It picks the native adapter through `default_agents()`.
  - On the diagnostics prompt, Claude called `mcp__niello__diagnostics-list` through Niello's MCP relay, and the panel auto-allowed it as class read. The answer was correct: OrderController.cs has 3 of the 4 errors.
  - On the Write prompt the panel showed Yes / Yes, allow all edits / No / No, and don't ask again. The harness pressed No. The tool call failed, the agent reported the denial, and no file exists.
  - Both runs had `PATH` reduced to one directory holding only a `claude` symlink (section 8).
- **ACP crate:** `agent-client-protocol` **2.2.0** (Apache-2.0), with `agent-client-protocol-schema` 1.9.1 (Apache-2.0), protocol version 1. The adapter runs it on `futures::executor::block_on` with no async runtime.
- **Budgets, all pass (section 9):**

  | Budget | Limit | Result |
  |---|---|---|
  | Session ready, warm (`initialize` to `session/new` response) | under 1.5 s | median 451 ms (429 to 594 ms, 5 runs). In the panel, from session start: median 469 ms. Brief 0005's Node adapter took 952 ms. |
  | Adapter RSS | under 30 MB | 6.5 MB, flat across a 2000-chunk stream (HWM 8.2 MB) |
  | Hot path at 200 chunks/s | no visible per-chunk cost | 1.0 to 1.1 % of one core; RSS unchanged |
  | Binary size, release, stripped | under 10 MB | 2.3 MB |

- **`default_agents()`** returns the native adapter first when it is found: at the configured path (`$NIELLO_CLAUDE_ACP`), beside the IDE's executable, or on `PATH`. `npx -y @agentclientprotocol/claude-agent-acp@0.85.0` always follows as the fallback. Tests cover the order and the fallback.
- **Session resume (`loadSession`) is deferred.** It is sized in section 7. `claude --resume` does not replay history on stdout, so resume needs a transcript reader.
- **Tests:**
  - adapter: 18, all passing (8 unit, 8 conformance against the fake `claude`, 2 golden mapping);
  - `niello-acp`: 11 (4 unit, 7 integration);
  - the root workspace gate is clean.
- **Not done:** Windows and macOS runs (out of scope; the discovery code is written), and reject-always against the real CLI (section 6).

## 2. Machine and method

| | |
|---|---|
| CPU / RAM | AMD Ryzen 9 7940HS (16 threads), 30 GB |
| OS | CachyOS, Linux 7.2.8, KDE Plasma Wayland |
| Toolchain | Rust 1.98.1, GPUI rev 20d29fc6 (spike only) |
| Claude Code | 2.1.287, the native ELF at `~/.local/bin/claude`, logged in with the owner's organization subscription |

GUI runs used brief 0005's nested `kwin_wayland --virtual` method (`spikes/0005-acp-panel/tools/shots.sh`, unmodified). The harness flag `--auto-answer deny` pressed "No", as in 0005.

**Real model prompts: five.** Two more than the brief's three or four.

- Three were for the `claude`-side recording: the diagnostics question, the Write, and a long answer interrupted after its first chunk. All were in one session.
- Two were the panel runs: diagnostics, and Write denied.
- One more prompt ran with an empty `CLAUDE_CONFIG_DIR` to record the logged-out shape. It failed before reaching the model.
- Every other run (benchmarks, handshakes, the resume probe) sent only the `initialize` or `mcp_status` control requests, which make no model call.

## 3. What was built

`agents/claude-acp/` is its own Cargo workspace, as the brief requires. It contains:

| File | Purpose |
|---|---|
| `src/agent.rs` | The ACP handlers: `initialize`, `authenticate`, `session/new`, `session/prompt`, `session/cancel`, and `session/request_permission` raised from `can_use_tool`. `session/new`, each turn, and each permission request run as tasks spawned on the connection, so `session/cancel` is handled while a turn streams. |
| `src/process.rs` | One `claude` child per session. A reader thread routes replies to the adapter's own control requests by `request_id` and sends everything else, in order, to an unbounded channel. A second thread copies the child's stderr to the log. |
| `src/translate.rs` | Pure translation of one turn from stream-json to `session/update`. |
| `src/mapping.rs` | Pure mappings: tool title, kind, locations and diff; tool results; the four permission options and their `control_response`; the `--mcp-config` document; prompt content. |
| `src/discovery.rs` | Finding `claude`: `--claude`, `$NIELLO_CLAUDE_PATH`, `PATH`, then `~/.local/bin`; `claude.exe` on Windows. The result is made absolute because the child runs in the session's cwd. Also the version check and the list of session variables to strip. |
| `src/fake_claude.rs`, built as the test-only binary `niello-fake-claude` | Replays a recorded session and checks each input against the recording. |
| `tools/record.py`, `tools/redact.py` | Record a real session and redact it into a fixture. |
| `examples/bench.rs` | The budget measurements. |
| `LICENSE` (MIT), `NOTICE`, `README.md` | `NOTICE` credits the Apache-2.0 Node adapter for the mapping and states that no adapter or SDK code was copied. |

The binary has three forms:

- `niello-claude-acp [--claude PATH] [--model M]` serves ACP on stdio.
- `niello-claude-acp auth login` runs `claude auth login`. This is the ACP terminal login method.
- `--version` and `--help`.

`NIELLO_CLAUDE_ACP_LOG=stderr|PATH` turns on the verbose log. stdout carries only ACP.

**Dependencies** (SPDX ids):

- `agent-client-protocol` 2.2.0: Apache-2.0
- `agent-client-protocol-schema` 1.9.1: Apache-2.0
- `futures` 0.3: MIT OR Apache-2.0
- `serde_json` 1: MIT OR Apache-2.0
- `uuid` 1: Apache-2.0 OR MIT
- Transitive dependencies: MIT, Apache-2.0, or both; `memchr` is Unlicense OR MIT; `unicode-ident` (build time) is (MIT OR Apache-2.0) AND Unicode-3.0. All are compatible with MIT and GPL-3.0-or-later.
- Dev-only: `niello-acp`, path, GPL-3.0-or-later. It is used only by the tests to drive the adapter as Niello does, and is not linked into the binary.

**Using the official crate.** It fitted without needing any hand-written ACP types:

- The v1 schema covers everything needed: terminal `AuthMethod`, `ToolCall.name`, `_meta`, and `McpServer` stdio and http.
- The `Agent.builder()` dispatch runs on any executor.
- The cost is about 100 crates at build time (`schemars`, `derive_more`, `strum`, `tracing`, `darling`). The binary is still 2.3 MB.
- The crate pins `agent-client-protocol-schema =1.9.1`, so the schema version moves only with the crate.

**`crates/acp`** gains:

- `NATIVE_CLAUDE_ADAPTER`, `NATIVE_CLAUDE_ADAPTER_ENV` (`NIELLO_CLAUDE_ACP`), `AdapterSearch`, `find_native_claude_adapter`, `native_claude_agent`, `npx_claude_agent` and `default_agents_with`;
- `default_agents()`, which now returns native then npx;
- four unit tests. These replace the old `claude_code_is_default`, which assumed npx was first.

One line of `crates/acp/tests/client.rs` now calls `npx_claude_agent()` instead of `default_agents()[0]`, because the first entry depends on the machine.

## 4. The `claude` side: observed protocol (2.1.287)

**Launch.**

```
claude --print --input-format stream-json --output-format stream-json --include-partial-messages
  --replay-user-messages --verbose --permission-prompt-tool stdio --permission-mode default
  --session-id <uuid> --mcp-config <file> --strict-mcp-config [--model M]
```

- The child runs in the session's cwd.
- Its environment loses `CLAUDECODE`, `CLAUDE_CODE_ENTRYPOINT`, `CLAUDE_CODE_SESSION_ID`, `CLAUDE_CODE_CHILD_SESSION`, `CLAUDE_CODE_BRIDGE_SESSION_ID`, `CLAUDE_CODE_MESSAGING_SOCKET`, `CLAUDE_CODE_MESSAGING_TOKEN`, `CLAUDE_CODE_SESSION_ATTENDED`, `CLAUDE_CODE_EXECPATH` and `CLAUDE_PID`, which is brief 0005's list.
- It also loses `CLAUDE_EFFORT`, which this machine's Claude Code session set as well.
- Nothing else is changed, so the login is inherited.

**Messages the adapter writes to stdin.**

| Message | Use |
|---|---|
| `control_request` `initialize` | The handshake, sent at `session/new`. The adapter waits for its reply, which takes about 440 ms warm. |
| `user` `{message: {role, content: [text...]}, parent_tool_use_id: null, session_id}` | One per `session/prompt`. |
| `control_request` `interrupt` | On `session/cancel`. |
| `control_response` `{subtype: success, request_id, response: {behavior, updatedInput \| message, updatedPermissions?, interrupt?}}` | The answer to `can_use_tool`. |

**Messages observed on stdout** (types, subtypes and counts from the recording):

| Type | Subtypes and blocks seen | Adapter use |
|---|---|---|
| `control_response` | To `initialize`: `commands`, `agents`, `models`, `account`, `pid`, `current_permission_mode`, `session_state`, `capabilities`, and more. To `interrupt`: `{still_queued}`. **The child also echoes each `control_response` it receives back to stdout.** | Routed by `request_id`. Unknown ids (the echoes) are dropped. From the `initialize` reply only `account.tokenSource != "none"` is read, for a log line. |
| `control_request` | `can_use_tool` with `tool_name`, `input`, `tool_use_id`, `permission_suggestions`, `display_name`, `description`, and `mcp_server {name, source}` for MCP tools | Becomes `session/request_permission`. Any other subtype gets an error reply. |
| `system` | `init` (cwd, session_id, tools, mcp_servers, model, permissionMode, claude_code_version and more), sent **per turn, after the user message, not at startup**; `status` (`requesting`); `thinking_tokens` | Ignored |
| `stream_event` | `message_start` (`message.id`); `content_block_start` (`text`, `thinking`, `tool_use {id, name}`); `content_block_delta` (`text_delta`, `thinking_delta`, `signature_delta`, `input_json_delta`); `content_block_stop`; `message_delta` (stop_reason, usage); `message_stop`. Each carries `parent_tool_use_id`. | Text and thought chunks; `tool_call` at tool-use start |
| `assistant` | One message per content block, sharing `message.id`. Blocks: `text`, `thinking` (with empty `thinking` text here, signature only), `tool_use {id, name, input}`. Also `wire_tool_inputs`, `tool_use_meta`, `aborted` (interrupted), and `error: "authentication_failed"` with `is_api_error_message` when logged out (`model: "<synthetic>"`). | `tool_call_update` with full input. Text from messages that streamed no deltas. Detects a missing login. |
| `user` | Replays (`isReplay: true`); `tool_result {tool_use_id, content (string or blocks, including `tool_reference`), is_error}` with `tool_use_result` and `tool_result_meta`; `"[Request interrupted by user]"` | Tool results; replays ignored |
| `rate_limit_event` | `rate_limit_info` with plan utilization | Ignored and never forwarded (it is account data) |
| `result` | `success` (`stop_reason` `end_turn`, `terminal_reason` `completed`); `error_during_execution` (`terminal_reason` `aborted_streaming` after an interrupt, `errors[]`); logged out: `success` with `is_error: true`. Also cost, usage, `permission_denials`, timings. | Ends the turn |

**Fields the adapter depends on:**

- `type`.
- `control_response.response.{subtype, request_id, response, error}`.
- `control_request.{request_id, request.subtype}` and `can_use_tool`'s `tool_name`, `input`, `tool_use_id`, `permission_suggestions`, `display_name`, `mcp_server`.
- `stream_event.event.type`, `.message.id`, `.content_block.{type, id, name}`, `.delta.{type, text, thinking}`, `parent_tool_use_id`.
- `assistant.message.{id, content[].type, text, thinking, id, name, input}`, `assistant.error`.
- `user.isReplay`, `user.message.content[].{type, tool_use_id, content, is_error}`.
- `result.{subtype, stop_reason, errors, result}`.
- `account.tokenSource`, for the log only.

**Watch list** (fields and behaviour likely to drift):

1. `--permission-mode default`. The brief's value is accepted, but `--help` lists `manual` as the name; `default` looks like a legacy alias. Switch to `manual` if `default` is dropped.
2. `--permission-prompt-tool stdio` is not documented as a value in `--help`. There is a newer `--permission-prompts host|none` flag.
3. The `initialize` control request needs no fields today. The SDK may start sending hooks or other options that become required.
4. The `can_use_tool` field names, especially `permission_suggestions`, whose shapes `addRules` and `setMode` are passed back verbatim for allow-always, and `mcp_server`.
5. The `control_response` echo. A future version could stop echoing, or reuse ids.
6. The `system init` timing: it is per turn, not at startup. Do not wait for it at session start.
7. Thinking text is empty, with signature only, on this account and model. If it becomes visible, `thinking_delta` already maps to `agent_thought_chunk`.
8. `result.subtype` values (`success`, `error_during_execution`, `error_max_turns`) and `terminal_reason`. Logged out is `success` with `is_error`, recognised only through `assistant.error == "authentication_failed"`.
9. The stream event names and `parent_tool_use_id`, which follow the Messages API streaming format.
10. `tool_reference` blocks from deferred MCP tool search (`ToolSearch`).
11. Newer fields not used yet: `wire_tool_inputs`, `tool_use_meta.display_name`, `tool_result_meta.non_execution_kind`, and `interrupt`'s `still_queued`.

The adapter refuses `claude` older than **2.1.287**, with a message telling the user to run `claude update`, and logs when the version is newer.

## 5. The ACP side: the mapping

`initialize` returns:

- protocol version `min(requested, 1)`;
- `agentCapabilities {loadSession: false, promptCapabilities: text only, mcpCapabilities {http: true, sse: false}}`;
- `agentInfo {name: "niello-claude-acp", title: "Claude Code"}`;
- when the client declares `auth.terminal`, as brief 0005's adapter required, the method `{type: terminal, id: claude-login, args: ["auth", "login"]}`. The panel shows it as `<adapter path> auth login`.

`authenticate` succeeds with no action.

`session/new`:

- generates a UUID that is both the ACP session id and `--session-id`;
- writes the client's `mcpServers` to a `0600` temporary file: stdio servers as `{type: stdio, command, args, env}`, HTTP servers as `{type: http, url, headers}`;
- starts the child and completes the `initialize` handshake;
- **then deletes the file.** The child keeps the config in memory. This was checked with an `mcp_status` request after deletion: the server was still connected. A token in the file therefore does not outlive session start, even if the adapter is killed.
- Model: `--model`, then `$NIELLO_CLAUDE_MODEL`, then `_meta.claudeCode.options.model`. None is hard-coded.

`session/prompt` and the stream:

| `claude` | ACP `session/update` |
|---|---|
| `text_delta` (top level) | `agent_message_chunk` |
| `thinking_delta` with text (top level) | `agent_thought_chunk` |
| `content_block_start` `tool_use` | `tool_call`, status pending, title from the tool name, `rawInput {}`, `name`, `_meta.claudeCode.toolName` |
| `assistant` `tool_use` (complete) | `tool_call_update`: title, kind, locations, diff content, `rawInput`. A full `tool_call` is sent if no start was streamed. |
| `user` `tool_result` | `tool_call_update`: `completed` or `failed`, result text as content (a successful edit keeps its diff), `rawOutput` |
| `assistant` text from a message with no deltas | `agent_message_chunk`. This covers synthetic messages, but not the login error. |
| `result` | Prompt response: `end_turn`, `max_tokens`, `refusal`, `max_turn_requests`; `cancelled` if the client cancelled; `-32000` auth required if logged out; otherwise a JSON-RPC error with the CLI's `errors`. |

Titles and kinds follow the Node adapter:

| Tool | Title | Kind |
|---|---|---|
| Read | `Read <path relative to cwd>`, with a line location | read |
| Write | `Write <path>` | edit, with a diff whose old text is null |
| Edit, MultiEdit, NotebookEdit | `Edit <path>` | edit, with a diff |
| Bash | the command | execute |
| Glob, Grep | ``Find `p` ``, ``grep `p` `` | search |
| WebFetch, WebSearch | `Fetch <url>`, `Search "<q>"` | fetch |
| Task, Agent | the description | think |
| TodoWrite | | think |
| ExitPlanMode | | switch_mode |
| MCP and others | the tool name | other |

**Permissions.** `can_use_tool` becomes `session/request_permission`:

- The `toolCall` carries `toolCallId = tool_use_id`, the mapped title, kind, locations and content, `rawInput`, and `_meta.claudeCode.{toolName, mcpServer}`. The panel's policy reads the last two.
- There are four options, mapped as follows:

| ACP option | `control_response` |
|---|---|
| `allow` (allow_once, "Yes") | `{behavior: allow, updatedInput}` |
| `allow_always` ("Yes, allow all edits during this session" when the child suggests `setMode acceptEdits`, otherwise "Yes, and don't ask again for X") | adds `updatedPermissions`: the child's own `permission_suggestions`, or a session `addRules` allow rule |
| `reject` (reject_once, "No") | `{behavior: deny, message: "User refused permission to run tool"}` |
| `reject_always` | adds `updatedPermissions` with a session `addRules` deny rule |
| client answers `cancelled`, or the turn was cancelled | `{behavior: deny, interrupt: true}` |

**`session/cancel`** sends the `interrupt` control request. The child stops streaming and ends with `error_during_execution` / `aborted_streaming`, which the adapter reports as `stopReason: cancelled`. SIGINT is not used.

## 6. Tests

`cargo test` in `agents/claude-acp`: 18 tests.

| File | Tests | What it covers |
|---|---|---|
| `src/*` | 8 unit | Version parsing and refusal, discovery order and absolutizing, tool mapping, tool results, the four permission outcomes, the MCP config |
| `tests/golden.rs` | 2 | Feeds every recorded message through the translator and compares the full ACP output, line by line, with `tests/fixtures/*.acp.jsonl`: 97 lines for the session (81 message chunks, 3 tool calls, 6 updates, turn ends `end_turn`, `end_turn`, `cancelled`), plus the logged-out session (`auth_required`). |
| `tests/conformance.rs` | 8 | The real adapter binary, driven by brief 0005's `AcpClient`, against `niello-fake-claude` replaying the redacted recording (listed below) |

The 8 conformance tests:

- `recorded_session_full_mapping_permissions_and_cancel`: the whole three-turn session. It checks:
  - the `initialize` response;
  - the permission request for the MCP tool, with server meta and four options, allowed once;
  - the ToolSearch and MCP tool calls, with content and raw output;
  - the answer text;
  - the Write permission request, with title, kind, diff and the "allow all edits" label, denied;
  - the tool call failed, the denial reported, and no file created;
  - cancel after the first chunk, ending `cancelled`;
  - on the child side: every flag, the cwd, no leaked session variables (the test sets `CLAUDECODE` and `CLAUDE_CODE_ENTRYPOINT`), the exact MCP config, the behaviours `allow` then `deny`, the control requests `initialize` then `interrupt`, and that the temporary config is gone.
- `allow_always_and_reject_always_carry_updated_permissions`: the child's own suggestion is passed back for allow-always; reject-always sends a session deny rule.
- `cancel_while_permission_pending`: interrupt first, then the client's `cancelled` answer becomes `{deny, interrupt: true}`, and the prompt ends `cancelled`.
- `logged_out_claude_is_auth_required`: `-32000`, no stray text, and the login method shown as `niello-claude-acp auth login`.
- `refuses_claude_older_than_validated`: 2.1.200 is refused with the message, and no child is started.
- `missing_claude_is_a_clear_error`.
- `model_comes_from_the_environment_never_hard_coded`.
- `runs_with_only_claude_on_path`: the automated no-Node proof. `PATH` is one directory holding a `claude` symlink to the fake, and a full turn runs.

**Fixtures** (`tests/fixtures/`):

- `claude-2.1.287-session.jsonl`: 207 records, both directions.
- `claude-2.1.287-logged-out.jsonl`.

`tools/redact.py` replaced:

- the session id with `{{SESSION_ID}}`;
- the cwd with `{{CWD}}`;
- message, request, tool-use and event ids with placeholders;
- thinking signatures with `REDACTED`.

It removed the initialize reply's account (email, organization, plan), rate-limit figures, memory paths, sockets, and the owner's skills, plugins, agents and commands. It fails if the home directory or user name remains.

The suite ran 10 times in a row without a failure.

**`cargo test -p niello-acp`:** 11 tests (4 unit, 7 integration). The new unit tests are:

- `npx_adapter_is_the_fallback`;
- `native_adapter_is_preferred_when_found` (configured path, then beside the IDE, then `PATH`; native first, npx second);
- `non_executable_files_are_skipped`;
- `terminal_auth_command_appends_method_args`, now covering both adapters.

**Root workspace:**

- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings` and `cargo test --workspace` are clean, with the shared target dir.
- The adapter package passes its own `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings` and `cargo test`.
- The spike's own `cargo test` (7 tests) passes against the changed `crates/acp`.

**Not verified against the real CLI:**

- Reject-always's `updatedPermissions` on a deny. Whether 2.1.287 honours a rule sent with a deny was not exercised; it would take one more real prompt.
- HTTP MCP servers. The config translation is tested, but no HTTP server was run through `claude`.

## 7. Session resume: deferred, sized

**Finding.** `claude --print --resume <id>` with stream-json was run on the recorded session, sending only `initialize`. It returns the handshake and **replays nothing**.

ACP `session/load` requires the agent to stream the whole history as `session/update` notifications before it responds. The Node adapter does this from Claude Code's on-disk transcript; it has a `resumed-session` module.

**Work for a follow-up brief:**

- a `session/load` handler that spawns the child with `--resume <id>` (and optionally `--fork-session`);
- a reader for `~/.claude/projects/<cwd-slug>/<id>.jsonl`, an undocumented format and so another drift surface, which replays user text, assistant text and tool calls through the existing translator;
- `loadSession: true` in `initialize`;
- a recorded transcript fixture with redaction, and tests.

**Estimate:** 300 to 400 lines plus tests, about one agent-day. One real prompt is needed to record a fixture.

Sessions started by the adapter are persisted by `claude` today, because `--no-session-persistence` is not passed. A later resume, or the user's own `claude --resume`, can therefore find them.

## 8. Manual run: the brief 0005 panel, unchanged, no Node

**Setup.**

- `spikes/0005-acp-panel` was built from this worktree (`cargo build --release`, shared target dir). Its source and `Cargo.lock` are unchanged.
- A symlink to the release adapter was placed beside the panel binary, in `target/release/`. This is the "ships beside the IDE" case, and `default_agents().remove(0)` returned it. The symlink was removed afterwards.
- The panel was started through a wrapper that sets `PATH` to a directory containing only `claude -> ~/.local/bin/claude`.
- In that directory, `command -v node`, `npm` and `npx` all report not found. Even `bash` is not found.
- The adapter log line confirms the discovery: `claude 2.1.287 at <dir>/nonode/bin/claude (Path)`.

**Run 1 (diagnostics).** `tools/shots.sh RUN -- --cwd <empty dir> --prompt "List the current errors in the Error List and tell me which file has the most."`

- Header: `Claude Code ready (niello-claude-acp 0.1.0, ACP v1). MCP: niello via stdio relay to 127.0.0.1:34551`.
- **User:** List the current errors in the Error List and tell me which file has the most.
- **Agent:** I'll pull the IDE's diagnostics list and tally errors per file.
- **Tool call** `ToolSearch` (other), completed. Arguments `{"query": "select:mcp__niello__diagnostics-list", "max_results": 1}`. Result `Tool: mcp__niello__diagnostics-list`.
- **Tool call** `mcp__niello__diagnostics-list` (other), completed. Arguments `{"severity": "error"}`. Result: the four errors, as `{"result":[...]}` (OrderController.cs lines 42, 57, 88; PricingService.cs line 15).
- **Permission** `mcp__niello__diagnostics-list` `{"severity":"error"}`. Options: Yes / Yes, and don't ask again for Diagnostics-List / No / No, and don't ask again for Diagnostics-List. The panel showed "Allowed without prompt: `diagnostics.list` is class read".
- **Agent:**
  > The Error List currently shows 4 errors. **OrderController.cs has the most**, with 3 of the 4.
  >
  > (A table of the 4 errors follows.)
  >
  > The three in `OrderController.cs` look independent: an undefined variable, a missing semicolon, and a string-to-int type mismatch. The one in `PricingService.cs` is a missing type reference, likely a missing using directive or project reference.
- Turn ended: `EndTurn`. Audit lines: `[audit] permission auto-allowed: diagnostics.list is class read` and `[audit] mcp tools/call diagnostics-list -> Some("diagnostics.list") class=Some(Read) args={"severity":"error"} ok=true 0.02 ms`.

**Run 2 (denial).** The same, with `--auto-answer deny --prompt "Create a file named notes.txt in the current directory containing the word hello."`

- **Agent:** I'll create the file now.
- **Tool call** `Write` (edit), pending, with a diff for `<cwd>/notes.txt`.
- **Permission** Write, options: Yes / Yes, allow all edits during this session / No / No, and don't ask again for Write. "No" was pressed.
- **Tool call** `Write notes.txt` failed: "User refused permission to run tool".
- **Agent:** The write to notes.txt was denied, so the file was not created. Nothing else is pending on this task. ...
- Turn ended: `EndTurn`. The working directory is empty: **no file exists**.

Screenshots are in `agents/claude-acp/screenshots/`:

- `linux-native-diagnostics.png`
- `linux-native-permission-prompt.png`
- `linux-native-permission-denied.png`

In the last two, the temporary working-directory path is painted over as `<test dir>`.

## 9. Budgets

**Session ready.** Measured from the adapter's `initialize` to its `session/new` response. This includes discovering `claude`, `claude --version`, spawning `claude`, and the `initialize` handshake. No prompt was sent.

| Run | Result |
|---|---|
| Warm, `examples/bench ready` (5 runs) | 440, 451, 453, 594, 429 ms (median 451) |
| First run of the series, after some minutes idle | 860 ms |
| "Cold": `posix_fadvise(DONTNEED)` on the `claude` binary and the adapter first (3 runs) | 442, 448, 442 ms |
| In the panel, `--bench-ready` (6 runs) | 469, 440, 444, 526, 474, 444 ms from session start (spawn plus `initialize` plus `session/new`). `initialize` alone: 1.5 to 2.6 ms. |

The eviction did not make the run cold: the same 244 MB `claude` binary is mapped by the Claude Code session that ran this brief, so its pages stay resident. A true cold start, first launch after boot, was not measured. The closest figure is the 860 ms first run.

For comparison, brief 0005's Node adapter took 496 ms to `initialize` and 952 ms to session ready. The native adapter spends almost all of its ready time inside `claude`'s own startup.

**Memory and hot path.** `examples/bench stream`: the fake `claude` streams 2000 text deltas at 200/s (10 s), and the adapter forwards each as `agent_message_chunk`.

| Run | Adapter CPU | RSS |
|---|---|---|
| 200/s, run 1 | 110 ms (1.1 %) | 6452 KiB before, 6496 to 6500 KiB during, 6500 KiB after; HWM 8.2 MB |
| 200/s, run 2 | 100 ms (1.0 %) | similar |
| 1000/s (10,000 chunks) | 3.4 % | 6.5 MB |

Each chunk costs one line parse and one notification serialization, a few allocations freed at once. Nothing grows: RSS stayed flat to within 50 KiB over the stream. The panel process's RSS was about 75 MB in the 0005 harness, which is unchanged and not the adapter's.

**Binary size.** 2,303,616 bytes (`cargo build --release`: thin LTO, one codegen unit, `strip = true`). It links only to libc and libgcc_s.

## 10. Exit criteria

| # | Criterion | Result |
|---|---|---|
| 1 | Builds on Linux with `cargo build --release`, runs with no Node on PATH, passes the conformance tests | **Pass.** Release build 2.3 MB; no-Node run in sections 6 and 8; 18 tests pass. |
| 2 | The brief 0005 panel works unchanged: tool call rendered, correct answer, permission prompt and denial honoured | **Pass** (section 8). |
| 3 | `default_agents()` prefers the native adapter when present and falls back to npx, with tests | **Pass** (sections 3 and 6). |
| 4 | Report documents every stream-json message type and control request subtype observed, the fields depended on, the version validated, and a drift watch list | **Done** (section 4). Validated: `claude` 2.1.287. |
| 5 | Resume implemented or deferred, and sized | **Deferred**, sized in section 7. |
| 6 | Risk 9 addressed factually | **Done** (section 11). |

## 11. Subscription terms (PLAN.md risk 9)

**What the official Node adapter does.**

- `@agentclientprotocol/claude-agent-acp` (Apache-2.0) runs the Claude Agent SDK (`@anthropic-ai/claude-agent-sdk`, under Anthropic's own license).
- The SDK spawns a Claude Code CLI binary. By default this is the native binary that npm installs as a platform-specific dependency of the SDK package (`claudeCliPath()` in the adapter). Setting `CLAUDE_CODE_EXECUTABLE` makes it use another one.
- It drives that binary in the same headless mode: stream-json in both directions, partial messages, `--replay-user-messages`, permission prompts over the stdio control channel, and user, project and local setting sources.
- The CLI uses the user's own login, from `claude` or the adapter's `--cli auth login`.

**What this adapter does differently.**

- **At the protocol level, nothing.** It starts the same kind of CLI process in the same headless mode, with equivalent flags and control requests, under the same user login.
- It runs **the user's installed `claude`**, not a copy downloaded with the SDK.
- It contains and runs **no Agent SDK code**. The wire protocol was implemented from observation of the CLI and from its public CLI reference.
- It reads, stores, logs and forwards **no credentials or account data**. It ignores the account block in the `initialize` reply, except whether a login exists, and drops `rate_limit_event`.
- It removes `CLAUDE_CODE_ENTRYPOINT` and the other session variables and sets no entry-point value of its own. The Node adapter's source sets none either; whether the SDK package does, and how the CLI then labels this traffic, was not checked.

**What remains for the owner to confirm against Anthropic's terms.** These are brief 0005's questions, sharpened:

1. Whether a third-party program (Niello, or another editor using this MIT adapter) may start and drive the user's installed Claude Code CLI headlessly under a consumer or organization subscription login (Pro, Max, Team, Enterprise). The owner's case is an organization plan.
2. Whether the Agent SDK's terms attach to the headless stream-json protocol itself, or only to the SDK package. Put another way: does avoiding the SDK change anything, or is the relevant fact that a program, not a person, drives the session?
3. Whether such an integration should identify itself to the CLI, for example with an entry-point or client name, and with what value.
4. Whether usage through the adapter counts toward the plan's limits like interactive use. The CLI reports plan utilization in `rate_limit_event`, which the adapter does not show.
5. Whether Niello may name Claude Code and document "use your Claude subscription" for this adapter, and distribute the adapter as a separate MIT binary for other editors.

## 12. Deviations and notes

- **Real prompts:** five instead of three or four (section 2). Three were needed to record the `claude` side before the adapter existed; the panel then needed two.
- **Commit trailers:** written by hand. `Signed-off-by` comes first, then the session's attribution trailers last, as brief 0005 did.
- **Not touched:**
  - `docs/briefs/README.md`'s index row, and the crate map in `CLAUDE.md` and `README.md` (the new `agents/` directory). Both are outside this brief's files; a follow-up should add the row and the map entry.
  - ADR-0003. It is not in scope; the adapter does not change its decisions.
  - The `protocol/` schemas. The adapter is a bridge between two external protocols, so no Niello schema was needed.
- **Workspace membership:** the adapter is its own workspace, as the brief says. Joining the root workspace later will add the ACP crate's dependency tree to the root `Cargo.lock`. The root `Cargo.lock` is unchanged by this brief.
- **Windows:**
  - Discovery looks for `claude.exe` (and `niello-claude-acp.exe` in `crates/acp`).
  - `std::process::Command` quotes arguments for `CreateProcess`, and `claude.exe` is a real executable, so `cmd.exe` quoting rules do not apply.
  - The temporary MCP config file gets no ACL change on Windows; only Unix sets mode `0600`.
  - **Not run.** macOS is also not run.
- **Shared target dir:** a symlink to the adapter was placed in `target/release/` for the panel run and removed afterwards. The spike was rebuilt there from this worktree's crates.
- **Recording leftovers:** the recorded `claude` session exists in the owner's `~/.claude/projects` (a temporary-directory project), as any `claude` session would.
