# Brief 0026 report: Run control for the agent debugging suite

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed). No display: every
test is headless.
Branch: `brief/0026-debug-run-control`, based on `main` at `3a5bc4e` (briefs 0022, 0023 and 0025 merged). Date:
2026-10-03.
Brief: [0026-debug-run-control.md](0026-debug-run-control.md).

## 1. Summary

- **Tracepoints on every adapter.** `toggle_breakpoint` takes `log_message` (Visual Studio's "When Hit... Print a
  message and Continue"): `{expression}` segments are evaluated in the hit frame (an error is written in place as
  `{expression: <error>}`), `$FUNCTION`, `$CALLER`, `$TID` and `$TNAME` are filled in, other `$` words stay text. Where
  the adapter has log points (`eludite-dbg-mono`) it gets `logMessage`; where it has none (netcoredbg, the fake by
  default) the breakpoint breaks and the shell evaluates the message and resumes before showing anything: the mode
  stays `running`, no stop is counted, no execution point is drawn. Either way the line goes to the Output window's
  Debug source and the `debug` ring, the breakpoint's `hits` count, and the margin draws Visual Studio's diamond.
- **Function breakpoints** by name (`function: "Namespace.Type.Method"`) through `setFunctionBreakpoints`, with
  condition, hit condition (counted by the shell where the adapter ignores it) and Delete when hit; refused naming the
  adapter where `capabilities.function_breakpoints` is false. **Exception settings per type**: `types`, `remove`,
  `clear`, sent as `filterOptions` with the comma-separated type names of each filter; refused naming the adapter where
  it has no filter options.
- **Four execute-class commands:** `run_until` (temporary breakpoints, resume, the next visible stop's summary, the
  points removed at that stop), `trace` (install tracepoints, continue or start, collect the lines in order, remove the
  points and put back what they replaced), `set_variable` (`setVariable`, else `setExpression`, else refused; the
  Locals and Watch windows show the new value) and `set_next_statement` (`gotoTargets` then `goto`; refused naming the
  adapter where it has none, which is netcoredbg and `eludite-dbg-mono`).
- **The person's side:** the Breakpoints window lists tracepoints (diamond), function breakpoints (by function) and
  the temporary points of `run_until` and `trace` (marked `(temporary)`), with a When Hit field, a Delete breakpoint
  when hit toggle and a New Function Breakpoint name box; Exception Settings shows Common Language Runtime Exceptions
  and its types with Thrown and User-Unhandled boxes, Add, Remove and Clear Types; the Locals window edits a selected
  value; Debug > Set Next Statement (Ctrl+Shift+F10) and Debug > New Breakpoint > Function Breakpoint...; Ctrl+Alt+B
  shows the Breakpoints window. Every edit is the same `eludite.debug.*` command an agent calls.
