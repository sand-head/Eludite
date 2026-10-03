# corpus/dap/

Recorded Debug Adapter Protocol sessions of the real debug adapters, and what the shell made of each (brief 0033,
proposal 0001 section 8 G). The shell's conformance tests (`crates/eludite/src/shell/debug/conformance_tests.rs`)
replay every recording through the headless shell on every CI platform, with no adapter installed: the replaying
adapter (`eludite_dap::replay`, feature `replay`) answers each request the shell sends from the recording and plays the
adapter's events in their recorded order, and the windows' state and the agents' answers must equal the golden file
beside the recording. A change to the shell's state machine, its emulations or its summaries is caught against what
the real adapters said, not against the fake adapter's idealized behavior.

| Adapter | Folder | Program | Recorded with |
|---|---|---|---|
| `eludite-dbg-mono` under Mono 6.8 (.NET Framework, brief 0022) | `mono/` | brief 0022's TestApp (`debuggers/mono/Eludite.Debugger.Mono.TestApp`) | Linux |
| lldb-dap 18.1.3 (Rust, brief 0029) | `lldb/` | a small Cargo program the test writes (`RS_MAIN` in the conformance tests) | Linux |
| netcoredbg (.NET) | `netcoredbg/` | the TestApp's source built for net10.0 | not yet: CI's Linux job records it (artifact `dap-corpus-netcoredbg`) for checking in |

## Scenarios

| Scenario | mono | lldb | netcoredbg | Covers |
|---|---|---|---|---|
| `launch-break-step` | yes | yes | to record | F9, F5 to a breakpoint, F10, F11, Shift+F11, F5 to the exit; a `%4` hit count on lldb-dap (shell-emulated); the agent's `stack` on Mono |
| `tracepoints-and-exceptions` | yes | | to record | a `>=24` hit count and a conditional tracepoint (the adapter's on Mono, the shell's on netcoredbg), break when `System.InvalidOperationException` is thrown, the agent's `exception_info` and `output` |
| `run-until-and-trace` | yes | | to record | the agent's `start`, `wait`, `stack` paging, `run_until`, `variables` paging (a page of the locals, of `string[25]`, of `int[1000]`), `set_variable`, `trace` to the exit |
| `attach-detach` | yes | yes | | attach to a waiting process (Mono's debugger agent; lldb-dap by pid), a breakpoint hit after attach, F10 on Mono, the agent's `stack` on lldb-dap, `stop` detaches |
| `pause-and-set-variable` | | yes | | the agent's `set_variable` (lldb-dap's `setVariable` answer), F10, F9 off, F5, the agent's `pause` of a spinning program (lldb-dap's `SIGSTOP` stop read as a pause), Shift+F5 |
| `function-breakpoints-and-panic` | | yes | | a function breakpoint, a tracepoint (lldb-dap's log point), the Rust panics row (`rust_panic`), the agent's `continue` to the panic and `stack` (standard library frames as external code), `stop` |

## Files

- `<adapter>/<scenario>.dap.json`: the recording. One JSON object: `adapter`, `version` (`lldb-dap 18.1.3 (stdio)`,
  `eludite-dbg-mono under mono 6.8.0.105`), `recorded_at`, `platform`, `description` (how the adapter was started),
  `ended` (which side closed the connection) and `messages: [{ t_ms, dir: "client" | "adapter", message }]`, one
  message per line so a re-recording diffs line by line. `t_ms` is milliseconds since the connection opened.
- `<adapter>/<scenario>.golden.json`: per step of the scenario, `eludite.debug.state` (without its `*_ms` timing
  fields), the agent's answer (the stop summary, a page of variables, `null` for a person's action) and the row texts
  of the Locals, Call Stack, Threads and Breakpoints windows as the headless harness reads them. Written by the
  recorder run from what the shell showed live, and required to equal the replay of the fresh recording.

### Scrubbing

Before a recording or a golden file is written, what belongs to the recording machine is replaced:

- absolute paths under the repository become `${ROOT}`, under the scenario's temporary folder `${TMP}`, the Mono
  executable `${MONO}`, the Rust toolchain's sysroot `${SYSROOT}`; the rest of such a path is written with `/`;
