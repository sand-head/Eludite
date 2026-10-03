# Brief 0033: The DAP conformance corpus

Status: done on Linux (Windows and macOS not run); [report](0033-report.md)
Phase: 2 (proposal 0001, brief G)
Plan reference: PLAN.md sections 2 (principle 6: a corpus of real inputs and golden outputs), 4.5, 10 (Phase 2), 11 (protocol conformance tests replayed against recorded sessions, CI on all three OSes); proposal 0001 sections 6 (the adapter matrix), 8 (G), 11 (adapter gaps hidden by shell emulation)
Related ADRs: ADR-0003, ADR-0007
Depends on: briefs 0022 (Mono), 0029 (lldb-dap), 0025 to 0028 (the suite). Runs after 0028 merges.

## Goal

Every adapter the suite supports has a recorded DAP session checked in under `corpus/dap/`, and the shell is tested against those recordings on every CI platform, with no adapter installed: a replaying adapter answers each request from the recording and plays the events in order, so a change to the shell's state machine, its emulations (hit counts, tracepoints, Run To Cursor, lldb-dap's pause and `setVariable` quirks, Mono's step-in) or its summaries is caught against what the real adapters actually said, not against the fake's idealized behavior. Recordings are made by a recorder in `eludite-dap` from the real-adapter tests that already exist (`crates/dap/tests/{mono,lldb,netcoredbg,attach,run_control,inspection}.rs`), scrubbed of machine paths, and a conformance test per adapter replays each one through the shell's headless harness and checks the windows' state and the agents' answers against golden JSON beside the recording. netcoredbg's recordings are made on a machine that can download it (CI, or the owner's) through the same recorder, since this machine cannot.

## Files in scope

- `corpus/dap/` (new): `README.md` (the format, how to record and re-record, the scrubbing rules, the license of each recording's program: the TestApp and the lldb temp program are this repository's, MIT per `corpus/debugging`'s rule), `<adapter>/<scenario>.dap.json` recordings (`mono/launch-break-step.dap.json`, `mono/attach-detach.dap.json`, `mono/tracepoints-and-exceptions.dap.json`, `mono/run-until-and-trace.dap.json`, `lldb/launch-break-step.dap.json`, `lldb/pause-and-set-variable.dap.json`, `lldb/function-breakpoints-and-panic.dap.json`, `netcoredbg/*.dap.json` when recorded), each with a `<scenario>.golden.json` (the state and summaries the shell produced when the recording was made).
- `crates/dap/src/record.rs` (new): the recorder: a `Connection` wrapper that logs every message in both directions with a monotonic timestamp to a JSON file (the `dap.json` format: `adapter`, `version`, `recorded_at`, `platform`, `messages: [{ t_ms, dir: "client" | "adapter", message }]`), scrubbing absolute paths to `${ROOT}` placeholders and pids to `${PID}`; `crates/dap/src/replay.rs` (new): the replaying adapter: given a recording, it serves a `Connection` that matches each client request by command and a normalized argument hash (paths substituted back, sequence numbers ignored) to the recorded response, and emits the recorded events that followed it, with timing compressed to zero unless `--real-time`; an unmatched request fails the test naming the first difference; `fake` feature gains nothing (the replayer is its own feature `replay`).
- The real-adapter tests under `crates/dap/tests/` gain a `RECORD_DAP=<dir>` mode that writes the recordings instead of only asserting (`common/mod.rs`); `crates/eludite/src/shell/debug/conformance_tests.rs` (new): per recording, a headless shell session over the replayer (through `DebugSetup.connect`) running the scenario's user actions (F5, F9 on recorded lines, F10, the agent commands) in the recorded order and comparing the windows' state and the agent answers to the golden file, with a `REGOLDEN=1` mode that rewrites goldens.
- `tools/dap-corpus/record.sh` and `record.ps1` (re-record every adapter present on the machine; prints which were skipped), `.github/workflows/ci.yml` (the Rust job runs the conformance tests on all three platforms; the Linux job also re-records Mono and lldb-dap and fails if a recording drifted from the checked-in one beyond timing, so the corpus is kept honest).
- `docs/briefs/README.md`, `corpus/README.md`, `CLAUDE.md` (a line under "What not to do": do not edit recordings by hand; re-record), `docs/briefs/0033-report.md` (new).

## Contract

- **The format** is JSON, one file per scenario, under 2 MB each (variables answers truncated to the first 50 rows when recording, with `truncated_by_recorder: true` noted in the message), with every absolute path under the repository or a temp dir replaced by `${ROOT}` or `${TMP}`, pids by `${PID}`, timestamps by `${TIME}`; the replayer substitutes the test's real values back.
- **Matching** is by `command` and the normalized `arguments` (after substitution; `seq` and `request_seq` ignored; `threadId` and `frameId` mapped through the recording's own ids). A request the recording did not see fails the test with the nearest recorded request and a diff. Events are replayed after the response they followed in the recording; `output` events keep their order.
- **The goldens** hold, per step: `eludite.debug.state` (without timing fields), the stop summary the agent received, and the windows' row texts (Locals, Call Stack, Threads, Breakpoints) as the headless harness reads them. A golden mismatch prints a diff.
- **Scenarios** cover the adapter matrix of proposal 0001 section 6 as far as each adapter supports: launch, breakpoint, step over, into, out, continue to exit; hit conditions (shell-emulated on netcoredbg and lldb-dap, adapter on Mono); tracepoints (adapter on Mono, shell elsewhere); function breakpoints; exception settings by type; pause; `set_variable`; `run_until` and `trace`; attach and detach (Mono, lldb-dap); the stop summary and `variables` paging.
- **Honesty.** A recording is only ever produced by the recorder from a real adapter; CLAUDE.md says so. Re-recording on Linux CI must reproduce the checked-in Mono and lldb-dap recordings (after scrubbing, ignoring `t_ms`), or the job fails with the diff, so adapter upgrades are noticed.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- `crates/dap`: the recorder round-trips a fake session to a file and back; scrubbing and substitution; the replayer answers a recorded request, fails an unknown one with the diff, and replays events in order; the TestApp's real Mono recordings are produced here (`RECORD_DAP`) and replayed in the same test run with identical client-visible events.
- `crates/eludite` conformance tests: every checked-in recording replays through the shell and matches its golden here; a deliberate golden edit fails with a readable diff (a test that compares against a modified golden in memory).
- CI: the conformance tests pass on Linux, Windows and macOS without any adapter installed (the recordings are the adapter); the Linux re-record step matches.
- No display: headless; the report says so. netcoredbg recordings: the report says which scenarios were recorded on CI (if the workflow is extended to upload them as an artifact for checking in) and which await the owner's machine.

## Budget

- A conformance test replays a scenario in under 2 s; the whole corpus under 30 s.
- Each recording under 2 MB; the corpus under 20 MB.
- No new dependency.

## Exit criterion

1. `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green with the Mono and lldb-dap recordings produced and replayed here; `dotnet build` and `dotnet test` unchanged.
2. The report lists the recordings, their sizes, what each covers, the adapters' behavior differences the goldens pinned (as a table), and how to re-record.
3. `CLAUDE.md`, `corpus/README.md` and the briefs index match the repository.

## Out of scope

- Recording LSP, MTP or ACP sessions (PLAN.md section 11 names them; they get their own briefs).
- Editing recordings by hand, ever.
- Windows and macOS runs here (CI runs the replays there).
