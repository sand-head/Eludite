# Briefs

A brief is the unit of delegated work. Work is issued as briefs so that one agent, in one worktree, can finish a task without reading the rest of the repo, and so the human can review a diff and a benchmark delta instead of a transcript (PLAN.md section 11).

## Format

Every brief is `docs/briefs/NNNN-short-slug.md` and has these sections, in this order:

1. **Goal.** One paragraph. What exists when the brief is done that does not exist now.
2. **Files in scope.** The paths the agent owns. The agent edits nothing outside this list.
3. **Contract.** The interfaces, schemas, protocols and versions the work must satisfy. Link to the schema in `protocol/` or the ADR.
4. **Proving test.** The test, command or procedure that demonstrates the goal, and how to run it.
5. **Budget.** The performance and resource limits the work must not regress, from PLAN.md section 9, plus any brief-specific limits.
6. **Exit criterion.** Measurable conditions, each checkable as pass or fail. Include the deliverables to hand back.
7. **Out of scope.** An explicit list of things the agent must not do, even if they look useful.

Optional header lines: Status (open, in progress, done), Plan reference, Depends on.

## Rules

- One brief per git worktree. Name the branch after the brief, for example `brief/0002-eludite-host-roslyn`.
- Briefs are written so a fresh agent with no other context can execute them. Name commands, versions, paths and thresholds.
- A brief that cannot be stated this precisely is not ready to be delegated. It goes back to design, and the result is an ADR or a sharper brief.
- Exit criteria are measurable. "Works well" is not one. "p95 under 50 ms over 1000 runs on the reference machine" is.
- Spike briefs produce throwaway code plus a written report. Production code lands through a later brief.
- If the agent finds the brief cannot be satisfied as written, it stops and reports. It does not widen scope.
- When a brief is done, mark its status and link the PR. Do not delete briefs.
- Follow CLAUDE.md for the definition of done, except where a spike brief says throwaway code is exempt from tests it would not need.

## Index

| Brief | Title | Phase | Result |
|---|---|---|---|
| [0001](0001-gpui-shell-and-docking-spike.md) | GPUI shell and docking prototype, Zed vendoring audit | 0 | [report](0001-report.md): Linux GO (provisional), Windows/macOS pending |
| [0002](0002-eludite-host-roslyn-spike.md) | `eludite-host` with Roslyn, time-to-IntelliSense | 0 | [report](0002-report.md): Linux done, D2 stands, Windows pending |
| [0003](0003-legacy-project-load-spike.md) | Legacy project load, WebForms code-behind IntelliSense | 0 | [report](0003-report.md): Linux done with user-space Mono, 27/29 load, completion passes; Windows pending |
| [0004](0004-icordebug-dap-spike.md) | ICorDebug proof over TCP DAP | 0 | open |
| [0005](0005-acp-claude-code-spike.md) | Claude Code via ACP in a GPUI panel, one MCP tool | 0 | [report](0005-report.md): Linux pass end to end; adapter is @agentclientprotocol/claude-agent-acp |
| [0006](0006-native-claude-acp-adapter.md) | Native Rust ACP adapter for Claude Code (no Node) | 0/1 | [report](0006-report.md): done on Linux; no Node, 0005 panel unchanged, 2.3 MB binary |
| [0007](0007-host-lsp-bridge.md) | Production LSP bridge between the shell and eludite-host | 1 | [report](0007-report.md): done on Linux; lifecycle under eludite/host/*, warming fixes referenced-project completion |
| [0008](0008-docking-production.md) | Docking and tool windows in the production shell | 1 | [report](0008-report.md): done on Linux; cold start ~100 ms, 19/19 real-input steps |
| [0009](0009-editor-core.md) | Vendor Zed's text crates and build the editor core | 1 | [report](0009-report.md): done on Linux; speed budgets pass, memory on a 100k-line file 249 MB vs 150 MB budget (follow-up) |
| [0010](0010-rename-to-eludite.md) | Rename the project to Eludite | 1 | [report](0010-report.md): done on Linux; full Linux suite green |
| [0011](0011-editor-memory.md) | Bring editor memory under budget | 1 | [report](0011-report.md): done on Linux; 100k-line C# 98 MB idle (was 241), growth 8 MB; mimalloc for tree-sitter only |
| [0012](0012-open-solution.md) | Open a solution end to end | 1 | [report](0012-report.md): done on Linux; editable in 16 ms, tree 0.5 s, first diagnostics 2.4 s on Eludite.slnx |
| [0013](0013-completion-and-hover.md) | Completion, hover and signature help in the editor | 1 | [report](0013-report.md): done on Linux; popup visible 12 ms p50, 29 ms p95 |
| [0014](0014-navigation.md) | Go to definition, Find All References and Error List filtering | 1 | [report](0014-report.md): done on Linux; F12 4 ms; references 508 ms p95 (Roslyn's fixed 500 ms batching) |
| [0015](0015-rename-and-code-actions.md) | Rename, code actions and the workspace-edit applier | 1 | [report](0015-report.md): done on Linux; light bulb 121 ms p95, rename preview 170 ms p95 |

The Phase 0 exit is a written go/no-go on ADR-0001, a list of vendored crates, and a sized brief for the .NET Framework debugger.

Phase 0 is complete on Linux (2026-10-01). The Windows runs are listed in [windows-checklist.md](windows-checklist.md).
