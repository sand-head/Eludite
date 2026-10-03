# Brief 0033 report: The DAP conformance corpus

Status: done on Linux. Windows and macOS: not run (no machines; CI replays the corpus there). CI: not run (nothing
pushed). No display: every test is headless. netcoredbg: not available here (its download is refused by this machine's
proxy); its three scenarios are written, skip here, and CI's Linux job records them and uploads them for checking in
(section 8).
Branch: `brief/0033-dap-conformance-corpus`, based on `main` at `ddc114a` (briefs 0022 to 0030 merged).
Date: 2026-10-03. Brief: [0033-dap-conformance-corpus.md](0033-dap-conformance-corpus.md).

## 1. Summary

- **The recorder** (`eludite_dap::record`) wraps a `Connection` and logs every DAP message in both directions with a
  monotonic `t_ms` to a `.dap.json` file, scrubbed: paths under the repository, the scenario's temporary folder, the
  Mono executable and the Rust sysroot become `${ROOT}`, `${TMP}`, `${MONO}`, `${SYSROOT}`; the session's process ids
  `${PID}`; timestamps in text `${TIME}`; a `variables` answer over 50 rows is cut to 50 with
  `"truncated_by_recorder": true`. `compare` is the re-record check.
- **The replaying adapter** (`eludite_dap::replay`, feature `replay`) serves a recording as a `Connection`: it matches
  each request by `command` and its scrubbed `arguments` to the recorded one, answers it, plays the events that
  followed it in their recorded order, and refuses a request the recording did not see, naming the nearest recorded
  request and the differences.
- **The conformance tests** (`crates/eludite/src/shell/debug/conformance_tests.rs`) run each scenario's actions (F9 on
  marked lines, F5, F10, F11, Shift+F11, Shift+F5, the Exception Settings, `eludite.debug.*` from an agent thread)
  through the headless shell over the replayer (`DebugSetup.connect`), and compare per step `eludite.debug.state`
  (without `*_ms`), the agent's answer and the Locals, Call Stack, Threads and Breakpoints rows with the golden file.
  With `RECORD_DAP=<dir>` each scenario first runs against the real adapter with the recorder on, writes the recording
  and the golden of what the shell showed live, and replays the fresh recording, which must show the same.
- **The corpus**: 8 recordings with their goldens: Mono 4 (`launch-break-step`, `attach-detach`,
  `tracepoints-and-exceptions`, `run-until-and-trace`), lldb-dap 4 (`launch-break-step`, `pause-and-set-variable`,
  `function-breakpoints-and-panic`, `attach-detach`); netcoredbg 3 written, to be recorded by CI.
- **CI**: the Rust job replays the corpus on Linux, Windows and macOS (in the test step, and alone in a step that
  prints the times); the Linux job installs Mono and lldb-dap 18, re-records Mono and lldb-dap (`record.sh --check
  mono lldb`) and fails on drift; it also records netcoredbg and uploads the recordings (artifact
  `dap-corpus-netcoredbg`).
