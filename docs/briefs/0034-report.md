# Brief 0034 report: Tuning the agent debugging suite from the proving run

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). netcoredbg: not run
here (as in brief 0030), so every scenario ran under `eludite-dbg-mono` with the corpus's `net472` build.
Branch: `brief/0034-debug-suite-tuning`, based on `main` at `a2043e4`. Date: 2026-10-03.
Brief: [0034-debug-suite-tuning.md](0034-debug-suite-tuning.md). Baseline: [brief 0030's report](0030-report.md) and
[its run](0030-run/). Recorded run: [0034-run/](0034-run/) ([numbers](0034-run/numbers.md),
[transcripts](0034-run/transcript.md), [calls](0034-run/calls.json), [screenshots](0034-run/screenshots/)).

## 1. Summary

- **What changed**: the tool descriptions (`start` and `restart` answer when the program runs; `run_until` is the way
  to stop on a named statement; opening a solution or folder replaces what is open; one spelling of null), the guide
  (a "find the statement that produces a wrong value" sequence, the start-then-`wait` rule, the cleanup rule, "the
  workspace is already open"; 1,896 words), `toggle_breakpoint`'s compact answer (the breakpoint's row, `verified`,
  `pending`, `session`, `breakpoints_total`: 59 to 401 B, was 491 to 2,881 B), `null` rendered as `null` on every
  adapter, `eludite-claude-acp` forwarding Claude Code's usage as ACP `usage_update`, and the Agents window showing it
  as a line under each turn (`tokens: 309k in (275k cache read, 34k cache write), 2.6k out, $0.88`). No other tool's
  behavior changed.
- **Real runs that reached the faulting statement within eight debug calls: 6 of 9 (brief 0030: 3 of 9).** Runs that
  stopped on it at all: **9 of 9 (was 3 of 9)**. Named in the answer with debugger-observed values: 9 of 9 (was 9 of 9).
  No run opened a solution or folder (was 4 of 9); no run spent calls on cleanup after its answer (was 9 of 9); every
  run ended its turn with the session at a break in the program (no `stop` at the end), as the guide now asks.
- **The three misses** (OffByOne runs 1 and 3, MissingCase run 2) all began with the guide's own advice, a conditional
  breakpoint on the suspect statement, and reached it six to ten calls later than the three the sequence takes, when
  the breakpoint silently did not stop (section 4): a line breakpoint on a `for` header binds only to its initializer under `eludite-dbg-mono`, so
  `i == count - 1` is never true there; and `coin == Coin.Quarter` fails to compile in Mono's evaluator ("Unknown
  identifier: Coin"), the breakpoint is not inserted, and `wait` answers `exited (1)` without saying why. The next
  change (section 7) is to make a breakpoint that cannot stop say so in the answers, and to correct the guide's two
  examples.
- **Tokens rose**: 309,367 to 817,129 input tokens per run (was 259,826 to 543,593) and 2,174 to 5,556 output (was
  2,282 to 3,685); $0.88 to $1.61 per run (was $0.86 to $1.24), $10.64 for the nine. The guide is 13,007 B (was 8,908
  B), and the five runs that recovered from a breakpoint that never stopped took 15 to 18 model turns (Claude Code's
  `num_turns`). The four runs whose first breakpoint stopped (MissingCase 3, NullField 1 to 3) took 9 to 11 turns and
  cost 309k to 513k input tokens and $0.88 to $1.13.
- **Usage events**: tokens are now read from the transcript's usage line (ACP `usage_update`), and in 9 of 9 runs they
  equal the counts in Claude Code's own stream `result` message.
- **The scripted scenarios** reach each faulting statement in **6 debug calls** (was 7): no `snapshot` after the
  breakpoint's `wait` (its summary has the locals) and the breakpoint set with `remove_after`. Answers per program
  6,763 / 5,457 / 8,661 B (was 10,676 / 8,784 / 13,464 B; budget 30 KB), `toggle_breakpoint` 257 / 260 / 338 B (budget
  500 B), 0.90 to 1.23 s from prompt to the turn's end (three runs each).
- **Checks**: `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test
  --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`) **681 passed,
  0 failed, 1 ignored**; `dotnet build dotnet/Eludite.slnx` 0 warnings, 0 errors; `dotnet test dotnet/Eludite.slnx` 173
  tests, 166 passed, 0 failed, 7 skipped (unchanged); `agents/claude-acp`: `cargo fmt --check` clean, `cargo clippy
  --all-targets -- -D warnings` clean, `cargo test` 19 passed, 0 failed.

