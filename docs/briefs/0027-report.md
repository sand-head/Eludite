# Brief 0027 report: Attach, restart and the debug policy for the agent debugging suite

Status: done on Linux. Windows and macOS: not run (no machines; the `ps` and `tasklist` process listings are written and
unit-tested on their parsers, not run). CI: not run (nothing pushed). No display: every test is headless.
Branch: `brief/0027-debug-attach-and-policy`, based on `main` at `9c2f5cf` (briefs 0022 to 0026 and 0029 merged).
Date: 2026-10-03. Brief: [0027-debug-attach-and-policy.md](0027-debug-attach-and-policy.md). Guide:
[docs/agents/debugging.md](../agents/debugging.md).

## 1. Summary

- **Attach and processes.** `eludite.debug.processes` (read) lists this machine's processes (`/proc` on Linux) with
  their command line (cut at 500 characters), runtime (`dotnet`, `mono`, `native`, `unknown`; `netfx` reserved for
  Windows), a Mono program's debugger agent, and `launched_by_eludite` (Ctrl+F5's or F5's program in this session, or a
  descendant), off the UI thread on a `debug-attach` thread. `eludite.debug.attach` (by `pid` or a unique
  `process_name`, `adapter`, `transport`, `mono`) starts a session through the adapter of the process's runtime
  (netcoredbg by `processId`, eludite-dbg-mono by the agent's `address` and `port`, lldb-dap by `pid`) with
  `session.attached`; every window, key and command works as for a launch; Stop detaches (`terminateDebuggee: false`)
  and the status bar says `Detached from <name> (process N); it keeps running.` The Attach to Process dialog (Debug >
  Attach to Process..., Ctrl+Alt+P) lists them with a filter box, Refresh and Attach, all through the bus.
- **Restart** (Debug > Restart, Ctrl+Shift+F5): DAP `restart` where the adapter advertises it, else Stop and the last
  start again (project, profile, debug flag, Cargo options, build before run as configured). Refused in design mode and
  for an attached session.
- **The `debug` policy** (`agents-policy.json`: `drive` allow/prompt/deny, `attach` prompt/deny, `evaluate`
  allow/prompt/deny) applied through ADR-0009 escalation hooks on every command that drives, evaluates or attaches;
  `prompt` makes the call dangerous (the Agents window asks; Always Allow writes `allow`), `deny` refuses it naming the
  policy; tool rules are checked first. An attach to a process Eludite did not start is dangerous (allowed once).
- **Allow Agents to Drive**: a per-session switch (`eludite.debug.allow_agents`, Debug menu check item, a status bar
  toggle while debugging, `state.agents_allowed`, the setting `debugger.allowAgentsByDefault`). While off, agents'
  driving commands are refused with `agents are not allowed to drive this session (Debug > Allow Agents to Drive)`;
  reads and the person's commands are unaffected; only the person can turn it on.
- **The person always wins**: a resuming command of the person's (Continue, a step, Run To Cursor, Break All, Stop,
  Restart) ends an agent's waiting `continue`/step/`run_until`/`wait`/`trace` at once with `interrupted_by: "user"`
  (`trace`: `stopped_by: "interrupted"` with its lines); the agent's next driving command is stale until it reads the
  state or quotes the current `stop`.
- **Transcript rows**: `Step Over → stopped at Calc.cs:6 (step)`, `Continue → exited (0)`, `Run Until → interrupted by
  you`, `Continue → refused: …`; the location opens the file at the line; the summary the agent received is folded
  under `Show snapshot`.
- **The guide** `docs/agents/debugging.md` (1,250 words) is the MCP resource `eludite://guides/debugging`
  (`resources/list`, `resources/read`, `resources/templates/list`; `initialize` advertises `resources`); the Agents
  window's start notice names it.
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, headless; three runs each on a quiet machine, load
  average 3; numbers under load in parentheses):

  | Budget | Result |
  |---|---|
  | `processes` under 300 ms p95 (20 calls), off the UI thread | 20 agent calls through the shell (the listing on a `debug-attach` thread): **p95 4.8 / 5.2 / 9.1 ms** (38 ms at load average 10). The listing itself: 2.9 ms for 26 processes. Pass |
  | `attach` to a running Mono TestApp to the first `stopped` (a breakpoint set) under 3 s warm | The TestApp started by `mono --debug --debugger-agent=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:PORT`: through the shell (agent `attach` by pid, then `wait`) **273 / 282 / 318 ms** (697 ms under load); through `eludite-dap` alone **216 / 221 / 222 ms**. Pass |
  | An interrupted wait answers within 50 ms of the person's command being applied | From the person's F10 applied to the agent's answer composed: **2.0 / 2.3 / 2.4 ms** (12.5 ms under load). Pass |
  | Frame cost unchanged | Brief 0026's measurement (an emulated tracepoint firing at 10/s): frame p99 **3.4 to 5.0 ms**, the debugger's message handling p99 **0.84 to 0.88 ms** (brief 0026: 3.5 to 6.1 and 1.1 to 1.5 ms). Pass (section 8, item 12) |
  | No new dependency | Pass: `Cargo.lock` unchanged; processes come from `/proc`, `ps` and `tasklist` through std |

