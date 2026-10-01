# Brief 0016 report: the Agents window in the production shell

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0016-agents-window`, rebased on `origin/main` at `77977e9` (main moved by two commits during the work:
the Solution Explorer window became Workspace, and the applier's line endings for created files). Date: 2026-10-02.
Brief: [0016-agents-window.md](0016-agents-window.md).

## 1. Summary

- **The manual flow works with the real native adapter** (`eludite-claude-acp` driving `claude` 2.1.287) in
  `dotnet/Eludite.slnx`, driven with real XTest keys and clicks in a nested KWin (section 6):
  - Ctrl+\, Ctrl+C shows the Agents window. "List the current errors and fix the first one": after two shell
    commands the driver denied, Claude called `mcp__eludite__diagnostics-list` (allowed without a prompt, class
    read), read the file, and proposed its fix with its own `Edit` tool. The edit was held as a pending change and its review view opened at the change (`-` / `+` line
    70). Accept applied it through the brief 0015 applier as one undo step; the Error List went from 1 error to 0 in
    584 ms; Claude checked `diagnostics-list` again and reported the fix.
  - "Run the shell command touch eludite-denied.txt in the solution folder": the permission prompt appeared in the
    window (`Bash`, class execute, Allow / Always Allow / Deny); Deny; Claude reported the refusal; no file exists.
  - Four real prompts in total (the flow ran twice: the second time after the review view learned to open at the
    first change). `dotnet/` was restored with `git checkout -- dotnet/`.
- **Budgets** (release build, nested KWin at 60 Hz, Wayland backend; load average 2.2 to 2.4 during the benchmarks):

  | Budget | Result |
  |---|---|
  | Window open to agent ready < 1.5 s warm | First frame 136 to 166 ms after `main`, then session start to ready **p50 489 to 512 ms** (15 starts: 441 to 789 ms), so window open to ready is about **0.65 s** (worst 0.95 s). Pass |
  | UI-thread frame cost < 8 ms p99 at 200 chunks/s | **p99 1.84 to 1.91 ms** (p50 1.0 ms, max 2.5 ms; 540 to 554 frames per 10 s run); applying a batch of events p99 0.035 to 0.052 ms; chunk send to applied p99 2.7 to 3.0 ms. Pass |
  | 2000-line diff renders < 50 ms | Edit to the first presented frame showing the diff (diff computed off the UI thread, then drawn): **p50 16.0 to 16.2 ms, p95 17.9 to 24.8 ms**; the worst of 60 runs 68 ms (p99 of one run). Pass at p95; see section 7 |

- **Tests:** `cargo test --workspace` 347 passed, 0 failed, 1 ignored (main before this brief: 304). 18 headless and unit
  tests in the Agents window, 6 session tests in `eludite-acp`, 4 new MCP conformance and boundary tests, the relay
  binary's 2 integration tests. fmt and clippy (`-D warnings`) clean; every commit passes them. The adapter package's
  own 18 tests pass unchanged.
- **New dependency:** `getrandom` 0.3 (MIT OR Apache-2.0) in `eludite-mcp`, for the endpoint token from the OS random
  source; it was already in the build through GPUI, so `Cargo.lock` gained an edge, not a crate. `tempfile`
  (MIT OR Apache-2.0, already a workspace dependency) became a dev-dependency of `eludite-acp`.

## 2. What was built

Commits, in order (each passes fmt, clippy and the workspace tests):

1. `d26778f` `protocol/schemas/` alone: `agents-start.input`, `agents-prompt.input`, `agents-cancel.input`,
   `agents-state.output` (shared by the three), `agents-permission.input/output`, `agents-review.input/output`,
   `solution-tree.input/output`, `command-spec.json` (the spec with `agent_visible`), `mcp-tool.json` (the MCP
   descriptor export), `agents-policy.json` (the policy file) and `agents-settings.json` (user-added agents).
2. `ab0d7f7` `eludite-commands`, `eludite-mcp`, `eludite-acp` to production (sections 2.1 to 2.3).
3. `c3d908c` The MCP endpoint inside the `eludite` process and `eludite --mcp-relay ADDR`.
4. `f6bbbc5` The Agents tool window: registry, sessions, transcript, prompt box.
5. `881b23a` Permissions: the policy file, the prompt, read/edit/execute/dangerous.
6. `65e2711` Pending changes: capture, the review view, accept and reject through the applier, the gutter marker,
   the agent's answer.
7. `1e432a1` The audit link from transcript rows to the audit entry and the applied change.
8. `2f1add7` `eludite.agents.*` and `eludite.solution.tree` on the bus; Ctrl+\, Ctrl+C; View > Agents.
9. `ead9ff6` The headless tests (and the fix they found: a login state that arrived before the handshake's end was
   overwritten by "ready").
10. `eed983a` The manual-run tooling, the benchmarks, the run's results and screenshots.
11. `1ecb95b` main's rename followed (the solution tree's title and schema say Workspace); then this report, the
    brief's Status line and the spike's README.

Every commit was re-checked after the rebase onto main (`git rebase -x`: fmt, clippy, tests; 304 to 347 tests).

### 2.1 The command bus (`crates/commands`)

- `CommandRegistry` registers from any thread at any time (`&self`, an `RwLock` of `Arc` entries; no lock held while a
  handler runs) and counts registrations (`generation()`), so the MCP list follows the bus.
- `CommandSpec.agent_visible`: true for the code-reading and code-editing commands, `diagnostics.list`,
  `eludite.solution.tree`, `eludite.help.about` and the `eludite.agents.*` commands; false for the window layout
  (`eludite.view.*`), the Error List's view filter, navigation history, committing the completion popup's selection,
  and closing the solution.
- `Caller` (user, or agent with its name, a process-unique call id and the agent's tool call id), set by the MCP
  boundary with `with_caller`; the shell's job queue carries it to the UI thread.
- `AuditLog` entries gained `seq`, the caller, an agent's arguments, and `edits` (each pending change and what became
  of it), joined by `attach_edit(call, record)`. Denied calls and the agent's own tool calls are recorded too.
- `policy` (the policy file, section 5), `agents` (the five commands) and `solution` (`eludite.solution.tree`).

### 2.2 MCP (`crates/mcp`)

- `McpServer::new(registry)` advertises `registry.agent_visible()` on every `tools/list` (`with_only` keeps an
  allow-list for the fixture example); `initialize` says `listChanged: true`, and a connection sends
  `notifications/tools/list_changed` within 200 ms of a registration after it listed the tools.
- Descriptors carry `_meta: {"eludite/command", "eludite/permission"}` (`protocol/schemas/mcp-tool.json`).
- Read runs; every other class goes to the `PermissionGate(spec, args, CallContext)`, which may block its thread;
  an `Invoker` hook runs allowed calls (the shell's holds edits for review); denied calls are audited.
- Each `tools/call` runs on its own `mcp-call` thread, so a call waiting for the user does not hold up the
  connection (replies may be out of order).
- The token is 128 bits from `getrandom`. The agent's tool call id is taken from the request's `_meta`
  (`claudecode/toolUseId`).

### 2.3 ACP (`crates/acp`)

- `session::AgentSession`: the spike's driver promoted. `acp-driver` (spawn, `initialize`, `session/new`, prompts),
  `acp-reader`, `acp-stderr`, and one `acp-writer` thread for every write (permission answers, cancel, shutdown), so
  a full pipe never blocks the UI. States starting, ready, needs login (from `_auth/status_update` or `-32000`),
  running, error, exited; events stamped with the session generation. `PermissionPolicy` answers requests on the
  reader thread; `answer(key, kind)` and `cancel()` (which answers pending requests `cancelled`).
- `settings`: the registry: native adapter, npx adapter, then `agents.json` (a configured agent with a built-in's name
  replaces it; `default` moves one first).
- The fake agent gained `edit` (a thought, a plan, then `eludite.workspace.apply_edit` through MCP), `write` (its own
  `Write` with a diff), `exit` (exits mid-turn) and a direct-TCP mode for in-process tests.
- `ToolCall::diffs()` and `SessionUpdate::plan_entries()` read the `diff` content and `plan` updates without new
  enum variants (decoding stays tolerant).

### 2.4 The shell (`crates/eludite`)

- `shell/agents.rs`: the registry (searched off the UI thread at startup), sessions, the event pump (one batch per
  wake-up), the policy store, the MCP gate and invoker, the `eludite.agents.*` target, and the status bar slot
  (`Claude Code: ready`).
- `shell/agents/window.rs`: the Agents window (docked right, tabbed with Workspace and Git Changes): agent picker,
  state, Start/Restart, login instructions, the virtualized transcript, the permission prompt (a dialog panel in the
  window with Allow, Always Allow, Deny), the pending changes list (Accept, Reject, Accept All, Reject All), and the
  prompt box (Enter sends, Shift+Enter new line, Escape cancels the turn). Every action runs an `eludite.agents.*`
  command.
- `shell/agents/transcript.rs`: rows for prompts, agent text (one row per line), thinking (collapsed), tool calls
  (name, kind, arguments, result, status: pending, running, allowed without prompt, awaiting permission, awaiting
  review, denied, failed, completed; and a note with the permission decision, the command and its audit entry, and
  the changes), plans, notices.
- `shell/agents/review.rs`: pending changes, the review view (a document tab `Review: File.cs`, inline diff,
  virtualized, opening at the first change), the gutter marker, accept and reject, the board the MCP threads wait on.
- `shell/agents/endpoint.rs`: the loopback endpoint and the stdio server descriptor (the token in the relay's
  environment, never on its command line).
- `eludite --mcp-relay ADDR`, and the harness flags `--agent`, `--transcript-out`, `--bench-agent-ready N`,
  `--bench-agent-stream PATH`, `--bench-diff N`.
- `crates/ui`: `transcript` (cards, badges, thinking block, plan) and `diff` (a Myers line diff with a work bound,
  hunks, diff rows); `crates/docking`: the `agents` window id; Ctrl+\, Ctrl+C and View > Agents.

## 3. The interception boundary

What an agent can change, and what Eludite does about it:

| Agent action | What Eludite does |
|---|---|
| Eludite's `eludite.workspace.apply_edit`, `eludite.editor.rename` (with `apply`), `eludite.editor.apply_code_action` through MCP | **Pending change.** The command runs as the agent; the applier's edit is captured instead of applied, split per file; the agent's MCP call waits on its own thread until every file is decided, then answers in the command's own output shape (state, summary, "Accepted by the user: A.cs. Rejected by the user (discarded, not applied): B.cs."). |
| The agent's own file tool when it asks permission with a diff (ACP `session/request_permission`, kind `edit`, `diff` content): Claude's `Write`, `Edit`, `MultiEdit` | **Pending change.** The request is held until review. Accept answers allow once (the agent writes the file itself) and, for a file open in an editor, brings the buffer to the same text through the applier as one undo step; Reject answers reject, and the agent is told "User refused permission". |
| Eludite's other edit-class commands (`eludite.editor.save`, `undo`, `redo`, `eludite.file.close`) through MCP | **Prompt** (not diffable): the window's permission prompt, unless the policy says `edit_buffer: accept`. |
| Execute and dangerous Eludite commands through MCP (`eludite.solution.open`, `eludite.agents.permission`, `eludite.agents.review`) | **Policy, else prompt** at the MCP boundary. |
| The agent's own tools that ask (Bash and others of kind `execute`, `fetch`, `delete`, edits without a diff) | **Policy, else prompt** at `session/request_permission`. |
| Tools the agent runs without asking: Claude's `Read`, `Glob`, `Grep`, `ToolSearch`, commands the user's own Claude Code allow rules permit, every edit under `acceptEdits` or `bypassPermissions`, and anything a shell command writes (`sed -i` inside an allowed `Bash`) | **Only audited.** The transcript shows the call as it streams, and the audit log records it with its arguments and outcome once it ends. The write itself is the agent's; Eludite cannot hold it. Open buffers are not reloaded from disk (no file watching in the shell yet). |

Claude's adapter sends `session/request_permission` for every MCP tool; Eludite answers those for its own tools at once
("Eludite checks X (class C) at its MCP boundary"), so a call is judged once, by the gate, and the user is never
asked twice. Other ACP agents that do not ask for MCP tools meet the same gate.

The run confirmed the last row: Claude's `Read` and `ToolSearch` never asked and appear as audited rows.

## 4. The policy file

`.eludite/agents-policy.json` beside the solution, committable ([schema](../../protocol/schemas/agents-policy.json)).
Missing means the defaults. Example after one Always Allow:

```json
{
  "rules": [
    {
      "command_prefix": "rm -rf obj/",
      "decision": "allow",
      "tool": "Bash"
    }
  ],
  "version": 1
}
```

- `version` (1). `edit_buffer`: `review` (default) or `accept`. `execute`: `prompt` (default), `allow` or `deny`.
  `dangerous`: `prompt` (default) or `deny`. Class read always runs and is not configurable.
- `rules`: checked in order for execute and dangerous calls; the first match decides (`allow` or `deny`). `tool` is the
  agent's name for the tool (`Bash`, `WebFetch`), or an Eludite tool with or without the `mcp__eludite__` prefix;
  `command_prefix` (optional) must start the call's `command` argument.
- Always Allow adds a rule with the call's exact command, writes the file off the UI thread with sorted keys (it
  diffs cleanly), and tells the agent "allow once", so the agent keeps asking and the policy keeps deciding.
- The class of an agent's own tool comes from its ACP kind: `read`, `search`, `think` are read; `edit`, `move` are
  edit_buffer; `delete`, `fetch` are dangerous; `execute`, `switch_mode`, `other` and unknown are execute. An Eludite
  tool has its command's class.
- Without an open solution there is no file: the defaults apply and Always Allow is not offered.

## 5. Tests

| Where | New | What |
|---|---|---|
| `eludite-commands` | 8 | caller scoping, audit join, runtime registration from another thread, agent arguments in the audit, the policy (defaults, rules, Always Allow persisted, sorted, validated), the agents commands, the solution tree |
| `eludite-mcp` | 4 (16 in all) | tools/list equals the bus's agent-visible commands and follows `mcp-tool.json` and `command-spec.json`; a command registered at runtime triggers `list_changed` and appears on the next list; a call waiting at the gate does not hold up the connection; the invoker gets the agent's tool call id |
| `eludite-acp` | 7 | settings and registry; `tests/session.rs` with the fake agent as a child process: lifecycle, streaming and permissions (policy and owner), cancel answering pending requests `cancelled`, logged out, exit mid-turn then a new session with the agent's own `Write`, a missing agent, the thought and plan |
| `eludite-ui` | 4 | statuses; the line diff (insertions, replacements, CRLF, minimal middle, 2000 lines fast, a rewrite degrading) |
| `eludite` unit | 7 | transcript rows, thinking, folding; text edits in UTF-16, splitting a workspace edit per file, outputs amended after review; the endpoint |
| `eludite` headless | 11 | below |
| `eludite` `tests/relay.rs` | 2 | the relay binary relays MCP; it refuses without the token and the endpoint hangs up on a wrong one |

The 11 headless GPUI tests run the fake agent in-process over pipes; its MCP calls reach the shell's real loopback
endpoint with the real token:

1. Start, View > Agents, type a prompt and Enter: the read tool runs without a prompt (note and audit entry), the shell
   command waits; the Deny button; the agent's reply; the audit records `diagnostics.list` and `Bash` as the agent's with
   their arguments, linked from their rows.
2. Always Allow writes the rule into `.eludite/agents-policy.json`; a new session reads it and the same command runs
   without a prompt.
3. An Eludite execute command from an agent prompts at the MCP boundary; Deny; the agent's call gets the reason; the
   denial is audited.
4. An edit of two files (one open, one closed) is held as two pending changes with nothing applied; the review view
   opens; the gutter marks the open file; accept one in its review view, reject the other in the window; the agent's
   answer names both; the audit entry carries both decisions; the transcript's link opens the file at the change (and
   the rejected one's review view); one undo takes the accepted change back.
5. The agent's own `Write` is a pending change with no prompt; Accept lets the agent write it.
6. 400 chunks stream in; the UI answers within 50 ms throughout; one row per line.
7. Escape in the prompt box cancels the turn while a permission request is pending (`cancelled`).
8. An agent that exits mid-turn is an error until Restart, which starts a new generation.
9. A logged-out agent shows its login command; the status bar says so.
10. An outer agent drives the window on the bus from another thread: start, prompt, permission (oldest), review,
    `eludite.solution.tree`; Ctrl+\, Ctrl+C and View > Agents.
11. The shell's endpoint lists exactly the bus's agent-visible commands, and a command registered at runtime appears.

The agents tests passed 6 times in a row.

## 6. Manual run (Linux) and screenshots

- **Script:** `crates/eludite/tools/agents-linux.sh OUT_DIR` (driver `tools/agents.py`, X11 backend in a nested Xwayland
  with real XTest keys and clicks; benchmarks on the Wayland backend). It refuses to run unless `dotnet/` is clean,
  puts one error into `HostRpcTarget.cs` (`timestamp` misspelled in `Ping`), and restores `dotnet/` afterwards.
- **Raw results:** [`crates/eludite/results/linux-agents.json`](../../crates/eludite/results/linux-agents.json)
  (both runs' traces and transcripts, the benchmarks, load averages).
- **Real prompts: four** (two per run; the first run's screenshots showed the review view at line 1, so the review
  view now opens at the first change and the flow ran again).

Transcript of the final run (abridged):

- **User:** List the current errors and fix the first one
- "I'll sync with origin, then build the .NET solution": `Bash` `git fetch origin ...` (execute, asked, denied by the
  driver), then `Bash` `dotnet build Eludite.slnx ...` (asked, denied). The owner's global `CLAUDE.md` asks every
  session to sync with origin first, and it reaches hosted sessions (brief 0005 gap 5).
- "The Eludite MCP server exposes a diagnostics list, so I'll read the IDE's current errors from there instead":
  `ToolSearch` (not asked; audited), then `mcp__eludite__diagnostics-list` `{"severity": "error"}`, allowed without
  prompt (class read), 0.04 ms on `mcp-call`: CS0103 `timestmp` at HostRpcTarget.cs 70:37.
- `Read` HostRpcTarget.cs (not asked; audited). `Edit` `timestmp` to `timestamp`: **awaiting review**, change #1;
  the review view `+1 -1` at line 70 (`linux-agents-pending-diff.png`).
- Accept (a real click in the review view): "Accept agent change: 1 edit in 1 file (1 open)" in 0.1 ms;
  `publishDiagnostics` with 0 errors 584 ms after the click; Claude called `diagnostics-list` again (`[]`) and reported
  the fix (`linux-agents-error-cleared.png`: the file opened from the transcript's link at line 70, the Error List
  with 0 errors).
- **User:** Run the shell command touch eludite-denied.txt in the solution folder
- `Bash` `touch .../dotnet/eludite-denied.txt`: awaiting permission, the prompt in the window
  (`linux-agents-permission-prompt.png`); Deny (real click); "The command was declined, so the file was not created"
  (`linux-agents-permission-denied.png`). No file exists.

Screenshots (in [`crates/eludite/screenshots/`](../../crates/eludite/screenshots/)):
[`linux-agents-pending-diff.png`](../../crates/eludite/screenshots/linux-agents-pending-diff.png),
[`linux-agents-error-cleared.png`](../../crates/eludite/screenshots/linux-agents-error-cleared.png),
[`linux-agents-permission-prompt.png`](../../crates/eludite/screenshots/linux-agents-permission-prompt.png), and
[`linux-agents-permission-denied.png`](../../crates/eludite/screenshots/linux-agents-permission-denied.png).

## 7. Budgets and measurements

Method: `agents-linux.sh` with `SKIP_DRIVE=1 RUNS=3` (each run: `--bench-agent-ready 5`, `--bench-agent-stream` with
the fake agent as a real child process, `--bench-diff 20`); release build, Wayland backend in the nested KWin at about
60 Hz. Load average (1 minute) 2.2 to 2.4; the machine also ran a desktop session (Discord, Steam) and the Claude Code
session directing this brief.

| Measure | Run 1 | Run 2 | Run 3 |
|---|---|---|---|
| `main` to first presented frame (ms) | 166 | 136 | 142 |
| Session start to ready p50 / max (ms; 5 starts each: spawn, `initialize`, `session/new` including `claude`'s own startup) | 489 / 518 | 512 / 789 | 492 / 515 |
| `initialize` alone (ms) | 1.5 to 2.9 | 1.6 to 2.9 | 1.4 to 3.0 |
| **Streaming frame work p50 / p95 / p99 / max** (ms; 2000 chunks at 200/s) | 0.96 / 1.39 / **1.89** / 2.51 | 1.02 / 1.36 / **1.91** / 2.18 | 1.04 / 1.38 / **1.84** / 2.42 |
| Apply per batch p99 (ms; about 1.0 event per batch) | 0.035 | 0.045 | 0.052 |
| Chunk sent to applied p50 / p99 (ms) | 0.11 / 2.71 | 0.16 / 2.98 | 0.15 / 2.79 |
| RSS at the end (MiB) | 94.2 | 93.6 | 93.9 |
| **2000-line diff: edit to first frame p50 / p95 / max** (ms; 20 runs) | 16.2 / 24.8 / 68.2 | 16.1 / 21.3 / 39.1 | 16.0 / **17.9** / 32.4 |

- Window open to ready is the first frame plus session start to ready: about 0.65 s at the median, 0.95 s at worst.
  The native adapter spends almost all of it inside `claude`'s own startup (brief 0006: 451 ms).
- Frame work is measured as brief 0005 did: from the Agents window's render to a deferred callback after the frame.
  The shell itself is not re-rendered while text streams: tool windows are cached views, and only the Agents window
  notifies.
- The diff time includes reading the file and computing the diff off the UI thread, and the wait for the next
  presented frame (the p50 of 16 ms is one 60 Hz refresh interval). The worst run of each process was its first
  review view; p95 holds the budget in all three runs.
- The first round of benchmarks (before the review view opened at the first change, load average 3.4 to 3.8) gave the
  same picture: ready p50 492 to 546 ms, frame work p99 1.79 to 1.89 ms, diff p95 19.5 to 27.2 ms.

## 8. Gaps and findings

1. **The interception boundary is the agent's permission requests** (section 3). Claude's own `Read`, allowed `Bash`,
   and edits under the user's allow rules or `acceptEdits` are only audited. Closing it needs the adapter to run
   `claude` with a mode where every edit asks (or ACP's client file system, `fs/write_text_file`, which neither
   adapter uses for writes), a change to `agents/claude-acp` this brief did not need for its flow and did not make.
2. **The user's own Claude Code configuration reaches hosted sessions**: in the run, the owner's global `CLAUDE.md`
   made Claude try `git fetch origin` first (denied). A setting for which sources the adapter loads is still open
   (brief 0005 gap 5).
3. **An accepted agent-tool edit of an open file** leaves the buffer dirty although the agent wrote the same text to
   disk; the shell does not watch files, so a write it did not see (the "only audited" rows) leaves an open buffer
   stale.
4. **Review is per file**, not per hunk; the diff is inline only (the Contract allows side-by-side or inline);
   "explain this hunk" is not built.
5. **An agent's MCP edit call waits for the review without a timeout** (cancelling the turn rejects its changes and
   answers it). `eludite.agents.review` from another thread returns `applying` for a closed file still being written
   instead of waiting.
6. **Capture is one-shot per agent command**: while an agent's rename or code action waits for its server answer, a
   user's applier call in that window would be captured instead (narrow, not seen; documented in `review.rs`).
7. **Prompt box**: plain key handling (no IME, no clipboard, no selection); agent text is shown as plain lines, not
   Markdown.
8. **MCP**: `server/discover` (revision 2026-07-28) is not implemented (Claude falls back to `initialize`); the relay
   does not check that its parent is the agent; a malformed policy file falls back to the defaults with a stderr line
   only.
9. **The audit log is in memory** (persistence is the audit brief's).
10. **The diff budget's worst single runs** (32 to 68 ms) were each process's first review view; p95 is within budget.
11. **`spikes/0005-acp-panel` no longer builds** against the current crates (its README says so and names the commit
    to build it from).
12. **Not updated** (outside this brief's files): `docs/briefs/README.md`'s index row and `CLAUDE.md`'s crate map
    (still accurate). Windows and macOS: not run; the code keeps platform-neutral paths (URIs and normalized ids, the
    relay is the running executable, loopback TCP).

## 9. Sizing the next briefs

**0017a: build with Output and Error List.** What exists now: the bus (agent-visible commands are MCP tools with no
extra work), the permission classes (build is execute: the policy file decides or the window asks), the Error List
with a severity model, the Output window id (an empty body), the host session and its notification path. Missing:
`protocol/schemas` for `eludite.build.solution`, `project`, `cancel` and the host's `eludite/build/*` (start, output
line batches, diagnostics, finished with a summary) first; the host side running `dotnet build` (and Build Tools
MSBuild for legacy projects) with a logger that streams lines and structured diagnostics; the Output window (a
virtualized, append-only text view with panes, Build first) meeting Ctrl+Shift+B to first line under 100 ms; build
diagnostics merged into the Error List next to live ones (VS's "Build + IntelliSense" source filter); the Build menu
items and Ctrl+Shift+B; agents get the typed build result. Estimate: one agent-week, about 3,000 lines with tests
(about this brief's size without the review view); the host's MSBuild logger is the uncertain part.

**0017b: run and debug with netcoredbg.** Missing: `protocol/schemas` for the DAP subset and the `eludite.debug.*`
commands first; `crates/dap` (89 lines today) grown into a client over stdio with the session state machine; locating
netcoredbg (MIT; bundling allowed after its license review) and launch profiles from `launchSettings.json`;
F5/Ctrl+F5/Shift+F5; a breakpoint glyph margin (an editor change, beside the light bulb margin); the Call Stack, Locals,
Watch and Breakpoints windows and the debug toolbar; agent-visible debug commands (set breakpoint, continue, step,
evaluate) with the safety rules of PLAN.md 5.5 (two drivers of one state machine). Estimate: one and a half
agent-weeks; the state machine shared by the user and an agent is the uncertain part. Agent-driven debugging itself
stays out of 0017b.

## 10. How to reproduce

```
cargo test --workspace
cargo build --release -p eludite -p eludite-acp
(cd agents/claude-acp && cargo build --release)
dotnet build dotnet/Eludite.slnx
crates/eludite/tools/agents-linux.sh OUT_DIR            # the manual run (2 real prompts) and RUNS=3 benchmark rounds
SKIP_DRIVE=1 crates/eludite/tools/agents-linux.sh OUT   # benchmarks only (no prompt)
```

On an unlocked desktop: `eludite --solution dotnet/Eludite.slnx`, Ctrl+\, Ctrl+C, type the prompt.
