# Brief 0030 report: The agent debugging proving scenario

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed); the netcoredbg step is
written and unverified. netcoredbg: not run here (github.com answers 403 to `tools/netcoredbg/fetch.sh`), so every
scenario here ran under `eludite-dbg-mono` with the corpus's `net472` build. No display: the headless tests need none;
the real run drew the shell on Xvfb with Mesa's software Vulkan.
Branch: `brief/0030-debug-proving-scenario`, based on `main` at `7740e7f` (briefs 0022 to 0027, 0029 and 0031 merged).
Date: 2026-10-03. Brief: [0030-debug-proving-scenario.md](0030-debug-proving-scenario.md). Recorded run:
[0030-run/](0030-run/) ([numbers](0030-run/numbers.md), [transcripts](0030-run/transcript.md),
[calls](0030-run/calls.json), [screenshots](0030-run/screenshots/)). Corpus: [corpus/debugging](../../corpus/debugging/).

## 1. Summary

- **The corpus** (`corpus/debugging/`, MIT): `OffByOne` (a loop bound `count - 1` skips the last price), `MissingCase`
  (a switch with no `Quarter` case falls to `default: return 0;`), `NullField` (the `Folder` constructor sets `Path`
  only under `if (parent != null)`, so the root's stays null and the check's `f.Path.Length` throws). Each multi-targets
  `net10.0;net472`, has no package references, and, run directly, prints `FAIL <Type.Method>: expected …, actual …` and
  exits with 1 under `dotnet` and under Mono 6.8. The expected answers (statement, line, revealing locals) are the table
  in `corpus/debugging/README.md`, which the tests read.