- **Tests:** `cargo test --workspace --no-fail-fast` with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME` and
  `ELUDITE_CHROME_NO_SANDBOX=1`: **635 passed, 0 failed, 1 ignored** (a doc example); the Mono, lldb-dap and Chrome tests
  ran; the netcoredbg tests (four, one new) returned early (section 6). fmt and clippy (`-D warnings`) clean.
  `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors. `dotnet test dotnet/Eludite.slnx`: 171 tests, 164 passed,
  0 failed, 7 skipped (unchanged; no .NET file changed).

## 2. What was built

Commits, in order:

1. `e88ee06` The brief's Status line.
2. `c80fd00` `protocol/schemas/` alone: `debug-attach.input.json` (with `x-eludite-escalates`),
   `debug-processes.{input,output}.json`, `debug-restart.input.json`, `debug-allow-agents.{input,output}.json`,
   `mcp-resource.json`; `agents-policy.json`'s `debug`; `agents_allowed` and `session.attached` in
   `debug-state.output.json`; `interrupted_by` in `debug-stop-summary.output.json`; `stopped_by: interrupted` in
   `debug-trace.output.json`; `debugger.allowAgentsByDefault` in `settings.json`; `x-eludite-escalates` on the fifteen
   debug inputs whose calls the policy can raise or refuse.
3. `39c5df3` `docs/agents/debugging.md`.
4. `52d0fee` `crates/dap`: `processes` (listing, runtime, launched set, parent walk, Mono agent, cwd, alive),
   `attach` (adapters and plans), the fake's `attach` (process id in the `process` event, detach on `disconnect`) and
   `restart` (behind `supportsRestartRequest`); tests (listing, plans, the fake, the real Mono and lldb-dap attaches).
   This commit was made while the new lldb-dap attach test failed in a full run of its file (a `;` in the gate instead
   of `&&`, my mistake); `61576d6` fixed the test (a pause sent before lldb-dap resumed the attached process is lost:
   it now pauses until a stop is reported).
5. `0ab5d25` `crates/commands`: `attach`, `processes`, `restart`, `allow_agents` (requests, validation, outputs,
   classes), `DebugRequest::drives`, `has_expression`, the escalation hooks (`debug_call`, `escalation`), the policy's
   `debug` object (`DebugPolicy::decide`, `decide_for`, `AlwaysAllow::Debug`), `PolicySnapshot::launched`
   (`LaunchedProcesses`); the shell compiled with the new commands refused until commit 9.
6. `a230f39` `crates/mcp`: `resources` (the guide compiled in), `resources/list`, `resources/read` (-32002 for an unknown
   uri), `resources/templates/list`, `resources` in `initialize`, the guide named in `instructions`.
7. `96eb6e6` `crates/ui`: Debug > Restart (Ctrl+Shift+F5), the Allow Agents to Drive check item (`MenuEntry::Check`,
   `MenuBar::set_checked`), Ctrl+Alt+P, `status_toggle` and `StatusBar::render_with`.
8. `5880ba8` The Mono and lldb-dap attach tests end their adapter after detaching (section 8, item 10).
9. `6df788f` The shell (section 2.1) and its tests.
10. `693cf0c` The gated netcoredbg attach test.
11. This report, the brief's Status line, the briefs index and the guide's link.

### 2.1 How the shell does it (`crates/eludite/src/shell/debug.rs`, `debug/state.rs`, `debug/windows.rs`)

