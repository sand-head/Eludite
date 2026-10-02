# Brief 0025: Inspection depth for the agent debugging suite

Status: open
Phase: 2 (proposal 0001, brief A)
Plan reference: PLAN.md sections 2 (principles 1, 3, 12), 4.5, 5.1, 5.3, 5.4, 5.5, 9, 10 (Phase 2); proposal 0001 sections 3, 4 (rules 1 to 4, 6, 7), 5.1, 5.3 (`pause`), 8 (A), 11
Related ADRs: ADR-0003, ADR-0007
Depends on: brief 0018 (debugging), brief 0020, brief 0022 (the Mono adapter; its capabilities feed `capabilities`). Runs after 0022 merges; must not touch brief 0023's or 0024's files.

## Goal

An agent at a break answers the questions of proposal 0001 section 3 with compact, budgeted answers instead of the whole state: "where am I and why" (`eludite.debug.snapshot`), "what is inside this variable" (`variables` with paging, depth and a filter), "what happened before this frame" (`stack` with paging and all threads), "what did the program print since my last call" (`output` with cursors), "what was thrown" (`exception_info`), "wait for it to settle" (`wait`), and "stop it, it is hanging" (`pause`, Visual Studio's Break All). Every resuming command's waiting answer becomes the stop summary. `state` gains `capabilities`, so an agent never tries what an adapter lacks, and `agent_driving`. The outputs follow proposal 0001 rule 2: 50 variables, 200 characters per value, 10 frames, 20 output lines and 5 levels at most by default, every list saying `total` and `truncated` and how to fetch more. The agent's frame is a parameter (rule 3): no read moves the windows' selection. Everything works on netcoredbg, on `eludite-dbg-mono` (brief 0022) and on the fake adapter, which gains the features the tests need.

## Files in scope

- `protocol/schemas/` first and alone: `debug-snapshot.input.json`, `debug-stack.{input,output}.json`, `debug-variables.{input,output}.json`, `debug-output.{input,output}.json`, `debug-exception-info.{input,output}.json`, `debug-pause.input.json`, `debug-wait.input.json`, the shared `debug-stop-summary.output.json` (the answer of `snapshot`, `wait`, `pause` and of `start`, `continue`, `step_over`, `step_into`, `step_out` and `run_to_cursor`), `debug-state.output.json` (`capabilities`, `agent_driving`, `output` cursors), and the descriptions of the resuming commands' input schemas (their answer is now the summary). Each new input schema's root `description` is written for a model: when to call it, what it costs, what to call next, as proposal 0001 section 9 asks of tool descriptions.
- `crates/commands/src/debug.rs`: the new requests, outputs and specs (classes per rule 7: `snapshot`, `stack`, `variables`, `output`, `exception_info` and `wait` are `read`; `pause` is `execute`).
- `crates/dap/**`: `pause`, `exceptionInfo`, `stackTrace` paging (`startFrame`, `levels`, `totalFrames`), `variables` paging (`start`, `count`, `filter`); decode `supportsVariablePaging`, `supportsDelayedStackTraceLoading`, `supportsSetVariable`, `supportsFunctionBreakpoints`, `supportsLogPoints`, `supportsExceptionFilterOptions`, `supportsGotoTargetsRequest`, `supportsDataBreakpoints`, `supportsStepBack`, `supportsRestartRequest`, `supportsModulesRequest`, `supportsReadMemoryRequest`, `supportsDisassembleRequest` in `Capabilities`; the fake adapter gains `pause` (a stop with reason `pause` while its program "runs" a long step), `exceptionInfo`, stack and variables paging, large variable sets and deep object graphs on request, and output lines per source.
- `crates/eludite/src/shell/debug.rs`, `debug/state.rs`, `debug/windows.rs` (nothing visible changes except the Debug menu and status bar below), `debug/tests.rs`; `crates/ui/**` for Debug > Break All (Ctrl+Alt+Break; `ctrl-alt-pause` on Linux keymaps, as Visual Studio on Linux keyboards sends it) and the status bar slot's `(agent driving)` suffix.
- `docs/briefs/README.md`, `docs/briefs/0025-report.md` (new).

## Contract

### The stop summary (`debug-stop-summary.output.json`)

- Fields: `mode`, `generation`, `stop`, `stopped` (`reason`, `thread`, `location` with `path`, `line`, `column`, `function`, `end_line`, `end_column` when known; `exception` brief with `type`, `message`, `break_mode` when the reason is an exception; `breakpoint` with `path`, `line`, `hits` when the reason is a breakpoint; `driver`), `frames` (the stopped thread's top `max_frames` frames as `stack` describes them, with `total`), `locals` (the top frame's, expanded to `depth` within `max_variables`, each row as `variables` describes it), `watches` (expression, value or error), `output` (program lines since `output_since` up to `max_output_lines`, with `next` and `dropped`), `exit_code` and `message` when the session ended, `capabilities`, `agent_driving`, `truncated` (true when any list was cut). Nothing else: no breakpoint list, no exception settings, no thread list (those are `state`).
- Budget parameters, all optional, with their defaults and maxima: `depth` 1 (max 5), `max_variables` 50 (max 500), `max_value_chars` 200 (max 10,000), `max_frames` 10 (max 200), `max_output_lines` 20 (max 1,000), `output_since` (a cursor; default: the last 20 lines).
- A value longer than `max_value_chars` is cut and ends with `…` (U+2026) plus ` (N chars)`; the row carries `value_truncated: true`.
- The summary at a timeout of a resuming command or `wait` has `mode` `running` (or `launching`, `building`) and no `stopped`; after the session ends it has `mode` `design`, `exit_code` when known and `message`.
- `start`, `continue`, `step_over`, `step_into`, `step_out`, `run_to_cursor`, `pause` and `wait` answer with the summary, computed when the debuggee settles (or at the timeout), with the default budgets unless the call gave `depth`, `max_variables`, `max_value_chars`, `max_frames`, `max_output_lines` or `output_since` (every resuming command's input schema gains them). From the UI thread (no wait) the summary reflects the mode at once, as today's state does.

### Reading commands

- `snapshot` (read): the parameters above plus `thread?` and `frame?` (defaults: the stopped thread and its top frame). Answers the summary for that frame. Refused when not in break mode unless the session is running: then the summary with `mode` `running` and the latest `output` is answered, so an agent can poll cheaply.
- `stack` (read): `thread?`, `start` (default 0), `count` (default 20, max 200), `all_threads?`, `stop?`. Output: `threads` (one entry when not `all_threads`): `id`, `name`, `frames` (`index`, `name`, `path`, `line`, `column`, `end_line`, `end_column`, `external`: true for frames without source, as Visual Studio's `[External Code]`), `total`, `truncated`. Uses DAP `stackTrace` with `startFrame` and `levels` when `supportsDelayedStackTraceLoading` is true, else fetches once and pages in the shell.
- `variables` (read): either `reference` (from an earlier row) or `thread?`, `frame?`, `scope` (`locals` default; `arguments` and `this` where the adapter's scopes name them, else the rows of `locals` filtered by `presentationHint`), plus `start` (0), `count` (50, max 500), `depth` (1, max 5), `filter?` (a name prefix, matched case-insensitively), `max_value_chars`, `stop?`. Output: `rows` (`name`, `value`, `type`, `reference`, `evaluate_name`, `indexed`, `named`, `value_truncated`, `children` when `depth` > 1, each child row the same shape with its own `truncated`), `total`, `truncated`, `next` (the `start` for the next page). References are valid for the stop they were issued in; an older `stop` is refused as stale (rule 3 of brief 0018).
- `output` (read): `source` (`program` default, `debug`: the shell's own debugger messages, `adapter`: the adapter's stderr), `since` (cursor, default 0), `max_lines` (20, max 1,000), `pattern?` (a substring, or `/regex/`). Output: `lines` (`seq`, `text`, `stream` when known: `stdout`, `stderr`), `next`, `dropped` (lines the ring overwrote since `since`), `total`. The shell keeps a ring of 10,000 lines per source with monotonically increasing sequence numbers per session (`state.console` keeps its tail and gains `next`).
- `exception_info` (read): `thread?`, `stop?`. With `supportsExceptionInfoRequest`: `type` (`exceptionId`), `message` (`description`), `break_mode`, `details` (`message`, `type_name`, `full_type_name`, `stack_trace`, `inner_exceptions` recursively, at most 5 deep), `supported: true`. Without it: `supported: false` and the stop's exception brief. Refused when the stop's reason is not an exception, naming the reason.
- `wait` (read): `until` (`stopped`, `terminated`, `output`: any new program line, `any`; default `any`), `wait_ms` (default 5,000, max 30,000), `stop?`, plus the budget parameters. Answers the summary when the condition holds (with `satisfied` naming which), or with `mode` `running` and `timed_out: true`. `wait` never resumes anything and is never refused for the mode; it is the agent's cheap poll. It wakes within 20 ms of the stop being shown.

### `pause` and the state

- `pause` (execute): Debug > Break All, enabled in `running` mode only; sends DAP `pause` for the stopped thread (or thread 0, all threads), moves nothing in the model until the `stopped` event with reason `pause` arrives, then is shown as any stop (execution point, windows, status bar `Debugging: App (break: pause, …)`). From an agent it waits like a resuming command and answers the summary. Refused in any other mode with the usual message.
- `state` gains `capabilities`: an object of booleans and strings: `pause: true`, `set_variable`, `exception_info`, `function_breakpoints`, `log_points` (`adapter` or `shell`: the shell emulates in brief 0026), `hit_conditions` (`adapter` or `shell`), `exception_filter_options`, `set_next_statement` (`supportsGotoTargetsRequest`), `data_breakpoints`, `step_back`, `restart`, `terminate`, `modules`, `memory`, `disassembly`, `delayed_stack_loading`, `variable_paging` (`supportsVariablePaging`; when false the shell pages), and `adapter` (its id: `coreclr`, `mono`, `fake`). The same object is in the summary. `agent_driving` is true when the last resuming command came from an agent (`last_driver` starts with `agent:`); the status bar slot then ends with `, agent driving`.
- Rule 3: `snapshot`, `stack`, `variables` and `exception_info` with an explicit `thread` or `frame` fetch through DAP without touching `model.thread`, `model.frame`, the Call Stack selection, the Locals window or the execution point. Only `select_frame` moves them.
- Rule 4 of brief 0018 (old answers dropped) holds: every DAP answer these commands wait for carries the generation and stop it was asked in.
- The person sees the same: the Locals window keeps rendering the model; `Break All` appears in the Debug menu with its key; nothing else in the windows changes.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands/src/debug.rs`: every new command parses and validates (defaults, maxima, `scope` values, `reference` versus `thread`/`frame`, `until` values); outputs match their schemas' `required` and `properties`.
- `crates/dap`: the fake adapter's `pause`, `exceptionInfo`, paging and large sets are tested through `DapClient`; `Capabilities` decodes netcoredbg's recorded `initialize` answer (the test in `types.rs`) with the new fields.
- `crates/eludite/src/shell/debug/tests.rs` (fake adapter, headless): `continue` with `wait_ms` answers the summary with `stopped.location`, `frames` (10 of 30, `total` 30, `truncated`), `locals` cut at 50 of 120 with `total` and `truncated`, a 1,000-character string value cut at 200 with `value_truncated`; `snapshot` with `depth: 3` nests children and stops at the budget; `snapshot` with `frame: 2` answers that frame's locals while the Call Stack window's selection and the execution point stay on frame 0; `stack` pages with `start` and `count`, `all_threads` lists both threads, external frames are marked; `variables` by `reference` pages a 10,000-element array in pages of 50 with `next`, `filter` by prefix, `depth: 2` expands, a stale `stop` is refused; `output` cursors: lines printed across two `continue`s are read by cursor without repetition, `dropped` after an overflow, `pattern` filters, `source: debug` holds the shell's messages; `exception_info` at an exception stop gives the details and is refused at a breakpoint stop; `wait` returns on the next stop within 20 ms of it (measured), times out with `mode` `running`, `until: output` returns on a printed line; `pause` from the menu and from an agent stops a running program with reason `pause` and the summary, and is refused in break mode; `capabilities` reflects the fake's `initialize`; `agent_driving` flips with the driver and the status bar shows it; the summary at session end has `exit_code`.
- Real adapters, gated like `crates/dap/tests/netcoredbg.rs` and `mono.rs`: `pause` of a running program, `exceptionInfo` at a first-chance exception, `stackTrace` paging and `variables` paging against netcoredbg (debugging `eludite-host`) and against `eludite-dbg-mono` (debugging brief 0022's TestApp); the report records what each adapter supports.
- Token cost (proposal 0001 risk 1): a test measures the summary's JSON size at the corpus stop (30 locals, 10 frames, 20 output lines) and asserts it is under 8 KB; the report gives the sizes of each command's default answer.
- No display: everything is headless; the report says so.

## Budget

- `snapshot` at a break answers under 50 ms p95 against the fake adapter and under 150 ms p95 against netcoredbg (20 calls each).
- `wait` wakes within 20 ms of the stop being shown.
- The summary with default budgets is under 8 KB of JSON at the corpus stop.
- Frame cost while an agent polls `snapshot` ten times a second stays under 8 ms p99 (the existing headless frame measurement).
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green, with the real-adapter tests running on this machine; `dotnet build` and `dotnet test` unchanged and green.
2. The report records the budget numbers, each command's default answer size, the adapter matrix for the new requests (netcoredbg, `eludite-dbg-mono`, fake), and what brief 0026 (run control: tracepoints, `run_until`, `trace`, function breakpoints, exception filters by type, `set_variable`, `set_next_statement`) needs from this one.
3. The briefs index matches the repository.

## Out of scope

- Tracepoints, `run_until`, `trace`, function breakpoints, exception filters by type, `set_variable`, `set_next_statement` (brief 0026, proposal 0001 B).
- `attach`, `processes`, `restart`, the Attach to Process dialog, the `debug` policy object, the "Allow agents to drive" toggle, `interrupted_by`, transcript rendering of debug commands, the agent guide resource (proposal 0001 C).
- `session` on every command and `sessions` (proposal 0001 D); `modules` (no first-wave adapter has it).
- Windows and macOS runs.
