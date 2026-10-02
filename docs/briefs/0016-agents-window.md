# Brief 0016: The Agents window in the production shell

Status: open
Phase: 1 (Phase 4 preview)
Plan reference: PLAN.md sections 2 (principles 2, 3), 5 (all), 8, 9
Related ADR: ADR-0003
Depends on: briefs 0005 and 0006 (ACP and MCP spikes, the native adapter), 0008 (docking), 0012 (commands and documents), 0015 (the workspace-edit applier; its report sizes this brief)

## Goal

Move agent hosting out of the spikes and into Eludite itself: an "Agents" tool window (VS-style, docked right by default beside Solution Explorer) that runs any ACP agent, with Claude Code through the native adapter first; the IDE's MCP server running inside the `eludite` process and exposing the real command bus (diagnostics, open and edit files, navigation, rename, code actions, solution tree); agent edits landing as pending diffs the user accepts or rejects per file through the brief 0015 applier; the permission model from PLAN.md 5.3; and an audit trail linking every change to the tool call that made it. After this brief, the agent that builds Eludite can work inside Eludite.

## Files in scope

- `protocol/schemas/` first and alone: command schemas for `eludite.agents.start`, `eludite.agents.prompt`, `eludite.agents.cancel`, `eludite.agents.permission` (answer a pending request), `eludite.agents.review` (accept or reject a pending change), plus the MCP tool descriptor export for every command the bus marks agent-visible
- `crates/acp/**`, `crates/mcp/**`: promote the spike code paths to production (streaming, permission requests, session lifecycle, the stdio relay to the in-process endpoint, tool list from the bus with schemas, dot-to-dash naming)
- `crates/eludite/**`: the Agents tool window, the transcript view (text, thinking, tool calls with arguments and results), the pending-change review view, status bar slot, settings for which agents are configured
- `crates/commands/src/**`: permission classes applied at the MCP boundary, agent-visible flag on specs, the audit log joined to applied edits
- `crates/ui/**`: transcript and diff-review widgets
- `crates/docking/**` only to register the window
- `agents/claude-acp/**` only for fixes the integration needs
- `docs/briefs/0016-report.md` (new)

Leave `spikes/0005-acp-panel` in place but mark it superseded in its README; do not delete it.

## Contract

- Agent registry: the native adapter when found (beside the executable, `ELUDITE_CLAUDE_ACP`, PATH), the npx adapter as fallback, and any ACP command the user adds in settings; the window shows the agent's name and state (starting, ready, needs login, running, error) and its login instructions when logged out.
- Transcript: streamed text as it arrives, thinking as a collapsed block, tool calls as rows showing name, arguments and result with a status (running, allowed without prompt, awaiting permission, denied, failed), and plan or progress updates if the agent sends them. Prompt box with Enter to send, Escape to cancel the turn.
- MCP server: runs in the `eludite` process on a token-checked loopback endpoint with a stdio relay binary (as the spike did), advertising every bus command flagged agent-visible with its schema; results are the commands' typed outputs. Read-class tools run without a prompt; edit-in-buffer tools produce pending changes; execute and dangerous tools prompt in the window (with the policy file per solution from PLAN.md 5.3, committable, so "always allow" persists per solution).
- Pending changes: an agent's `eludite.workspace.apply_edit` (or an edit it makes through a file tool that the IDE observes) is held as a pending change per file with a side-by-side or inline diff, Accept and Reject per file and Accept All; accepted changes go through the brief 0015 applier as one undo step; rejected ones are discarded and the agent is told; the editor shows a pending-change gutter marker meanwhile. Document which of the agent's own file tools (direct writes bypassing the IDE) can be intercepted and how, and what cannot.
- Audit: every tool call is logged with its arguments, result status and the edit summary; the transcript row links to the applied change; `eludite.agents.*` commands are themselves on the bus so an outer agent could drive an inner one (not exercised).
- Permissions requested by the adapter (`session/request_permission`) map to the same policy and dialog.
- The UI thread never waits on the agent process or the MCP endpoint; prove it with the 200 chunks per second streaming harness from brief 0005.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless tests with the scripted fake ACP agent from brief 0005: start, prompt, streamed transcript rows, a read tool call answered without prompt, an edit tool producing a pending change that is accepted (applied as one undo) and another rejected, an execute tool prompting and being denied, cancel mid-turn, agent exit and restart, logged-out state.
- MCP conformance: `tools/list` reflects the bus, schemas validate, a bus command added at runtime appears on the next list.
- Manual, recorded with three screenshots: in `dotnet/Eludite.slnx` with the real native adapter, prompt "List the current errors and fix the first one"; the agent calls `diagnostics-list`, proposes an edit that appears as a pending diff, the user accepts it, the error clears. Then a prompt that triggers a shell command to show the permission prompt and deny it. Revert the files afterwards.

## Budget

- Window open to agent ready under 1.5 s warm (brief 0006 measured 450 ms for the adapter alone).
- UI-thread frame cost under 8 ms p99 while streaming at 200 chunks per second.
- Pending-change diff view renders a 2000-line file diff under 50 ms.

## Exit criterion

1. The manual flow works with screenshots.
2. Headless and MCP tests green; workspace fmt, clippy, tests green.
3. The report states what the interception boundary is (which agent actions become pending changes, which are only audited), the policy file format, and sizes brief 0017a (build with Output and Error List) and 0017b (debug with netcoredbg).

## Out of scope

- A first-party agent (pinned per PLAN.md decision 5).
- Background agents in worktrees, multi-agent orchestration, the full review queue (Phase 4).
- Agent-driven debugging (needs 0017b).
- Windows and macOS runs.
