# Brief 0026: Run control for the agent debugging suite

Status: done on Linux (Windows and macOS not run); [report](0026-report.md)
Phase: 2 (proposal 0001, brief B)
Plan reference: PLAN.md sections 2 (principles 1, 3), 4.5 (tracepoints, `run_until`, `trace` as shell-side features on every adapter), 5.1, 5.5, 9, 10 (Phase 2); proposal 0001 sections 4 (rules 1, 2, 4, 6, 7), 5.2, 5.3, 6, 8 (B), 11
Related ADRs: ADR-0003, ADR-0007
Depends on: brief 0025 (the stop summary, `capabilities`, `output` cursors); brief 0022 (the Mono adapter). Runs after 0025 merges; must not touch the browser briefs' files.

## Goal

An agent gets the debuggee from here to there in one round trip and instruments it without stopping: tracepoints ("When Hit... Print a message and Continue") on every adapter, `eludite.debug.run_until` (one-shot breakpoints, resume, answer with the stop), `eludite.debug.trace` (install tracepoints, run, collect the lines, remove them), function breakpoints by name, exception settings per exception type, `eludite.debug.set_variable`, and `eludite.debug.set_next_statement` where the adapter supports it. Where the adapter lacks a feature the shell implements it (proposal 0001 rule 6: tracepoints and hit counts cost a stop and a resume per hit on netcoredbg; the report measures the overhead); where the shell cannot (set next statement, data breakpoints) the command is refused with a message naming the adapter and the state's `capabilities` says so beforehand. The person gets the same features in the Breakpoints window (tracepoint rows with Visual Studio's diamond glyph, function breakpoints, "When Hit..." settings) and in Exception Settings (a per-type tree).

## Files in scope

- `protocol/schemas/` first and alone: `debug-toggle-breakpoint.input.json` (`log_message`, `function`, `remove_after`), `debug-exception-settings.input.json` (`types`), `debug-state.output.json` (breakpoint rows gain `log_message`, `function`, `kind`; exception settings gain `types`), `debug-run-until.input.json`, `debug-trace.{input,output}.json`, `debug-set-variable.{input,output}.json`, `debug-set-next-statement.input.json`.
- `crates/commands/src/debug.rs`: the requests, outputs and specs (classes per rule 7: `run_until`, `trace`, `set_variable`, `set_next_statement` are `execute`; `toggle_breakpoint` with a `log_message` containing `{expression}` is `execute` for that call, which the spec documents and the MCP gate applies through the policy's rules by input, since the class on the spec is per command: the spec stays `execute` as today, and the brief notes it).
- `crates/dap/**`: `setFunctionBreakpoints`, `setExceptionBreakpoints` with `filterOptions`, `setVariable`, `setExpression`, `gotoTargets` and `goto`, `logMessage` on source breakpoints; the fake adapter gains function breakpoints, `filterOptions`, `setVariable`, `goto` behind a flag, log points behind a flag, and a program with a hot loop for the overhead measurement.
- `crates/eludite/src/shell/debug.rs`, `debug/state.rs`, `debug/windows.rs` (Breakpoints window: tracepoint and function rows, their glyphs, the When Hit editor; Exception Settings: the per-type tree with Add and Remove), `debug/tests.rs`, `crates/editor/**` only for the tracepoint glyph in the margin; `crates/ui/**` for the glyph and menu items (Debug > New Breakpoint > Function Breakpoint..., Ctrl+K Ctrl+B is Visual Studio's Breakpoints window key, already there or added).
- Persistence (`debug/state.rs` `Persisted`): tracepoints, function breakpoints and exception types persist per solution, with a version bump and a migration test.
- `docs/briefs/README.md`, `docs/briefs/0026-report.md` (new).

## Contract

### Breakpoints

- `toggle_breakpoint` with `action: "set"` accepts `log_message`: a tracepoint. On hit, the message is written to the Output window's Debug source and to the `debug` output ring (brief 0025's cursors) and the debuggee continues without a visible stop. `{expression}` segments are evaluated in the hit frame (`evaluate` with context `watch`); an evaluation error is written in place as `{expression: <error>}`. Visual Studio's `$FUNCTION`, `$CALLER`, `$TID`, `$PID` and `$ADDRESS`-style specials: `$FUNCTION`, `$CALLER`, `$TID`, `$TNAME` are supported, the rest left as text. An empty `log_message` turns the tracepoint back into a breakpoint. With `supportsLogPoints` the adapter gets `logMessage` (and the shell writes the adapter's `output` events of category `console` for it as today); without it the shell emulates: the breakpoint is set as usual, the stop is handled by evaluating and resuming at once, never shown (as hit counts are), and `hits` counts.
- `toggle_breakpoint` with `function: "Namespace.Type.Method"` (and `action: "set"` or `"delete"`) manages a function breakpoint through `setFunctionBreakpoints` (with `condition` and `hit_condition` as source breakpoints); a function breakpoint has no `path` or `line`; the Breakpoints window names it by the function, and its `verified` follows the adapter. Refused with a message when `capabilities.function_breakpoints` is false.
- `remove_after` (boolean, default false): the breakpoint is deleted at its first visible stop (`run_until` uses it; the person gets it as "Delete breakpoint when hit").
- Breakpoint rows in the state gain `kind` (`line`, `tracepoint`, `function`), `log_message`, `function`, `remove_after`.

### Exception settings

- `exception_settings` gains `types`: a list of `{ "type": "System.InvalidOperationException", "break_when_thrown": bool, "break_when_user_unhandled": bool }`, plus `remove` (a type name) and `clear`. With `supportsExceptionFilterOptions` the shell sends `filterOptions` with `condition` set to the comma-separated type names for each filter (netcoredbg and `eludite-dbg-mono` accept it); without it the command is refused naming the adapter. The Exception Settings window shows the tree: Common Language Runtime Exceptions with the two columns, then the types under it with Add (a text box) and Remove.

