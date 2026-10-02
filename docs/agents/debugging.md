# Debugging with Eludite: a guide for agents

Eludite's debugger is a set of sessions (one per started project or attached process, usually one) that you and the person at the keyboard drive together, through the same commands.
Each `eludite.debug.<name>` command is the MCP tool `eludite-debug-<name>` (Claude Code shows it as
`mcp__eludite__eludite-debug-<name>`). Every answer is budgeted: lists say `total` and `truncated`, values are cut at
`max_value_chars`, and the commands that run the program answer with the compact stop summary. Read this once per
session; the tool descriptions say the rest.

## 1. Read `snapshot` before acting

Call `eludite.debug.snapshot` first, and again whenever you are unsure what state the debugger is in. It never runs
program code and never moves the person's windows. It answers:

- `mode`: `design` (no session), `building`, `launching`, `running`, `break`, `stopping` or `running_without_debugging`;
- `stop`: a number that grows with every break; quote it (section 3);
- `stopped`: why and where it stopped (`reason`, `location` with `path`, `line` and `function`, the exception or the
  breakpoint and its hit count);
- `frames` and `locals` of the stopped thread, within `max_frames`, `depth` and `max_variables`;
- `watches`, the program's `output` since a cursor, the adapter's `capabilities`, and `agents_allowed` in
  `eludite.debug.state`.

To look deeper without changing anything: `eludite.debug.stack` (page with `start` and `count`, `all_threads`),
`eludite.debug.variables` (by `reference`, or by `thread`, `frame` and `scope`; `filter` by name prefix; page with
`start`), `eludite.debug.exception_info` at an exception stop. Pass `thread` and `frame` explicitly: your reads never
move the windows' selected frame. `eludite.debug.select_frame` does, so use it only when you mean to show the person a
frame.

`capabilities` says what this session's adapter can do: `set_variable`, `set_next_statement`, `function_breakpoints`,
`exception_filter_options`, `restart`, `pause`, and whether tracepoints and hit conditions are done by the `adapter` or
the `shell`. Check it before trying something an adapter may refuse.

`eludite.debug.evaluate` runs code in the debuggee (property getters, method calls). Prefer `variables` for reading
values; use `evaluate` when you need a computed expression.

## 2. Prefer `run_until` and `trace` over single steps

Each command costs a round trip. Get to where you need to be in one call:

- **`eludite.debug.run_until`** with `points` (`path`, `line`, optional `condition`): sets one-shot breakpoints, resumes,
  and answers the summary of the first stop. The points are removed at that stop (`remove_after`, default true).
- **`eludite.debug.trace`** with `points` (`path`, `line`, `message` with `{expression}`s, optional `condition`):
  installs tracepoints, runs (`run: continue` from a break, or `run: start` to launch), and answers the lines they
  printed, in order, with each hit, until `until` holds (`terminated`, `stopped`, or `hits` with `count`) or `wait_ms`
  runs out. Use it to watch a value change across many iterations without stopping each time. A visible stop ends it
  (`stopped_by: stopped`, with the summary).
- **Breakpoints that stay:** `eludite.debug.toggle_breakpoint` with `condition`, `hit_condition` (`5`, `>=5`, `%2`),
  `log_message` (a tracepoint that prints and continues), or `function` (`Namespace.Type.Method`).
  `eludite.debug.exception_settings` with `types` stops on specific exception types.

Single steps (`eludite.debug.step_over`, `step_into`, `step_out`) and `eludite.debug.run_to_cursor` are for the last
few lines, when you need to watch one statement at a time. `eludite.debug.continue` resumes until the next breakpoint,
exception or exit. `eludite.debug.pause` breaks a running program (a hang, a long loop). `eludite.debug.set_variable`
changes a value at a break; `eludite.debug.set_next_statement` moves the execution point where the adapter allows it.

To start: `eludite.debug.start` (F5; it builds first by default), or `eludite.debug.attach` to a running process
(`eludite.debug.processes` lists them with their `runtime` and `launched_by_eludite`). Attaching to a process Eludite
did not start asks the person first. `eludite.debug.restart` starts the same configuration again;
`eludite.debug.stop` ends the session (an attached process is detached and keeps running).

Every resuming command takes `wait_ms` (default 5,000, at most 30,000) and answers once the program settles: the
summary of the next stop, the end of the session (`mode: design` with `exit_code`), or `timed_out: true` with
`mode: running`. While it runs, `eludite.debug.wait` waits without driving (`until`: `stopped`, `terminated`,
`output`, `any`).

