# Proposal 0001: The agent debugging suite

Status: Proposed, 2026-10-02
Plan reference: PLAN.md sections 2 (principles 1 to 3), 4.5, 5.1, 5.3, 5.4, 5.5, 10 (Phase 4), 11
Related: brief 0018 (debugging), brief 0020 (settings, F5 builds first), ADR-0007, proposal 0002 (browser control and JavaScript debugging)
Builds on: `crates/commands/src/debug.rs`, `crates/dap`, `crates/eludite/src/shell/debug/`

## 1. Goal

An agent can do everything with the debugger that a person can do from the Debug menu and the debugger windows, through commands that return compact, typed answers sized for a model's context, and the person watching sees every step in the same windows. PLAN.md 5.5 calls this the headline feature. Brief 0018 built the foundation: thirteen `eludite.debug.*` commands, one state machine for two drivers, and a state output the windows and the agent both read. This proposal fills in the rest of the suite, defines the rules that make a long agent-driven session safe and legible, and sequences the work into briefs.

The proving scenario is PLAN.md's Phase 4 exit: an agent takes a failing test to a reviewed, passing fix without the human leaving the IDE. For this proposal the measurable version is: given a seeded bug in the corpus and the prompt "this test fails, find out why", a real Claude Code session reaches the faulting statement with the relevant locals in its context in at most eight tool calls, and a scripted fake agent does it in a headless test on every CI platform.

## 2. What exists today

From briefs 0018 and 0020 (see the 0018 report, sections 2.2 and 3):

- Commands: `start` (project, debug or run, profile, build first), `stop`, `continue`, `step_over`, `step_into`, `step_out`, `run_to_cursor`, `toggle_breakpoint` (toggle, set with condition and hit condition, enable, delete, delete all), `evaluate` (frame, context, expand), `state`, `select_frame`, `watch`, `exception_settings`.
- Resuming commands take `wait_ms` (up to 30 s), so an agent's `continue` returns when the debuggee stops again, with the new state.
- The two-driver rules: one queue, refused never queued, stale stops refused by `stop` and `generation`, old adapter answers dropped, both drivers read one state, editing breakpoints always allowed.
- `debug-state.output.json`: mode, generation, stop, session, stopped (reason, location, driver), threads, frames of the current thread, top-level locals, watches, breakpoints, exception settings, the console tail, `last_driver`.
- Hit counts and Run To Cursor are implemented in the shell because netcoredbg lacks them, which sets the pattern for everything an adapter lacks.

## 3. What is missing

Organized by the questions an agent asks while debugging.

| Question | Gap today |
|---|---|
| "Where am I and why did it stop?" | `state` answers, but with the whole model every time (all breakpoints, all watches). No one-call summary sized for context, no exception details beyond the stop reason. |
| "What is inside this variable?" | `evaluate` with `expand` gives one level. No paging through a 10,000-element list, no depth-limited expansion, no filtering. |
| "What happened before this frame?" | The current thread's frames only; no paging for deep stacks, no other threads' stacks without selecting each. |
| "What did the program print?" | A console tail with no cursor, so an agent cannot read "everything since my last call". |
| "Run until X without stopping everywhere" | Breakpoints plus `continue` work, but every hit stops and costs a round trip. No tracepoints (log without stopping), no "run to these points, then tell me". |
| "Stop it, it is hanging" | No `pause`. |
| "Debug the already-running process" | No attach, no process list, no Attach to Process dialog. |
| "Change a value and continue" | No `set_variable`. |
| "Break on this exception type only" | `all` and `user-unhandled` filters only; netcoredbg advertises `supportsExceptionFilterOptions`, so a condition such as `System.InvalidOperationException` is possible and unused. |
| "Debug this test" | No Test Explorer yet (PLAN.md 4.6). |
| "Debug the server and the browser" | One session at a time. A full-stack launch (Kestrel under netcoredbg, the page under js-debug) needs two. |
| "Is the agent allowed to do this?" | Debug commands are `execute` class; there is no debug-specific policy, no way for the person to stop an agent from driving while they drive. |
| "What is the agent doing to my session?" | Transcript rows show tool calls generically; the debugger windows follow the shared model but there is no "agent is driving" signal. |

## 4. Design rules

These extend the two-driver rules of brief 0018 and apply to every command below.