## 2. Before and after

Brief 0030's numbers are from [its report](0030-report.md) and [run](0030-run/numbers.md); brief 0034's from
[0034-run/numbers.md](0034-run/numbers.md). Same corpus, same prompt (only the run folder in the project's path
differs), same Claude Code (2.1.288, `claude-fable-5-1`), same adapter (`eludite-dbg-mono` under Mono 6.8, `net472`),
same machine and driver. "Reached" means a stop summary the agent received located at the README's faulting line,
counted at the debug call that brought it.

### Real runs (Claude Code, three per program)

| Program | Debug calls, 0030 → 0034 | Reached (at debug call), 0030 → 0034 | Within eight, 0030 → 0034 | Named, 0030 → 0034 | Input tokens, 0030 → 0034 | Output tokens, 0030 → 0034 | Cost, 0034 | Wall time, 0030 → 0034 |
|---|---|---|---|---|---|---|---|---|
| OffByOne | 6, 6, 5 → 14, 10, 10 | no, no, no → 13, 8, 9 | 0 → 1 of 3 | 3 → 3 of 3 | 259,826; 351,681; 260,935 → 593,020; 642,085; 649,967 | 2,897; 2,928; 2,282 → 5,422; 5,556; 4,031 | $1.61, $1.35, $1.18 | 43.1, 81.5, 52.6 → 86.1, 129.4, 65.7 s |
| MissingCase | 12, 8, 8 → 9, 13, 4 | 5, 5, 4 → 8, 12, 3 | 3 → 2 of 3 | 3 → 3 of 3 | 497,775; 322,552; 411,421 → 602,506; 817,129; 337,858 | 3,290; 3,201; 3,089 → 4,329; 4,441; 2,174 | $1.27, $1.26, $1.04 | 68.1, 59.9, 48.5 → 112.4, 64.6, 37.8 s |
| NullField | 8, 9, 10 → 4, 4, 6 | no, no, no → 3, 3, 3 | 0 → 3 of 3 | 3 → 3 of 3 | 317,763; 497,216; 543,593 → 309,367; 314,913; 513,078 | 3,638; 3,685; 3,246 → 2,606; 2,684; 3,195 | $0.88, $0.92, $1.13 | 77.1, 58.3, 53.1 → 66.7, 65.3, 53.2 s |
| **All nine** | 5 to 12 → 4 to 14 | 3 → **9 of 9** | 3 → **6 of 9** | 9 → 9 of 9 | 259,826 to 543,593 → 309,367 to 817,129 | 2,282 to 3,685 → 2,174 to 5,556 | $0.86 to $1.24 → $0.88 to $1.61 | 43.1 to 81.5 → 37.8 to 129.4 s |

