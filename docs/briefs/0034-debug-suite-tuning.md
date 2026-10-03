# Brief 0034: Tuning the agent debugging suite from the proving run

Status: in progress
Phase: 2 and 4 (proposal 0001, follow-up to brief F)
Plan reference: PLAN.md sections 5.5, 5.7, 10 (Phase 4 exit), 11; proposal 0001 sections 9 and 11; brief 0030's report (section "recommended changes")
Related ADRs: ADR-0003
Depends on: brief 0030 (the corpus, the scripted scenarios, the recorded run and its driver), brief 0028 (sessions in the guide).

## Goal

The recorded Claude Code runs of brief 0030 named the faulting statement in nine of nine runs but stopped the debugger on it within eight calls in only three, and the report says why: the guide promises that `start` and `restart` wait for a stop when they answer as soon as the program runs; nothing tells the agent to stop on the statement it names; cleanup calls cost one to three calls per run; `toggle_breakpoint` answers with the whole state; four runs re-opened the workspace; the adapter forwards no usage; `null` prints two ways. This brief applies those changes to the tool descriptions, the guide, the two answers and the ACP adapter, re-runs the scripted scenarios and the recorded Claude Code run, and reports the new numbers against the old ones. The threshold stays proposal 0001's: eight calls.

## Files in scope

- `protocol/schemas/` first and alone: `debug-start.input.json` and `debug-restart.input.json` (the description says the answer comes when the program runs or at the first stop within `wait_ms`, and to call `wait` or set breakpoints first; the `ActiveDebugFramework` rule of brief 0030 for multi-targeted projects), `debug-toggle-breakpoint.output.json` (new: a compact answer: the breakpoint row, `verified`, the session, and `breakpoints_total`, instead of the whole state), `debug-run-until.input.json` (the description recommends it for "stop on the statement you name" with a condition), `debug-stop-summary.output.json` and `debug-variables.output.json` (`null` values are rendered as `null` on every adapter), `workspace-open-folder.input.json` and `solution-open.input.json` (descriptions say when the workspace is already open and that opening replaces it).
- `docs/agents/debugging.md`: the sequence for "find the faulting statement" (read the program, set a breakpoint on the suspect statement or use `run_until` with a condition, `wait`, read the summary, name the location the summary shows), the `start` and `wait` rule, the cleanup rule (prefer `remove_after`, leave the session at the stop; `stop` only when asked), the note that the workspace is already open, and the sessions section of brief 0028 kept.
- `crates/commands/src/debug.rs` (the compact `toggle_breakpoint` answer; null normalization in the row rendering), `crates/eludite/src/shell/debug.rs` and `debug/state.rs` (the same), `crates/eludite/src/shell/debug/tests.rs` (updated expectations).
- `agents/claude-acp/**`: forward Claude Code's usage (input, output, cache read and write tokens, cost when given) as ACP `usage_update` session updates; `crates/acp` and the Agents window's transcript show the turn's totals (a line under the turn: `tokens: 260k in (cache), 2.9k out`), `crates/eludite/src/shell/agents/transcript.rs`.
- `crates/eludite/src/shell/agents/scenario.rs` and `tests.rs` (the scripted scenarios re-run with the new answers; the call counts asserted again), `crates/eludite/tools/debug-agent-linux.sh` (unchanged unless the tokens come from the adapter now), `docs/briefs/0034-run/` (the new recorded run, same shape as `0030-run/`), `docs/briefs/README.md`, `docs/briefs/0034-report.md` (new).

## Contract

- Tool descriptions and the guide say what the tools actually do; no tool's behavior changes except `toggle_breakpoint`'s answer (compact) and `null` rendering (one spelling). The guide stays under 2,000 words.
- The ACP adapter emits `usage_update` for every assistant turn with the counts Claude Code's stream gives; the Agents window shows the turn totals; the proving driver reads them from the transcript rather than from a wrapper around `claude`.
- The scripted scenarios still reach the faulting statement within eight calls (seven or fewer); the recorded run is repeated three times per program with the same prompt, and the report compares calls, reached-or-not, tokens and time with brief 0030's table. If the real runs still miss the threshold, the report says what the transcripts show and proposes the next change; it does not loosen the threshold.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands`: the compact `toggle_breakpoint` answer matches its schema; null rendering.
- `crates/eludite`: the scripted scenarios pass with the new answers (call counts and sizes asserted; the answers shrink: `toggle_breakpoint` under 500 bytes); the transcript shows a usage line for a fake agent's `usage_update`.
- `agents/claude-acp`: a recorded Claude Code stream with usage produces `usage_update` events (unit test on the adapter's parser).
- The recorded run under `docs/briefs/0034-run/` with its numbers.

## Budget

- Scripted scenario answers under 30 KB per program (brief 0030 measured 8.8 to 13.5 KB).
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the scenarios running against the Mono adapter; `dotnet build` and `dotnet test` unchanged; the ACP adapter's own checks (`cd agents/claude-acp && cargo test`) green.
2. The report gives the before and after table and the recommendation for the next change if any.
3. The briefs index matches the repository.

## Out of scope

- Changing the corpus or the prompt; tuning the model; the Test Explorer.
- Windows and macOS runs.