- **Persistence:** version 2 of the per-solution file adds tracepoints, function breakpoints, Delete when hit and
  exception types; temporary points are never saved; a version 1 file loads as it was (tested).
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, headless; load average 6 to 7 from parallel builds
  on the machine; three runs each unless noted):

  | Budget | Result |
  |---|---|
  | Emulated tracepoint overhead on netcoredbg < 15 ms per hit mean | **Not measured on netcoredbg** (it cannot be fetched here; the gated test, which builds a 200-iteration loop program and asserts the budget, compiles and skips). Measured instead on `eludite-dbg-mono` with the shell's emulation forced (`Debugger::shell_log_points`, a test switch that ignores `supportsLogPoints`), 100 hits of the TestApp's loop: **6.3 to 7.0 ms mean** from the `stopped` event to the resume's answer (`trace`'s `overhead_ms_per_hit`); end to end **9.4 to 10.2 ms per hit** between the first and last line, against **3.5 to 4.3 ms per hit** with the adapter's own log points (so the emulation adds about 6 ms per hit). The same emulation through `eludite-dap` alone (no shell): 2.6 to 3.4 ms mean. Pass on `eludite-dbg-mono`; netcoredbg not run |
  | `run_until` adds < 20 ms over a plain `continue` to the same line (fake) | Median of 20 agent calls each: continue 11.5 / 11.7 / 13.0 ms, `run_until` 12.7 / 13.2 / 17.7 ms: **+1.2, +1.6, +4.8 ms** (means +0.8, +2.8, +2.3 ms). Pass |
  | `set_variable` round trip < 150 ms p95 against netcoredbg | **Not measured on netcoredbg** (the gated test times 20 calls and asserts it). Against `eludite-dbg-mono`: DAP `setVariable` p95 **0.7 to 2.2 ms** (max 84 to 106 ms, the first call); the whole agent command through the shell p95 **8.5 to 13.1 ms** (it includes the test's agent thread). Not run for netcoredbg |
  | Frame cost < 8 ms p99 while a tracepoint fires 10 times a second | A whole headless frame (`Window::draw` of the shell, debug build) plus the UI thread's handling of the debugger's messages since the previous frame, while an emulated tracepoint fires 20 times at 10/s: **p99 3.5 to 6.1 ms** (max 5.2 to 8.1 ms); the message handling's share **p99 1.1 to 1.5 ms**. Pass (see section 8, item 12) |
  | No new dependency | Pass |

- **Tests:** `cargo test` **558 passed, 0 failed, 1 ignored** (a doc example), 72 more than after brief 0025, with
  `ELUDITE_DBG_MONO` set so the Mono tests run; run crate by crate because the disk filled up (section 8, item 1).
  `dotnet test` unchanged: 171 tests, 164 passed, 0 failed, 7 skipped. fmt and clippy (`-D warnings`) clean.

## 2. What was built

Commits, in order:

1. `20cb19b` The brief's Status line.
2. `cb20daa` `protocol/schemas/` alone: `log_message`, `function`, `remove_after` in `debug-toggle-breakpoint.input.json`;
   `types`, `remove`, `clear` in `debug-exception-settings.input.json`; `kind`, `function`, `log_message`,
   `remove_after`, `temporary` on breakpoint rows and `types` on exception settings in `debug-state.output.json` (path
   and line are no longer required: a function breakpoint has neither); `debug-run-until.input.json`,
   `debug-trace.{input,output}.json`, `debug-set-variable.{input,output}.json`, `debug-set-next-statement.input.json`;
   descriptions of `capabilities` and of the stop summary's commands (also in `debug-stop-summary.output.json`).
3. `bedf29f` `crates/dap`: `logMessage` on `SourceBreakpoint`; `FunctionBreakpoint`, `ExceptionFilterOptions`,
   `SetVariable`/`SetExpression` arguments and answer, `GotoTargets`/`Goto` types; `supportsSetExpression`;
   `StartPlan` gains `function_breakpoints` and `exception_options` (sent in the handshake where supported);
   `set_function_breakpoints_arguments`, `set_exception_breakpoints_arguments`. The fake adapter: function breakpoints,
   `filterOptions`, `setVariable` and `setExpression` (advertised by default, refused when turned off), log points and
   `gotoTargets`/`goto` behind flags, `fake::hot_loop`. Tests (section 5), including the real-adapter ones.
4. `4a5d7c1` `eludite-commands`: the four commands, the new fields, validation, outputs (`TraceOutput`,
   `SetVariableOutput`, `BreakpointKind`), classes; the shell compiles with them refused until commit 7.
5. `39fd90c` `crates/editor`: `BreakpointGlyph::{Tracepoint, TracepointDisabled, TracepointUnbound}` drawn as a
   diamond (hollow when disabled or unbound). `crates/ui`: Debug > Set Next Statement (Ctrl+Shift+F10), Debug > New
   Breakpoint > Function Breakpoint..., Ctrl+Alt+B (Debug.Breakpoints).
6. `457467c` `protocol/schemas/` again, a description only: a visible stop ends `trace`'s collecting whatever `until`
   says.
7. `d4ed85d` The shell: the commands, tracepoint emulation and attribution, function breakpoints, exception plans,
   persistence version 2, the windows, and the headless tests.
8. `2f259be` Unit tests of message parsing, log line matching and the exception plan.
9. `9381293`, `75c5308` Two test races fixed (one of brief 0025's: the fake records `pause` on its own thread).
10. This report, the brief's Status line and the briefs index.

### 2.1 How the shell does it (`crates/eludite/src/shell/debug.rs`, `debug/state.rs`)

- **Emulated tracepoints.** `on_stop` already counted hits and resumed when a hit condition was not met; a breakpoint
  with a message the adapter does not print now goes to `Debugger::start_trace_hit`: one `evaluate` per
  `{expression}` in the top frame (`Pending::TraceEval`, tagged with the generation and a hit id), then the line, then
  `continue` (`Pending::TraceResume`, which times the hit from the `stopped` event). The model never enters break
  mode for it, so `stop`, the windows, the execution point and `wait` see a running debuggee. A message with a `$`
  special is always emulated (DAP log messages have no specials).
- **Adapter log points.** With `supportsLogPoints`, `setBreakpoints` carries `logMessage`. The adapter prints the
  line as a `console` output event without saying which breakpoint it belongs to (Mono's does not), so the shell
  matches each complete console line against the messages of the tracepoints the adapter prints
  (`state::message_matches`: literal text in order, `{expression}`s matching anything); a match counts a hit, goes to
  the `debug` ring (the console text already reaches the Output window) and is recorded for `trace`.
- **Records.** Every tracepoint line is a `TraceRecord` (generation, point, hit, text, time, emulated, overhead), kept
  per window (20,000, dropped in blocks). `trace` remembers where its records start, counts its own on the UI thread
  and, at `count` or `max_hits`, disables its points at once (before the emulated hit's resume), so the debuggee runs
  on unhindered. The job loop's task waits (real-time timer and `debug_waiter`, never the UI thread) until the job is
  done, the session ends, a visible stop comes or `wait_ms` runs out, then asks the UI thread for the answer
  (`finish_trace`), which also removes the points and puts back the breakpoints they replaced; a stop's answer is
  completed with the stop summary off the UI thread.
- **`run_until`** marks its new points `temporary` and `remove_after`; at the next visible stop `on_stop` removes every
  temporary point that is not a running trace's, and a Delete-when-hit breakpoint at its own stop. A line that already
  has a breakpoint keeps it. The answer is `Followup::Settle` (the stop summary).
- **`set_variable`** in the frame the windows show (or by a reference the model holds) is sent from the UI thread
  (`Pending::SetValue`); another frame is resolved by the agent's `Reader` (`stackTrace`, `scopes`) and then sent the
  same way. The answer updates the Locals and Watch nodes (by scope reference and name, or parent reference and name)
  and re-evaluates the watches. `setExpression` needs an expression: the frame's variable name, or the member's
  `evaluateName` from the model.
- **`set_next_statement`** sends `gotoTargets`, whose answer sends `goto` and resumes the model like a step; the agent's
  `Settle` waits for a stop past the one it started from, or fails with the reason (no target on the line, the
  adapter's error).
- **Function breakpoints** live beside the line breakpoints (`state::FunctionBreakpoint`), are sent in the handshake
  and on every edit, and match a stop by `hitBreakpointIds` or the top frame's function name.

## 3. The answers' shapes

| Answer | Example |
|---|---|
| `run_until`, `set_next_statement` | The stop summary of brief 0025 (`stopped.reason`: `breakpoint`, `goto`, ...) |
| `trace` | `lines` (`seq`, `path`, `line`, `hit`, `time_ms`, `text` cut at 1,000 characters), `hits`, `truncated`, `stopped_by` (`terminated` with `exit_code`, `stopped` with `summary`, `hits`, `timeout`), `points` (`path`, `line`, `hits`, `verified`, `message`), `overhead_ms_per_hit` and `emulated` when the shell printed, `generation` |
| `set_variable` | `name`, `value`, `type`, `reference`, `request` (`setVariable` or `setExpression`), `stop`; `pending: true` from the UI thread |
| `toggle_breakpoint`, `exception_settings` | `eludite.debug.state` with the new row and settings fields |

## 4. Adapter matrix for the new features

"Adapter": the adapter does it; "shell": the shell does it on that adapter; "refused": the command is refused naming
the adapter, and `capabilities` says so beforehand. netcoredbg from its recorded `initialize` answer and brief 0018's
probe (not run here); `eludite-dbg-mono` measured; the fake as configured by default (flags in parentheses).

| Feature | netcoredbg 3.2.0-1092 (documented; not run here) | eludite-dbg-mono (measured) | Fake adapter |
|---|---|---|---|
| Tracepoints | shell (`supportsLogPoints` not advertised): a stop, the evaluations and a resume per hit | adapter (`logMessage`; lines told apart by message); shell for messages with `$` specials, or with the test switch | shell; adapter with `supportsLogPoints` |
| `$FUNCTION`, `$CALLER`, `$TID`, `$TNAME` | shell | shell (the message is then emulated) | shell |
| Function breakpoints | adapter (`supportsFunctionBreakpoints`; stop reason not observed) | adapter: stop reason `function breakpoint` in `Calculator.Twice` (line 46), hits counted by the shell | adapter (refused when turned off) |
| Hit conditions on function breakpoints | shell | adapter | shell |
| Exception types (`filterOptions`) | adapter (`supportsExceptionFilterOptions`) | adapter: a `FormatException, InvalidOperationException` condition stops at the throw, `FormatException` alone runs to the end | adapter (refused when turned off) |
| `set_variable` | adapter (`setVariable`; `setExpression` also advertised) | adapter (`setVariable`; no `setExpression`): `a = 10` makes `Add(2, 3)` print `result 26`; a string into an `int` fails with Mono's conversion error | adapter (`setVariable`, `setExpression`; either can be turned off) |
| `set_next_statement` | refused (`gotoTargets` fails with E_NOTIMPL, brief 0018) | refused (`gotoTargets` not implemented, brief 0022) | adapter with `supportsGotoTargetsRequest` (`stopped` reason `goto`), refused without |
| `run_until`, `trace` | shell | shell | shell |
| `capabilities` | `function_breakpoints`, `exception_filter_options`, `set_variable`: true; `log_points`: `shell`; `set_next_statement`: false | `function_breakpoints`, `exception_filter_options`, `set_variable`: true; `log_points`: `adapter`; `set_next_statement`: false | `function_breakpoints`, `exception_filter_options`, `set_variable`: true; `log_points`: `shell`; `set_next_statement`: false |

## 5. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/commands/src/debug.rs` | 2 new, 2 updated | Every new input parses and validates: `log_message` (empty turns it back), `function` (only with `set` or `delete`, without `path`, `line` or `log_message`), `remove_after`; `types` entries (defaults, names without commas or spaces, at most 100), `remove`, `clear`; `run_until` with 1 to 50 points, conditions, `remove_after` defaulting to true; `trace` with `until`/`count` pairs, `max_hits` bounds, `run: start` with the start parameters only, `stop` only with `continue`; `set_variable` by name or reference; `set_next_statement`; classes (all four and `toggle_breakpoint` execute); every new output against its schema, a brief 0025 row without `kind` read as a line breakpoint |
| `crates/dap/tests/run_control.rs` | 6 new | The request shapes (`logMessage`, `setFunctionBreakpoints`, `filterOptions`, `setVariable`, `setExpression`, `gotoTargets`, `goto`); the fake through `DapClient`: function breakpoints bound by name (one unbound with a message), stopping on entry with `hitBreakpointIds`, a condition, refused when off and left out of the handshake; filter options by type (sent at start, two types, refused when off); values changed by `setVariable` (a local, a member by reference, an `int` refusing a string) and `setExpression`, read back; `goto` behind its flag; log points behind their flag (interpolation, `{{`, an unknown name), and breaking without it |
| `crates/dap/tests/mono.rs` | 1 new | The real `eludite-dbg-mono` (section 6): filter options by type, `setVariable` timed and its effect on the program's result, a function breakpoint, the emulated tracepoint through `eludite-dap` timed over 100 hits, the adapter's own log points (100 lines, no stop) |
| `crates/dap/tests/netcoredbg.rs` | 1 new (skips here) | The real netcoredbg on `eludite-host`: a function breakpoint on `Ping`, `setVariable` timed against the 150 ms budget, filter options naming `StreamJsonRpc.LocalRpcException`; then a 200-iteration loop program written and built in a temporary folder, its breakpoint handled as an emulated tracepoint, the 15 ms budget asserted |
| `crates/editor` | 1 new, 1 extended | Tracepoint glyphs are diamonds, filled when enabled; they stay on their lines as text is inserted |
| `crates/ui` | 1 new, 1 extended | Set Next Statement after Run To Cursor with Ctrl+Shift+F10, the function breakpoint item, Ctrl+Alt+B on Windows > Breakpoints |
| `shell/debug/state.rs` | 1 new, 1 updated | Message segments and specials, log line matching, the exception plan (filters versus options), a version 1 file read into version 2's shape |
| `shell/debug/tests.rs` (fake adapter, headless) | 10 new, 1 updated | `tracepoints_print_and_continue_without_a_visible_stop`: interpolated lines with `$FUNCTION`, `$TID`, `$TNAME`, `$CALLER`, `{{x}}`, `$PID` left as text and an evaluation error in place, in the `debug` ring and the Output window, the mode `running`, stop 0, no execution point, hits counted, the diamond, an empty message making it a breakpoint again; then with log points the adapter gets `logMessage`, a `$FUNCTION` message is still emulated, no stop. `function_breakpoints_bind_by_name_and_stop`: added in the window's name box, stop reason `function breakpoint`, verified and hits, a condition from the window's editor, deleted from the window, refused naming the adapter. `exception_types_go_as_filter_options_and_the_window_shows_the_tree`: a type by command and one by the window's Add, `filterOptions` at launch, the stops follow the types, Remove, a type's Thrown box, Clear back to plain filters, refused without the capability. `run_until_stops_at_the_first_point_reached_and_removes_them`: the window shows the points temporary until the stop, both removed, a condition skips a point, stale `stop` refused, `remove_after: false` keeps them, refused outside break mode. `run_until_costs_little_more_than_continue` (the budget). `trace_collects_tracepoint_lines_until_its_condition`: `until: terminated` (lines in order with hits, `exit_code`, overhead), the user's breakpoint on a traced line restored, `until: hits` with `count`, `max_hits` truncating and disabling, `until: stopped` with the summary, refusals by mode. `trace_with_run_start_launches_first`. `set_variable_changes_values_the_windows_show`: an int (the Locals window and a watch), a string member by reference, the adapter's error, the person's Locals value box, `setExpression` when `setVariable` is missing, another frame, refused with neither. `set_next_statement_moves_the_execution_point_where_the_adapter_can`: the execution point and Locals move, Ctrl+Shift+F10, a line without a target, refused without the flag. `run_control_persists_per_solution_and_a_version_1_file_loads`: the When Hit field and Delete breakpoint when hit in the window, version 2 saved (no temporary point), loaded by another window, a version 1 file loaded and saved as version 2. `a_tracepoint_firing_ten_times_a_second_costs_the_ui_little` (the budget). The capabilities assertion of brief 0025 updated (the fake now has `setVariable` and function breakpoints) |
| `shell/debug/tests.rs` (real adapter, headless) | 1 new | `run_control_works_against_eludite_dbg_mono`: `trace` with `run: start` over the TestApp's loop emulated (forced) and by the adapter, timed; a function breakpoint, exception types, `set_variable` timed, `run_until`, and `set_next_statement` refused naming `mono`, in one session |

## 6. How the real-adapter tests ran

`dotnet build dotnet/Eludite.slnx`, then `ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe`
for the Rust tests. All three Mono tests in `crates/dap/tests/mono.rs` and both shell tests against the adapter ran
against Mono 6.8.0.105 at `/usr`. The three netcoredbg tests returned early ("skipped: netcoredbg was not found…",
counted as passed): `tools/netcoredbg/fetch.sh` downloads from github.com, which this machine's proxy answers with 403.
The TestApp's loop has 100 iterations (`debuggers/mono` is not in this brief's files), so the Mono overhead is over 100
hits, not the brief's 200.

## 7. What brief 0027 (proposal 0001 C: attach and policy) needs from this one

- **The policy's `evaluate` knob** should cover `set_variable`, `toggle_breakpoint` with a `log_message` containing
  `{` (it runs code at every hit), `trace` (its messages do too) and `evaluate`. `toggle_breakpoint` is now of the
  execute class as a whole (section 8, item 3), so the gate already asks for it; a rule by input can tell plain
  breakpoints from tracepoints.
- **`drive`** should cover `run_until`, `trace`, `set_next_statement` and `set_variable` as mutating commands.
- **`interrupted_by: "user"`:** `trace`'s wait (`Followup::Trace`) ends at a visible stop or the end of the session;
  a person's resume during it is not distinguished yet. `run_until` answers through `Followup::Settle` like the other
  resuming commands, so it gets whatever brief 0027 adds there.
- **Attach:** an attached session has tracepoints, function breakpoints and exception types like a launched one: the
  handshake's `StartPlan` carries `function_breakpoints` and `exception_options` for `attach` too.
- **Transcript rows:** `trace`'s answer has what a row needs (`hits`, `stopped_by`, `overhead_ms_per_hit`); `run_until`
  and `set_next_statement` answer the stop summary like a step.
- **netcoredbg** still needs a machine that can fetch it: the gated test measures the emulated overhead (proposal 0001
  risk 3) and `setVariable` against their budgets.

## 8. Deviations and findings

1. **The disk filled up.** The machine's free space went from about 15 GB to 2.5 MB during the final
   `cargo test --workspace` (another build on the machine, sccache's 3.9 GB cache, this worktree's 7.9 GB `target`).
   Only build artifacts inside this worktree were deleted (superseded test executables in `target/debug/deps`;
   nothing outside it, no `cargo clean`), and the tests were run crate by crate (`cargo test -p <crate>` for all 18
   workspace crates, deleting each crate's test executables after its run): 558 passed, 0 failed, 1 ignored. The
   one-command `cargo test --workspace` could not be completed for lack of space; fmt and `cargo clippy --workspace
   --all-targets -- -D warnings` ran whole and are clean.
2. **netcoredbg not run** (section 6): its budgets are measured on `eludite-dbg-mono` instead, with the emulation forced
   for the tracepoint overhead, as the brief allowed. The gated tests compile, skip cleanly and assert the budgets.
3. **`toggle_breakpoint` is now of the execute class.** The brief says "the spec stays `execute` as today", but it was
   `read` (brief 0018). The class is per command and a tracepoint's `{expression}` runs debuggee code, so the spec is
   `execute` as the brief's contract states; consequence: under the default policy (`execute: prompt`) an agent's
   breakpoint edit is asked like its `continue`. A plain breakpoint can be allowed by a policy rule for the tool.
4. **Ctrl+Alt+B, not Ctrl+K Ctrl+B.** The brief names Ctrl+K, Ctrl+B as Visual Studio's Breakpoints window key;
   Visual Studio's Debug.Breakpoints is Ctrl+Alt+B (Ctrl+K, Ctrl+B is the Code Snippets Manager), so Ctrl+Alt+B is
   bound (CLAUDE.md invariant 5). Debug > New Breakpoint > Function Breakpoint... shows the Breakpoints window, whose
   New Function Breakpoint name box sets it (its menu line therefore shows Ctrl+Alt+B too); Visual Studio's own key for
   that item was not bound.
5. **Fields beyond the contract's lists:** `temporary` on breakpoint rows (so the window can mark `run_until`'s and
   `trace`'s points), `points`, `emulated` and `generation` in `trace`'s output, `exit_code` with `stopped_by:
   terminated`, `request` and `pending` in `set_variable`'s output, `thread` and the budget parameters on
   `set_next_statement`, `path`/`line` of `set_next_statement` defaulting to the caret like Run To Cursor.
6. **A visible stop ends `trace`** whatever `until` says (`stopped_by: stopped` with the summary): nothing prints while
   the debuggee is in break mode. Recorded in a description-only schema commit (`457467c`).
7. **Function breakpoints cannot be tracepoints** (`log_message` with `function` is refused): DAP function breakpoints
   have no log message. `toggle_breakpoint` with `function` takes `set` or `delete` only.
8. **Exception types and the category boxes:** a category box that is on is its filter without a condition (every
   type breaks); while it is off, the types checked in that column become the filter's condition. A filter is sent
   either in `filters` or in `filterOptions`, never both (`eludite-dbg-mono` would read both as one filter restricted to
   the types). Types set while no session runs are kept; on an adapter without filter options the launch uses the
   plain filters and says so in the Debug output.
9. **Adapter log points are attributed by their text:** a console line that matches no tracepoint's message is the
   adapter's own message; one that matches two tracepoints' messages counts for the first. Messages whose literal text
   is distinctive avoid that.
10. **`run_until` keeps an existing breakpoint on a point's line** (it stops there anyway); a point on a tracepoint's
    line therefore does not stop there.
11. **The person's `set_variable` surface** is a value box under the Locals window for the selected row (the brief's
    window list names the Breakpoints and Exception Settings windows only; rule 1 wants every command reachable). The
    parent references the box needs are given to the window beside its rows (`VarsWindow::set_parents`) because
    `FlatRow` is also built by `bench.rs`, outside this brief's files.
12. **The frame-cost budget is measured headless in a debug build**, as brief 0025 did: the whole draw of the test's
    shell plus the debugger's message handling; a release build on a display should be measured with `--bench-debug`.
13. **Test races fixed:** brief 0025's Break All test read the fake's request log right after the click (the fake
    records it on its own thread); it now waits for it. Under load two other existing tests timed out once and passed
    alone (`the_context_menu_sets_the_startup_project_and_builds_and_it_persists`, and brief 0019's
    `a_crash_restarts_the_generic_server_and_replays_its_documents`, already noted as flaky by brief 0025); both passed
    in the two final runs of `cargo test -p eludite`.
14. **Not run:** Windows, macOS, CI.

## 9. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test` (with `ELUDITE_DBG_MONO`), crate by crate (section 8, item 1) | 558 passed, 0 failed, 1 ignored |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 171 tests: 164 passed, 0 failed, 7 skipped (as before; no .NET code changed) |

## 10. How to reproduce

```
dotnet build dotnet/Eludite.slnx                          # eludite-dbg-mono and the TestApp
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace
cargo test -p eludite -- run_until_costs trace_collects a_tracepoint_firing run_control_works --nocapture --test-threads=1
cargo test -p eludite-dap --test mono runs_under -- --nocapture
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite-dap --test netcoredbg -- --nocapture
```