1. **One command for both drivers, always.** Every new command is on the bus with schemas in `protocol/schemas/` first, `agent_visible: true`, and a place in the Debug menu, a window or a dialog for the person. `wait` and `snapshot` have no button because a person gets the same thing by looking, but they are still bus commands any caller may invoke (invariant 3 forbids agent-only APIs, not commands without a menu item).
2. **Outputs are budgeted.** Every read answers within a default budget and says when it cut something off. Defaults: 50 variables per call, 200 characters per value, 10 frames, 20 output lines, 5 levels of expansion at most. Every list carries `total`, `truncated` and a cursor or reference to fetch more. A model should never need to parse a 50 KB answer to find one value.
3. **The agent's frame is a parameter, not state.** `evaluate`, `variables`, `set_variable` and `snapshot` take `thread` and `frame` explicitly and default to the stop's top frame. An agent's call never moves the windows' selected frame. `select_frame` remains the person's window action (an agent may call it, but it is then visibly driving the windows).
4. **Resuming commands settle before they answer.** As today, a resuming command with `wait_ms` returns the next stop or `terminated`, or `running` on timeout. The answer is the compact stop summary (section 5.1), not the full state.
5. **The person always wins.** Any resuming command from the person while an agent's call is waiting ends that wait with `interrupted_by: "user"` and the state the person caused. The agent's next resuming call is refused as stale (existing rule 3) until it re-reads the state. A per-session toggle "Allow agents to drive" (Debug toolbar and status bar slot, default on, policy-settable) refuses every agent resuming and mutating command with a clear message while off; reads keep working.
6. **Shell-side implementations of what adapters lack.** Hit counts and Run To Cursor already are. Tracepoints, `run_until`, `trace` and `wait` follow the same pattern and work on every adapter. Capabilities the shell cannot emulate (set next statement, data breakpoints, memory, disassembly) are advertised per session in the state's `capabilities` so an agent does not try them blindly.
7. **Side effects are named.** `evaluate` may run user code (property getters, method calls); it stays `execute` class. Reads that cannot run code (`state`, `stack`, `output`, `variables` without a format expression, `exception_info`, `wait`, `snapshot`) are `read` class. Attaching to a process Eludite did not start is `dangerous`.
8. **Everything is audited and visible.** Each agent debug command is an audit entry (brief 0016) and a transcript row rendered as the person would see it in the Debug toolbar: `Step Over → stopped at Program.cs:42 (breakpoint)`. Screens the agent read (snapshots) are linked from the row so the person can see what the agent saw.

## 5. The command surface

All ids are `eludite.debug.*`; schemas are `protocol/schemas/debug-<name>.{input,output}.json`. Class is the permission class of PLAN.md 5.3. "Shell" means the shell implements it on every adapter; "adapter" means it needs the DAP capability and is refused with a message otherwise.

### 5.1 Reading

| Command | Class | Input | Output | Where |
|---|---|---|---|---|
| `snapshot` | read | `thread?`, `frame?`, `depth` (default 1), `max_variables`, `output_since?` | The stop summary: `mode`, `generation`, `stop`, `stopped` (reason, location, exception brief), top frames, locals expanded to `depth` within the budget, watches, output since the cursor, `capabilities`, `truncated` | shell |
| `stack` | read | `thread?`, `start`, `count`, `all_threads?` | Frames with source locations, external-code marks, `total` | adapter (`stackTrace`) |
| `variables` | read | `reference` or (`thread?`, `frame?`, `scope`), `start`, `count`, `depth`, `filter?` (name prefix) | Rows with `reference` for children, `total`, `truncated` | adapter (`variables`), paging in shell |
| `output` | read | `source` (`program`, `debug`, `adapter`), `since` (cursor), `max_lines`, `pattern?` | Lines, `next` cursor, `dropped` if the ring overflowed | shell |
| `exception_info` | read | `thread?` | Type, message, inner exceptions, stack text, `break_mode` | adapter (`exceptionInfo`), netcoredbg has it |
| `wait` | read | `until` (`stopped`, `terminated`, `output`, `any`), `wait_ms`, `stop?` | The stop summary, or `running` on timeout, or `interrupted_by` | shell |
| `processes` | read | `filter?` | Candidate processes for attach: pid, name, command line, runtime (`dotnet`, `netfx`, `mono`, `native`), `launched_by_eludite` | shell |
| `modules` | read | | Loaded modules with paths and symbol state | adapter (`modules`); netcoredbg lacks it |
| `state` | read | (unchanged) | The full model, plus `capabilities` and `agent_driving` | existing |

### 5.2 Breakpoints and settings

`toggle_breakpoint` is extended rather than duplicated, as the Breakpoints window's one command:

- `log_message`: a tracepoint. On hit the shell evaluates each `{expression}` in the message in the hit frame, appends the line to the `debug` output source, and resumes without showing a stop. VS calls this "When Hit... Print a message and Continue". `execute` class when the message contains an expression, since it runs code.
- `function`: a function breakpoint by name (`Namespace.Type.Method`), through `setFunctionBreakpoints`.
- `exception_settings` gains `types`: a list of exception type names with `break_when_thrown` and `break_when_user_unhandled` each, mapped to `exceptionFilterOptions` conditions where the adapter supports them (netcoredbg does) and refused where it does not.
- `data_breakpoint` ("Break when value changes"): adapter only; none of the first-wave adapters supports it, so it is in the schema and the Breakpoints window's menu as a disabled item until one does.

### 5.3 Execution

| Command | Class | Notes |
|---|---|---|
| `pause` | execute | Break All (Ctrl+Alt+Break). Missing today. |
| `attach` | execute for a process Eludite launched (Ctrl+F5), dangerous otherwise | `pid` or `process_name`, `adapter?`, `transport?` (D7). The Attach to Process dialog is the person's surface. |
| `restart` | execute | Stop then start with the same configuration (Ctrl+Shift+F5). |
| `set_variable` | execute | `thread?`, `frame?`, `name` or `reference` + `name`, `value`. Through `setVariable` or `setExpression`. |
| `set_next_statement` | execute | `path`, `line`. Adapter only (`goto`); netcoredbg refuses, ICorDebug and CodeLLDB can. |
| `run_until` | execute | `points` (path, line, condition), `wait_ms`, `remove_after` (default true). Sets one-shot breakpoints, resumes, answers with the stop summary. One round trip for "get me to X". |
| `trace` | execute | `points` (path, line, message, condition), `run` (`start`, `continue`), `until` (`terminated`, `stopped`, `hits` with a count), `wait_ms`, `max_hits`. Installs tracepoints, runs, answers with the collected lines in order, then removes them. The agent's "instrument and run" in one call. |
| `continue`, the steps, `run_to_cursor`, `stop`, `start` | execute | Unchanged; their waiting answer becomes the stop summary. |

### 5.4 Sessions

`session` becomes an optional argument on every command, defaulting to the active session. `sessions` (read) lists them. `start` accepts a `compound` launch (a named set of launch configurations, PLAN.md section 7: start Kestrel, then attach the browser debugger). The Call Stack and Threads windows gain a session selector as VS's do with multiple processes. This is the piece proposal 0002 needs for full-stack debugging.

### 5.5 Policy

`agents-policy.json` gains a `debug` object: `drive` (`allow` default, `prompt`, `deny`: whether agents may resume, mutate or start sessions; reads are always allowed), `attach` (`prompt` default, `deny`), `evaluate` (`allow` default, `prompt`, `deny`: whether agents may run `evaluate`, `set_variable` and expression tracepoints). Rules by tool name keep working for finer control.

### 5.6 Test debugging

`eludite.test.debug` (filter or test id, breakpoints) belongs to the Test Explorer brief (MTP, PLAN.md 4.6). It launches the test host under the right adapter and hands the session to this suite; nothing here is test-specific.

## 6. Adapter matrix

What each adapter gives the suite. "shell" means the shell provides it on that adapter.

| Feature | netcoredbg (.NET Core) | Mono soft debugger (proposed, .NET Framework off Windows) | `eludite-dbg-netfx` (ICorDebug, Windows, brief 0004) | CodeLLDB or lldb-dap (Rust) | vscode-js-debug (browser, Node) |
|---|---|---|---|---|---|
| Breakpoints, stepping, stack, locals, evaluate | adapter | adapter | adapter (v1 scope) | adapter | adapter |
| Hit conditions | shell | adapter | shell until built | adapter | adapter |
| Tracepoints | shell | shell | shell | adapter (`supportsLogPoints`) or shell | adapter |
| Function breakpoints | adapter | adapter | adapter | adapter | not applicable |
| Exception filters by type | adapter (`exceptionFilterOptions`) | adapter | adapter | adapter (C++ throw, signals) | adapter (caught, uncaught) |
| Pause | adapter | adapter | adapter | adapter | adapter |
| Attach | adapter | adapter (agent already listening) | adapter | adapter | adapter (CDP) |
| Set variable | adapter | adapter | adapter | adapter | adapter |
| Set next statement | no | no | adapter | adapter | no |
| Exception info | adapter | adapter | adapter | partial | adapter |
| Modules, memory, disassembly | no | no | later | adapter | no |
| Data breakpoints | no | no | no | adapter (watchpoints) | no |

The Mono adapter is the one described in ADR-0007's option 2: a C# DAP server on Mono.Debugging.Soft (MIT, mono/debugger-libs), launching `mono --debug --debugger-agent=...`. It is sized in section 8 and runs the same suite with no shell changes beyond adapter selection by target framework and platform.

