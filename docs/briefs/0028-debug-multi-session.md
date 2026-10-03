# Brief 0028: Multiple debugging sessions

Status: in progress
Phase: 2 (proposal 0001, brief D)
Plan reference: PLAN.md sections 2 (principles 1, 3, 12), 4.5, 5.5, 7 (compound launch configurations), 8 (Visual Studio names and layout), 9, 10 (Phase 2); proposal 0001 sections 4, 5.4, 8 (D); proposal 0002 section 4.4 (what this brief exists for)
Related ADRs: ADR-0003, ADR-0007
Depends on: brief 0027 (attach, restart, the debug policy). Runs after 0027 merges.

## Goal

Eludite debugs more than one process at a time, as Visual Studio does with multiple startup projects: `session` is an optional argument on every `eludite.debug.*` command (default: the active session), `eludite.debug.sessions` lists the sessions, `start` accepts a `compound` (the solution's multiple startup projects, or an explicit list of projects with their action), the Call Stack and Threads windows gain a session selector, the status bar names every session, and Stop Debugging ends them all while `stop` with a `session` ends one. Breakpoints, exception settings and watches are shared across sessions and sent to every adapter. The two-driver rules of brief 0018 and the budgets and summaries of brief 0025 hold per session. This is the piece proposal 0002's full-stack debugging (Kestrel under netcoredbg beside the page under js-debug) stands on; the JavaScript adapter itself is proposal 0002 brief D.

## Files in scope

- `protocol/schemas/` first and alone: `debug-sessions.{input,output}.json`; `session` added to every `debug-*.input.json` of a command that acts on a session (`stop`, `continue`, the steps, `run_to_cursor`, `evaluate`, `select_frame`, `watch`, `snapshot`, `stack`, `variables`, `output`, `exception_info`, `wait`, `pause`, `run_until`, `trace`, `set_variable`, `set_next_statement`, `restart`, `allow_agents`, `state`); `debug-start.input.json` (`compound`: `"startup"` for the solution's multiple startup projects, or a list of `{ project, debug?, profile? }`); `debug-state.output.json` (`sessions`: id, name, mode, runtime, adapter, process id, `active`; `session.id`; the rest describes the active session); `debug-stop-summary.output.json` (`session`); `workspace-set-startup-project.input.json` (`projects`: a list of `{ project, action: start | start_without_debugging | none }` for Visual Studio's multiple startup projects, beside the existing single form).
- `crates/commands/src/debug.rs`, `crates/commands/src/project.rs`.
- `crates/eludite/src/shell/debug.rs`, `debug/state.rs` (the model becomes a list of sessions with one active; breakpoints, exception settings and watches move to the shared part), `debug/windows.rs` (the session selector in Call Stack and Threads; Locals and Watch follow the active session), `debug/tests.rs`, `shell.rs` for the Startup Projects dialog's menu item (Project > Set Startup Projects..., and the Workspace context menu's "Set as Startup Project" keeps working), `crates/ui/**` for the selector control and the dialog (a list of the solution's projects with an Action column, as Visual Studio's), `shell/explorer.rs` for bold names of every startup project.
- `docs/briefs/README.md`, `docs/briefs/0028-report.md` (new).

## Contract

- **Sessions.** Each session has a small integer `id` (1, 2, ..., never reused in a shell run), a `name` (the project's name, or the attached process's), its own generation, stop counter, mode, adapter, client, stack, locals, output rings, `agents_allowed`, `last_driver`. One session is `active`: the one the Locals, Watch, Call Stack and Threads windows show and the execution point follows. A stop in a non-active session makes it active (Visual Studio switches to the process that broke) unless the person selected another session in the last 2 seconds; the Call Stack and Threads selectors switch sessions by hand. The status bar slot reads `Debugging: App (break: breakpoint, Program.cs line 12), Web (running)`, the active session first.
- **Commands.** `session` (an id) is optional on every command listed above; the default is the active session; an unknown or ended id is refused with the list of live ids. `state` without `session` describes the active session plus `sessions`; with `session` it describes that one. `sessions` (read) lists them with their modes. `stop` without `session` ends every session (Debug > Stop Debugging, Shift+F5), with one the named session only; `restart` and `pause` likewise default to the active session. `start` with `compound: "startup"` launches every startup project with its action (build before run once for the whole set, then launch in solution order); with a list, those projects; a plain `start` is a compound of one. A compound's answer (with `wait_ms`) is the stop summary of the first session to break, or `running` with every session's mode when none breaks within the wait. Agents' resuming commands wait on their session only.
- **Shared state.** Breakpoints, tracepoints, function breakpoints, exception settings and watches are one set; a change is sent to every live adapter, and the Breakpoints window shows one row per breakpoint with the bound state per session (`verified` per session, shown as a tooltip and in `state.breakpoints[].sessions`).
- **Multiple startup projects.** `eludite.workspace.set_startup_project` with `projects` sets Visual Studio's multiple startup projects; the Startup Projects dialog edits them; they persist per solution in brief 0018's store (version bump, migration test); the Workspace window shows every startup project in bold. F5 starts the compound. `startup_project` in the state keeps naming the first one with action `start` for compatibility.
- **Rules.** Brief 0018's two-driver rules hold per session (one session's `stop` counter never refuses a command on another). Brief 0025's summaries carry `session`. Brief 0027's `allow_agents` and `interrupted_by` are per session. Rule 4 of brief 0018 (old answers dropped) uses the session id with the generation.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/commands`: `session` parses on every command; `compound` forms; `set_startup_project` with `projects`; outputs match their schemas.
- `crates/eludite` headless tests (two fake adapters): a compound of two projects launches two sessions, both break at their breakpoints, the windows show the one that broke first, the selector switches, Locals follow; `sessions` lists both with modes; an agent steps session 2 by id while session 1 stays at its break, and `state` for each is right; `stop` with a session ends one and the other keeps running; Shift+F5 ends both; a breakpoint toggled while both run binds in both (per-session `verified`); the status bar text; the Startup Projects dialog sets two projects, they persist and show bold, F5 starts both; an agent's `continue` on session 1 is refused as stale by session 1's stop counter but not by session 2's; a session that exits is removed from the list and the other becomes active; the Debug menu's enabling with mixed modes (one break, one running) follows the active session.
- Real adapters, gated: two `eludite-dbg-mono` TestApp sessions at once (break in both, step one, stop one); netcoredbg plus Mono if netcoredbg is available.
- No display: headless; the report says so.

## Budget

- A two-project compound reaches both `running` in under 1.5 times the single-project launch time (fake adapter, measured).
- Frame cost unchanged (under 8 ms p99 headless measurement) with two sessions stopping alternately ten times a second.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the Mono real-adapter tests running; `dotnet build` and `dotnet test` unchanged and green.
2. The report records the budget numbers and what proposal 0002 brief D (js-debug attach to a tab, compound with netcoredbg) needs from the session model.
3. The briefs index matches the repository.

## Out of scope

- The JavaScript adapter and attaching to a browser tab (proposal 0002 D); the Rust adapter (proposal 0001 E2).
- A `launch.json`-style file of named configurations beyond the solution's startup projects (proposed later if VS's multiple startup projects prove insufficient).
- Windows and macOS runs.
