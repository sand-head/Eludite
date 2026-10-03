# Brief 0036 report: Breakpoints that cannot stop say so, and the Mono adapter resolves names as Visual Studio does

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). netcoredbg: not run
here (as in briefs 0030 and 0034), so every scenario ran under `eludite-dbg-mono` with the corpus's `net472` build.
Branch: `brief/0036-debug-breakpoint-failures`, based on `main` at `51ba9a9`. Date: 2026-10-03.
Brief: [0036-debug-breakpoint-failures.md](0036-debug-breakpoint-failures.md). Baselines: [brief 0030's
report](0030-report.md) ([run](0030-run/)) and [brief 0034's report](0034-report.md) ([run](0034-run/)). Recorded run:
[0036-run/](0036-run/) ([numbers](0036-run/numbers.md), [transcripts](0036-run/transcript.md),
[calls](0036-run/calls.json), [screenshots](0036-run/screenshots/)).

## 1. Summary

- **What changed**: a breakpoint the adapter refused (no code on the line) or whose condition it rejected now says so
  in the answers an agent reads: `message` on the row per session (`state.breakpoints[].sessions[].message`), `verified`
  and `message` in an agent's `toggle_breakpoint` answer while a session is live (the shell waits up to 500 ms for the
  adapter's answer; the person's toggle never waits), `breakpoints_failed` (path, line, session, message) in every stop
  summary and the end-of-session summary of a session with such a breakpoint, and `points_failed` in `run_until` and
  `trace` answers for points that never bound. The guide's two examples are corrected (the condition on the loop
  body's first statement, `i == count - 2`, the header unconditional; `MissingCase.Coin.Quarter` qualified, with the
  note that Eludite's Mono adapter and netcoredbg resolve `Coin.Quarter`) and it says how to read `breakpoints_failed`
  (1,983 words, under 2,000). `eludite-dbg-mono` resolves type names in conditions, tracepoint expressions and
  `evaluate` as Visual Studio does (the method's enclosing types, its namespace and parents, the file's `using`
  directives read from the source the debug information names, `System`, a unique simple name across the loaded
  assemblies; an ambiguous name is reported with both candidates), and breakpoints set while the debuggee loads types
  all bind (the "Collection was modified" race).
- **Real runs that reached the faulting statement within eight debug calls: 8 of 9 (brief 0034: 6 of 9; brief 0030: 3
  of 9).** Runs that stopped on it at all: 8 of 9 (0034: 9 of 9; 0030: 3 of 9). Named in the answer with
  debugger-observed values: 9 of 9 (as in both). Debug calls per run: 3 to 6 (0034: 4 to 14; 0030: 5 to 12).
- **The one miss** (OffByOne run 1) is not a breakpoint that failed: it stopped in the body on the last pass
  (`i == count - 2` on line 15, as the guide now says), stepped once to line 16, then ran to `return total;` on line
  17 with `run_until`, passing the header's bound check without stopping on line 13. Its answer names line 13 and the bound
  with the locals at 15, 16 and 17 (`i = 4`, `count = 5`, `total = 54`) in five debug calls. Runs 2 and 3 took the two
  `step_over`s from line 16 to the header and stopped on it at their fifth call.
- **No breakpoint failed in the nine runs**: every MissingCase run used the guide's qualified
  `coin == MissingCase.Coin.Quarter`, every OffByOne run the body condition. So the gain over brief 0034 comes from the
  corrected examples; the failure reporting and the name resolution are proved by the tests and the new scripted
  scenario (section 6), not by this run.