- **The scripted scenario** (`crates/eludite/src/shell/agents/scenario.rs`): the fake ACP agent's new `planned`
  scenario, driven by a planner that sees only the prompt, `tools/list`, the guide (`resources/read
  eludite://guides/debugging`) and the answers, calls Eludite's real MCP endpoint from the Agents window. Per program:
  `start`, `wait`, `eludite.file.open` and `eludite.editor.find` (not debug calls), `toggle_breakpoint`, `start` (or
  `restart` from the exception's break), `wait`, `snapshot` (`depth: 2`), one `step_over` quoting the stop.
- **The real scenario**: Claude Code 2.1.288 (`claude-fable-5-1`, the model Claude Code reported) through
  `eludite-claude-acp` in the Agents window, the brief's prompt, three runs per program, recorded by
  `crates/eludite/tools/debug-agent-linux.sh`.
- **Proving numbers** (debug calls are `eludite.debug.*` calls of the turn, counted from the audit log for the fake and
  from the transcript for the real runs; "reached" means a stop summary the agent received is located at the README's
  faulting line):

  | Program | Fake: debug calls, reached at call | Fake: time (3 runs) | Fake: all answers | Real: debug calls (3 runs) | Real: reached (at debug call) | Real: named in the answer | Real: tokens in / out (3 runs) | Real: wall time |
  |---|---|---|---|---|---|---|---|---|
  | OffByOne | 7, reached at 7 | 0.99 to 1.09 s | 10,676 B | 6, 6, 5 | no, no, no (stopped on line 17, `return total;`, after the loop) | yes, yes, yes (line 13) | 259,826 / 2,897; 351,681 / 2,928; 260,935 / 2,282 | 43.1, 81.5, 52.6 s |
  | MissingCase | 7, reached at 7 | 0.95 to 1.08 s | 8,784 B | 12, 8, 8 | yes (5), yes (5), yes (4) | yes, yes, yes (line 28) | 497,775 / 3,290; 322,552 / 3,201; 411,421 / 3,089 | 68.1, 59.9, 48.5 s |
  | NullField | 7, reached at 7 | 1.30 to 1.43 s | 13,464 B | 8, 9, 10 | no, no, no (stopped on line 33, the dereference, where it throws) | yes, yes, yes (line 16) | 317,763 / 3,638; 497,216 / 3,685; 543,593 / 3,246 | 77.1, 58.3, 53.1 s |

  **Real runs that reached the faulting statement within eight debug calls: 3 of 9** (the three `MissingCase` runs).
  All nine named the right statement and line with debugger-observed values in the answer. Input tokens are Claude
  Code's own count for the turn (nearly all cache reads: 207,671 to 504,796 per run); the turn cost $0.86 to $1.24 by
  Claude Code's `total_cost_usd`. The fake meets the threshold (7 debug calls) on all three programs.
- **Why six real runs missed the threshold**: Claude read `Program.cs` first (9 of 9 runs), saw the bug, and used the
  debugger to confirm it where the evidence shows rather than on the faulting statement: for `OffByOne` a `trace` with
  `run: start`, a tracepoint in the loop body and a breakpoint on `return total;` (four hits instead of five, `i = 4`,
  `count = 5`, `total = 54`); for `NullField` a breakpoint or the exception stop on the dereference at line 33 and
  `evaluate root.Path` in `Main`'s frame. No call was wasted against the definition of the task (each run's answer is
  right and backed by values the debugger returned); the miss is against the brief's measure, a stop on the statement.
  Section 7 proposes what the guide should say if that stop is what the owner wants.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds; load average 4 to 5 from another worktree's builds):

  | Budget | Result |
  |---|---|
  | Each scripted scenario under 15 s warm, adapter launch included | Prompt to the turn's end, two Mono launches included, three runs each: OffByOne **0.99 / 1.00 / 1.09 s**, MissingCase **0.95 / 0.95 / 1.08 s**, NullField **1.30 / 1.35 / 1.43 s**. Pass (the test asserts 15 s) |
  | Each summary under 8 KB | Fake: largest debug answer **2,881 B** (a `toggle_breakpoint`'s state). Real: largest **3,197 B** (a `trace` with four lines and its stop summary). Pass (asserted per call in the test) |
  | A scenario's answers under 40 KB | Fake: **8,784 to 13,464 B** for every call (debug and the two editor calls). Real: debug answers **7,111 to 19,387 B** per run; every tool answer of the turn, the 8,908 B guide and Claude's own `Read`s included, **18,614 to 32,473 B**. Pass |
  | No new dependency | Pass: `Cargo.lock` unchanged; the driver uses the Python standard library, xdotool and ImageMagick |

- **Tests**: `cargo test --workspace --no-fail-fast` with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME` and
  `ELUDITE_CHROME_NO_SANDBOX=1`: **660 passed, 0 failed, 1 ignored** (a doc example); the three scenarios ran against
  `eludite-dbg-mono`. fmt and clippy (`-D warnings`) clean. `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors.
  `dotnet test dotnet/Eludite.slnx`: 173 tests, 166 passed, 0 failed, 7 skipped in three runs; a fourth run, at load
  average 20 from the other worktree's build, failed `MonoAdapterTests.Pause_breaks_a_sleeping_program_and_disconnect_
  ends_it` once (debuggers/mono, untouched here; it passed in the three runs after). `corpus/debugging/build.sh`: three
  builds, 0 warnings, 0 errors.

## 2. What was built

Commits, in order:

1. `84b9ea4` The brief's Status line.
2. `71def3b` `crates/dap/src/launch.rs`: a multi-targeted project runs Visual Studio's `ActiveDebugFramework` (from the
   project's `.user` file) when it lists it, else its first framework as before (section 8, item 1); a test.
3. `e1da809` `crates/acp/src/fake_agent.rs`: the `planned` scenario (`Planner`, `Seen`, `Step`, `Next`,
   `PlannerHandle`), `McpClient::list_tools` (following `nextCursor`) and `read_resource`, the prompt's text kept; a
   test against a loopback MCP server (two pages of tools, the guide, a call, the answer's size).
4. `4da0e01` `corpus/debugging/`: the three programs, their project files and `launchSettings.json`,
   `Directory.Build.props`, `build.sh`, `build.ps1`, `LICENSE` (MIT), `README.md` with the expected answers.
5. `9b8a237` `crates/eludite/src/shell/agents/scenario.rs` (the planner, three unit tests) and the tests in
   `agents/tests.rs` (section 6); `agents.rs` declares the module.
6. `beaa625` `.github/workflows/ci.yml`: the Rust job installs the .NET SDK, fetches netcoredbg (cached), builds the
   corpus and runs the scenario step.
7. `6590285`, `76672c5`, `4a91ac3`, `89e1dbf` `crates/eludite/tools/debug-agent-linux.sh` and `debug_agent.py`: the
   recorded run (section 5), the load average per run, the report's wording and the whole final answers.
8. `d599439` `docs/briefs/0030-run/`: the recorded run.
9. `2710d5d` `corpus/README.md`: the corpus entries.
10. This report, the brief's Status line and the briefs index.

## 3. The corpus

| Program | The seeded bug | Check message | Faulting statement | Locals that reveal it |
|---|---|---|---|---|
| `OffByOne` | `Basket.Total` sums `prices` with `for (var i = 0; i < count - 1; i++)` | `FAIL Basket.Total: expected 75, actual 54` | line 13, the loop's header | `count` = 5, `total` = 0 (and `i` stops at 4) |
| `MissingCase` | `Coins.Cents` switches over `Coin` without a `Quarter` case | `FAIL Coins.Cents(Quarter): expected 25, actual 0` | line 28, `return 0;` under `default:` | `coin` = `Quarter` |
| `NullField` | `Folder`'s constructor assigns `Path` only under `if (parent != null)`; `Tree.Describe` dereferences the root's null `Path` | `FAIL Tree.Describe: expected 21,10,5, actual NullReferenceException (…)` | line 16, `if (parent != null)` | `parent` = null, `name` = "root", `this.Path` = null |

Design choices: one `Program.cs` per program, its `Main` the self-check; the faulting statement one step from the
function's first statement (the planner's one number); primitive locals reveal `OffByOne` (a `List<int>`'s children
differ by adapter); `NullField` throws unhandled under a debugger (the stop the brief names) and, run directly, an
`AppDomain.UnhandledException` handler prints the `FAIL` line and exits with 1 (`dotnet` and Mono differ only in the
message's final period, so the README's message is a prefix). `Directory.Build.props` stops MSBuild's upward search and
sets C# 7.3 (both frameworks), `Optimize` off, portable PDBs and `DeterministicSourcePaths` off (breakpoints bind by
path). Off Windows the `net472` build takes its reference assemblies from the SDK's implicit
`Microsoft.NETFramework.ReferenceAssemblies` package (restored from NuGet once); no package is referenced.

## 4. The scenario scripts

The planner (`DebugAgent`, `steps: 1` for each program) decides each call from the last answer:

| # | OffByOne and MissingCase (they exit) | NullField (it throws) |
|---|---|---|
| 1 | `eludite.debug.start` `{project, wait_ms: 20000}` → `mode: running` | the same |
| 2 | `eludite.debug.wait` `{until: stopped, wait_ms, depth: 2}` → `mode: design`, `exit_code: 1`, the `FAIL` line names `Basket.Total` / `Coins.Cents` | the same → `stopped.reason: exception` at line 33, locals two deep: `f` with `Parent` and `Path` `(null)` |
| – | `eludite.file.open` the project folder's `Program.cs` | `eludite.file.open` the stop's file |
| – | `eludite.editor.find` `" Total("` / `" Cents("` → the declaration's line | `eludite.editor.find` `"f.Parent."` (no match), `"f.Path."` (the stopped line), then `"public Folder("` |
| 3 | `eludite.debug.toggle_breakpoint` `{path, line: declaration + 2, action: set}` | the same, in the constructor |
| 4 | `eludite.debug.start` → running | `eludite.debug.restart` → running (Mono: stop and start) |
| 5 | `eludite.debug.wait` → the breakpoint | the same |
| 6 | `eludite.debug.snapshot` `{depth: 2}` | the same |
| 7 | `eludite.debug.step_over` `{stop, wait_ms: 5000, depth: 2}` → the faulting line | the same |

The answer names the file, line and function and lists the locals two deep, for example `The wrong value comes from
the statement at Program.cs:16 in NullField.Folder..ctor(string name, NullField.Folder parent). Its locals there:
this = {NullField.Folder}, this.Name = "root", this.Parent = (null), this.Path = (null), name = "root", parent = (null)`.
The planner refuses to start without the eight tools it uses in `tools/list` and a guide that mentions `snapshot` and
`stop`; a failed call ends the turn naming it.

## 5. The recorded real run

`crates/eludite/tools/debug-agent-linux.sh OUT_DIR [PROGRAM...]` (`RUNS=3`), per run:

1. A copy of the program and the corpus's `Directory.Build.props` in `OUT_DIR/<Program>-<n>/work/` with a one-project
   `<Program>.slnx`, built there (so its PDB names the copy's sources), and, where netcoredbg is missing,
   `<Program>.csproj.user` choosing `net472`. The session's folder is that copy: Claude Code reads files in its working
   folder without asking, and the corpus README next to the originals holds the answers.
2. `work/.eludite/agents-policy.json`: `execute: allow`, a rule denying Claude's `Bash`, the debug policy's defaults
   (agents drive and evaluate; Allow Agents to Drive is on by default).
3. `eludite --solution work/<Program>.slnx --open-file …/Program.cs --agent "Claude Code" --bounds-out …
   --transcript-out transcript.json` with a fresh config folder and `ELUDITE_TRACE_LSP=1`; `claude` runs through a
   generated wrapper (`ELUDITE_CLAUDE_PATH`) that copies its stream-json output, time-stamped, to `stream.jsonl`.
4. `debug_agent.py drive` (xdotool; the Python XTest drivers need `python3-xlib`, which this machine lacks): Ctrl+\,
   Ctrl+C shows the Agents window, a click in the prompt box, the prompt typed, Enter; permission prompts answered
   (none came); the turn's end from the trace; screenshots `-started` and `-end`. The wall time starts when the window
   takes the prompt (`agents spawned`): on lavapipe the window works through the 400 typed characters for 23 to 28 s
   after xdotool sent them, which is excluded.
5. `debug_agent.py summarize`: the calls from the transcript (name, input, the answer's bytes), their times from the
   stream (tool use to tool result), the stops (`debug_location`, else the answer's `stopped.location`), whether the
   answer names the line and the statement, Claude Code's `result` usage, cost, turns and model.
6. `debug_agent.py report`: `numbers.md`, `calls.json`, `transcript.md` (paths under the run folder written `$OUT/`).

What Claude did, in all nine runs: `ToolSearch` for the debug tools' schemas, `ReadMcpResourceTool` for the guide
(8,908 B), `Bash` `find` over the project (Claude Code ran it without asking, so the policy's Bash rule never applied),
`Read` of `Program.cs` (and of `launchSettings.json`, the project file or the policy file), then the debugger, then
cleanup. In four runs it first called `eludite.solution.open` with the `.csproj` or `eludite.workspace.open_folder` on
the work folder, replacing the workspace the window had open. Debug calls in order: see [numbers.md](0030-run/numbers.md).

### Bytes per call

| Call | Fake (each program) | Real (range over the runs) |
|---|---|---|
| `start` (answers `running`) | 557 | 557 |
| `wait` (first) | 736 (OffByOne exit), 743 (MissingCase exit), 2,728 (NullField exception, depth 2) | 1,793 (MissingCase breakpoint), 2,697 to 2,810 (NullField, depth 2) |
| `toggle_breakpoint` (answers the debug state) | 1,777 / 1,823 / 2,881 (after a session ended) | 491 to 701 (before any session); 2,178 to 2,627 to delete (after one) |
| `start` / `restart` again | 557 | – |
| `wait` (at the breakpoint) | 2,307 / 1,675 / 2,134 | – |
| `snapshot` depth 2 | 2,285 / 1,653 / 2,112 | – |
| `step_over` depth 2 | 2,193 / 1,506 / 1,969 | 1,929 |
| `trace` (run: start, 4 lines and the stop) | – | 3,186 to 3,197 |
| `continue` | – | 711 to 718 (to the exit); 1,712 to 2,821 (to a breakpoint, depth 2) |
| `evaluate` | – | 90 to 114 |
| `variables` | – | 339 to 1,097 |
| `state` | – | 294 |
| `stop` | – | 2,409 to 2,579 |
| `eludite.file.open`, `eludite.editor.find` | 126 to 143 | – |

## 6. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/dap/src/launch.rs` | 1 extended | `ActiveDebugFramework` in `<project>.user` puts a listed framework first; one the project does not list is ignored |
| `crates/acp/src/fake_agent.rs` | 1 new, 1 extended | The planned scenario reads `tools/list` (two pages) and the guide, calls what the planner picks through MCP with a permission request, gives the planner the answer's structured content and size, and answers; `planned` parses |
| `shell/agents/scenario.rs` | 3 new | The planner's sequence after an exit (start, wait, open, find, the breakpoint two lines below the declaration, start, wait, snapshot, step quoting the stop, the answer); after an exception (no wait when the start settled, the null member found by its dereference on the stopped line, the constructor, restart; a failed call ends the turn); it refuses without its tools or the guide |
| `shell/agents/tests.rs` | 1 new | `the_corpus_programs_fail_their_checks_with_the_expected_message_and_line`: each README line holds its statement; each program, run directly under `dotnet` (net10.0) and `mono` (net472), exits with 1 and prints the README's message (skips a runtime or an unbuilt program with a message) |
| `shell/agents/tests.rs` | 3 new | `a_scripted_agent_finds_the_{off_by_one,missing_case,null_field}_in_the_corpus`: the fake agent in the Agents window with the brief's prompt, against netcoredbg where found, else `eludite-dbg-mono` (the `.user` file written for the run and removed), the corpus built (skip with a message otherwise); the turn ends; the audit log's agent calls equal the planner's and at most eight are `eludite.debug.*`; the last summary's location is the README's file and line; the README's locals are in it with their values and in the answer; each debug answer under 8 KB, all under 40 KB; under 15 s; the call list with bytes and times printed |

## 7. Recommended changes (follow-ups; nothing here was changed)

1. **`start` and `restart` answer as soon as the program runs.** The guide's section 2 says every resuming command
   "answers once the program settles: the summary of the next stop, the end of the session, or `timed_out`", and
   `start`'s description says `wait_ms` is "how long to wait for the debuggee to break, exit or settle"; the shell
   settles a start at `mode: running` (only `wait`'s description says it is the way to wait after starting). Cost: one
   `wait` per start (2 of the fake's 7 calls; 6 of the 9 real runs). Either make a start with `wait_ms` wait for the
   first stop or the end, as the other resuming commands do (a shell change, so a brief), or say in `start`'s and
   `restart`'s descriptions and the guide: "answers when the program runs (`mode: running`); call
   `eludite.debug.wait` with `until: stopped` next, or start with `eludite.debug.trace` `run: start`".
2. **Stop on the statement you name.** The proving measure (and the person watching) wants a stop on the faulting
   statement with its locals. Claude confirmed after it (`return total;`) or at the symptom (the dereference). A guide
   line under section 2: "Before you name a statement as the cause, stop on it once: `run_until` its line, with a
   `condition` for the iteration that matters (`i == count - 1`), then read `snapshot`." With that, `OffByOne` would
   have been toggle/trace plus one `run_until` (within eight in every run) and `NullField` a `restart` with a breakpoint
   in the constructor.
3. **Cleanup costs calls.** In 9 of 9 runs Claude spent one to three calls after its answer was known: deleting its
   breakpoints (`toggle_breakpoint` `delete` or `delete_all`), continuing to the exit or stopping. The guide could say:
   prefer `run_until` (its points are removed at the stop) or `toggle_breakpoint` with `remove_after`; leave the session
   at the stop that shows the bug, since the person decides what to keep.
4. **`toggle_breakpoint` answers the whole debug state** (2.2 to 2.9 KB once a session has run, 0.5 KB before): the
   largest frequent answer after the stop summaries. A compact answer (the breakpoint's row, its verified state, the
   count) would cut the fake scenario's bytes by up to a fifth.
5. **No need to open anything first.** In 4 of 9 runs Claude opened the project as the solution or the folder as the
   workspace before debugging, which replaced what the person had open. `start`'s description could say "`project`
   takes a project file's path; the solution need not contain it", and the guide's section 2 could repeat it.
6. **Token reporting.** `eludite-claude-acp` sends no ACP `usage_update`; Claude Code's stream has the turn's usage in
   its `result` message. Forwarding it (agents/claude-acp, a later brief) would let the Agents window and this measure
   read tokens without a wrapper.
7. **Null rendering differs by adapter** (`(null)` under eludite-dbg-mono, `null` under netcoredbg). Agents coped; the
   summary could normalize it.

## 8. Deviations, decisions and findings

1. **A file outside the brief's list: `crates/dap/src/launch.rs`.** The shell runs a multi-targeted project's first
   target framework, so the corpus's `net10.0;net472` projects could only ever run under netcoredbg, which this machine
   cannot fetch; the brief's tests and real run need the `net472` build under `eludite-dbg-mono`. The smallest change
   that does it without editing the corpus per machine is Visual Studio's own mechanism: `ActiveDebugFramework` in the
   project's `.user` file (what VS's Debug toolbar framework list writes). A dozen lines and a test; no Debug toolbar
   list exists, and `debug-start.input.json`'s description does not mention it (protocol/ is outside this brief: a
   follow-up). The tests write `<Program>.csproj.user` in the corpus for their run and remove it (`*.user` is ignored by
   git); the real run writes it in its copy.
2. **Also outside the list, minimally:** `crates/eludite/src/shell/agents.rs` (the `#[cfg(test)] mod scenario;` line)
   and `corpus/README.md`'s rewrite (it said "Empty until Phase 0 spike 3"; it now lists `legacy/` and `debugging/`).
3. **The scripted sequence has a `wait` after each start** (section 7, item 1): seven debug calls where the contract's
   list reads five or six. `NullField`'s first look at the locals is that `wait`'s `depth: 2` answer, not a separate
   `snapshot`, and the exception is read from the summary's `stopped.exception`, not `exception_info`, to stay within
   eight.
4. **What the fake knows that is not in an answer**: per program, how many steps the faulting statement is from the
   function's first statement (one), that a console program's code is in its project folder's `Program.cs` when no
   frame names a file, and that the first statement is two lines below the declaration (braces on their own lines).
   Line numbers, files, functions, the null member and every stop come from answers.
5. **The real run debugs a copy** of each program in the run folder, so the session's working folder does not contain
   `corpus/debugging/README.md` (the answers); the prompt names the copy's project file. The copy is the program
   unchanged plus the `.user` file.
6. **Tokens** come from Claude Code's stream (`result.usage`), captured by a wrapper around `claude`, not from ACP
   usage events (the adapter sends none) nor the transcript's size (also recorded: `transcript_bytes` in each
   `summary.json`).
