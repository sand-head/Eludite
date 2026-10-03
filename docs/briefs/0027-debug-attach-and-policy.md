# Brief 0027: Attach, restart and the debug policy for the agent debugging suite

Status: open
Phase: 2 (proposal 0001, brief C)
Plan reference: PLAN.md sections 2 (principles 1, 3, 5), 4.5 (attach to process, the Attach to Process dialog, the per-session agent-drive toggle), 5.2 (the IDE MCP server), 5.3 (the `debug` policy object), 5.5, 8 (dialogs), 9, 10 (Phase 2); proposal 0001 sections 4 (rules 5, 7, 8), 5.1 (`processes`), 5.3 (`attach`, `restart`), 5.5, 7, 8 (C), 9
Related ADRs: ADR-0003, ADR-0007, ADR-0009 (per-call permission escalation, brief 0024)
Depends on: brief 0025 (the stop summary, `wait`, `capabilities`), brief 0026 (run control), brief 0024 (ADR-0009's escalation hook and the policy plumbing it adds to `crates/commands`). Runs after both merge; must not touch the browser briefs' files beyond the shared hook.

## Goal

The person and the agent can start a debugging session from a running process, restart one, and the person stays in charge of what agents may do to a session. `eludite.debug.attach` (by pid or process name, through the adapter chosen by the process's runtime) and `eludite.debug.processes` (the candidates, with their runtime and whether Eludite launched them) exist with the Attach to Process dialog (Debug > Attach to Process..., Ctrl+Alt+P) as the person's surface; `eludite.debug.restart` is Debug > Restart (Ctrl+Shift+F5). `agents-policy.json` gains the `debug` object of proposal 0001 section 5.5 (`drive`, `attach`, `evaluate`), applied through ADR-0009's escalation hook; a per-session "Allow agents to drive" toggle in the Debug toolbar and status bar refuses agents' resuming and mutating commands while off. The person always wins (rule 5): a resuming command from the person ends an agent's waiting call with `interrupted_by: "user"`. Agent debug commands render in the Agents window's transcript as a person would see them in the Debug toolbar (`Step Over → stopped at Program.cs:42 (breakpoint)`) with the snapshot the agent received expandable, and the agent guide `docs/agents/debugging.md` is an MCP resource (`eludite://guides/debugging`) listed to every hosted agent.

## Files in scope

- `protocol/schemas/` first and alone: `debug-attach.input.json`, `debug-processes.{input,output}.json`, `debug-restart.input.json`, `debug-allow-agents.{input,output}.json` (the toggle, a command like every window action), `agents-policy.json` (the `debug` object), `debug-state.output.json` (`agent_driving` exists; add `agents_allowed`), `debug-stop-summary.output.json` (`interrupted_by`), `mcp-resource.json` (new: how a guide is exposed as an MCP resource, `resources/list` and `resources/read` shapes), `command-spec.json` only if the hook's description field needs a debug-specific note.
- `docs/agents/debugging.md` (new): the guide of proposal 0001 section 9, in order: read `snapshot` before acting; prefer `run_until` and `trace` over single steps; pass `stop` on every resuming call; read `output` by cursor; what to do on `interrupted_by: "user"`; what the policy may refuse and how it reads.
- `crates/commands/src/debug.rs` (requests, outputs, specs: `attach` is `execute` for a process Eludite launched and `dangerous` otherwise through the hook; `processes` is `read`; `restart` is `execute`; `allow_agents` is `execute`), `crates/commands/src/policy.rs` (the `debug` object: `drive` `allow` default, `prompt`, `deny`; `attach` `prompt` default, `deny`; `evaluate` `allow` default, `prompt`, `deny`; its decision for a debug command given the request kind), `crates/commands/src/audit.rs` only if the audit entry needs a field.
- `crates/dap/**`: `attach` plans per adapter (netcoredbg: `processId`; `eludite-dbg-mono`: `address` and `port` of a program started with `--debugger-agent=...,server=y`, so attach to a Mono process means the person started it that way and the dialog says so), process listing (`/proc` on Linux, `ps` on macOS, `tasklist` or the Windows API on Windows; runtime detection: `dotnet` and the `.dll` in the command line, `mono`, else `native`), `restart` through DAP `restart` when `supportsRestartRequest`, else stop and start with the same configuration; the fake adapter gains `attach` and `restart`.
- `crates/mcp/**`: `resources/list`, `resources/read` and `resources/templates/list` (empty), with `eludite://guides/debugging` served from the guide file compiled in; `initialize` advertises `resources`.
- `crates/eludite/src/shell/debug.rs`, `debug/state.rs`, `debug/windows.rs` (the Attach to Process dialog: a list with pid, name, command line, runtime, "launched by Eludite", a filter box, Refresh, Attach; the toggle in the status bar slot), `debug/tests.rs`, `shell/agents/transcript.rs` and `agents/window.rs` (the debug rows), `shell.rs` for the menu items' enabling (Restart and Attach), `crates/ui/**` (menu items, Ctrl+Alt+P, Ctrl+Shift+F5, the toggle's control), `shell/agents.rs` (listing the resource to sessions; the gate uses the policy's `debug` decision).
- `docs/briefs/README.md`, `docs/briefs/0027-report.md` (new).

## Contract

### Attach and processes

- `processes` (read): `filter?` (a substring of the name or command line). Output: `processes` (`pid`, `name`, `command_line` cut at 500 characters, `runtime`: `dotnet`, `netfx`, `mono`, `native`, `unknown`; `launched_by_eludite`: the pid of a program this shell started with Ctrl+F5 or F5 in this session, or its descendant), `total`, `truncated` (at most 500 rows). Runs off the UI thread (a `debug-attach` worker), and the dialog shows it with a Refresh button.
- `attach` (execute; dangerous when the process is not `launched_by_eludite`, through ADR-0009's hook, which also reads the policy's `debug.attach`: `deny` refuses with the policy named): `pid` or `process_name` (unique match required, else refused listing the matches), `adapter?` (`coreclr`, `mono`, `netfx`; default from the runtime), `transport?` (ADR-0007: `{ kind: "tcp", host, port }` for an adapter elsewhere), `mono?` (`address`, `port` of a Mono program's debugger agent), `wait_ms` and the budget parameters. Starts a session in mode `launching` then `running` (or `break` when `stopAtEntry`-like behavior applies), with `session.attached: true` and no `program` build; every debugger window, key and command works as for a launch; Stop detaches (`disconnect` with `terminateDebuggee: false`) and the process keeps running, which the status bar says. On Windows, `netfx` is refused with brief 0004's message as in brief 0022. The dialog's Attach runs the same command.
- `restart` (execute): `wait_ms` and the budget parameters. With `supportsRestartRequest` the adapter restarts; otherwise the shell stops (as Shift+F5) and starts the same configuration (project, profile, debug flag, build-before-run as configured). Refused in `design` mode and for attached sessions (Visual Studio disables Restart there too), with the reason.

### The debug policy and the toggle

- `agents-policy.json` gains `debug`: `drive` (`allow` default, `prompt`, `deny`): whether agents may start sessions (`start`, `attach`, `restart`) and resume or mutate them (`continue`, the steps, `run_to_cursor`, `run_until`, `trace`, `pause`, `stop`, `set_variable`, `set_next_statement`, breakpoint edits with `log_message` expressions); `attach` (`prompt` default, `deny`); `evaluate` (`allow` default, `prompt`, `deny`): `evaluate`, `set_variable` and expression tracepoints. Reads are always allowed. Through ADR-0009's hook: `prompt` escalates the call to `dangerous` (the Agents window prompts, Always Allow writes `allow`), `deny` refuses with the policy named. Rules by tool name keep working and are checked first.
- `allow_agents` (execute): `enabled` (boolean). Per session, default on, the setting `debugger.allowAgentsByDefault` for the default (brief 0020's settings store). While off, every agent resuming, starting or mutating command is refused with `agents are not allowed to drive this session (Debug > Allow Agents to Drive)`; reads keep working. The control: a check item in the Debug menu and a toggle in the status bar slot; `state.agents_allowed` says it; the person's own commands are never affected.

### The person always wins (rule 5)

- While an agent's resuming command (or `wait`, `run_until`, `trace`) waits for the debuggee to settle, a resuming command from the person (continue, a step, Run To Cursor, Stop, Restart) ends the wait at once: the agent's answer is the stop summary of whatever state the person caused with `interrupted_by: "user"` (and `trace` returns what it collected with `stopped_by: "interrupted"`). The agent's next resuming call is refused as stale until it re-reads the state (brief 0018 rule 3, through `stop`). Documented in the guide.

### Transcript rendering and the guide

- Agent debug commands render in the Agents window as one line each: the action as the Debug menu names it and the result as the status bar would (`Step Over → stopped at Program.cs:42 (breakpoint)`, `Continue → exited (0)`, `Run Until → interrupted by you`), the location clickable (opens the file at the line, as Error List rows do), and the stop summary the agent received expandable (the existing tool-result expansion). Refused calls render the refusal.
- `docs/agents/debugging.md` is served as the MCP resource `eludite://guides/debugging` (`text/markdown`); `resources/list` lists it; Claude Code's session gets it listed at start (the ACP `initialize`/MCP capabilities already carry the endpoint). The guide is under 2,000 words and every command it names exists.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands`: the new commands parse and validate; the `debug` policy parses, defaults and decides for each request kind; the hook escalates `attach` for a foreign pid and `evaluate` under `prompt`.
- `crates/dap`: process listing on Linux (this process and a child it spawns appear with the right runtime), attach plans per adapter, `restart` with and without the capability against the fake.
- `crates/eludite` headless tests (fake adapter): `attach` to a fake-launched process starts a session with `attached: true`, windows populated, Stop detaches and the process keeps running; `processes` lists the Ctrl+F5 program as launched by Eludite; the dialog filters, refreshes and attaches through the bus; `restart` with the capability and without it (stop and start, build-before-run honored); the toggle off refuses an agent's `continue` and `set_variable` but not `snapshot`, and the person's F10 still works; `state.agents_allowed` and the status bar; the policy `drive: deny` refuses an agent's `start`, `prompt` escalates to a prompt the window shows; an agent's `continue` with `wait_ms` is interrupted by the person's F10 and answers `interrupted_by: "user"`, and its next `continue` with the old `stop` is refused; `trace` interrupted returns its lines with `stopped_by: "interrupted"`; transcript rows read as specified for a step, a continue to exit and a refusal, with a clickable location; `resources/list` and `resources/read` serve the guide through the MCP endpoint and a session lists it.
- Real adapters, gated: `attach` to a .NET program started with `dotnet` under netcoredbg (skips here: netcoredbg is not available on this machine) and to a Mono TestApp started with `--debugger-agent=...,server=y` under `eludite-dbg-mono`; `restart` of a Mono session.
- No display: headless; the report says so.

## Budget

- `processes` on this machine under 300 ms p95 (20 calls) and off the UI thread.
- `attach` to a running Mono TestApp to the first `stopped` (a breakpoint already set) under 3 s warm.
- An interrupted wait answers within 50 ms of the person's command being applied.
- Frame cost unchanged; no new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the Mono real-adapter tests running; `dotnet build` and `dotnet test` unchanged and green.
2. The report records the budget numbers, the policy decision table, the per-adapter attach matrix, and what proposal 0001 brief D (multi-session) and brief F (the proving scenario) need.
3. The briefs index matches the repository; `docs/agents/debugging.md` is linked from `docs/briefs/README.md` or `CLAUDE.md`'s document list.

## Out of scope

- Multi-session, compound launch, session selectors (proposal 0001 D); the seeded-bug corpus and the recorded Claude Code run (brief F); the conformance corpus (brief G).
- Attaching to a Mono process that was not started with a debugger agent (Mono has no late attach); attaching on Windows to .NET Framework (brief 0004).
- Remote process listing (the `transport` argument reaches a remote adapter; listing processes on that machine waits for a brief of its own).
- Windows and macOS runs.