- **Attach.** An agent's call is resolved off the UI thread first (`DebugBus::apply` → `resolve_attach`: a name to the
  one process with it, or refused listing the matches; an unknown pid refused). On the UI thread the model begins a
  session in mode `launching` (allowed from `design` and from `running_without_debugging`, whose program keeps running:
  its handle moves aside) and `attach_thread` runs on `debug-attach`: the process from the listing, the adapter
  (explicit, else by runtime), the plan, the adapter reached (`transport`'s TCP, the test connector, or the located
  netcoredbg, eludite-dbg-mono under Mono, lldb-dap), then the same `Launched`, `Connected`, `Started` messages as a
  launch with `StartKind::Attach`. The session row: the process's name as `project`, its first argument as `program`,
  the rest as `args`, `/proc/<pid>/cwd`, `attached: true`.
- **Detach.** Stop sends `disconnect` with `terminateDebuggee: false` tagged `Pending::Detach`; its answer ends the
  session (an adapter may stay up after a detach), with the detached message unless the process exited.
- **Processes.** From an agent: `Followup::Processes`, the listing on a `debug-attach` thread. From the UI thread: the
  listing is started (`DebugMsg::Processes` brings it back to the dialog) and the last one answers at once.
  `launched_by_eludite` comes from `Debugger::launched` (Ctrl+F5's pid, F5's debuggee from the `process` event) and
  `processes::launched_set`. The escalation hooks get the same knowledge as `PolicySnapshot::launched`
  (`Debugger::launched_processes`: by pid a parent-chain walk, by name a listing), set at shell start and when an agent
  starts.
- **Restart.** `capabilities.restart`: DAP `restart` with no arguments, the model resumed (`Pending::Restart`; a failure
  falls back to stop and start). Otherwise `restart_pending` holds the last start (`StartArgs`, recorded by every
  `debug_start`), the session stops, and `maybe_restart` starts it again once the mode is `design` (checked after every
  message batch). An agent's restart answers in the new session (`Followup::Settle::fresh`).
- **Allow Agents to Drive.** `DebugModel::agents_allowed`, reset at each `begin` to `agents_next` (set while no session
  ran) or `agents_default` (the setting). `apply_debug` refuses an agent's `drives()` request while it is off before
  any other rule, and an agent's `allow_agents` with `enabled: true` always.