7. **The Bash rule never fired**: Claude Code auto-approves its read-only `find` without a permission request, so no
   request reached the Agents window in any run.
8. **The real runs' wall times** include the agent's start (about 0.5 s) and were taken at load average 1.8 to 14.5
   (another worktree's builds), on software rendering; they are not the reference machine's.
9. **The driver uses xdotool**, not the Python XTest helpers (`python3-xlib` is not installed here).
10. **No `tools/netcoredbg/fetch.ps1`**: the brief names one, but none exists and `tools/` is outside this brief. CI runs
    `fetch.sh` under Git Bash on Windows (it already handles `MINGW*`), converts the path with `cygpath -w`, and caches
    on the hash of `fetch.sh`, which holds the pin (there is no PIN file). The macOS asset is arm64 (macos-latest).
    Unverified: CI was not run. With `ELUDITE_NETCOREDBG` set in the Rust job, the netcoredbg-gated tests of earlier
    briefs (`crates/dap/tests/netcoredbg.rs`, the shell's netcoredbg tests) will run in CI for the first time since
    brief 0018 and may need attention.
11. **The scenario under netcoredbg is unverified**: written for it (the adapter found first, `null` and `(null)`
    both read as null), never run.
12. **`dotnet test` flake**: one of four runs failed `Pause_breaks_a_sleeping_program_and_disconnect_ends_it` at load
    average 20; no .NET file changed in this brief.

## 9. How to reproduce

```
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
dotnet build dotnet/Eludite.slnx
corpus/debugging/build.sh
cargo test -p eludite --bin eludite -- a_scripted_agent_finds the_corpus_programs --nocapture --test-threads=1
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite --bin eludite -- a_scripted_agent_finds --nocapture
# The recorded run (costs subscription tokens: nine prompts):
(cd agents/claude-acp && cargo build --release) && cargo build -p eludite
RUNS=3 crates/eludite/tools/debug-agent-linux.sh /tmp/0030-run
```

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`) | 660 passed, 0 failed, 1 ignored |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 173 tests: 166 passed, 0 failed, 7 skipped (three runs; one run at load 20 failed one Mono adapter test, section 8 item 12) |
| `corpus/debugging/build.sh` | three projects, two frameworks each, 0 warnings, 0 errors |
