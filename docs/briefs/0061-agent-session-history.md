# Brief 0061: Agent session history and switching in the Agents window

Status: done on Linux, except the history-list screenshot and the bench's `--sessions N`; see [0061-report.md](0061-report.md)
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 3, 12), 5.2 ("Multiple agents run concurrently"), 8
Depends on: briefs 0057, 0058 and 0059 (the window as it is after them), brief 0047 (the per-workspace state
directory), brief 0016 (the window, the record format of `--transcript-out`)

## Goal

The Agents window keeps every conversation instead of one. A person starts a new session without losing the
current one, switches between the sessions that are running, and comes back to a session from an earlier day: its
transcript is shown from Eludite's own record, and when the agent can resume it (ACP `session/load`) the next prompt
continues it. Sessions live in the person's per-workspace state, never in the repository. The list is a history
button in the window's header, as Visual Studio's Copilot Chat has, and every action is a command on the bus. The
native Claude Code adapter learns to resume a session with `claude --resume`.

Today `Agents` (`crates/eludite/src/shell/agents.rs`) holds one `session: Option<AgentSession>` and one transcript;
Restart discards the conversation; nothing is written to disk except `--transcript-out` for the manual runs.

## Files in scope

- `protocol/schemas/` first and alone: `agents-sessions.input.json` and `.output.json` (new), `agents-switch.input.json`
  (new), `agents-new-session.input.json` (new), `agents-state.output.json` (`session`), `agents-start.input.json` (the
  `session` option); then `crates/commands/src/agents.rs`.
- `crates/acp/src/protocol.rs` (`LoadSessionRequest`, `LoadSessionResponse`, `agentCapabilities.loadSession` is already
  decoded as `load_session`), `crates/acp/src/client.rs` (`load_session`), `crates/acp/src/session.rs` (`AgentSession::resume`,
  the replay's updates as `SessionEvent::Replay`), `crates/acp/src/fake_agent.rs` (`--load`: advertises `loadSession`,
  answers `session/load` for an id it was given earlier in the same process or any id with `--load-any`, replaying two
  message chunks first).
- `agents/claude-acp/src/agent.rs`, `process.rs` (`Launch::resume`), `translate.rs` (the replay), its tests, a recorded
  fixture (`tools/record.py`, `redact.py`), `README.md`.
- `crates/eludite/src/shell/agents.rs` (sessions instead of a session), `crates/eludite/src/shell/agents/sessions.rs`
  (new: the store), `crates/eludite/src/shell/agents/transcript.rs` (`Transcript::from_json`), `crates/eludite/src/shell/agents/window.rs`
  (the history button, the list, New session), `crates/eludite/src/shell/agents/tests.rs`, `crates/eludite/src/shell.rs`
  only for the status bar slot, `crates/eludite/src/settings.rs` only for `workspace_state_dir`'s reuse.
- `crates/eludite/tools/agents.py` and `agents-linux.sh` (one screenshot), `docs/briefs/README.md`, this file,
  `docs/briefs/0061-report.md` (new).

## Contract

### Sessions in the shell

- A session is `{id, agent, acp_session_id?, title, started, last_activity, model?, live}`. `id` is Eludite's own
  (UUID v4), `acp_session_id` the agent's once `session/new` or `session/load` answered, `title` the first prompt's
  first 60 characters (one line, "New session" until a prompt is sent), `live` while the agent process runs.
- `Agents` keeps every live session at once (`BTreeMap<SessionId, LiveSession>`, each with its `AgentSession`,
  generation, transcript, usage, options, waiting permissions and the audit name), and one `current`. Events from
  every live session keep applying to their own transcript off screen; the window renders the current one. Pending
  changes keep their global ids and gain the session they belong to; the review view is unchanged. Permission
  prompts of a session that is not current are answered only after switching to it; the history list shows a badge
  (`?`) on a session waiting for an answer and a dot on a running one.
- Memory: at most 8 live sessions; starting a ninth stops the oldest idle one (its record stays). A stopped or
  exited session is not live; its transcript is kept in memory until the shell closes and always on disk.

### The store

- `<workspace state dir>/agents/sessions/<id>.json` (brief 0047's `workspace_state_dir`, mode 0600, the folder
  0700 on Unix): `{"version": 1, "id", "agent", "acp_session_id", "title", "started", "last_activity", "model",
  "ended": bool, "usage": {used, size, cost?}, "transcript": <the record of brief 0016's --transcript-out, extended
  by `Transcript::to_json` as of brief 0059>}`. Written when a turn ends, when a permission is answered, when the
  session stops or the agent exits, and at most once per 2 s otherwise, from a background task. The 100 most recent
  by `last_activity` are kept; older files are deleted when a new one is written. Nothing is written for a session
  with no prompt yet.
- `Transcript::from_json` rebuilds the rows from the record: prompts with their time, agent text as Markdown,
  thoughts (collapsed), tool rows as completed, denied or failed with their arguments and result, plans, usage lines,
  notices; the change links read `Change #n: name (accepted)` from the record and still open the review view when the
  change is known to the shell, else do nothing. The round trip `to_json(from_json(r)) == r` holds for every record the
  tests produce.
- The policy file, the audit log and the review board are unchanged: a restored transcript never re-runs anything.

### Switching and resuming

- `eludite.agents.sessions` (`{}` → `{current, sessions: [{id, agent, title, started, last_activity, model?, live,
  running, waiting}]}`, newest first; class `read`).