### Execution

- `run_until` (execute): `points` (1 to 50 of `{ path, line, condition? }`), `wait_ms` (default 5,000, max 30,000), `remove_after` (default true), `stop?`, plus brief 0025's budget parameters. Sets the points as breakpoints (temporary when `remove_after`), resumes, answers the stop summary of the next visible stop (whatever its reason, as Run To Cursor does), removing the temporary points at that stop. The points appear in the Breakpoints window while they exist (so the person sees what the agent did), marked temporary.
- `trace` (execute): `points` (1 to 50 of `{ path, line, message, condition? }`), `run` (`continue` default, `start` to launch first with the start parameters), `until` (`terminated` default, `stopped`: the next visible stop, `hits` with `count`), `wait_ms` (default 10,000, max 30,000), `max_hits` (default 1,000, max 10,000). Installs the points as tracepoints, runs, collects the lines in order with `seq`, `path`, `line`, `hit` (the point's hit count) and `time_ms` since the run began, then removes the points. Output: `lines`, `hits`, `truncated` (hit `max_hits`: the points are disabled and the debuggee keeps running), `stopped_by` (`terminated`, `stopped` with the stop summary, `hits`, `timeout`), `overhead_ms_per_hit` (the measured mean when emulated in the shell, absent when the adapter did it).
- `set_variable` (execute): `thread?`, `frame?`, `name` or `reference` + `name`, `value`, `stop?`. Through `setVariable` (with the variables reference of the scope or the parent) when `supportsSetVariable`, else `setExpression` when `supportsSetExpression`, else refused. Output: the row as `variables` describes it (`name`, `value`, `type`, `reference`). The Locals and Watch windows refresh the changed value (the model's node is updated, watches re-evaluated).
- `set_next_statement` (execute): `path`, `line`, `thread?`, `stop?`. Through `gotoTargets` then `goto` when `supportsGotoTargetsRequest`; refused naming the adapter otherwise (netcoredbg and `eludite-dbg-mono` refuse; the fake supports it behind its flag, so the path is tested). The execution point moves; the Call Stack and Locals refresh as after a step (the adapter sends `stopped` with reason `goto`).
- Every resuming command here answers the stop summary when it waits; `trace` answers its own output.
- Rule 2 of brief 0018 holds: `run_until`, `trace`, `set_next_statement` need break mode (or `trace` with `run: start` needs design mode) and are refused, not queued, otherwise; `set_variable` needs break mode.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands/src/debug.rs`: parsing and validation of every new input (point counts, `until` with `count`, `log_message` with `function`, `types` entries); outputs match their schemas.
- `crates/dap`: the fake's new features through `DapClient`; `filterOptions` encoding; `setVariable`, `setExpression`, `gotoTargets` and `goto` request shapes.
- `crates/eludite/src/shell/debug/tests.rs` (fake adapter, headless): a tracepoint on an adapter without `supportsLogPoints` writes the interpolated line to the Debug source and the `debug` ring and never shows a stop (the execution point and mode stay `running`); with the flag on, the adapter does it and the shell does not stop; `$FUNCTION` and `$TID`; an evaluation error appears in place; a function breakpoint by name binds and stops with reason `function breakpoint`, is listed by function in the Breakpoints window, and is refused on an adapter without the capability; exception settings with `types` send `filterOptions` and the window shows the tree, Remove and Clear; `run_until` with two points stops at the first reached and removes both, the Breakpoints window shows them temporary meanwhile, `remove_after: false` keeps them, a condition skips a point; `trace` with `until: terminated` returns the lines in order with hits, `until: hits` with `count` stops collecting, `max_hits` truncates and disables, `until: stopped` returns the summary, `run: start` launches first; `set_variable` changes an int and a string, the Locals window shows the new value and a watch re-evaluates, and `setExpression` is used when `setVariable` is missing; `set_next_statement` moves the execution point on the fake with the flag and is refused without it; tracepoints, function breakpoints and exception types persist per solution and a version 1 file loads; the person's edits (When Hit editor, Add type) go through the same commands.
- Real adapters, gated like `crates/dap/tests/netcoredbg.rs` and `mono.rs`: `filterOptions` by type on netcoredbg and `eludite-dbg-mono`; `setVariable` on both; a function breakpoint on both; the emulated tracepoint overhead on netcoredbg measured over 200 hits of a hot loop in a test program (the report's number for proposal 0001 risk 3), and the adapter's own log points on `eludite-dbg-mono`.
- No display: headless; the report says so.

## Budget

- Emulated tracepoint overhead on netcoredbg under 15 ms per hit mean (brief 0018 measured about 10 ms per stop and resume); `trace` reports it.
- `run_until` adds under 20 ms over a plain `continue` to the same line (fake adapter, measured).
- `set_variable` round trip under 150 ms p95 against netcoredbg.
- Frame cost unchanged (under 8 ms p99 in the existing headless measurement) while a tracepoint fires ten times a second.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the real-adapter tests running on this machine; `dotnet build` and `dotnet test` unchanged and green.
2. The report records the budget numbers, the per-adapter matrix for the new features (adapter, shell, refused), and what brief 0027 (proposal 0001 C: attach and policy) needs from this one.
3. The briefs index matches the repository.

## Out of scope

- `attach`, `processes`, `restart`, the Attach to Process dialog, the `debug` policy object and the drive toggle, `interrupted_by`, transcript rendering, the agent guide (proposal 0001 C).
- Data breakpoints (no first-wave adapter supports them; the schema entry and the disabled menu item wait for an adapter that does).
- Multi-session (proposal 0001 D). Edit and Continue. Windows and macOS runs.
