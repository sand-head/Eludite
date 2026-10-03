# corpus/

Real inputs with their expected outputs, used as golden tests (PLAN.md section 11, "Verification over trust").

Each entry states its origin, its license, which subsystems use it, and the expected outputs checked in beside it.

| Entry | What it is | Used by | Expected outputs |
|---|---|---|---|
| [`legacy/`](legacy/) | Real-world legacy .NET Framework, WebForms and WCF solutions, cloned at pinned commits by `fetch.sh` from `manifest.json` (not checked in) | Brief 0003's legacy project-load spike (`tools/legacy-load`) | In `manifest.json` and brief 0003's report |
| [`debugging/`](debugging/) | Three seeded-bug C# console programs (`OffByOne`, `MissingCase`, `NullField`), `net10.0;net472`, MIT, built by `build.sh` / `build.ps1` | Brief 0030's agent debugging proving scenario: the scripted fake agent's headless tests (`crates/eludite/src/shell/agents/tests.rs`) under netcoredbg or `eludite-dbg-mono`, and the recorded Claude Code run (`crates/eludite/tools/debug-agent-linux.sh`) | The faulting statement, its line and the locals that reveal it, in `debugging/README.md` |
