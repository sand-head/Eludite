# Brief 0036: Breakpoints that cannot stop say so, and the Mono adapter resolves names as Visual Studio does

Status: done on Linux (Windows and macOS not run); [report](0036-report.md)
Phase: 2 and 4 (proposal 0001, second follow-up to brief F)
Plan reference: PLAN.md sections 4.5, 5.5, 5.7, 10 (Phase 4 exit), 11; proposal 0001 sections 4 (rule 2: outputs say what was cut or failed), 9, 11; brief 0034's report section 7 (items 1 to 3)
Related ADRs: ADR-0003
Depends on: brief 0034 (the tuned guide, the compact `toggle_breakpoint` answer, usage events, the second recorded run).

## Goal

Brief 0034's three remaining misses came from breakpoints that could not stop and did not say so: a condition the adapter rejected (`coin == Coin.Quarter`: Mono's evaluator does not resolve unqualified type names), a line that binds only to a loop initializer, and a race inside `eludite-dbg-mono` while pending breakpoints resolve. After this brief: a breakpoint the adapter could not set, or whose condition it rejected, is reported in the answers an agent reads (the `toggle_breakpoint` answer while a session is live, and a `breakpoints_failed` list in the stop summary and the end-of-session summary), the guide's two examples are correct (the condition on the first statement of the loop body; type names qualified, or unqualified where the adapter resolves them), `eludite-dbg-mono` resolves unqualified type names in conditions and evaluations against the stopped method's namespace and `using` directives as Visual Studio does, and its "Collection was modified" race is fixed. The recorded Claude Code run is repeated once more (three runs per program) and reported against briefs 0030 and 0034.

## Files in scope

- `protocol/schemas/` first and alone: `debug-toggle-breakpoint.output.json` (`message` and `verified` per live session on the answered row; already partly there: check), `debug-stop-summary.output.json` (`breakpoints_failed`: path, line, session, message; present while any breakpoint of the session failed to bind or had its condition rejected), `debug-run-until.input.json` and `debug-trace.input.json` (the example conditions corrected; a note that a point on a `for` header binds to its initializer), `dap-mono.md` (name resolution in conditions and evaluations).
- `docs/agents/debugging.md` (the two examples; one sentence on reading `breakpoints_failed`).
- `crates/commands/src/debug.rs`, `crates/eludite/src/shell/debug.rs` and `debug/state.rs` (carry the adapter's `breakpoint` event message and the `setBreakpoints` answer's `message` into the rows and the summaries; `run_until` and `trace` answers say when a point never bound), `crates/eludite/src/shell/debug/tests.rs` (the fake adapter rejects a condition and refuses a line).
- `debuggers/mono/Eludite.Debugger.Mono/**` (the evaluator's name resolution: the stopped frame's method's namespace, enclosing types, and the file's `using` directives from the method's source when the PDB or MDB gives the file, else the assembly's namespaces by unique simple name; the pending-breakpoint race: the collection mutated during resolution guarded or copied), `debuggers/mono/Eludite.Debugger.Mono.TestApp/Program.cs` (an enum in a namespace and a condition on it), `debuggers/mono/Eludite.Debugger.Mono.Tests/**` (a condition with an unqualified enum name stops; the race test: a program loading several assemblies while 20 pending breakpoints resolve, run 10 times).
- `crates/eludite/src/shell/agents/scenario.rs` and `tests.rs` (the scripted scenarios still pass; a scenario asserts the failure text appears in the answers when a condition is wrong), `docs/briefs/0036-run/` (the third recorded run), `docs/briefs/README.md`, `docs/briefs/0036-report.md` (new).

## Contract

- A `setBreakpoints` answer with `verified: false` and a `message`, or a `breakpoint` event with `verified: false`, puts the message on the row (`state.breakpoints[].sessions[].message`), and while any such breakpoint exists in a session, every stop summary and the end-of-session summary of that session carry `breakpoints_failed` with the path, line and message; the `toggle_breakpoint` answer carries `verified` and `message` for the live sessions when the adapter has answered (the shell waits up to 500 ms for the answer from an agent; the UI never waits). `run_until` and `trace` say `points_failed` with the same shape when a point never bound.
- `eludite-dbg-mono` resolves `Coin.Quarter` inside `MissingCase.Program.Main` as `MissingCase.Coin.Quarter`: the stopped method's namespace and its parents, the enclosing types, then the file's `using` directives (parsed from the source file the debug information names, cached per file), then a unique simple name across the loaded assemblies; an ambiguous name is reported as such. The same resolution serves `evaluate`, conditions and tracepoint expressions. The race: pending breakpoints are resolved from a snapshot of the collection, and a breakpoint added during resolution is resolved on the next assembly load or at once.
- The guide's examples: a condition on the first statement of the loop body (`i == count - 2` is the last iteration there for an off-by-one loop) and the header unconditional; enum names qualified in the example, with a note that Eludite's Mono adapter and netcoredbg resolve unqualified ones. The guide stays under 2,000 words.
- The recorded run: three per program with brief 0030's prompt, reported against 0030 and 0034; the eight-call threshold unchanged.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/eludite` headless tests (fake adapter): a rejected condition shows its message on the row, in the `toggle_breakpoint` answer and in `breakpoints_failed` of the next `wait` and of the end-of-session summary; a line the adapter refuses likewise; `run_until` with a point that never binds reports `points_failed`; the message clears when the breakpoint is edited and binds.
- `debuggers/mono` tests: the unqualified enum condition stops at the right iteration; `evaluate` of `Coin.Quarter` works at a break in the namespace; an ambiguous name is reported; the race test passes 10 of 10.
- The scripted scenarios pass; the recorded run exists under `docs/briefs/0036-run/`.

## Budget

- The scripted scenarios' answers stay under 30 KB per program; a `toggle_breakpoint` answer under 600 bytes with the failure message.
- Name resolution adds under 20 ms to a condition's first evaluation in a session (measured by the adapter test).
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the Mono scenarios running; `dotnet build` zero warnings and `dotnet test` green with the new adapter tests running.
2. The report gives the three-run comparison (0030, 0034, 0036) per program and the next recommendation, if any.
3. The briefs index matches the repository.

## Out of scope

- Column breakpoints and `breakpointLocations` (brief 0034's item 4; a later brief).
- Changing the corpus or the prompt; trimming the guide.
- Windows and macOS runs.