- **Tokens fell**: 283,623 to 499,803 input tokens per run (0034: 309,367 to 817,129; 0030: 259,826 to 543,593) and
  1,907 to 4,010 output (0034: 2,174 to 5,556); $0.84 to $1.39 per run (0034: $0.88 to $1.61), **$9.14 for the nine**
  (0034: $10.64). Model turns (Claude Code's `num_turns`): 8 to 11 (0034: 9 to 18). Usage lines equal the stream's
  `result` in 9 of 9 runs.
- **The scripted scenarios** still reach each faulting statement in 6 debug calls, answers 6,826 / 5,520 / 8,850 B
  per program (budget 30 KB), `toggle_breakpoint` 264 / 267 / 436 B; the new scenario (a wrong condition,
  `coin == Money.Quarter`) reads the adapter's reason in the end-of-session summary's `breakpoints_failed`, sets
  `coin == Coin.Quarter` (unqualified, resolved by the adapter) and reaches line 28 at its eighth debug call (nine in all).
- **Budgets**: a `toggle_breakpoint` answer carrying the failure message is under 600 B (asserted; 274 to 345 B in the
  real runs, which had none); resolving a type name adds 1.5 ms (the condition's `Coin`, from the method's namespace)
  and 4.0 ms (`Shape`, through a `using` directive) to the first evaluation (budget 20 ms, asserted); no new dependency.
- **Checks**: `cargo fmt --check` clean; `cargo clippy --workspace --all-targets -- -D warnings` clean; `cargo test
  --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, on Xvfb) **710 passed, 0 failed, 1 ignored**; `dotnet build
  dotnet/Eludite.slnx` 0 warnings, 0 errors; `dotnet test dotnet/Eludite.slnx` 179 tests, 172 passed, 0 failed, 7
  skipped.

## 2. Before and after

Brief 0030's numbers are from [its report](0030-report.md) and [run](0030-run/numbers.md), brief 0034's from [its
report](0034-report.md) and [run](0034-run/numbers.md), this brief's from [0036-run/numbers.md](0036-run/numbers.md).
Same corpus, same prompt (only the run folder in the project's path differs), same Claude Code (2.1.288,
`claude-fable-5-1`), same adapter process (`eludite-dbg-mono` under Mono 6.8, `net472`), same machine and driver
(`crates/eludite/tools/debug-agent-linux.sh`, unchanged since brief 0034). "Reached" means a stop summary the agent
received located at the README's faulting line, counted at the debug call that brought it.

### Real runs (Claude Code, three per program)

| Program | Debug calls, 0030 → 0034 → 0036 | Reached (at debug call), 0030 → 0034 → 0036 | Within eight, 0030 → 0034 → 0036 | Named, all three briefs | Input tokens, 0030 → 0034 → 0036 | Output tokens, 0030 → 0034 → 0036 | Cost, 0034 → 0036 | Model turns, 0034 → 0036 | Wall time, 0030 → 0034 → 0036 |
|---|---|---|---|---|---|---|---|---|---|
| OffByOne | 6, 6, 5 → 14, 10, 10 → **5, 6, 6** | no, no, no → 13, 8, 9 → **no, 5, 5** | 0 → 1 → **2 of 3** | 3 of 3 | 259,826; 351,681; 260,935 → 593,020; 642,085; 649,967 → 403,140; 499,803; 468,931 | 2,897; 2,928; 2,282 → 5,422; 5,556; 4,031 → 2,832; 2,510; 2,817 | $1.61, $1.35, $1.18 → $1.39, $1.06, $1.00 | 18, 15, 15 → 11, 11, 11 | 43.1, 81.5, 52.6 → 86.1, 129.4, 65.7 → 60.4, 60.0, 49.0 s |
| MissingCase | 12, 8, 8 → 9, 13, 4 → **3, 4, 3** | 5, 5, 4 → 8, 12, 3 → **3, 3, 3** | 3 → 2 → **3 of 3** | 3 of 3 | 497,775; 322,552; 411,421 → 602,506; 817,129; 337,858 → 283,623; 385,831; 310,359 | 3,290; 3,201; 3,089 → 4,329; 4,441; 2,174 → 1,912; 2,388; 1,907 | $1.27, $1.26, $1.04 → $0.91, $0.97, $0.84 | 18, 18, 9 → 8, 9, 8 | 68.1, 59.9, 48.5 → 112.4, 64.6, 37.8 → 27.5, 60.4, 32.2 s |
| NullField | 8, 9, 10 → 4, 4, 6 → **6, 3, 4** | no, no, no → 3, 3, 3 → **5, 3, 3** | 0 → 3 → **3 of 3** | 3 of 3 | 317,763; 497,216; 543,593 → 309,367; 314,913; 513,078 → 352,760; 300,468; 366,266 | 3,638; 3,685; 3,246 → 2,606; 2,684; 3,195 → 4,010; 2,762; 2,258 | $0.88, $0.92, $1.13 → $1.04, $1.03, $0.91 | 11, 9, 11 → 11, 10, 9 | 77.1, 58.3, 53.1 → 66.7, 65.3, 53.2 → 98.8, 43.1, 39.2 s |
| **All nine** | 5 to 12 → 4 to 14 → **3 to 6** | 3 → 9 → **8 of 9** | 3 → 6 → **8 of 9** | 9 of 9 | 259,826 to 543,593 → 309,367 to 817,129 → 283,623 to 499,803 | 2,282 to 3,685 → 2,174 to 5,556 → 1,907 to 4,010 | $10.64 → **$9.14** (0030: $0.86 to $1.24 per run) | 9 to 18 → 8 to 11 | 43.1 to 81.5 → 37.8 to 129.4 → 27.5 to 98.8 s |

| Behavior | 0030 | 0034 | 0036 |
|---|---|---|---|
| First breakpoint stopped where the run meant it to | not measured | 4 of 9 (5 never stopped: 3 on a `for` header's initializer, 2 with `Coin.Quarter` rejected; MissingCase 2 also hit the race) | 9 of 9 |
| A breakpoint the adapter refused or rejected, and whether the answers said so | none seen | 2 of 9 runs, silent (found with `state` and `output`) | 0 of 9 |
| Opened a solution or folder first | 4 of 9 | 0 of 9 | 0 of 9 |
| Cleanup calls after the answer was known | 9 of 9 | 0 of 9 | 0 of 9 |
| Session left at a break when the turn ended | 0 of 9 | 9 of 9 | 9 of 9 |
| A `wait` after each `start` | 6 of 9 | 9 of 9 | 9 of 9 |
| Debug answers per run | 7,111 to 19,387 B | 3,610 to 18,751 B | 2,560 to 9,165 B |
| Every tool answer of the turn (the guide and Claude's `Read`s included) | 18,614 to 32,473 B | 21,658 to 33,469 B | 18,882 to 26,447 B (the guide: 13,007 → 13,583 B) |
| `toggle_breakpoint` answer | 491 to 2,627 B | 59 to 401 B | 274 to 345 B |

Wall times include the agent's start and were taken at load average 1.4 to 2.6, except OffByOne 1 (6.3: another
worktree's build); on software rendering, not the reference machine. NullField 1's 98.8 s is its own: three
`toggle_breakpoint`s and 4,010 output tokens at load 2.4.

### Scripted scenarios (the fake agent)

| Program | Debug calls, 0030 → 0034 → 0036 | Reached at debug call | Answers, 0030 → 0034 → 0036 | `toggle_breakpoint` answer, 0030 → 0034 → 0036 | Time (3 runs), 0030 → 0034 → 0036 |
|---|---|---|---|---|---|
| OffByOne | 7 → 6 → 6 | 7 → 6 → 6 | 10,676 → 6,763 → 6,826 B | 1,777 → 257 → 264 B | 0.99 to 1.09 → 0.92 to 0.96 → 0.94 to 1.07 s |
| MissingCase | 7 → 6 → 6 | 7 → 6 → 6 | 8,784 → 5,457 → 5,520 B | 1,823 → 260 → 267 B | 0.95 to 1.08 → 0.90 to 0.97 → 0.91 to 0.98 s |
| NullField | 7 → 6 → 6 | 7 → 6 → 6 | 13,464 → 8,661 → 8,850 B | 2,881 → 338 → 436 B | 1.30 to 1.43 → 1.16 to 1.23 → 1.25 to 1.34 s |
| MissingCase, wrong condition (new) | 9 | 8 | 7,379 B | 303, 304 B | 1.49 to 1.64 s |

NullField's `toggle_breakpoint` is set while the first session is at the exception's break: the agent's answer now
waits for that session's adapter and carries its binding (`verified`, the session row), 98 B more; the other two are
set after the program ended. The new scenario: `start` and `wait` (exit 1), `toggle_breakpoint` with
`coin == Money.Quarter`, `start`, `wait` (exit 1, 950 B, with `breakpoints_failed` carrying the adapter's reason,
which names `Money`), `toggle_breakpoint` with `coin == Coin.Quarter`, `start`,
`wait` at line 28 with `coin = Quarter`, `step_over`.

## 3. What was built

Commits, in order:

1. `3ba7c9a` `protocol/schemas/`, first and alone: `debug-stop-summary.output.json` (`breakpoints_failed` and
   `points_failed`, the `failed` row: path, line or function, session, message), `debug-toggle-breakpoint.output.json`
   (`verified` and `message` for the live sessions once the adapter answered), `debug-trace.output.json`
   (`points_failed`), `debug-run-until.input.json` and `debug-trace.input.json` (the corrected examples; a point on a
   `for` header binds to its initializer), `dap-mono.md` (type names in expressions, the failed-breakpoint `message`s,
   breakpoints set while types load, `setBreakpoints` keeping an unchanged breakpoint).
2. `f55a3bc` The brief's Status line and index row: in progress.
3. `6cac266` `debuggers/mono/Eludite.Debugger.Mono`: `TypeNames.cs` (the resolution order of section 1, per frame,
   with each resolution and its time in the adapter's log), `SourceScopes.cs` (the `using` directives and namespace
   blocks of a C# file, file-scoped namespaces, aliases, `global using`, comments, strings, raw strings and records
   understood; cached per file and time stamp), `MonoEvaluator.cs` (conditions, tracepoint expressions and `evaluate`
   go through it; an ambiguous name fails naming both types), `MonoAdapter.cs` (a condition that fails is reported as a
   `breakpoint` `changed` event with `verified: false` and the evaluator's message; breakpoint list changes under the
   library's start-up lock, a failed insertion redone at once, an unchanged breakpoint kept by `setBreakpoints`);
   `Eludite.Debugger.Mono.TestApp/Program.cs` (the `coins` and `load` modes: an enum in a namespace, imported types, an
   ambiguous `Kind`, types and six framework assemblies loading); the tests (section 6), with `SourceScopes.cs`
   compiled into the test project.
4. `a29ffa2` `crates/commands/src/debug.rs` (`FailedBreakpoint`, `breakpoints_failed`, `points_failed`, the answer's
   `message`), `crates/eludite/src/shell/debug.rs` and `debug/state.rs` (the adapter's `setBreakpoints` answer and
   `breakpoint` event message on the row per session, cleared when it binds; the lists in the summaries; the agent's
   500 ms wait for the adapter's answer to a `toggle_breakpoint`), `crates/dap/src/fake.rs` (the fake adapter refuses a
   line without code and rejects `x == Type.Member` conditions as Mono's evaluator once did), `debug/tests.rs`.
5. `338cd33` `docs/agents/debugging.md`: the two examples and the sentence on `breakpoints_failed`.
6. `bfd9afd` `shell/agents/scenario.rs` and `tests.rs`: the planner takes a wrong condition and its replacement
   (`DebugAgent::with_condition`), reads `breakpoints_failed` and sets the breakpoint again; the new scenario.
7. `65b2539`, `465ea80` `dap-mono.md` and `MonoAdapter.cs`: a pending breakpoint still unbound although its code has
   loaded is inserted again only at a stop (the debuggee suspended), not at assembly loads, where inserting it while the
   library resolved the same breakpoint could leave it two requests.
8. `44f4942` `corpus/dap/mono/`: re-recorded with `tools/dap-corpus/record.sh` and `REGOLDEN=1` (the adapter no longer
   resolves a pending breakpoint twice, so one "Resolved pending breakpoint" line and its `breakpoint` event are gone).
9. `0df5dd6` `shell/agents/tests.rs`: one corpus scenario at a time per program (the new scenario and the MissingCase
   one share the project's `.user` file and build output).
10. `e59ef97` `crates/dap/src/fake.rs`: `condition_holds`'s doc comment, which commit 4 left above `no_code`.
11. `d3ee4ad` `docs/briefs/0036-run/`: the recorded run.
12. This report, the brief's Status line and the briefs index.

## 4. What the transcripts show

All nine runs followed the guide's sequence: `ToolSearch`, the guide, a `find` and a `Read` of `Program.cs`, a
breakpoint on the suspect statement (eight with a `condition` and `remove_after`), `start`, `wait` with `until: stopped`
and `depth: 2`. Every first breakpoint stopped where it was meant to:

- **MissingCase 1, 2, 3** (3, 4, 3 calls; reached at 3): `coin == MissingCase.Coin.Quarter` on line 28, `start`, `wait`
  at line 28 with `coin = Quarter`. Run 2 added `variables` for the caller's frame (`i = 0`, `expected[0] = 25`).
  Brief 0034's runs 1 and 2 had set `Coin.Quarter` unqualified (the old example), which Mono's evaluator rejected
  silently; they took 9 and 13 calls.
- **NullField 1, 2, 3** (6, 3, 4 calls; reached at 5, 3, 3): `parent == null` on line 16, `start`, `wait` at line 16
  with `parent = null`, `this.Path = null`. Run 1 also set a breakpoint on the dereference (line 33, first
  unconditional, then `f.Parent == null`) before starting and continued to it after; run 3 stepped once to show the
  branch skipped.
- **OffByOne 1, 2, 3** (5, 6, 6 calls): `i == count - 2` on line 15, the body's first statement, as the guide now says;
  `wait` stopped there with `i = 3`, `count = 5`, `total = 49`. Runs 2 and 3 stepped over three times (16, the header
  13, the `return` 17) and so stopped on the faulting line at their fifth call. Run 1 stepped to 16 and then ran to 17
  with `run_until`, which skips the header: its answer is right and shows `i = 4` at the `return` with one price
  unsummed, but it never stopped on line 13, so it does not count as reached. Brief 0034's runs had set the old
  example, `i == count - 1`, on the header, which binds to the initializer and never stopped; they took 10 to 14 calls.

No breakpoint failed, so no answer carried `breakpoints_failed` or a `toggle_breakpoint` `message`; no run called
`state` or `output` to find out why a breakpoint did not stop (brief 0034: MissingCase 1 and 2 did). The calls after the
stop were all evidence (steps to the header, the caller's frame, the dereference), none recovery.

## 5. The adapter's name resolution and the race

- **Where**: two passes, the same for `evaluate`, watches, conditions and tracepoint expressions. Mono.Debugging's own
  pre-pass asks the adapter (`MonoEvaluator.ResolveType`) about every identifier before evaluating: it answers the
  frame's enclosing types, its namespaces and `System` (from the source file's declarations when the library gives only
  the method's name, as it does for a condition). When the evaluator still reports an unknown identifier or type (after
  locals, parameters and members, C#'s order), the expression is evaluated again with the name qualified by
  `TypeNames`: enclosing types, then each namespace level innermost first with its own types and then the aliases and
  namespaces its `using` directives import, then `System`, then a unique simple name across the method's assembly, the
  public types of the debuggee's other assemblies (outside Mono's `lib/mono`) and the types the library has seen load.
  Two candidates at one step fail with `'Kind' is ambiguous between A.Kind and B.Kind: qualify it`, which for a
  condition becomes the breakpoint's message. Results are cached per scope; misses are forgotten at every stop.
- **The file's directives** come from the source path the PDB or MDB gives for the frame, parsed once per file and time
  stamp (`SourceScopes`): `using`, `using X = Y`, `global using`, block and file-scoped namespaces; `using static` is
  ignored, as are directives inside comments and strings.
- **The budget**: the adapter logs each resolution with its time; the test reads the log and asserts under 20 ms:
  1.5 ms for the condition's `Coin` (the namespace, in the pre-pass) and 4.0 ms for `Shape` (a `using` directive, after
  the evaluator's own failure). A whole `evaluate` of `Shape.Circle` from the test client took 25.7 ms (the failed
  evaluation, the lookup, the evaluation again); a second name, 3.2 ms.
- **The race**: Mono.Debugging 2017 inserts a breakpoint on its operation thread from type tables its event thread
  fills as types load, without a lock, and resolves pending breakpoints from a snapshot of its pending list. The adapter
  changes the breakpoint list only under the lock the library's start-up enumeration takes, inserts a breakpoint whose
  insertion failed again at once (the library's message is logged, not shown), and at the next stop inserts again any
  breakpoint still pending although a loaded type of its file has code on its line. The race test sends twenty log
  points one more at a time while the program loads its types and six framework assemblies: in the run for this report,
  10 of 10 runs bound all 21 breakpoints; in 5 of them the adapter redid 1 or 2 insertions.

## 6. Tests

| Where | Test | What it proves |
|---|---|---|
| `shell/debug/tests.rs` | `a_rejected_condition_is_reported_on_the_row_the_answers_and_the_summaries` | The fake adapter rejects `x == Coin.Quarter` at its first hit: the program runs past it, the next `wait` lists it in `breakpoints_failed` (path, line, session, `Unknown identifier: Coin`), the row carries the message per session; set while the session runs, an agent's `toggle_breakpoint` waits for the adapter and answers `verified: false` with the message (under 600 B), the person's toggle does not wait (`pending`); edited to a condition that binds, the message leaves the row, the answer and the next summary |
| `shell/debug/tests.rs` | `the_end_of_session_summary_lists_what_failed_in_the_session` | A program that runs to its end past a rejected condition: the end-of-session summary (`design`, exit 1) carries `breakpoints_failed`; the row says nothing once the session is over |
| `shell/debug/tests.rs` | `a_line_the_adapter_refuses_is_reported_like_a_rejected_condition` | A line without code: the adapter's `The breakpoint location is invalid` on the row, in `breakpoints_failed` and in a live `toggle_breakpoint` answer; deleted, it leaves the list |
| `shell/debug/tests.rs` | `run_until_and_trace_report_points_that_never_bound` | `run_until` with a point without code stops at the other point and lists the first in `points_failed` (not in `breakpoints_failed`, since temporary points are removed at the stop); `trace` likewise |
| `NameResolutionTests.cs` | `An_unqualified_enum_name_resolves_in_a_condition_a_tracepoint_and_evaluations` | Against the real adapter: `coin == Coin.Quarter` stops at the right iteration (`i = 2`, the purse's quarter); `evaluate` of `Coin.Quarter`, a type a `using` imports, one found by unique simple name, a nested type's constant, a cast; a tracepoint's `{coin == Coin.Quarter}`; `Kind.Warm` ambiguous with both names; the resolution times under 20 ms |
| `NameResolutionTests.cs` | `A_condition_naming_an_ambiguous_type_is_reported_on_the_breakpoint` | A condition with an ambiguous name: a `breakpoint` event with `verified: false` and the message naming both types; the program runs to its end without stopping |
| `NameResolutionTests.cs` | `Breakpoints_sent_while_types_and_assemblies_load_all_bind_in_ten_runs` | The race: twenty log points sent one more at a time while types and six assemblies load; all 21 breakpoints bound in each of 10 runs and no "Could not set breakpoint" |
| `SourceScopesTests.cs` | `A_block_namespace_with_top_level_usings`, `Nested_namespaces_keep_their_usings_at_their_level_and_statements_are_not_directives`, `A_file_scoped_namespace_records_and_strings_with_braces` | The `using` scanner (compiled into the test project directly; it has no Mono dependency): aliases, `global using`, `using static` ignored, block, nested and file-scoped namespaces, a `using` statement that is no directive, braces in strings, raw strings and records |
| `shell/agents/tests.rs` | `a_scripted_agent_reads_why_a_wrong_condition_never_stopped` | Against `eludite-dbg-mono`: `coin == Money.Quarter` never stops; the end-of-session summary's `breakpoints_failed` carries the adapter's reason; the planner sets `coin == Coin.Quarter` and reaches line 28 with `coin = Quarter` within ten debug calls, answers under 30 KB |
| `shell/agents/tests.rs` | `a_scripted_agent_finds_the_{off_by_one,missing_case,null_field}_in_the_corpus` | Unchanged: six debug calls, `toggle_breakpoint` under 500 B, answers under 30 KB, no `(null)` |
| `shell/debug/conformance_tests.rs` | the Mono recordings | The re-recorded `corpus/dap/mono/` replays through the shell and matches its goldens |

## 7. Recommended next changes

The threshold stays eight calls. With 8 of 9 runs within it and every first breakpoint stopping, the suite needs no
further tuning for these three programs. In order:

1. **Column breakpoints** (brief 0034's item 4, unchanged; `debug-toggle-breakpoint.input.json` `column`, DAP
   `breakpointLocations`): the only miss stopped on the body and the `return` but not on the header, because a line
   breakpoint on a `for` header binds to its initializer. A breakpoint in the bound's column, as Visual Studio's F9 with
   the caret in the condition sets, would let an agent stop on the faulting expression itself in three calls.
2. **Exercise the failure path in a real run** (no code change): this run's breakpoints all bound, so
   `breakpoints_failed` was proved only by the tests and the scripted scenario. A fourth program whose natural first
   condition fails (a type of another namespace without a `using`, or a line without code) would measure it with
   Claude Code. The brief excluded changing the corpus, so it is a recommendation for a later brief.
3. **netcoredbg**: every run so far is under `eludite-dbg-mono`; the same nine on netcoredbg (where `Coin.Quarter`
   resolves and `for` headers bind to the condition too) when a machine can fetch it.

## 8. Deviations, decisions and findings

1. **The race fix's shape**: the brief says pending breakpoints resolve "from a snapshot of the collection, and a
   breakpoint added during resolution is resolved on the next assembly load or at once". The library already resolves
   from a snapshot; the adapter serializes its list changes with the library's lock, inserts a failed insertion again
   at once, and inserts a breakpoint still stuck again at the next stop rather than at the next assembly load: doing it
   at an assembly load, while the library's event thread may be resolving the same breakpoint, left it two requests in
   the race test (commit 7). 10 of 10 runs bind all 21.
2. **Files outside the brief's list**: `crates/dap/src/fake.rs` (the fake adapter refuses and rejects as Mono does, for
   the shell's tests), `protocol/schemas/debug-trace.output.json` (`points_failed`, which the contract asks of `trace`),
   `corpus/dap/mono/*` (re-recorded by the recorder, never by hand: the adapter's change removed a duplicate
   resolution). All are named in the commits that carry them.
3. **`setBreakpoints` keeps an unchanged breakpoint** (same id, binding and hit count) instead of deleting and
   inserting it again; without it each new breakpoint of a file re-inserted every other one of the file and reopened
   the race. `dap-mono.md` says so.
4. **The fake adapter's rejected condition** is any comparison to a dotted name whose first part is an uppercase
   identifier and no local (`x == Coin.Quarter`): it stands for Mono's old evaluator and for any adapter that rejects a
   condition, and needs no type table.
5. **The run's `toggle_breakpoint` answers** (274 to 345 B) carry no `message`: none failed. The 600 B budget with a
   message is asserted by the shell test.
6. **The run folder** was this session's scratchpad (`…/scratchpad/0036-run`), as in brief 0034; paths under it are
   written `$OUT/` in the recorded files. Nine real runs, none repeated, none failed for an environmental reason.
7. **No language server**, as in briefs 0030 and 0034: the solution loads without semantic features; the debug tools
   do not need it.
8. **Commit messages** carry no trailers, per CLAUDE.md.

## 9. How to reproduce

```
export DOTNET_ROOT=$HOME/.dotnet PATH=$HOME/.dotnet:$PATH DOTNET_NOLOGO=1
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
dotnet build dotnet/Eludite.slnx
corpus/debugging/build.sh
debuggers/mono/Eludite.Debugger.Mono.Tests/bin/Debug/net10.0/Eludite.Debugger.Mono.Tests -method '*ten_runs' -method '*An_unqualified*' -showLiveOutput
cargo test -p eludite --bin eludite -- a_scripted_agent --nocapture --test-threads=1
# The recorded run (costs subscription tokens: nine prompts, about $1 each):
(cd agents/claude-acp && cargo build --release) && cargo build -p eludite
RUNS=3 crates/eludite/tools/debug-agent-linux.sh "$OUT"     # then copy $OUT/report/* and the screenshots
```

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `DISPLAY` on Xvfb; no `ELUDITE_CHROME`, so the external-Chrome tests skip) | 710 passed, 0 failed, 1 ignored (a doc example) |
| `cargo test -p eludite --bin eludite -- a_scripted_agent --nocapture --test-threads=1`, three times | 5 passed each time, none skipped (section 2) |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 179 tests: 172 passed, 0 failed, 7 skipped (as in brief 0034) |
| The three brief tests of `NameResolutionTests.cs`, run alone with live output | 3 passed; the race test 10 of 10 runs, all 21 breakpoints bound; resolution 1.5 and 4.0 ms |