## 7. The person's side

- **Debug toolbar additions:** Break All, Restart, Attach to Process (dialog listing `processes`), and the "Allow agents to drive" toggle. The status bar slot reads `Debugging: App (break, agent driving)` while an agent's command caused the last stop.
- **Transcript rows** for agent debug commands show the action and the result on one line, with the stop location clickable (opens the file at the line) and the snapshot the agent received expandable.
- **Breakpoints window:** tracepoint and function-breakpoint rows with their glyphs (VS's diamond for tracepoints), the per-type exception settings tree in Exception Settings.
- **Output window:** the `debug` source carries tracepoint lines and the agent's `trace` output, so the person sees the same lines the agent collected.

## 8. Briefs

In order. Sizes are agent-weeks from the brief 0018 baseline (one week covered the DAP client, the state machine and seven windows).

| Brief | Content | Size | Depends on |
|---|---|---|---|
| A. Inspection depth | `snapshot`, `stack` and `variables` paging, `output` cursors, `exception_info`, `pause`, `wait`, the output budgets, `capabilities` in the state, the stop summary as every resuming command's answer | 1 | none |
| B. Run control | tracepoints, `run_until`, `trace`, function breakpoints, exception filters by type, `set_variable`, `set_next_statement` where supported | 1 | A |
| C. Attach and policy | `attach`, `processes`, `restart`, the Attach to Process dialog, the `debug` policy object, the drive toggle, interrupted waits, transcript rendering, the agent guide resource (section 9) | 1 | A |
| D. Multi-session | `session` on every command, `sessions`, compound launch, window session selectors | 1 | A; needed by proposal 0002 |
| E1. Mono adapter | `debuggers/mono`: C# DAP server over Mono.Debugging.Soft; selection by target framework and platform; fetch and discovery like netcoredbg | 1 | none |
| E2. Rust adapter | CodeLLDB or lldb-dap discovery and fetch, cargo launch configuration from the target, Rust rows in the Workspace window's Debug menu | 0.5 | none (sized in the 0018 report) |
| E3. JavaScript adapter | vscode-js-debug fetch and discovery (needs Node at run time), attach to the embedded browser over CDP or to an external Chrome | 0.5 | D, proposal 0002 |
| F. Proving scenario | The seeded-bug corpus, the fake-agent headless test on every platform, the recorded Claude Code run with its transcript and numbers | 0.5 | A to C |
| G. Conformance corpus | Recorded DAP sessions per adapter replayed in CI (PLAN.md section 11) | 0.5 | E |

Brief 0004 (ICorDebug) is unchanged by this proposal and remains the Windows item.

## 9. Guidance for agents

The tool descriptions are the first guidance, and they are written for a model: each says when to use the tool, what it costs, and what to call next. A short guide, `docs/agents/debugging.md`, is exposed as an MCP resource (`eludite://guides/debugging`) and says, in order: read `snapshot` before acting; prefer `run_until` and `trace` over single steps; pass `stop` on every resuming call; read `output` by cursor; what to do when a call returns `interrupted_by: "user"`. Claude Code's ACP session gets the resource listed at start.

## 10. Changes to PLAN.md on acceptance

- 4.5: add tracepoints, `run_until` and `trace` as shell-side features on every adapter; attach and the Attach to Process dialog in Phase 2; the per-session agent-drive toggle.
- 5.3: the `debug` policy object.
- 5.5: replace the paragraph with a pointer to this proposal's rules (section 4) and command surface (section 5).
- 10: move "agent-driven debugging" from Phase 4 to Phase 2, since briefs A to C need nothing from Phase 3; Phase 4 keeps test-to-fix orchestration.

## 11. Risks

- **Context cost.** If the stop summary is too large in practice, agents stop reading it; the budgets in rule 2 are defaults and the proving scenario (brief F) measures tokens per call.
- **Expression evaluation side effects.** `evaluate` can mutate state in the debuggee (a property getter with effects). The policy knob exists; the guide tells agents to prefer `variables` over `evaluate` for plain reads.
- **Adapter gaps hidden by shell emulation.** Tracepoints emulated in the shell cost a stop and a resume per hit (about 10 ms with netcoredbg per the 0018 numbers); a hot tracepoint slows the debuggee visibly. `trace` caps hits with `max_hits` and reports the overhead.
- **Node for js-debug.** The owner prefers not to depend on Node (brief 0006). js-debug is the only complete permissive browser debugger with source maps. The fallback is a Rust DAP adapter over CDP's `Debugger` domain, about two agent-weeks for breakpoints, stepping, scopes and evaluate, more for source maps; it is listed in proposal 0002 as a later option.
