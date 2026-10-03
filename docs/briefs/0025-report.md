# Brief 0025 report: Inspection depth for the agent debugging suite

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). No display: every
test is headless.
Branch: `brief/0025-debug-inspection-depth`, based on `main` at `5143b89` (brief 0022 merged). Date: 2026-10-03.
Brief: [0025-debug-inspection-depth.md](0025-debug-inspection-depth.md).

## 1. Summary

- **An agent at a break gets compact, budgeted answers.** Seven new `eludite.debug.*` commands: `snapshot` (the stop
  summary for any thread and frame), `stack` (paged, one thread or all, external frames marked), `variables` (by
  reference or by thread, frame and scope; paged, expanded to a depth, filtered by a name prefix), `output` (three
  sources read by cursor, with a substring or `/regex/` filter), `exception_info`, `wait` and `pause` (Debug > Break
  All, Ctrl+Alt+Break). `start`, `continue`, the steps, `run_to_cursor`, `pause` and `wait` answer with the stop
  summary (`debug-stop-summary.output.json`) once the debuggee settles. `state` gains `capabilities`,
  `agent_driving` and `console.next`; the status bar slot ends with `, agent driving` while an agent drove last.
- **Reads never move the windows** (proposal 0001 rule 3): a `snapshot` of frame 2 answers that frame's locals while
  the Call Stack selection, the Locals window and the execution point stay on frame 0 (tested). Every adapter request
  an agent's read waits for is tagged with the session generation and the stop; an answer for an older stop reaches
  the read as a stale error, never as a value (rule 4).
- **The UI thread never waits on the adapter.** An agent's command is applied on the UI thread like any other;
  whatever it then waits for (the debuggee to settle, `wait`'s condition, the adapter's answers to a read) is awaited
  in a task of its own, so a 30-second `wait` holds neither the UI nor the next agent's command.
- **Real adapters:** `pause`, `exceptionInfo`, `stackTrace` paging and `variables` paging pass against
  `eludite-dbg-mono` debugging brief 0022's TestApp, both through `eludite-dap` and through the shell (section 6).
  The netcoredbg tests compile and skip: this machine's proxy refuses the release download (`tools/netcoredbg/fetch.sh`
  gets HTTP 403 from github.com), so nothing was measured against netcoredbg.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, headless; load average 2.4 to 6.7 while another agent
  built in a second worktree; three runs each unless noted):

  | Budget | Result |
  |---|---|
  | `snapshot` at a break < 50 ms p95 against the fake adapter (20 calls) | **p95 2.8 to 8.5 ms** (max 11.8 ms), six runs of 20 agent calls at the 120-locals deep stop. Pass |
  | `snapshot` < 150 ms p95 against netcoredbg (20 calls) | **Not measured**: netcoredbg cannot be fetched here. Against `eludite-dbg-mono` instead (`snapshot` with `depth: 2` at the `Calculator.Add` break, through the shell): **p95 7.2 to 11.5 ms** (max 31 to 42 ms, the first call). Not run for netcoredbg |
  | `wait` wakes within 20 ms of the stop being shown | From the locals of the stop reaching the windows to the agent holding the answer (the summary composed): **2.5 to 10.8 ms** (six runs). Pass |
  | The summary with default budgets < 8 KB at the corpus stop | **7,589 bytes** (30 locals, 10 of 13 frames, 20 output lines, one watch, the capabilities); 7,984 bytes with `depth: 2`. Pass |
  | Frame cost < 8 ms p99 while an agent polls `snapshot` ten times a second | The agent's share of the UI thread per frame: **p99 0.31 to 0.45 ms**. A whole headless frame (`Window::draw` of the shell, debug build, plus that share): p99 8.1 to 9.2 ms with the test alone, 5.6 to 18.2 ms with the other tests in parallel. The agent adds under half a millisecond; the absolute 8 ms figure belongs to a release build presenting on a display, which this machine cannot run (section 8, item 9) |
  | No new dependency | Pass: `/regex/` patterns use a small matcher in `eludite-commands` instead of the `regex` crate |