- the process ids the session names (the `process` event's `systemProcessId`, `attach`'s `pid` or `processId`, the
  adapter's own, an attached program's) become `${PID}`, as a number anywhere and as a word in text from 1000 up;
- timestamps in text (`2026-10-03T12:34:56.789Z`, `2026-10-03 12:34:56`) become `${TIME}`;
- a `variables` answer keeps its first 50 rows and says `"truncated_by_recorder": true` (no current scenario needs it);
- in golden files only: the adapter's description (it names the machine's adapter binary) becomes `${ADAPTER}`, and an
  attached process's name, command line and folder (read from the machine's process table, not DAP) `${PROCESS}`.

The replayer substitutes the test's own paths and process id back into the adapter's messages, and scrubs the
shell's requests by the same rules before matching them, so a recording made on Linux replays on Windows and macOS.

### Replay rules

- A request is matched to the first recorded request not yet matched with the same `command` and the same scrubbed
  `arguments` (`seq` ignored; a path's `.exe` suffix ignored). Thread, frame and variable ids need no mapping: the
  shell only learns the recording's own ids, from the recorded answers.
- An adapter message is played once every request recorded before it has arrived, and a response only once its own
  request has; its `request_seq` is the shell's own `seq`. Events (`output` among them) keep their recorded order.
- Timing is compressed to zero (`ReplayOptions::real_time` keeps it).
- A request the recording did not see is answered `success: false` and fails the test with the nearest recorded
  request (the next unanswered one of that command, or the closest) and the first differences, recorded then sent.
- A golden mismatch fails with each differing step and the JSON paths that differ, `expected X, got Y`.

## Recording and re-recording

Recordings are only ever produced by the recorder (`eludite_dap::record`) from a real adapter. Never edit one by hand,
and never edit a golden file by hand: re-record.

```
tools/dap-corpus/record.sh                      # every adapter found here, into corpus/dap/; review git diff
tools/dap-corpus/record.sh mono lldb            # only these (they must be present)
tools/dap-corpus/record.sh --check mono lldb    # into a temporary folder; fail when one differs from the checked-in one
tools\dap-corpus\record.ps1 [-Check] [lldb] [netcoredbg]   # Windows (Mono is not debugged there)
```

The script builds `eludite-dbg-mono` and the TestApp (`dotnet build`), then runs the conformance tests with
`RECORD_DAP=<folder>`: each scenario runs first against the real adapter with the recorder on the connection, writing
`<folder>/<adapter>/<scenario>.dap.json` and its golden file, then replays that fresh recording in a second shell,
which must show the same thing. `DAP_CORPUS_CHECK=1` also compares each fresh recording with the checked-in one, and
`DAP_CORPUS_REQUIRE=mono,lldb` fails instead of skipping an adapter that is missing.

The comparison ignores timing: `t_ms`, `recorded_at`, `seq` and `request_seq`, lldb-dap's `statistics`, `continued`
events, how the debuggee's output is cut into `output` events (compared as one text per category), the order of the
adapter's messages after the session's end (`exited`, `terminated` and the `disconnect` answer come in either order
from lldb-dap 18) and its `output` there (lldb-dap 18 aborts as it exits and prints a crash report with this run's
addresses). Everything else must be identical, including the adapter's `version`, so an adapter upgrade fails CI's
Linux job until the corpus is re-recorded and the diff reviewed.

`REGOLDEN=1` rewrites the golden files from a replay of the checked-in recordings, for a deliberate change to the
shell's state or summaries; review the diff.

Needs: Mono and the .NET SDK for `mono/`; lldb-dap (`ELUDITE_LLDB_DAP` or on PATH), cargo and the toolchain pinned in
`rust-toolchain.toml` for `lldb/` (`attach-detach` attaches to a process lldb-dap did not start: on Linux with Yama
that needs `sysctl kernel.yama.ptrace_scope=0`; the program runs under `setarch -R` so its addresses do not move);
netcoredbg (`ELUDITE_NETCOREDBG`, from `tools/netcoredbg/fetch.sh`) and the .NET SDK for `netcoredbg/`.

## License

The recordings and golden files are MIT ([`LICENSE`](LICENSE)), as is `corpus/debugging`. The programs they record
are this repository's: the TestApp (`debuggers/mono/Eludite.Debugger.Mono.TestApp`) and the Cargo program the
conformance tests write.