- `eludite.agents.switch` (`{session}`): makes it current. A live session shows at once. A stored one: when the
  agent that owns it is in the registry and advertises `loadSession`, the shell starts the agent and runs
  `session/load` with the stored `acp_session_id`, the same `cwd` and MCP servers as `session/new`; the replay the
  agent sends (ACP: `user_message_chunk`, `agent_message_chunk`, tool calls) is counted and discarded, since the
  shell's record is richer (it carries the review links and audit numbers), unless the shell has no record for the
  session, in which case the replay builds the transcript. The state is `ready` after the load, and the next prompt
  continues the conversation. When the agent cannot load sessions, the transcript shows with a notice "This agent
  cannot resume a session; start a new one to continue" and the prompt box is disabled; `eludite.agents.start` with
  `restart: true` then begins a new session with the same agent.
- `eludite.agents.new_session` (`{agent?}`): a new live session with the selected (or named) agent, made current;
  the previous one keeps running. `eludite.agents.start` keeps its meaning (start or restart the current session)
  and gains `session?` to act on another.
- `eludite.agents.prompt`, `cancel`, `permission`, `configure` act on the current session; `permission` also
  accepts a request key of another session (keys stay unique across sessions).
- `agents-state.output.json` gains `session: {id, title, started, last_activity, live}`.
- Startup: the window opens on no session; the history list shows the stored ones; the status bar's agent slot reads
  `Claude Code: ready · Sonnet 5.5 · <title>`.

### The native adapter

- `initialize` answers `loadSession: true`. `session/load {sessionId, cwd, mcpServers}` spawns `claude` with
  `--resume <sessionId>` instead of `--session-id` (verified: `claude --help` on 2.1.289 lists `--resume <session-id>`,
  "continues that session in the background under the same ID, or starts a copy and says so when the session is
  already running"), writes the MCP config as `session/new` does, runs the `initialize` control request, and replays
  the conversation from Claude Code's own session file when it can read it (`~/.claude/projects/<cwd with / and .
  replaced by ->/<sessionId>.jsonl`; user text as `user_message_chunk`, assistant text as `agent_message_chunk`, tool
  uses as completed `tool_call`s; anything it cannot map is skipped). A missing file replays nothing. The response
  carries `modes` and `configOptions` as `session/new` does. A `--resume` that `claude` refuses fails the load with the
  message.
- `session/load` for an id the adapter already has live is `invalid_params`.

### The window

- Header, after the state: a history button (`agents-history`, the clock glyph, tooltip "Sessions") and a New
  session button (`agents-new`, `+`, tooltip "New session"). The history list (`agents-history-menu`, rows
  `agents-session-<id>`) shows, newest first, each session's title, agent and relative time (`2 min ago`, `yesterday`,
  else the date), a dot while it runs and `?` while it waits; the current one checked; Up, Down, Enter, Escape. A
  click runs `eludite.agents.switch`. At most 50 rows; "Older sessions are on disk" when there are more.
- The agent picker's choice starts a new session on `Start` when no session is current; with a current session the
  picker is the session's agent and changing it runs `new_session` with that agent (the old session keeps running).
- The transcript list scrolls to the end on a switch; the prompt box's text is per session (kept with the live
  session, empty for a stored one).

## Proving test

- `crates/eludite` `agents::tests` with the fake agent: two live sessions with `stream` (300 chunks) and `edit`;
  switching mid-stream keeps both streaming (the off-screen one's row count grows while the other is shown); a
  permission request in the off-screen session shows `?` in the list and is answered after switching; `sessions`
  lists both with the right titles and flags; the store has one file per session with a prompt after each turn, mode
  0600; `Transcript::from_json(to_json(t)) == t` for the edit scenario's transcript; a new `Shell` on the same state
  directory lists the stored sessions; switching to a stored session of a `--load` fake agent runs `session/load`
  with the stored id, the replay is discarded (counted in the event trace) and the next prompt continues; switching
  to a stored session of an agent without `loadSession` shows the notice and a disabled prompt box; the 101st
  session deletes the oldest file; a ninth live session stops the oldest idle one.
- `crates/acp`: `session/load` round trip against the fake agent; the `Replay` events.
- `agents/claude-acp`: a fixture recorded from `claude` 2.1.289 with `--resume` of a session started earlier in the
  same recording (two `initialize` control exchanges, no model call), replayed through `fake_claude`; the session
  file reader on a checked-in sample (redacted) producing the replay updates; a missing file replaying nothing.
- `crates/commands`: the three new specs validate their schemas.
- Manual, one screenshot (`linux-agents-history.png`): the history list open with three sessions, one running.

## Budget

- Switching between two live sessions draws the new transcript within one frame; a stored transcript of 2,000 rows
  rebuilds in under 50 ms (debug) and draws within one frame after.
- Writes never block the UI thread (the record is serialized on the UI thread, written on a background task;
  serializing a 2,000-row record under 10 ms).
- Eight live `stream` sessions at 200 chunks per second each keep frame p99 under 8 ms with one on screen
  (`--bench-agent-stream` gains `--sessions N`).
- No new dependency.

## Exit criterion

1. Every test above is green; fmt, clippy and the workspace tests pass.
2. The screenshot exists; the report gives the rebuild and write timings, the fixture's `claude --resume` behavior
   (whether it continued or copied the session), and what the replay from Claude Code's session file could and could
   not map.
3. `docs/briefs/README.md` has this brief's row; the adapter README names `--resume`.

## Out of scope

- Sharing sessions between workspaces, exporting or importing them, deleting them from the window (the files can be
  deleted by hand; a later brief adds Delete).
- Renaming a session (the title is the first prompt).
- Background agents in worktrees and the review queue (PLAN.md 5.2, 5.6, Phase 4).
- Resuming through the Node adapter (it advertises `loadSession`; if it works unchanged, say so in the report).