- **Tests:** `cargo test --workspace` **486 passed, 0 failed, 1 ignored** (a doc example), 18 more than after brief
  0022, with `ELUDITE_DBG_MONO` set so the Mono tests run. `dotnet test` unchanged: 164 passed, 0 failed, 7 skipped
  (section 9). fmt and clippy (`-D warnings`) clean.

## 2. What was built

Commits, in order:

1. `ebff417` The brief's Status line.
2. `496ca36` `protocol/schemas/` alone: the input schemas of `snapshot`, `stack`, `variables`, `output`,
   `exception_info`, `pause` and `wait` (each root `description` written for a model: when to call it, what it costs,
   what to call next), the outputs of `stack`, `variables`, `output` and `exception_info`, the shared
   `debug-stop-summary.output.json`, `capabilities`, `agent_driving` and `console.next` in `debug-state.output.json`,
   and the budget parameters and the new answer in the resuming commands' input schemas.
3. `a8579a9` `crates/dap`: the thirteen capabilities decoded (and merged from `capabilities` events),
   `presentationHint` on frames, scopes and variables, member counts on scopes and variables, `exceptionInfo`'s
   `details`, `ClientEvent::Stderr` (the adapter's stderr line by line); the fake adapter's `pause`, exception details,
   `stackTrace` and `variables` paging, large values, other threads' stacks and output per category; the tests of
   section 6.
4. `fdee279` `eludite-commands`: the seven commands, their validation and classes, the budget, `cut_value`, the
   outputs, the `/regex/` matcher; the shell compiles against them (the new commands refused until commit 7).
5. `1fe480a` `protocol/schemas/` alone again, descriptions only: rows leave out an `evaluate_name` equal to the name
   (section 8, item 3), and the summary's watches are the selected frame's.
6. `af43fb5` The fake adapter's program can exit with a code at the end of a run (the summary at session end).
7. `3b813c2` `crates/ui`: Debug > Break All after Continue, on `ctrl-alt-pause` (shown as Ctrl+Alt+Break).
8. `ba2de06` The shell: the commands, the stop summary, the output rings, `capabilities`, `agent_driving` and the
   status bar, and the headless tests.
9. This report, the brief's Status line and the briefs index.

### 2.1 The shell (`crates/eludite/src/shell/debug.rs`, `debug/state.rs`)

- **The job loop.** `apply_debug` applies every command on the UI thread, in order, and returns what an agent's call
  waits for (`Followup`): `Settle` (a resuming command, `start`, `pause`), `Wait`, `Ended` (`stop`), `Eval`, or a
  read (`Snapshot`, `Stack`, `Variables`, `ExceptionInfo`). The job loop spawns a task per follow-up that awaits it
  and composes the answer; the waits use one real-time timer thread (`real_timer`), because the test executor's
  timers follow a simulated clock that a `wait` timing out has to see pass.
- **Reads through the adapter.** A `Reader` holds the generation and stop it reads; `Debugger::agent_request` sends a
  request as `Pending::Agent { generation, stop, reply }` only while that stop is current, and `on_response` hands
  the answer over only if it still is (else `stale: …`). Requests of one level are sent together (a depth-3
  expansion costs three round trips, not one per value). What the windows already hold serves first: the Call
  Stack's frames, the Locals window's rows of the selected frame (with the true row count and the scope's
  reference, now kept in the model) and the exception the shell read at the stop.
- **The summary.** `DebugModel::summary_base` (mode, output, the end of the session, capabilities, who drives) and
  `summary_stopped` (reason, thread, location, exception brief, breakpoint brief with hits, driver) are shared by the
  model-only summary (the UI thread's answer) and the agent's (`Reader::summary`), which reads the frames and locals
  of any thread and frame and expands them breadth-first within `max_variables`.
- **Paging.** `stack`: `stackTrace` with `startFrame` and `levels` when `supportsDelayedStackTraceLoading`, else one
  `stackTrace` paged in the shell. `variables`: `start` and `count` (one row more, to learn whether more follow, when
  the adapter gave no member count) when `capabilities.variable_paging`, else everything once, paged in the shell;
  totals come from `namedVariables` and `indexedVariables` (remembered per reference for the stop) or from reading.
- **Output.** Three `OutputRing`s per session (10,000 lines each, sequence numbers from 0): `program` (DAP `stdout`
  and `stderr`, and Ctrl+F5's streams), `debug` (every line the shell writes about the session) and `adapter` (DAP
  `console`, `important` and other categories, and the adapter's stderr). Partial lines wait for their newline. The
  Output window's Debug source is unchanged.
- **Break All.** `pause` sends DAP `pause` (the given thread, the last one that stopped, else 0) and changes nothing
  until the `stopped` event; a refused pause sets the message and fails the agent's call. `wait`'s conditions:
  `stopped` (break mode with its locals and watches loaded; with `stop`, a newer stop), `output` (a program line past
  the cursor), `terminated`, `any`; every condition also ends when the session does.
- **`settled`** now also waits for an exception stop's `exceptionInfo` answer and for the watches, so a summary never
  misses them.

### 2.2 The rules the commands follow

| Command | Class | Mode | Answer from the UI thread |
|---|---|---|---|
| `snapshot` | read | break, or running (then `mode: running` and the latest output) | The model's summary (the selected frame); another thread or frame is refused |
| `stack` | read | break; `stop` checked | From the Call Stack's frames when they cover the page, else refused |
| `variables` | read | break; `stop` checked | Refused (it waits for the adapter) |
| `output` | read | any | Always answered |
| `exception_info` | read | break at an exception; `stop` checked; otherwise refused naming the reason | The stopped thread's (read at the stop); another thread refused |
| `wait` | read | any, never refused | What holds now (`satisfied`, or `timed_out`) |
| `pause` | execute | running only | Sends the pause; answers `mode: running` |
| `start`, `continue`, steps, `run_to_cursor` | execute | as before | The model's summary at once, as `state` was |

## 3. Each command's default answer size

Measured with `serde_json::to_string` in the headless tests (`size:` lines).

| Answer | At the corpus stop | At the deep stop | Notes |
|---|---|---|---|
| `snapshot` / the summary of `continue`, steps, `wait`, `pause` | **7,589** | 5,558 | Corpus: 30 locals, 10 of 13 frames, 20 output lines, 1 watch. Deep: 50 of 120 locals (one value cut at 200 characters), 10 of 30 frames, 2 output lines, no watch |
| `snapshot` with `depth: 2` | 7,984 | | |
| `stack` (20 frames) | 2,312 (13 frames) | 2,891 | All threads with `count: 3`: 711 |
| `variables` (50 rows) | 2,451 (30 rows) | 3,143 | |
| `output` (20 lines) | 2,381 | 167 (2 lines) | `debug` source, a whole session's 4 lines: 254 |
| `exception_info` | | 470 | Type, message, break mode, stack trace, one inner exception |
| The summary after the session ended | | 864 | `mode: design`, `exit_code: 3`, `message`, 5 output lines |
| `state` (for comparison) | 8,224 | 15,957 | The whole model: every local of the frame, breakpoints, threads |

The corpus stop is a realistic one built in the test (`the_summary_fits_in_8_kb_and_polling_costs_the_ui_little`):
frames such as `Contoso.Orders.Services.OrderService.CreateAsync(Contoso.Orders.Domain.Customer customer,
System.Collections.Generic.IReadOnlyList<Contoso.Orders.Domain.Line> lines)`, locals of domain types, an 80-character
string, log lines of 80 to 90 characters. The paths are the test's short temporary ones; a real solution's paths add
about 40 bytes per frame and location (some 500 bytes in all).

## 4. Adapter matrix for the new requests

netcoredbg from its documented capabilities (brief 0018 report, section 5, and its recorded `initialize` answer, now
decoded with the new fields in `crates/dap/src/types.rs`' test); `eludite-dbg-mono` measured here; the fake as it
is configured by default.

| | netcoredbg 3.2.0-1092 (documented; not run here) | eludite-dbg-mono (measured) | Fake adapter |
|---|---|---|---|
| `pause` | DAP `pause` (proposal 0001's matrix; brief 0018 did not exercise it). `capabilities.pause: true` | Yes: `stopped` reason `pause` 33 to 50 ms after the request; the stop is in `Thread.SleepInternal()` (external code, `presentationHint: subtle`) over `Program.Main` | Yes, while a statement marked `runs_until_paused` runs; refused when nothing runs |
| `exceptionInfo` | `supportsExceptionInfoRequest: true` (type and description, break mode; brief 0018 used it for the console line) | Yes: `exceptionId`, `description`, `breakMode: always`, `details` with `fullTypeName`, `typeName` (the full name too) and the stack trace with file and line; inner exceptions in `details.innerException` | Yes, with details, stack trace and inner exceptions |
| `stackTrace` paging | `supportsDelayedStackTraceLoading` not advertised: the shell reads the stack once and pages it | Yes: `startFrame`, `levels`, `totalFrames` (2 frames at the TestApp's throw, frame 1 is `Main` at `call-fail`) | Yes (advertised; can be turned off) |
| `variables` paging | Not advertised (`supportsVariablePaging` is not a DAP capability): the shell reads all and pages | Yes: `start: 190, count: 50` of `Many`'s 201 locals answers `l190` to `l199` and `last` in 2.1 to 3.1 ms; scopes carry `presentationHint: locals` and no counts, so `total` is learned by asking one row more; `capabilities.variable_paging` is true for adapter `mono` (brief 0022 report, section 9) | Yes (advertised; can be turned off; counts given) |
| `scope: arguments` | One `Locals` scope, no hints: refused with a message | One `Locals` scope, no hints: refused with a message | The fake marks parameters with `presentationHint.kind: parameter` (not a DAP kind) |
| External frames | `[Native Frames]` without source: `external` | `presentationHint: subtle` or no source: `external` | `[Native Frames]` and other threads' frames: `external` |
| `capabilities` | `set_variable`, `exception_info`, `function_breakpoints`, `exception_filter_options`, `terminate`: true; `log_points` and `hit_conditions`: `shell`; the rest false | `set_variable`, `exception_info`, `function_breakpoints`, `exception_filter_options`, `terminate`, `delayed_stack_loading`, `variable_paging`: true; `log_points` and `hit_conditions`: `adapter`; the rest false | `exception_info`, `terminate`, `delayed_stack_loading`, `variable_paging`: true; `log_points` and `hit_conditions`: `shell`; `adapter: fake` |
| Program output | `stdout`/`stderr` categories (brief 0018) | `stdout`/`stderr`; its own messages as `console` (the `adapter` source) | Per category, as scripted |

## 5. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/commands/src/debug.rs` | 4 new | Every new command parses and validates: defaults, maxima, budget parameters on exactly the summary commands, `reference` versus `thread`/`frame`/`scope`, `scope` and `until` values, `pattern` compiling; classes per rule 7; every output against its schema with a recursive checker (required members, unknown members, enums, integer bounds, local `$ref`s); the regex matcher's syntax, anchors, classes, repetition, refusals and long lines; value cutting |
| `crates/dap/src/types.rs` | 1 extended | netcoredbg's recorded `initialize` answer decodes the new capabilities (function breakpoints, setVariable, exception filter options; none of the others); `eludite-dbg-mono`'s; exception details |
| `crates/dap/tests/inspection.rs` | 3 new | Through `DapClient` against the fake: stack pages of a 30-call stack with `totalFrames`, another thread's external frame, scopes with counts, a 10,000-element array paged by index, a five-level object graph; the same without the paging capabilities (everything answered); `pause` refused while idle and stopping a running statement with reason `pause`, output per category, `exceptionInfo` with stack trace and inner exception |
| `crates/dap/tests/mono.rs` | 1 new | The real `eludite-dbg-mono`: `pause` of the sleeping TestApp (reason `pause`, `Main` on the stack), `exceptionInfo` at the first-chance `InvalidOperationException` (filter `all`) with its stack trace, `stackTrace` with `startFrame`/`levels`/`totalFrames`, `variables` pages of Main's and `Many`'s locals |
| `crates/dap/tests/netcoredbg.rs` | 1 new (skips here) | The real netcoredbg on `eludite-host`: `pause` of the host idling on stdin, the paused thread's stack paged (or answered whole when netcoredbg pages nothing), every thread's top frame, `exceptionInfo` at the first-chance `LocalRpcException` that `eludite/solution/close` throws before initialization, `variables` with `start`/`count` on `this`; prints what netcoredbg does with the paging arguments |
| `shell/debug/state.rs` | 2 new | The rings: partial lines and streams, cursors, patterns, the tail, overflow with `dropped`, a cursor from an older session, flush at the end; refusals by mode (`snapshot`, `pause`, stale `variables`, `exception_info` naming the stop's reason); `agent_driving`; breadth-first rows within a budget and value cutting |
| `shell/debug/tests.rs` (fake adapter, headless) | 5 new | `an_agent_reads_a_deep_stop_within_the_budgets`: `continue` with `wait_ms` answers the summary with `stopped.location`, the breakpoint and its hits, frames 10 of 30 (`total` 30, `truncated`), locals 50 of 120 (`total`, `truncated`, `next`), a 1,000-character string cut at 200 with `value_truncated`, the capabilities of the fake's `initialize` (also in `state`); `snapshot` with `depth: 3` spends its budget breadth-first and marks what it left out, `variables` nests an object graph to depths 3 and 5; `snapshot` with `frame: 2` answers that frame while the Call Stack selection, the frame, the Locals rows and the execution point stay on frame 0; `stack` pages with `start` and `count`, `all_threads` lists both threads (the stopped one first), external frames marked; `variables` pages a 10,000-element array by 50 with `next`, filters by prefix (case-insensitively), expands with `depth: 2`, reads `scope: arguments`, refuses a stale `stop`; `snapshot` p95 and the default answers' sizes. `without_the_adapters_paging_the_shell_pages`: the same pages with the paging capabilities off (one read, paged in the shell). `output_by_cursor_exception_info_and_wait`: lines printed across two `continue`s read by cursor without repetition, `stream` per line, `dropped` after a 10,050-line flood, `pattern` (`/regex/`), `source: debug` holds the shell's messages and `adapter` the console ones; `exception_info` refused at a breakpoint stop naming `breakpoint`, then type, message, break mode, stack trace and inner exception at the exception; `wait` timing out with `mode: break` and `running`, `until: output` returning on a printed line, `until: stopped` returning on the next stop (measured); `pause` from an agent. `break_all_from_the_menu_and_an_agent_and_who_drives`: `pause` refused in break mode; `agent_driving` and the status bar flip with the driver (`Debugging: App (break: step, Calc.cs line 6, agent driving)`); Debug > Break All from the menu stops the running program with reason `pause` (status bar, execution point); the summary at the session's end has `exit_code: 3`; Ctrl+Alt+Break stops it again. `the_summary_fits_in_8_kb_and_polling_costs_the_ui_little`: the corpus stop under 8 KB, the frame cost while polling |
| `shell/debug/tests.rs` (real adapter, headless) | 1 new | `the_reads_work_against_eludite_dbg_mono`: the TestApp as a net472 project's build output under the real adapter: `start` then `wait` until the breakpoint, the capabilities (`adapter: mono`, paging, log points by the adapter), `snapshot` 20 times timed, every thread's stack and a page of it, `continue` to `Many` with 50 of 201 locals, `variables` from 190, then a `Sleep` launch profile, `wait until output` and `pause` with an external top frame. Skips with a message without Mono or the build |
| `shell/debug/tests.rs` | 1 updated | The agent test of brief 0018 reads the summary (`frames.rows`, `locals.rows`, `agent_driving`) |
| `crates/ui/src/menu.rs`, `keymap.rs` | 1 new, 1 extended | Break All after Continue with Ctrl+Alt+Break; the key in the keymap table |

## 6. How the real-adapter tests ran

`dotnet build dotnet/Eludite.slnx`, then `ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace`. Both Mono tests in `crates/dap/tests/mono.rs` and the shell's
`the_reads_work_against_eludite_dbg_mono` ran against Mono 6.8.0.105 at `/usr`. Both tests in
`crates/dap/tests/netcoredbg.rs` returned early ("skipped: netcoredbg was not found…", counted as passed):
`tools/netcoredbg/fetch.sh` downloads the release asset from github.com, which this machine's proxy answers with 403.

## 7. What brief 0026 (run control) needs from this one

- **Capabilities to branch on:** `capabilities.function_breakpoints`, `log_points` and `hit_conditions` (`adapter` or
  `shell`), `exception_filter_options`, `set_variable` and `set_next_statement` are in the state and the summary.
  netcoredbg gives `shell` for log points (tracepoints must be emulated); `eludite-dbg-mono` gives `adapter` for both
  and `set_next_statement: false` (Mono.Debugging has `SetNextStatement`, the adapter does not expose `goto`).
- **Answers:** `run_until` should answer the stop summary through `Followup::Settle` (the budget parameters come with
  `Budget` and `take_budget`); `trace` can collect its lines from the `debug` ring by cursor (`OutputRing::read`),
  where tracepoint messages written with `console_line` land, so the person sees them in the Output window and the
  agent reads them without repetition; `trace`'s `until` maps onto `wait_satisfied`'s conditions.
- **Requests:** `setVariable`, `setExpression` and `goto` are requests at a stop; `Reader::one` and
  `Pending::Agent` already tag them with the generation and the stop.
- **Shell emulation:** the hit-count path in `on_stop` (resume at once without showing the stop) is where a shell
  tracepoint evaluates its `{expressions}` and resumes; `on_stop` clears the per-stop member counts.
- **Summary details:** `stopped.breakpoint` is filled for source breakpoints only; a function breakpoint's stop
  (reason `function breakpoint` from `eludite-dbg-mono`) needs its own brief.
- **The fake adapter** has none of `setFunctionBreakpoints`, `setVariable`, `logMessage`, `gotoTargets`/`goto` or
  `exceptionFilterOptions` conditions yet; it now has `extra_capabilities` to advertise them, `exit_at_end`, prints per
  statement and statements that run until paused.
- **netcoredbg** still needs a machine that can fetch it to confirm this brief's netcoredbg test and measure
  `snapshot` against it.

## 8. Deviations and findings

1. **netcoredbg not run** (section 6): its column in section 4 is from its documented capabilities, and the
   150 ms `snapshot` budget against it is not measured. The gated test compiles and skips cleanly.
2. **A second, description-only schema commit** (`1fe480a`) came after the commands commit and before the shell code
   that relies on it; it changes two `description`s and no structure.
3. **`evaluate_name` is left out of `variables` and summary rows when it equals `name`** (most locals). With it the
   corpus summary was 8,427 bytes, over the 8 KB budget; without it, 7,589. The schemas say so. `state`'s rows are
   unchanged.
4. **Fields beyond the contract's lists, each needed to use an answer:** `frames.thread`, `locals.thread`,
   `locals.frame` and `locals.next` in the summary (which frame the rows are, and how to page them); `stop` in the
   outputs of `stack`, `variables` and `exception_info` (the stop their references belong to, to pass back);
   `next` per thread in `stack`; `total` and `truncated` in `output` and the summary's `output` (rule 2); `thread` on
   `pause` (the contract's "the stopped thread, or thread 0"). `timed_out` is also set on a resuming command whose
   `wait_ms` ran out, not only on `wait`.
5. **Watches appear in a summary of the selected frame only.** The Watch window's values are evaluated in the
   selected frame; evaluating them in another frame would run code, which a `read` command must not.
6. **`scope: arguments` is refused against netcoredbg and `eludite-dbg-mono`:** neither has an `Arguments` scope or
   marks parameters (DAP has no such hint); the message says to read `locals`, where parameters come first. `this` is
   found by name. The fake marks parameters with the non-standard kind `parameter` to test the filtering.
7. **From the UI thread** (no caller there today), reads that need the adapter are refused with a message, and
   `snapshot` answers the model's summary of the selected frame; agents and the MCP server call off the UI thread.
8. **Break All is enabled in the Debug menu whenever a session could take it, like Continue and the steps:** item
   enabling is computed in `crates/eludite/src/shell.rs` (outside this brief's files) from whether the command is
   registered; the command refuses every mode but running with the usual message.
9. **The frame-cost budget is measured headless, in a debug build:** no existing headless frame measurement exists
   (the `--bench-*` harnesses need a display). The test draws the whole shell with `Window::draw` every 16 ms while an
   agent polls `snapshot` at 10/s and adds the UI-thread time of the agent's commands since the previous frame: the
   agent's part is 0.31 to 0.45 ms p99; the debug-build draw dominates (8 to 9 ms p99 alone, more with other tests
   running). A release build on a display should be measured with `--bench-debug` when one is available.
10. **The summary's `truncated` counts the output only when it was read from a cursor** (`output_since`); the default
    tail of 20 lines is not "cut" (older lines are read with `output`).
11. **`/regex/` patterns** use the `regex` crate (MIT OR Apache-2.0, already in the build through GPUI and
    `eludite-browser`), so the syntax is Rust's `regex` syntax; a line is matched on its first 4,096 characters. (The
    brief's agent first wrote a small backtracking matcher to add no dependency; the merge replaced it, since the
    crate was already in the build and is linear-time.)
12. **`capabilities.adapter` is `fake`** when the connection's description is the fake adapter's (tests only); real
    sessions give the `initialize` `adapterID` (`coreclr`, `mono`).
13. **The fake adapter advertises delayed stack loading and variable paging by default** (netcoredbg does neither);
    `extra_capabilities` turns them off, and a test covers the shell's own paging that way.
14. **`ClientEvent::Stderr`** is a new variant of `eludite-dap`'s public enum (the `adapter` output source).
15. **A flaky unrelated test:** one full run of `cargo test -p eludite` under load failed
    `shell::rust_tests::a_crash_restarts_the_generic_server_and_replays_its_documents` (brief 0019's, "replayed after
    the restart"); it passed in every other run, including the final workspace run.
16. **Not run:** Windows, macOS, CI.

## 9. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` (with `ELUDITE_DBG_MONO`) | 486 passed, 0 failed, 1 ignored |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 171 tests: 164 passed, 0 failed, 7 skipped (as after brief 0022: the skips need the Roslyn language server build, the legacy corpus or Mono's MSBuild); no .NET code changed |

## 10. How to reproduce

```
dotnet build dotnet/Eludite.slnx                          # eludite-dbg-mono and the TestApp
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace
cargo test -p eludite debug::tests -- --nocapture         # the `timing:` and `size:` lines
cargo test -p eludite-dap --test mono -- --nocapture      # the real adapter's pause and paging timings
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite-dap --test netcoredbg -- --nocapture
```