| Behavior | 0030 | 0034 |
|---|---|---|
| Opened a solution or folder first (`eludite.solution.open`, `eludite.workspace.open_folder`) | 4 of 9 | 0 of 9 |
| Cleanup calls after the answer was known (deleting breakpoints, `continue` to the exit, `stop`) | 9 of 9 (1 to 3 calls) | 0 of 9 |
| Session left at a break when the turn ended | 0 of 9 | 9 of 9 |
| A `wait` after each `start` | 6 of 9 | 9 of 9 |
| First breakpoint on the faulting line | 3 of 9 (MissingCase; the others on the `return` or the dereference) | 9 of 9 (7 with a `condition`) |
| Debug answers per run | 7,111 to 19,387 B | 3,610 to 18,751 B |
| Every tool answer of the turn (the guide and Claude's `Read`s included) | 18,614 to 32,473 B | 21,658 to 33,469 B (the guide: 8,908 → 13,007 B) |
| `toggle_breakpoint` answer | 491 to 701 B (no session); 2,178 to 2,627 B (after one) | 59 to 401 B |
| Tokens from | a wrapper around `claude` | the transcript's usage line (ACP `usage_update`); equal to the stream's in 9 of 9 |

Wall times include the agent's start and were taken at load average 1.6 to 3.1, except NullField 2 and 3 (10.2 and
14.6: another worktree's build); on software rendering, not the reference machine.

### Scripted scenarios (the fake agent, three runs each)

| Program | Debug calls, 0030 → 0034 | Reached at | Answers, 0030 → 0034 | `toggle_breakpoint` answer, 0030 → 0034 | Time, 0030 → 0034 |
|---|---|---|---|---|---|
| OffByOne | 7 → 6 | 7 → 6 | 10,676 → 6,763 B | 1,777 → 257 B | 0.99 to 1.09 → 0.92 to 0.96 s |
| MissingCase | 7 → 6 | 7 → 6 | 8,784 → 5,457 B | 1,823 → 260 B | 0.95 to 1.08 → 0.90 to 0.97 s |
| NullField | 7 → 6 | 7 → 6 | 13,464 → 8,661 B | 2,881 → 338 B | 1.30 to 1.43 → 1.16 to 1.23 s |

## 3. What was built

Commits, in order:

1. `549155c` `protocol/schemas/`, first and alone: `debug-start` and `debug-restart` say the answer comes when the
   program runs (a break or the end only when it came first), to set breakpoints with `remove_after` before and call
   `wait` with `until: stopped` after, that `project` takes any project file's path, and the `ActiveDebugFramework`
   rule; `debug-toggle-breakpoint.output.json` (new) and the input's description; `debug-run-until` recommends it for
   "stop on the statement you name" with a `condition`; `debug-stop-summary` and `debug-variables`: `null` on every
   adapter; `workspace-open-folder` and `solution-open`: the workspace is usually open already and opening replaces
   it; `debug-state.output.json`'s description no longer lists `toggle_breakpoint` among the commands answering with
   it.
2. `8f62d06` The brief's Status line and index row: in progress.
3. `82125ba` `agents/claude-acp`: the translator emits `usage_update` before each `result` ends the turn (section 5);
   the golden file and a parser test; the README.
4. `a932dc4` `crates/commands/src/debug.rs` (`ToggleBreakpointOutput`, `BreakpointEdit`, `null_spelling`, the output
   schema bound to `toggle_breakpoint`), `crates/eludite/src/shell/debug.rs` and `debug/state.rs` (the compact answer;
   `null_spelling` on locals, variables, watches and evaluations), `debug/tests.rs`.
5. `537cdbc` `crates/acp/src/protocol.rs` (`Usage`, `TurnTokens`, `SessionUpdate::usage`), `fake_agent.rs` (the
   `stream` scenario ends with a `usage_update`), `shell/agents/transcript.rs` (`Row::Usage`, `TurnUsage`: one line per
   turn, replaced by a later update of the turn, the turn's cost as the running total's growth, the record's form in
   `--transcript-out`; `toggle_breakpoint`'s line reads `added at Program.cs:13`), `window.rs` (the row drawn as a
   notice), the tests.
6. `4d2f5a6` `docs/agents/debugging.md`: the sections listed in section 1; brief 0028's sessions section kept.
7. `adc16a1` `shell/agents/scenario.rs` and `tests.rs`: the planner sets its breakpoint with `remove_after`, checks the
   compact answer's row, drops the `snapshot` (six debug calls), reads `null` only; the tests assert seven or fewer
   debug calls, `toggle_breakpoint` under 500 B, all answers under 30 KB and no `(null)`.
8. `46fa84b` `crates/eludite/tools/debug-agent-linux.sh` and `debug_agent.py`: tokens and cost from the transcript's
   usage line, checked against the stream (still captured, for each call's time and the model); a breakpoint row's
   location no longer counts as reached (only a stop); the cost column; `BRIEF` names the report.
9. `68b59f2` `docs/briefs/0034-run/`: the recorded run.
10. `6a52236` `crates/commands/src/debug.rs`: `cut_value`'s doc comment, which commit 4 left above `null_spelling`.
11. This report, the brief's Status line and the briefs index.

## 4. What the transcripts show

All nine runs followed the guide's new sequence: `ToolSearch`, the guide, a `find` and a `Read` of `Program.cs`, a
breakpoint on the faulting statement (seven with a `condition` and `remove_after`), `start`, `wait` with `until:
stopped` and `depth: 2`. Where the breakpoint stopped, the run was done in three or four debug calls:

- **NullField 1, 2, 3** (4, 4, 6 calls, reached at 3): `parent == null` on line 16, `start`, `wait` at line 16 with
  `parent = null`, `this.Path = null`; one `step_over` to show the branch skipped (run 3 also ran to the dereference to
  show the symptom).
- **MissingCase 3** (4 calls, reached at 3): an unconditional breakpoint on line 28, `start`, `wait` at line 28 with
  `coin = Quarter`, then `variables` for the caller's frame.

Where the breakpoint did not stop, the program ran to its end, `wait` answered `exited (1)` (748 to 755 B) and Claude
spent calls finding out why:

- **OffByOne 1, 2, 3** (14, 10, 10 calls; reached at 13, 8, 9): each set the guide's example, `i == count - 1`, on
  line 13, the `for` header. `eludite-dbg-mono` binds a line breakpoint to the line's first sequence point, the
  initializer `var i = 0`, which runs once (with `i = 0`): the condition is false and nothing stops. Visual Studio
  binds an F9 on that line to the initializer too; it stops in the bound check only with a breakpoint placed in the
  condition's column, which Eludite has no way to set. Claude worked it out from the stops (run 1: "the Mono adapter
  binds that line only to the loop initializer"), then reached line 13 by a breakpoint in the body at
  `i == count - 2` and steps (runs 1 and 3) or an unconditional breakpoint on line 13 and two `run_until`s (run 2).
- **MissingCase 1 and 2** (9 and 13 calls; reached at 8 and 12): `coin == Coin.Quarter` on line 28. Mono's evaluator
  does not resolve `Coin` from the method's namespace ("Unknown identifier: Coin. Could not insert breakpoint at
  …Program.cs:28"; `MissingCase.Coin.Quarter` would compile), so the breakpoint is not inserted. The message is only in
  the Debug output's tail, which `wait`'s exit answer does not carry: both runs called `state` and `output` to find it.
  Run 2's first start also hit "Could not set breakpoint at location …Program.cs:28 (Collection was modified;
  enumeration operation may not execute.)", a race inside `eludite-dbg-mono` while it resolved the pending breakpoint
  as the assembly loaded; it then started again without building, tried a function breakpoint and continued to line
  28.

No call was wasted against the definition of the task in the six runs that stopped on the statement within eight
calls; in the three that did not, the extra calls were spent recovering from a breakpoint that never stopped and gave
no sign in the answer the agent read next.

## 5. The usage event design

- **Source**: Claude Code's stream-json `result` message ends each turn with `usage` (the turn's `input_tokens`,
  `cache_read_input_tokens`, `cache_creation_input_tokens`, `output_tokens`), `total_cost_usd` (the session's running
  total) and `modelUsage` (per model, with `contextWindow`).