- **Budgets** (Ubuntu 24.04 container, 4 cores, debug builds, headless, another agent's work running beside):

  | Budget | Result |
  |---|---|
  | A scenario replays in under 2 s | Alone (`--test-threads=1`), per scenario: **358 to 665 ms** (Mono `launch-break-step` 358, `tracepoints-and-exceptions` 400, `run-until-and-trace` 466, `attach-detach` 665; lldb `attach-detach` 407, `pause-and-set-variable` 517, `function-breakpoints-and-panic` 612, `launch-break-step` 629). In parallel with the others: 390 to 1061 ms over three runs. The test asserts the 2 s. Pass |
  | The whole corpus under 30 s | The 12 conformance tests (8 replays, 3 netcoredbg skips, the golden-diff test): **1.5 / 1.7 / 2.2 s** in parallel, **3.6 / 3.7 / 4.1 / 4.8 s** one at a time. Pass |
  | Each recording under 2 MB | Largest: lldb `launch-break-step` **94 KB** (the test asserts 2 MB). Pass |
  | The corpus under 20 MB | **687 KB** (recordings 240 KB, goldens 448 KB), plus README and LICENSE. Pass |
  | No new dependency | `Cargo.lock` unchanged. Pass |

  Recording the 8 scenarios from the real adapters and replaying each fresh recording: 31 to 37 s (`record.sh --check
  mono lldb`: 42 s including two `dotnet build`s). The re-record check reproduced the checked-in recordings in 4 of 4
  full runs here.
- **Tests:** `cargo test --workspace --no-fail-fast` with `ELUDITE_DBG_MONO`, `ELUDITE_CHROME` and
  `ELUDITE_CHROME_NO_SANDBOX=1`: **696 passed, 1 failed, 1 ignored**; the failure is brief 0028's load-sensitive
  `a_compound_of_two_reaches_running_within_one_and_a_half_single_launches` (compound 12.9 ms against single 8.0 ms
  with the whole suite running; alone it passed 3 of 3; section 9, item 11). fmt and clippy (`-D warnings`) clean.
  `dotnet build dotnet/Eludite.slnx`: 0 warnings, 0 errors. `dotnet test dotnet/Eludite.slnx`: 173 tests, 166 passed, 0
  failed, 7 skipped (no .NET file changed).

## 2. What was built

Commits, in order:

1. `41fc330` The brief's Status line.
2. `c34cb0d` `crates/dap`: `record.rs` (the recorder, the scrubber and substitution, the file format, `compare`),
   `replay.rs` (the replaying adapter, feature `replay`), the `replay` feature for the dap tests and the shell's tests;
   `tests/replay.rs` (the recorder and replayer against the scripted fake); `tests/common/mod.rs`'s `RECORD_DAP` mode
   (`common::recorded`) in the real-adapter tests (`mono.rs`, `lldb.rs`, `netcoredbg.rs`), and `mono.rs`'s
   `eludite_dbg_mono_recording_replays_with_the_same_events`.
3. `883db32` The recorder logs the client's message before sending it (an answer is never logged before its request);
   the replayer holds a response until its own request has arrived; `compare` compares in groups (section 3) and the
   session's end as a set; their tests.
4. `5f193fe` The conformance tests: 11 scenarios (Mono 4, lldb-dap 4, netcoredbg 3), record and replay modes,
   `REGOLDEN=1`, `DAP_CORPUS_CHECK=1`, `DAP_CORPUS_REQUIRE`, and the golden-diff test.
5. `5f7586d` The 8 recordings and their goldens, made by the recorder (`RECORD_DAP=corpus/dap`).
6. `34e6924` `tools/dap-corpus/record.sh` and `record.ps1`.
7. `6572987` `.github/workflows/ci.yml` (section 7).
8. `7c009c5` `corpus/dap/README.md` and `LICENSE`, the `corpus/README.md` row, the `CLAUDE.md` line.
9. This report, the brief's Status line and the briefs index.

## 3. The format and the rules

A recording is one JSON object written with one message per line:

```
{
  "adapter": "lldb",
  "version": "lldb-dap 18.1.3 (stdio)",
  "recorded_at": "2026-10-03T08:44:42Z",
  "platform": "linux-x86_64",
  "description": "/usr/bin/lldb-dap-18  (stdio)",
  "ended": "client",
  "messages": [
    {"t_ms":2,"dir":"client","message":{"type":"request","seq":1,"command":"initialize","arguments":{...}}},
    {"t_ms":164,"dir":"client","message":{"type":"request","seq":2,"command":"attach","arguments":{"pid":"${PID}"}}},
    ...
  ]
}
```

- **Scrubbing** is in `corpus/dap/README.md`. A path is replaced only at a path boundary (`${TMP}x` stays), the rest of
  it written with `/`; a process id is replaced as a number anywhere and as a word in text when it is 1000 or more.
  Goldens are scrubbed by the same rules, plus `${ADAPTER}` (the adapter's description, which names this machine's
  binary) and `${PROCESS}` (an attached process's name, command line and folder, from the process table).
- **Substitution.** The replayer writes the test's own paths (with the platform's separator after the placeholder) and
  process id back into the adapter's messages, and scrubs the shell's requests by the recorder's rules before matching,
  so a Linux recording replays on Windows (a path's `.exe` suffix is ignored when matching) and macOS.
- **Matching.** The first unmatched recorded request with the same `command` and arguments (`seq` aside). The contract's
  "`threadId` and `frameId` mapped through the recording's own ids" holds without a map: the shell only ever learns
  the recording's ids, from the recorded answers, and sends them back.
- **Order.** An adapter message is played once every request recorded before it has arrived, and a response once its
  own request has (commit 3: the recorder could log lldb-dap's fast answer before the request it answered); its
  `request_seq` becomes the shell's `seq`. Timing is compressed to zero; `ReplayOptions::real_time` keeps the gaps.
- **Refusal.** An unknown request is answered `success: false` and fails the step, for example `the client sent \`next\`
  {"threadId":2} which the recording did not see; nearest recorded: message #41 (not sent yet), differences (recorded,
  then sent): .threadId: expected 1, got 2`.
- **The re-record check** (`record::compare`) ignores `t_ms`, `recorded_at`, `seq`, `request_seq` and lldb-dap's
  `statistics`, and compares in groups that each keep their order: the client's requests; the adapter's responses; its
  events except `output` and `continued`; its `output` text joined per category (how the debuggee's writes are cut into
  events is timing); and the session's end (from the first `disconnect` or `terminate`, or `terminated`) as a set
  without its `output`. `continued` is left out because lldb-dap reports one or not depending on how fast the debuggee
  stops again; the end is a set because lldb-dap 18 sends `exited`, `terminated` and the `disconnect` answer in either
  order (seen in the first re-record run here); its `output` there is a crash report (section 6). Everything else must
  be identical, the adapter's `version` included.

## 4. The recordings

| Recording | Messages | Size | Golden | Steps | Covers |
|---|---|---|---|---|---|
| `mono/launch-break-step` | 73 | 17.5 KB | 31.2 KB | 7 | launch, a breakpoint, F10, F11 into `Twice`, Shift+F11, the agent's `stack`, F5 to the exit |
| `mono/tracepoints-and-exceptions` | 74 | 18.8 KB | 45.9 KB | 10 | a `>=24` hit count (the adapter's), a conditional tracepoint (the adapter's log point), break when `System.InvalidOperationException` is thrown (exception settings by type), deleting a breakpoint in break mode, `exception_info` with the stack trace, the Debug output of the tracepoints |
| `mono/run-until-and-trace` | 78 | 20.5 KB | 72.1 KB | 11 | the agent's `start`, `wait`, `stack` paging, `run_until`, `variables` paging (the locals, `string[25]` from 20, `int[1000]` from 0 and 990), `set_variable`, `trace` to the exit |
| `mono/attach-detach` | 41 | 10.5 KB | 25.4 KB | 5 | attach to the TestApp waiting on Mono's debugger agent (`--debugger-agent=...,suspend=y`, port 47033), the breakpoint, F10, `stop` detaches |
| `lldb/launch-break-step` | 147 | 94.1 KB | 79.4 KB | 10 | launch a Cargo program, a breakpoint, F11 into `add`, F10, Shift+F11, a `%4` hit count (the shell's) to the 4th and 8th hit, deleting it, F5 to the exit |
| `lldb/pause-and-set-variable` | 64 | 35.6 KB | 78.3 KB | 9 | the agent's `start` and `wait`, `set_variable` (`count = 40`, then F10 shows 50), F9 off, F5 to a spinning loop, the agent's `pause`, Shift+F5 |
| `lldb/function-breakpoints-and-panic` | 55 | 29.2 KB | 71.6 KB | 9 | a function breakpoint (`twice`), a tracepoint (lldb-dap's log point), the Rust panics row, deleting the function breakpoint, the agent's `continue` to `rust_panic` and `stack`, `stop` |
| `lldb/attach-detach` | 28 | 13.4 KB | 43.8 KB | 6 | attach by pid to the Cargo program waiting (`wait`: it calls `tick` every 5 ms), the breakpoint in `tick`, the agent's `stack`, F9 off, `stop` detaches |
| `netcoredbg/launch-break-step`, `tracepoints-and-exceptions`, `run-until-and-trace` | | | | 7, 10, 11 | the Mono scenarios' steps against the TestApp's source built for net10.0; hit counts and tracepoints are the shell's there. Not recorded here |

The contract's scenario list, by adapter: launch, breakpoint, the three steps and continue to the exit (Mono, lldb-dap);
hit conditions (adapter on Mono, shell on lldb-dap; shell on netcoredbg when recorded); tracepoints (adapter on Mono
and on lldb-dap, which supports log points; shell on netcoredbg); function breakpoints (lldb-dap); exception settings by
type (Mono; lldb-dap's is the Rust panics row); pause (lldb-dap); `set_variable` (both); `run_until` and `trace`
(Mono); attach and detach (both); the stop summary (every `wait`, `continue`, `pause`, `run_until` answer) and
`variables` paging (Mono). Not covered: pause on Mono, `run_until`/`trace` and function breakpoints under lldb-dap,
exception settings by type under netcoredbg until it is recorded. No recorded `variables` answer reached 50 rows, so
no recording carries `truncated_by_recorder` (the rule is unit-tested).

The programs: brief 0022's TestApp; a Cargo program the test writes (`RS_MAIN`, with `// MARK:` lines), built with
`debug = 2` and `RUST_BACKTRACE=0` under the pinned toolchain. For `lldb/attach-detach` it runs under `setarch -R` (no
address space randomization, as lldb-dap launches programs), so the instruction pointers in the recording are the
same every run.

## 5. The adapters' differences the goldens pinned

| Behavior | `eludite-dbg-mono` (Mono 6.8.0.105) | lldb-dap 18.1.3 | What the shell shows (pinned) |
|---|---|---|---|
| Hit conditions | The adapter evaluates `>=24` (capability `hit_conditions: "adapter"`): one `stopped` per visible stop | `hitCondition` is read as a number, so the shell counts (`"shell"`): 12 `stopped` events and 6 automatic `continue`s for 6 visible stops in `launch-break-step` | Mono's Breakpoints row says `hits 1`, `hits 2` at the 24th and 25th hit (the stops it saw); lldb-dap's says `hits 4`, `hits 8` (every hit) |
| Tracepoints | Log points, with the condition (`i % 25 == 0`): 4 lines, `hits 4` | Log points; but the one pass over the `print` line prints `count=17 total=90` three times (the line's breakpoint was sent twice with `setBreakpoints`, and the function breakpoints twice) | Mono: 4 lines in the Debug source, `hits 4`. lldb-dap: its three lines, and the row says `hits 3` (section 9, item 10) |
| Step into, step out | F11 from `Add` line 41 into `Twice` line 46; Shift+F11 back to line 41 (the call line, mid-statement) | F11 from `main` line 19 into `add` line 2; Shift+F11 back to `main` line 19 | `reason: step` for each |
| Pause | (not recorded) | Stops with `signal SIGSTOP` | `reason: pause` at the spinning line (`native::adapt`) |
| `set_variable` | `setVariable` answers `value` | Answers `value` | `count = 40 : int`, then 50 after F10 |
| Exceptions | `exceptionBreakpointFilters` `all` with a type condition; `exceptionInfo` with the .NET stack trace | No .NET exceptions: the Rust panics row is a `rust_panic` function breakpoint | Mono: `reason: exception` at `Fail()` line 97; lldb-dap: `reason: breakpoint` at `__rustc::rust_panic` with no source, the standard library frames external, and Locals `<error> = no variable information is available in debug info for this compile unit` |
| Attach | No `process` event; `breakpoint` events as the breakpoint binds | A `process` event (`startMethod: attach`); the process runs on after `configurationDone` | `attached: true`, `stop` detaches (no `exited`) in both |
| `continued` events | One per resume | One per resume, and not always (timing) | Not used for state |
| Exit | `exited`, `terminated`, then (after the shell's `disconnect`) its answer | `exited`, `terminated` and the `disconnect` answer in either order; then the adapter aborts (`free(): invalid pointer`, a crash report on stderr) | Design mode in both |
| Thread ids | `1`, `Thread 1` | The OS thread id (= the pid for the main thread, `${PID}` in the recording), named `conformance` | As recorded |
| Paging | `variables` of `int[1000]` answers 10 range rows (`[0..99]` ...) whatever `start`/`count` say | Honors `start` and `count` (brief 0029) | Mono's `start: 990` answers no rows with `total: 990` (section 9, item 8) |

## 6. Tests

| Where | Tests | What they prove |
|---|---|---|
| `crates/dap/src/record.rs` | 5 | Scrubbing and substitution (paths at boundaries, pids as numbers and words, timestamps, separators); `variables` cut to 50 with the flag; frames split across writes; the text format (one message per line, reads back), timing and `seq` not drift, a changed field drift; the grouped comparison (output cut differently, `continued`, the end's order and the crash output are not drift; changed output text or a missing `exited` is) |
| `crates/dap/tests/replay.rs` (fake adapter) | 4 | A fake session recorded to a file reads back scrubbed and replays to a client with the same events (this run's paths and pid substituted back); an unknown request fails with the nearest recorded one and the diff; answers and events wait for the requests recorded before them, in order; `real_time` keeps the gaps |
| `crates/dap/tests/mono.rs` (real) | 1 new | `eludite_dbg_mono_recording_replays_with_the_same_events`: the TestApp's session recorded from Mono and replayed in the same run (43 messages, 10.2 KB, replay 6.5 ms): identical client-visible events and answers |
| `crates/dap/tests/{mono,lldb,netcoredbg}.rs` | `RECORD_DAP` mode | With `RECORD_DAP=<dir>`, every real-adapter session is also written to `<dir>/<adapter>/client/<test>.dap.json` (16 files for Mono and lldb-dap here, 3.7 to 245 KB, all scrubbed); not checked in (section 9, item 1) |
| `crates/eludite/src/shell/debug/conformance_tests.rs` | 12 | The 8 recordings replay through the shell and match their goldens; the 3 netcoredbg scenarios skip until recorded; `a_golden_edit_fails_with_a_readable_diff` (the F11 row edited in memory fails with `step 4 (F11):` and `.windows.locals[0]: expected "x = 6 : int", got "x = 5 : int"`). With `RECORD_DAP` each also records from the real adapter and requires the fresh replay to equal the live snapshots |

## 7. CI (`.github/workflows/ci.yml`)

- **All three platforms**: the conformance tests run in the test step (no adapter needed) and again alone in `DAP
  conformance corpus` (`cargo test --workspace -- "conformance_tests::" --nocapture`), which prints each replay's time.
- **Linux**: `mono-devel` and `lldb-18` join the system packages; `Re-record the DAP corpus (Mono, lldb-dap)` sets
  `kernel.yama.ptrace_scope=0` (lldb-dap attaches to a process it did not start) and runs `tools/dap-corpus/record.sh
  --check mono lldb` with `RECORD_DAP=$RUNNER_TEMP/dap-corpus`: the script builds `eludite-dbg-mono` and the TestApp,
  `DAP_CORPUS_REQUIRE=mono,lldb` fails a missing adapter, and a recording that differs fails the job with the diff.
- **netcoredbg** (Linux): `Record the DAP corpus (netcoredbg)` runs `record.sh --check netcoredbg` with the netcoredbg
  the job already fetches (`ELUDITE_NETCOREDBG`), `continue-on-error: true` until its first recordings are checked in,
  and `Upload the DAP recordings` uploads `$RUNNER_TEMP/dap-corpus/netcoredbg` as `dap-corpus-netcoredbg`.
- Installing lldb-dap 18 on the Linux job also lets `crates/dap/tests/lldb.rs` and the shell's real lldb-dap tests run
  in its test step (they skipped there before; they pass here).

## 8. Re-recording, and what is left for netcoredbg

```
dotnet build dotnet/Eludite.slnx                 # or let the script build eludite-dbg-mono and the TestApp
tools/dap-corpus/record.sh                       # every adapter here, into corpus/dap/; then review git diff corpus/dap
tools/dap-corpus/record.sh --check mono lldb     # CI's check: into a temporary folder, fail on drift
REGOLDEN=1 cargo test -p eludite --bin eludite conformance_tests::   # goldens from the checked-in recordings
```

netcoredbg: on a machine where `tools/netcoredbg/fetch.sh` works (CI, or the owner's), `ELUDITE_NETCOREDBG=$(tools/
netcoredbg/fetch.sh) tools/dap-corpus/record.sh netcoredbg` writes `corpus/dap/netcoredbg/{launch-break-step,
tracepoints-and-exceptions,run-until-and-trace}.{dap,golden}.json`; or take them from CI's `dap-corpus-netcoredbg`
artifact. Then: check them in, add `netcoredbg` to the Linux re-record step (`--check mono lldb netcoredbg`) and drop
the netcoredbg step's `continue-on-error`. None of the three is recorded yet; the recorder path for netcoredbg (the
TestApp's source built for net10.0 with `dotnet build`, then launched by the shell) has not run anywhere, so the first
CI run is its first run.

## 9. Deviations, decisions and findings

1. **The corpus is recorded through the shell, not by the `crates/dap` tests.** The goldens are what the shell made of
   the session, and the replay must see the shell's own requests, so each corpus recording is made by the conformance
   test itself (`RECORD_DAP`) with the recorder on the shell's connection to the real adapter, and replayed in the same
   run. The `crates/dap` real-adapter tests have the brief's `RECORD_DAP` mode too (client-level sessions under
   `<dir>/<adapter>/client/`), used by the Mono record-then-replay proof; those are not checked in (their requests are
   the client tests', not the shell's, and they have no goldens).
2. **`lldb/attach-detach` is added** (the brief's file list names three lldb-dap scenarios; its contract asks for
   attach on lldb-dap). The Cargo program gained `tick` and a `wait` mode for it: a breakpoint on the `linger` spin
   loop is never hit after attach (the loop jumps past the breakpoint's address). The earlier lldb-dap recordings were
   re-recorded with the new program.
3. **An agent's `start` and `attach` answers are not in the goldens**: whether the answer is `running` or already the
   first stop depends on how fast the adapter breaks (the replay answers with the stop, the real adapter sometimes
   not). The next step, the agent's `wait until stopped`, carries the stop summary.
4. **No thread or frame id map** (section 3); the effect the contract asks for holds.
5. **Tracepoints are the adapter's on lldb-dap**, not the shell's as the brief's scenario line says: lldb-dap 18
   supports log points (`log_points: "adapter"`); the shell emulates them only for netcoredbg.
6. **The re-record check's groups** (section 3) are wider than "ignoring `t_ms`": without them two runs of lldb-dap
   differ in `continued` events, in how stdout is cut into events, in the order of `exited` and `terminated`, and in
   the crash report it prints as it exits. With them, 4 of 4 full re-record runs here matched.
7. **Commit 3's message names the recorder only**; it also holds the replayer's "a response waits for its own
   request" rule. Not rewritten (no rebase).
8. **Finding: Mono's `variables` paging of a chunked array.** `eludite-dbg-mono` answers `int[1000]` with 10 range rows
   (`[0..99]` ...) and ignores `start`/`count`, so the agent's `variables` with `start: 990` answers no rows and
   `total: 990`. The golden pins it; a fix (the shell paging the ranges, or the adapter honoring `start`) is a
   follow-up, not this brief's scope.
9. **Finding: lldb-dap 18 aborts after `disconnect`** (`free(): invalid pointer` and an LLVM crash report on stderr) in
   every lldb-dap recording. The shell is unaffected (the session has ended); the recordings keep the output, the check
   ignores it.
10. **Finding: lldb-dap 18 repeats a log point's message.** In `function-breakpoints-and-panic` the tracepoint's line
    runs once and lldb-dap sends its message three times, after the shell re-sent the line's `setBreakpoints` (on
    configuration) and `setFunctionBreakpoints` (when the function breakpoint was deleted); the shell counts each as a
    hit (`hits 3`). Also pinned: lldb-dap answers the launch's empty `sourceMap` with `error: 'settings set' takes more
    arguments` on the console. Both are follow-ups for the native session (brief 0029's code), not this brief's.
11. **Load-sensitive test**: `a_compound_of_two_reaches_running_within_one_and_a_half_single_launches` (brief 0028)
    failed once in the full run (12.9 ms against 8.0 ms; the 12 conformance tests now share its binary's parallel
    load) and passed alone 3 of 3.
12. **README.md is not updated**: it is outside the brief's files, and its layout lines (`corpus/`, `tools/`) still
    hold in general terms. `CLAUDE.md`'s `tools/` row now names `dap-corpus/`.
13. **Windows and macOS replays are unverified here.** The replay forces Mono's Linux launch configuration on every
    platform (.NET Framework runs under Mono there); lldb-dap's launch arguments are built for the platform the test
    runs on, and paths inside lldb's `initCommands` strings depend on the scrubber's separator handling. CI's first
    Windows and macOS runs are the proof.
14. **`record.ps1` was not run** (no PowerShell here); it mirrors `record.sh` without Mono, which is not debugged on
    Windows.

## 10. How to reproduce

```
export DOTNET_ROOT=/root/.dotnet PATH=/root/.dotnet:$PATH DOTNET_NOLOGO=1
dotnet build dotnet/Eludite.slnx
export ELUDITE_DBG_MONO=$PWD/debuggers/mono/Eludite.Debugger.Mono/bin/Debug/net472/eludite-dbg-mono.exe
cargo test -p eludite-dap                                         # recorder, replayer, Mono record-then-replay
cargo test -p eludite --bin eludite conformance_tests:: -- --nocapture --test-threads=1   # replay times
tools/dap-corpus/record.sh --check mono lldb                      # the re-record check
```
