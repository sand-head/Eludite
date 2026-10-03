# Brief 0030: The agent debugging proving scenario

Status: open
Phase: 2 and 4 (proposal 0001, brief F; PLAN.md's Phase 4 exit in its measurable form)
Plan reference: PLAN.md sections 2 (principle 6: a corpus of real inputs and golden outputs), 5.2, 5.5, 5.7, 10 (Phase 2: agent-driven debugging; Phase 4: an agent takes a failing test to a reviewed, passing fix), 11 (verification over trust); proposal 0001 sections 1 (the proving scenario), 8 (F), 9, 11 (context cost)
Related ADRs: ADR-0003
Depends on: briefs 0025, 0026, 0027 (the commands, the policy and the guide), 0022 (the Mono adapter, the adapter available on this machine). Runs after 0027 merges.

## Goal

The suite is proven against its own definition of done: given a seeded bug in a corpus program and the prompt "this program's self-check fails, find out why", a real Claude Code session hosted in the Agents window reaches the faulting statement with the relevant locals in its context in at most eight debug tool calls, and a scripted fake agent does the same in a headless test that runs on every CI platform. The corpus (`corpus/debugging/`) is checked in with its expected answers; the recorded Claude Code run (transcript, tool calls, tokens, time) is checked in beside the report; and the test measures what proposal 0001 section 11 worries about, the tokens each answer costs. Until the Test Explorer exists (PLAN.md 4.6, a later brief), the "failing test" is a console program whose `Main` runs the check and exits non-zero; the Test Explorer brief will point `eludite.test.debug` at the same corpus.

## Files in scope

- `corpus/debugging/` (new): `README.md` (what each program seeds, the faulting statement and the locals that reveal it, as the expected answers; which adapter runs it), three SDK-style C# console programs that multi-target `net10.0;net472` so netcoredbg and `eludite-dbg-mono` both run them: `OffByOne` (a loop that skips the last element of a list, the check compares a sum), `MissingCase` (a switch over an enum that falls through to a default, the check expects a value), `NullField` (an object graph where one node's field stays null after a constructor path, the check dereferences it and throws); each with a `Properties/launchSettings.json`, no external packages; `corpus/debugging/build.sh` and `build.ps1` (`dotnet build` all three); `corpus/README.md` (the new entry).
- `crates/eludite/src/shell/agents/scenario.rs` (new) and `agents/tests.rs`: the scripted fake agent's debugging scenario (through `eludite_acp::fake_agent`'s script steps from brief 0024: tool calls over the real MCP endpoint), one scenario per corpus program, and the headless tests that run them against the adapter found on the machine (netcoredbg, else `eludite-dbg-mono` with the `net472` build; skip with a message when neither nor the built corpus is found); `crates/acp/src/fake_agent.rs` if a script step is missing.
- `crates/eludite/tools/debug-agent-linux.sh` and `debug_agent.py` (new): the recorded real run: starts the shell on the Xvfb screen (`crates/eludite/tools/xvfb-linux.sh`, or the nested KWin on the owner's machine), opens the corpus program, starts a Claude Code session through the Agents window with the prompt, waits for the turn to end, and writes the transcript (`--transcript-out`), the tool calls with their sizes, the token counts from the ACP usage events, and screenshots to OUT_DIR; the driver uses xdotool or the existing XTest drivers.
- `docs/briefs/0030-run/` (new): the recorded run: `transcript.md` (the full transcript as the Agents window showed it), `calls.json` (each tool call: name, input, output size in bytes, time), `screenshots/*.png`, `numbers.md`.
- `.github/workflows/ci.yml`: the Rust job on every platform fetches netcoredbg through `tools/netcoredbg/fetch.sh` (and `.ps1` on Windows; cached on the PIN as Chrome is) and builds the corpus with `dotnet build` (the .NET SDK is installed with `actions/setup-dotnet` in that job too), so the fake-agent scenario runs on Linux, Windows and macOS.
- `docs/briefs/README.md`, `docs/briefs/0030-report.md` (new).

## Contract

- **The scenario, scripted.** For each corpus program the fake agent runs, in order and through the MCP tools only: `eludite.debug.start` (with `wait_ms`), reads the summary's `output` and `exit_code` (the check's message names the function), `eludite.debug.toggle_breakpoint` on the function's first line (found through `eludite.editor.find` or the summary's frames when the program stopped on the exception; `NullField` stops at the throw with `exception_info`), `eludite.debug.start` again or `run_until`, `snapshot` with `depth: 2`, and at most two `step_over` or `variables` calls; it then asserts the summary's `stopped.location` is the faulting statement from `corpus/debugging/README.md` and the named locals are in the answer with the revealing values. At most eight debug tool calls per program, counted by the test from the audit log. The fake agent uses only what a model would see: the tool descriptions, the guide resource (brief 0027) and the outputs.
- **The scenario, real.** The same prompt to a real Claude Code session through the Agents window ("The program `<path>` fails its self-check when run. Debug it with Eludite's tools, find the statement that produces the wrong value, and tell me the statement, the line and the local variable values that show it. Do not edit files."), with the policy allowing agents to drive and `evaluate`, recorded on this machine for all three programs. The report gives, per program: the debug tool calls (count and names), whether the faulting statement was reached (the location in a summary the agent received) and named in the answer, the total tokens (input and output, from the ACP usage events, or the transcript's size when the agent sends none), the wall time, and the size in bytes of each answer. Three runs per program; the report says how many of nine reached the statement within eight calls. The proving threshold is the proposal's: at most eight calls in the run reported. If a run misses it, the report says why (which call was wasted, what the tool description or the guide should have said) and the brief proposes the change as a follow-up, not as a change here.
- **Token cost.** The headless test records the bytes of every answer and asserts the summaries stay under 8 KB (brief 0025's budget); the report tabulates bytes per call for the fake and the real runs.
- **The corpus** is MIT (a `LICENSE` file in `corpus/debugging/`), built by `dotnet build`, runs on .NET 10 and under Mono 6.8 (`net472`), and each program's check fails deterministically with a message naming the function and the expected and actual values. No network, no packages.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `corpus/debugging`: each program builds and, run directly, exits with code 1 and the expected message (a test in `crates/eludite` runs them when `dotnet` is found; under Mono when `mono` is found).
- The three scripted scenarios pass headless against netcoredbg (CI, and here if the fetch works) and against `eludite-dbg-mono` (here), each within eight debug calls, and the test prints the call list and sizes.
- The recorded real run exists under `docs/briefs/0030-run/` with its numbers; `tools/debug-agent-linux.sh` reproduces it.
- CI: the Rust job's scenario step passes on the three platforms (the report records the Linux run here; the others are "not run").

## Budget

- Each scripted scenario finishes in under 15 s warm on this machine, adapter launch included.
- The summaries the agents receive stay under 8 KB each; the whole scripted scenario's answers under 40 KB.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the scenarios running against `eludite-dbg-mono` here; `dotnet build` and `dotnet test` unchanged and green; `corpus/debugging/build.sh` builds the corpus.
2. The report gives the real run's numbers per program (calls, reached or not, tokens, time, bytes per answer), the fake run's, and the changes to tool descriptions or the guide it recommends.
3. The briefs index and `corpus/README.md` match the repository.

## Out of scope

- Proposing and applying a fix (Phase 4's full exit: a reviewed, passing fix); only finding the statement is measured here.
- The Test Explorer and `eludite.test.debug`; the conformance corpus of recorded DAP sessions (proposal 0001 brief G).
- Tuning the model's behavior beyond the tool descriptions and the guide.
- Windows and macOS runs.