- **The adapter** (`agents/claude-acp/src/translate.rs`) sends one ACP `session/update` `usage_update` before the
  prompt's response, for every turn whose `result` carries `usage`: ACP's own fields `used` (the tokens in context at
  the turn's last top-level model call, from that message's `usage`), `size` (the turn model's `contextWindow`, 0 when
  absent) and `cost` (`{amount: total_cost_usd, currency: "USD"}`), and the turn's tokens in
  `_meta.claudeCode.usage` under the names of ACP's `Usage` (`inputTokens` uncached, `cachedReadTokens`,
  `cachedWriteTokens`, `outputTokens`, `thoughtTokens` when given, `totalTokens`) plus `model`. ACP's `usage_update`
  has no per-turn token fields, so the turn's counts go in `_meta` rather than a bespoke update.
- **The shell** (`crates/acp/src/protocol.rs`) keeps `usage_update` as an unknown update kind, as it does `plan`, so
  new fields never break decoding, and reads it with `SessionUpdate::usage`. The Agents window
  (`shell/agents/transcript.rs`) shows one line under the turn: with the turn's tokens, `tokens: 593k in (533k cache
  read, 60k cache write), 5.4k out, $1.61`; from an agent that sends only ACP's fields, `context: 53k of 200k tokens`.
  A later update of the same turn replaces the line; the cost is the turn's own (the running total less the previous
  turn's). `--transcript-out` records the line's text and every count.
- **The driver** reads the turn's tokens and cost from that record. The stream is still captured (each tool call's
  time comes from it) and its `result` is compared with the usage line: equal in 9 of 9 runs.

## 6. Tests

| Where | Test | What it proves |
|---|---|---|
| `agents/claude-acp/tests/golden.rs` | the golden mapping, and a usage test | A recorded Claude Code session's `result` messages produce one `usage_update` each, just before the turn's end, with `used`, `size`, `cost` and `_meta.claudeCode.usage` (thinking tokens included when given), the counts as the stream gives them |
| `crates/commands/src/debug.rs` | `the_toggle_breakpoint_answer_is_compact_and_follows_its_schema`, `a_null_reads_null_on_every_adapter` | The compact answer conforms to `debug-toggle-breakpoint.output.json` and is under 500 B; only a bare `(null)` is rewritten |
| `crates/acp/src/protocol.rs` | `a_usage_update_decodes_with_the_turns_tokens` | The adapter's update decodes with its turn tokens, round-trips unchanged, and an update with ACP's fields only has no turn tokens |
| `shell/agents/transcript.rs` | `a_usage_update_is_one_line_under_its_turn`, the debug-line test | A fake agent's `usage_update` is one line under its turn, replaced by a later one, recorded in `--transcript-out`, the next turn's cost its growth; `toggle_breakpoint`'s compact answer reads `added at Program.cs:13` |
| `shell/debug/tests.rs` | updated expectations | The shell's `toggle_breakpoint` answers with the row (added, changed, deleted, deleted all) and no `(null)` in the summaries |
| `shell/agents/scenario.rs` | the planner's unit tests | Six debug calls; the breakpoint with `remove_after`; a breakpoint answer on another line ends the turn |
| `shell/agents/tests.rs` | `a_scripted_agent_finds_the_{off_by_one,missing_case,null_field}_in_the_corpus` | Against `eludite-dbg-mono`: the README's statement and locals at six debug calls (at most seven asserted), `toggle_breakpoint` under 500 B, all answers under 30 KB, no `(null)`, under 15 s |

## 7. Recommended next changes

The threshold stays eight calls. The misses come from breakpoints that cannot stop and do not say so, so in order:

1. **Say in the answer when a breakpoint could not be set or its condition does not compile** (a shell brief:
   `crates/eludite/src/shell/debug.rs`, `debug-stop-summary.output.json`, `debug-toggle-breakpoint.output.json`). The
   adapter's message ("Unknown identifier: Coin", "Could not insert breakpoint") reaches the Debug output but not the
   answers the agent reads: carry it on the breakpoint's row (`sessions[].message`, already in the state) into the
   `toggle_breakpoint` answer when a session is live, and add `breakpoints_failed` (path, line, message) to a stop
   summary or an end-of-session summary while any breakpoint failed in that session. With it, MissingCase 1 and 2
   would have read the cause in `wait`'s answer and saved the `state` and `output` calls (two calls each; run 1 then
   reaches at 6).
2. **Correct the guide's two examples** (`docs/agents/debugging.md`, `debug-run-until.input.json`): a line breakpoint
   on a `for` header stops only at its initializer, so put the condition on the first statement of the body (`i ==
   count - 2` there is the last iteration) or set the header unconditionally; and qualify type names in conditions
   (`MissingCase.Coin.Quarter`), which every evaluator accepts. With these, OffByOne's runs would have needed one
   breakpoint in the body, `start`, `wait` and two `step_over`s to the header (five calls); MissingCase 1 and 2 three.
3. **Fix the two `eludite-dbg-mono` defects** (a `debuggers/mono` brief): resolve unqualified type names in conditions
   and evaluations against the stopped method's namespace and `using`s, as Visual Studio does; and the "Collection was
   modified" race while pending breakpoints resolve at assembly load.
4. **Column breakpoints** (larger; `debug-toggle-breakpoint.input.json` `column`, DAP `breakpointLocations`): Visual
   Studio's F9 with the caret in a `for` condition binds there. It would let the guide's "stop on the loop bound" work
   as written.
5. **Tokens**: the guide grew from 8,908 to 13,007 B. It is read once per run, so its cost is small next to the turns;
   the token rise follows the recovery turns (15 to 18 model turns in the five runs with a breakpoint that never
   stopped, 9 to 11 in the four others). Items 1 and 2 address that; trimming the guide is not recommended before them.

## 8. Deviations, decisions and findings

1. **The run folder** was this session's scratchpad (`…/scratchpad/0034-run`) rather than `/tmp/0030-run`, so the
   prompt's project path differs from brief 0030's in that prefix; the prompt's text is otherwise identical. Paths
   under the run folder are written `$OUT/` in the recorded files.
2. **Nine real runs, none repeated**: no run failed for an environmental reason. A free dry run of the driver before
   them (`DRY=1`, the fake agent's `stream` scenario as "Claude Code") checked the usage line's path end to end; it is
   not part of the record.
3. **Not changed after the run**: the guide's conditional-breakpoint examples, which the transcripts show misled three
   runs, were left as recorded so the numbers describe the tuned suite; the fix is recommendation 2.
4. **Files outside the brief's list, small**: `crates/eludite/src/shell/agents/window.rs` (one line: draw the usage
   row), `protocol/schemas/debug-state.output.json` and `debug-toggle-breakpoint.input.json` (descriptions only, so
   they stay true of the compact answer), `crates/acp/src/fake_agent.rs` (the `stream` scenario's `usage_update`, for
   the transcript test).
5. **`debug-agent-linux.sh` changed**: the brief keeps it unchanged "unless the tokens come from the adapter now"; they
   do. The stream wrapper stays for per-call times and the cross-check.
6. **No language server**: as in brief 0030's run (its screenshots show the same status), the solution loads without
   semantic features here (no Roslyn build under `tools/roslyn-pin`); the debug tools do not need it.
7. **Load**: NullField runs 2 and 3 ran at load average 10.2 and 14.6 (another worktree's build); their wall times
   (65.3 and 53.2 s) are within the others'.
8. **The "reached" rule** now ignores a `toggle_breakpoint` row's location (the compact answer has one): only a stop
   summary counts, as in brief 0030.
9. **Commit messages** carry no trailers, per CLAUDE.md.

## 9. How to reproduce

```
export DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH DOTNET_NOLOGO=1
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
dotnet build dotnet/Eludite.slnx
corpus/debugging/build.sh
cargo test -p eludite --bin eludite -- a_scripted_agent_finds --nocapture --test-threads=1
(cd agents/claude-acp && cargo test)
# The recorded run (costs subscription tokens: nine prompts, about $1 each):
(cd agents/claude-acp && cargo build --release) && cargo build -p eludite
RUNS=3 crates/eludite/tools/debug-agent-linux.sh "$OUT"     # then copy $OUT/report/* and the screenshots
```

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`) | 681 passed, 0 failed, 1 ignored (a doc example) |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 173 tests: 166 passed, 0 failed, 7 skipped |
| `agents/claude-acp`: `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` | clean, clean, 19 passed, 0 failed |
| `corpus/debugging/build.sh` | three projects, two frameworks each, 0 errors |