- **Rule 5.** `Debugger::interrupt` grows with every resuming command, Break All and Restart of the person's (after the
  model's checks pass); each agent follow-up records it when its command is applied (the job loop passes it) and
  answers `interrupted(…)` (the model's summary with `interrupted_by: "user"`) as soon as it changes; the waiters are
  woken by `refresh_debug`. `agent_stale` then refuses the agent's next driving command until a `snapshot`, `state` or
  `wait`, or a quoted current `stop`.
- **Status bar and menu.** The slot ends with `, agents not allowed` while off (else `, agent driving` as before); the
  toggle `debug-allow-agents` sits after the left slots while a session runs; `DebugMenuState` (atomics updated by
  `refresh_debug`) enables Restart (a session that is not attached and not stopping) and Attach to Process... (design
  or Ctrl+F5) and checks Allow Agents to Drive.
- **The dialog** (`windows::AttachDialog`): rows with process, ID, runtime (and the Mono agent's address), Launched by
  Eludite, command line; the filter box narrows the shown rows as it is typed, Refresh and Enter in the box run
  `processes` with the filter, a click selects, a double click, Enter or Attach runs `attach` with the pid and closes;
  Escape and Cancel close; a line says how a Mono program must be started to be attached to.
- **Transcript** (`agents/transcript.rs`, `agents/window.rs`): `debug_line` turns an `eludite.debug.*` call's command,
  arguments and outcome into the line and location, kept on the row's `McpLink`; the window draws the line above the
  card with the location link (`OpenLocation` → `open_at`, the bus's `eludite.file.open`) and `Show snapshot` /
  `Hide snapshot`, which shows the card's result.

## 3. The policy decision table

For an agent's call, after the tool rules (a rule naming the tool turns the policy's refusal into a dangerous call that
the rules decide). "class" is the call's effective class; `execute` is then decided by `execute` (prompt by default).

| Command | Declared | `drive: allow` (default) | `drive: prompt` | `drive: deny` | `evaluate: prompt` | `evaluate: deny` |
|---|---|---|---|---|---|---|
| `state`, `snapshot`, `stack`, `variables`, `output`, `exception_info`, `wait`, `processes` | read | read | read | read | read | read |
| `start`, `restart`, `stop`, `continue`, steps, `run_to_cursor`, `run_until`, `pause`, `set_next_statement` | execute | execute | dangerous, Always Allow writes `drive: allow` | refused | execute | execute |
| `trace` | execute | execute | dangerous | refused | dangerous when a message has an `{expression}` | refused when a message has one |
| `set_variable` | execute | execute | dangerous (Always Allow writes both keys that asked) | refused | dangerous | refused |
| `evaluate` | execute | execute | execute | execute | dangerous, Always Allow writes `evaluate: allow` | refused |
| `toggle_breakpoint` plain | execute | execute | execute | execute | execute | execute |
| `toggle_breakpoint` with a `log_message` `{expression}` | execute | execute | dangerous | refused | dangerous | refused |
| `attach` to a process Eludite started | execute | execute | dangerous | refused | execute | execute |
| `attach` to any other process (or through `transport`) | execute | dangerous, allowed once | dangerous, allowed once | refused | dangerous | dangerous |
| `watch`, `select_frame`, `exception_settings`, `allow_agents` | read/execute | as declared | as declared | as declared | as declared | as declared |

`attach: deny` refuses every agent attach. Allow Agents to Drive off refuses (in the shell, whatever the policy) every
`drives()` request: the rows from `start` to `toggle_breakpoint` with an expression and both `attach` rows; `evaluate`
and the reads keep working. An agent's `allow_agents` with `enabled: true` is refused always.

## 4. Per-adapter attach matrix

| | netcoredbg 3.2.0-1092 (documented; not run here) | eludite-dbg-mono (measured) | lldb-dap 18.1.3 (measured) | Fake adapter | eludite-dbg-netfx |
|---|---|---|---|---|---|
| Runtime that selects it | `dotnet` (`dotnet <app>.dll`) | `mono` (first argument `mono`, `mono-sgen*`) | `native` | any, through the test connector | `netfx` |
| `attach` arguments | `processId` (plus `name`, `type`, `request`, `justMyCode`) | `address`, `port` of the program's agent (from `--debugger-agent=…,server=y,address=HOST:PORT`, or `mono`) | `pid` | `processId` or `pid` (named in its `process` event) | refused: brief 0004's message on Windows, "Windows only" elsewhere |
| Attach to first stop | gated test (`netcoredbg_attaches_to_a_dotnet_process_and_detaches`) skips | 216 to 222 ms (`eludite-dap`), 273 to 318 ms (shell) to a TestApp waiting with `suspend=y` | attach 380 to 525 ms to a running `sleep`; `pause` then stops it | immediate | |
| A program without the debugger's support | | Refused before connecting: "Mono has no late attach: … start the program with `mono --debug --debugger-agent=transport=dt_socket,server=y,address=127.0.0.1:PORT,suspend=n`" | needs ptrace (works as root here) | | |
| Detach (`terminateDebuggee: false`) | documented | the program runs on and exits with its code (3); **the adapter stays up and busy-loops** until killed (section 8, item 10) | the process keeps running; the adapter exits | `terminated` without `exited` | |
| Restart | `supportsRestartRequest` not advertised: stop and start | not implemented: stop and start (measured in the shell test) | `supportsRestartRequest`: DAP `restart` (not exercised through the shell here) | behind `supportsRestartRequest` | |

## 5. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/dap/src/processes.rs` | 5 new | Runtimes from command lines (`dotnet` with a `.dll`, a script running it, `mono`, `mono-sgen`, native, unknown, Windows image names); joining and cutting command lines; the Mono agent's address (server only, a bare port); `/proc/<pid>/stat` with parentheses in the name, `ps` and `tasklist` output; launched sets through parents |
| `crates/dap/src/attach.rs` | 2 new | Plans per adapter (netcoredbg `processId`, Mono `address`/`port` and the no-agent refusal, lldb-dap `pid`, netfx on Windows and elsewhere); adapters by runtime |
| `crates/dap/tests/attach.rs` | 3 new | The Linux listing: this process (native), a `dotnet App.dll` stand-in (dotnet, parent this process), a program run as `mono` with its agent, a plain program; the launched set and the parent walk; cwd; `alive` before and after a kill; no kernel threads. The fake through `DapClient`: `attach` with the plan's `processId`, a breakpoint stop, detach without `exited`. `restart` with the capability (a new `process` event, the same breakpoint stops again) and refused without it |
| `crates/dap/tests/mono.rs` | 1 new | The real eludite-dbg-mono attaching to a waiting TestApp through the listing's runtime and agent address, the budget, detach, the program's own exit code and output |
| `crates/dap/tests/lldb.rs` | 1 new | The real lldb-dap attaching to a running process by pid, `pause`, detach, the process alive |
| `crates/dap/tests/netcoredbg.rs` | 1 new (skips here) | netcoredbg attaching to `dotnet eludite-host.dll` by `processId`, the `Ping` breakpoint, detach, the host alive |
| `crates/commands/src/debug.rs` | 3 new, 1 updated | The four commands parse and validate (targets, adapters, `mono`, `transport`, bounds, the dialog form); classes and `escalates`; `drives()`; outputs against their schemas (`interrupted_by`, `stopped_by: interrupted`, `attached`, `agents_allowed`, a brief 0026 state without it read as allowed); the hooks: defaults, a foreign pid and name dangerous and once, a launched pid and name not, `transport` foreign, `drive: deny` refusing every driving command and not `evaluate`, `attach: deny`, `prompt` raising with Always Allow's keys, expressions in tracepoints, plain breakpoints untouched, rules first; classes through the registry |
| `crates/commands/src/policy.rs` | 1 new | The `debug` object parses, rejects unknown keys and values, matches the schema, defaults, decides, combines a foreign attach with `drive: prompt`, remembers `allow` and saves sorted; `decide_for` lets a rule decide |
| `crates/commands/src/settings.rs` | 1 updated | `debugger.allowAgentsByDefault` in the schema's list, default true |
| `crates/mcp/src/tests.rs` | 2 new, 1 updated | `initialize` advertises `resources`; `resources/list`, `resources/read`, `resources/templates/list` against `mcp-resource.json`; the text is the file; -32002 and -32602; the guide under 2,000 words, every `eludite.debug.*` it names a command, its sections in proposal 0001's order, the toggle's message in it |
| `crates/ui` | 1 new, 1 extended | Restart after Stop Debugging with Ctrl+Shift+F5, Attach to Process... with Ctrl+Alt+P, the check item's arguments; the keys in the keymap table |
| `shell/debug/tests.rs` (fake adapter, headless) | 9 new, 1 updated | `attach_to_the_ctrl_f5_program_lists_it_and_stop_detaches` (processes: the Ctrl+F5 program launched by Eludite with runtime dotnet, this process not, the budget; the hook's classes; an agent's attach: `attached`, the fake got `attach` with the pid, the windows populated, a step; Restart refused and disabled; Shift+F5 detaches, the status bar, the process alive; unknown pid, unknown name and the dialog form refused for agents). `the_attach_dialog_filters_refreshes_and_attaches_through_the_bus`. `restart_uses_the_adapters_restart_or_stops_and_starts_again` (Ctrl+Shift+F5, the build before the new launch, a new generation, the break; an agent's restart answering in the new session; refused in design). `restart_goes_through_the_adapter_where_it_has_restart`. `allow_agents_off_refuses_their_driving_but_not_their_reads_or_the_person` (the status bar toggle, the slot, the menu check, four refusals, reads, the agent's enable refused, the person's F10, the menu item back on, the agent turning it off, the setting for the next session, the toggle set in design mode). `the_person_always_wins_an_agents_wait` (F10 ends a `wait` with `interrupted_by`, the 50 ms budget, stale with and without the old stop, Break All ending a `continue` on a loop, a quoted current stop accepted). `an_interrupted_trace_answers_its_lines`. `agent_debug_commands_read_in_the_transcript_as_the_debug_toolbar_would` (a scripted fake agent through the MCP endpoint: six rows read as specified, the location, the fold, the click opening Calc.cs at line 6). `the_debug_policy_refuses_or_asks_through_the_agents_window` (`drive: deny` refusing the start, audited with the reason, the read running; `drive: prompt` asking with the reason, Always Allow writing `drive: allow`, the start running). `output_by_cursor_exception_info_and_wait` updated (section 8, item 14) |
| `shell/debug/tests.rs` (real adapter) | 1 new | `attach_and_restart_against_eludite_dbg_mono`: the listing's Mono row with its agent, attach by pid with a breakpoint set, the budget, detach and the program's exit code 3; then F5 and an agent's restart (stop and start) breaking at the same line |
| `shell/agents/tests.rs` | 1 new | The endpoint serving the guide (`initialize`, `resources/list`, `resources/read`, templates) and the start notice naming it |
| `shell/agents/transcript.rs` | 1 new | `debug_line` for a step, a continue to exit, an interrupted `run_until`, a refusal, Start Without Debugging, `trace`, `evaluate`, `processes`, and nothing for other commands |

## 6. How the real-adapter tests ran

`dotnet build dotnet/Eludite.slnx`, then
`ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe`. The Mono tests
(four in `crates/dap/tests/mono.rs`, one new; three in the shell, one new) ran against Mono 6.8.0.105 at `/usr`; the
lldb-dap tests of `crates/dap/tests/lldb.rs` (three, one new) against lldb-dap 18.1.3 on `PATH`; the Chrome tests with `ELUDITE_CHROME=/opt/pw-browsers/chromium-1194/chrome-linux/chrome`
and `ELUDITE_CHROME_NO_SANDBOX=1`. The four netcoredbg tests returned early ("skipped: netcoredbg was not found…",
counted as passed): `tools/netcoredbg/fetch.sh` downloads from github.com, which this machine's proxy refuses.

## 7. How to reproduce

```
dotnet build dotnet/Eludite.slnx
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test --workspace
cargo test -p eludite -- attach_to_the_ctrl the_person_always attach_and_restart_against a_tracepoint_firing --nocapture --test-threads=1
cargo test -p eludite-dap --test attach --test mono --test lldb -- --nocapture
ELUDITE_NETCOREDBG=$(tools/netcoredbg/fetch.sh) cargo test -p eludite-dap --test netcoredbg -- --nocapture
```

## 8. Deviations, decisions and findings

1. **"An agent's `continue` with `wait_ms` is interrupted by the person's F10" cannot happen**: an agent's resuming
   command waits only while the debuggee runs, and F10 is refused then (brief 0018 rule 2: commands are refused, not
   queued, outside break mode). The test interrupts an agent's `wait` with F10 and an agent's `continue` with the
   person's Break All. Break All, which the brief's list of resuming commands leaves out, counts as the person taking
   over (proposal 0001 rule 5's intent: "the state the person caused").
2. **The interrupted answer is immediate**: the model's summary at the moment the person's command was applied (after
   F10, `mode: running`), not the stop the person's step reaches; the 50 ms budget asks for that.
3. **Stale until read**: after an interrupted wait, the agent's next driving command is refused whether it quotes the old
   stop or none, until it reads (`snapshot`, `state`, `wait`) or quotes the current stop (rule 5's "until it re-reads
   the state"). Other reads do not clear it.
4. **`attach.adapter` also takes `lldb`** (native processes through lldb-dap; the brief lists `coreclr`, `mono`,
   `netfx`): the matrix of brief 0029 has lldb-dap attaching by pid, and a `native` process needs an adapter.
5. **`PolicySnapshot` gained `launched`** (`LaunchedProcesses`, a check by pid or name): whether a process is one
   Eludite started is the shell's state, which ADR-0009's `PolicyView` did not carry (its "Revisit when: a hook needs
   state beyond the input and PolicyView, such as the target of a running session"). The ADR is outside this brief's
   files and was not edited; it should get a note that `PolicyView` now also says which processes the shell started.
6. **Schemas beyond the brief's list** (in the schema commit): `debug-trace.output.json` (`stopped_by: interrupted`),
   `settings.json` (`debugger.allowAgentsByDefault`, which the brief names) and `x-eludite-escalates` on the fifteen
   debug inputs the `debug` policy can raise or refuse (ADR-0009 asks every escalating input to say so).
   `command-spec.json` needed no change.
7. **Files edited outside the brief's list, minimally:** `crates/commands/src/settings.rs` (the schema test's key list
   and section count), `crates/commands/src/browser.rs` (a test's `PolicySnapshot` literal), `crates/eludite/src/shell/
   settings.rs` (applying `debugger.allowAgentsByDefault`), `crates/eludite/src/shell/agents/tests.rs` (the endpoint
   test), `crates/dap/tests/*` (the brief names `crates/dap/**`).
8. **Allow Agents to Drive does not govern `evaluate`** (nor watches, frame selection, exception settings, plain
   breakpoints): the switch is about driving; running code through expressions is the policy's `evaluate` key. A
   tracepoint with an expression is driving (the brief lists it under `drive`).
9. **No Debug toolbar exists**: the switch is the Debug menu's check item and the status bar toggle, as the contract's
   control list says; the toggle is drawn after the left slots (a control beside the debug slot, which is text).
10. **eludite-dbg-mono stays up after a detach and busy-loops (about 90% of a core) until killed.** Found because two
    adapters leaked by early test runs loaded the machine. `debuggers/mono` is outside this brief; the shell ends an
    attached session when the `disconnect` is answered and kills the adapter (it would otherwise wait for an exit that
    never comes), and the `eludite-dap` test kills it. A follow-up for the adapter: exit (or idle) after a detach.
11. **"A session lists it"**: Eludite's MCP `initialize` advertises `resources`, so an MCP client such as Claude Code
    lists the guide when it connects; the Agents window's start notice names it (`Starting <agent> (Eludite's MCP
    resources for the agent: eludite://guides/debugging (…))`). The scripted fake agent does not call `resources/list`
    itself (`crates/acp` is outside this brief); the test reads the resources through the session's endpoint.
12. **Frame cost**: the first full runs showed the message-handling share at 3 to 15 ms p99; the cause was the two
    leaked adapters spinning (load average 10); with them gone the numbers match brief 0026's (section 1). The test's
    absolute assertion (share under 8 ms) failed once in a full run under that load.
13. **Windows and macOS listings** are written (`tasklist /FO CSV /NH`, `ps -axww -o pid=,ppid=,args=`), their parsers
    tested, not run. `tasklist` has no command lines or parents: runtimes there come from the image name (`dotnet.exe`,
    `mono*.exe`), `netfx` is not detected (a PE header check of the executable would be needed) and
    `launched_by_eludite` covers the roots only.
14. **An existing test changed**: brief 0025's `output_by_cursor_exception_info_and_wait` had its agent's
    `wait until output` satisfied by output after the person's F5; rule 5 now ends that wait. The agent now continues
    without waiting and waits from the output cursor it holds.
15. **The UI thread's `processes`** answers the last listing and refreshes asynchronously (it never waits); the dialog
    shows the new rows when they arrive.
16. **Restart through DAP `restart`** sends no arguments (the adapter reuses its configuration, as lldb-dap and the fake
    do); not exercised with lldb-dap through the shell. Hit counts carry over.
17. **Attaching while Ctrl+F5's program runs** moves its handle aside (not killed by the session); its later output is
    not shown (the session generation changed).
18. **An attached session's Stop ends at the `disconnect` answer**, not at the adapter's exit.
19. **Not run:** Windows, macOS, CI, netcoredbg.

## 9. What briefs 0028 and 0030 need

**0028 (multi-session):**
- Attach is refused while a debugging session runs; with sessions it adds one (the dialog's Attach and `attach` with a
  `session` answer). `launched` should be per session (the debuggee of each), and the hook's `LaunchedProcesses` the
  union.
- `interrupt`, `agent_stale`, `agents_allowed` and `restart_pending` live on `Debugger` (one session): they move to the
  session. Allow Agents to Drive is per session by contract; the status bar toggle and the menu check follow the
  active session.
- `DebugMenuState` reads one model; with sessions, Restart and Attach follow the selected session.
- Transcript rows should name the session when there are several (`Step Over [App] → …`).
- The detach path (`Pending::Detach`) and the attach thread are per session already in shape (tagged by generation).

**0030 (the proving scenario):**
- The scripted fake agent can read the guide (`resources/read`) and call `processes` and `attach`; `McpClient` needs a
  public request for `resources/*` (`crates/acp`).
- The scenario's policy file: `execute: allow` and the `debug` defaults run it without prompts; a foreign attach still
  prompts (dangerous).
- The transcript's debug rows (`debug` and `debug_location` in `to_json`, and `--transcript-out`) are the recorded run's
  evidence; `interrupted_by` and the stale refusal are the person-wins part of the scenario.
- netcoredbg still needs a machine that can fetch it (the attach test is written).

## 10. Checks

| Check | Result |
|---|---|
| `cargo fmt --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace --no-fail-fast` (with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME`, `ELUDITE_CHROME_NO_SANDBOX=1`) | 635 passed, 0 failed, 1 ignored |
| `dotnet build dotnet/Eludite.slnx` | succeeded, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | 171 tests: 164 passed, 0 failed, 7 skipped (as before; no .NET file changed) |