## 3. Pass `stop` on every resuming call

The person may step while you think. Quote the `stop` of the summary you based your decision on:

```json
{"stop": 7, "wait_ms": 5000}
```

If the program has moved since (another stop, a resume, a new session), the command is refused as `stale` instead of
acting on a state you did not see, and nothing happens. Read `snapshot` again and decide again. Commands are refused,
never queued: a second command while the program runs is refused with the mode it is in.

## 4. Read `output` by cursor

The summary carries the program's last `max_output_lines` lines and `output.next`, a cursor. Pass it back as
`output_since` on your next resuming call, or as `since` to `eludite.debug.output`, to get only the lines written
after it, without repeats. `eludite.debug.output` reads three sources: `program` (stdout and stderr), `debug` (the
debugger's messages and tracepoint lines) and `adapter` (the debug adapter's own messages); `pattern` filters by a
substring or a `/regular expression/`. `dropped` says how many lines the ring overwrote before you read them.

## 5. When a call returns `interrupted_by: "user"`

The person always wins. If they continue, step, run to the cursor, break, stop or restart while your command (a
resuming command, `wait`, `run_until` or `trace`) is waiting, your wait ends at once. The answer is the summary of the
state the person caused, with `interrupted_by: "user"` (`trace` answers the lines it collected with
`stopped_by: "interrupted"`).

Then:

1. Do not repeat your last command. The person is looking at something; your plan is out of date.
2. Your next resuming command is refused as stale until you read the state again: call `eludite.debug.snapshot`
   (or `state`, or `wait`), or quote the current `stop`.
3. Read what changed, and tell the person what you were about to do before you drive again.

## 6. What the policy may refuse, and how it reads

The person stays in charge of what you may do to a session.

- **Allow Agents to Drive.** A per-session toggle (Debug > Allow Agents to Drive, and the status bar while debugging).
  While it is off, every command of yours that starts, attaches, restarts, resumes or changes the session is refused
  with:

  `agents are not allowed to drive this session (Debug > Allow Agents to Drive)`

  Your reads (`snapshot`, `state`, `stack`, `variables`, `output`, `exception_info`, `wait`, `processes`) keep working.
  Do not retry: ask the person to turn the toggle on, or describe what you would do. Only the person can turn it on
  (`eludite.debug.allow_agents` with `enabled: true` is refused for you).
- **The solution's policy** (`.eludite/agents-policy.json`, its `debug` object):
  - `drive`: `allow` (default), `prompt` or `deny`, for starting, attaching, restarting, resuming and changing the
    session;
  - `attach`: `prompt` (default) or `deny`, for attaching to a process Eludite did not start;
  - `evaluate`: `allow` (default), `prompt` or `deny`, for `evaluate`, `set_variable` and tracepoints whose messages
    have `{expressions}`.

  `prompt` makes the call dangerous: the person is asked in the Agents window, and your call waits for the answer.
  `deny` refuses it at once, with a `permission denied` message that names the policy, for example
  `the solution's policy sets debug.drive to deny`.

  The tool's `_meta` `eludite/escalates` says which calls can be raised or refused this way. A denial is the person's
  decision: do not look for another command that does the same thing.
- **The permission class.** Debug commands that run the program are class execute: depending on the policy the person
  may be asked before each. A refusal with `the user denied it` means they said no.

Everything you do is audited, and the Agents window shows each of your debug commands as the person would read it in
the Debug toolbar, for example `Step Over → stopped at Program.cs:42 (breakpoint)`, with the summary you received.

## 7. More than one session

- Every started project and every attached process is a session with an `id`. `eludite.debug.sessions` lists them (id, name, mode, active); `eludite.debug.state` and the stop summaries carry `session`.
- Every command that acts on a session takes `session`; without it, the active session (the one the windows show) is used. Name the session when more than one is live: the active one changes when another session breaks.
- `start` with `compound: "startup"` starts the solution's startup projects together; a compound answer is the first session to break, or every session's mode on a timeout. Naming a project that is already being debugged starts another instance.
- `stop` without `session` ends every session; with one, that session only. Breakpoints, exception settings and watch expressions are shared by all sessions; the stop counter, `allow_agents` and `interrupted_by` are per session.
