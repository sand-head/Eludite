# Brief 0010: Rename the project from Eludite to Eludite

Status: open
Phase: 1
Plan reference: PLAN.md section 14 (decision 6 is superseded)

## Goal

The owner renamed the project to Eludite on 2026-10-02 and the GitHub repository is already `sand-head/Eludite`. After this brief, nothing in the tree is called Eludite except a one-line historical note in the README and the git history. Every crate, binary, namespace, protocol method, schema id, environment variable, config path, tool name, fixture, script, document and screenshot caption uses the new name, and the full Linux check suite is green.

## Files in scope

Everything in the repository except `docs/PLAN.md` section 14's table rows for decisions 1 to 9 (append decision 10 instead) and the `.git` directory. Do not move or rename the checkout directory itself; the human does that after merge.

## Contract

Rust:
- Crate names `eludite-*` become `eludite-*`; crate identifiers `eludite_*` become `eludite_*`; the app binary `eludite` becomes `eludite`; `eludite-dbg-netfx` becomes `eludite-dbg-netfx`; `eludite-claude-acp` becomes `eludite-claude-acp`; the spike packages under `spikes/` follow.
- Environment variables `ELUDITE_*` become `ELUDITE_*`. Config directory segment `eludite` becomes `eludite` (layouts, logs). Window title, status bar, about command and user-visible strings say Eludite.
- The MCP server name and relay use `eludite`, so tools appear as `mcp__eludite__*`; update the recorded fixtures in `crates/acp`, `crates/mcp` and `agents/claude-acp/tests/fixtures` consistently rather than redacting them again.
- `vendor/` is untouched except its README.

Protocol:
- `eludite/host/*`, `eludite/ping`, `eludite/solution/*`, `eludite/languageServer/status` become `eludite/...`; the `eluditeGeneration` member becomes `eluditeGeneration`; `x-eludite-*` schema annotations become `x-eludite-*`; schema `$id` values use `https://github.com/sand-head/Eludite/...`; `host-rpc.md` and every schema file are updated together with the Rust and .NET bindings and the drift tests that hold them to each other.

.NET:
- `Eludite.slnx` becomes `Eludite.slnx`; projects `Eludite.Host`, `Eludite.Web`, `Eludite.Wcf`, `Eludite.TestBridge` and their test projects become `Eludite.*` (directories, csproj files, namespaces, assembly names); the host executable `eludite-host` becomes `eludite-host`, including every place that locates it (`crates/lsp`, the bench driver, `tools/legacy-load`, HOST.md). `Directory.Build.props` product name follows.
- `global.json` is unchanged.

Docs and tooling:
- `README.md`, `CLAUDE.md`, `CONTRIBUTING.md`, ADRs, briefs and reports, `docs/PLAN.md` (title, every mention), CI workflow names, scripts under `tools/`, `bench/`, `corpus/`, screenshots' captions in reports. Reports are historical, but they are renamed too for consistency; add one line to `README.md`: "Eludite was called Eludite until 2026-10-02."
- Append decision 10 to PLAN.md section 14: "Name (v0.4) | Eludite, replacing Eludite, 2026-10-02. Repository sand-head/Eludite."
- `docs/briefs/README.md` index row for this brief.

Mechanics:
- Prefer a scripted, case-aware replacement (`Eludite`→`Eludite`, `eludite`→`eludite`, `ELUDITE`→`ELUDITE`) followed by `git mv` for files and directories, then a manual review of every hunk that is not a pure case-aware substitution. Commit as a small number of single-line commits (for example: the mechanical replacement; the file and directory moves; the fixture and schema updates; the docs), so the diff is reviewable.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers, no co-author or sign-off lines.

## Proving test

- `grep -ri eludite --exclude-dir=.git --exclude-dir=target .` returns only the README historical line, PLAN.md section 14's decision rows 6 and 10, and this brief and its report.
- `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace` green; the same for `agents/claude-acp`, `vendor` (its own workspace) and each `spikes/*` package (`cargo check` is enough for spikes).
- `dotnet build dotnet/Eludite.slnx` with zero warnings; `dotnet test dotnet/Eludite.slnx` green.
- `tools/legacy-load/runner` and `bench/roslyn-200/driver` build.
- The `eludite` binary opens a window titled "Eludite" (nested compositor if the session is locked; one screenshot in the report is enough).
- `cargo run -p eludite-claude-acp -- --help` (or equivalent) runs and names itself Eludite.

## Budget

Not applicable; no behavior changes.

## Exit criterion

1. The grep in Proving test is clean.
2. Every check in Proving test passes on Linux.
3. The report lists every file moved and every non-mechanical edit, and anything that still carries the old name with the reason.

## Out of scope

- Renaming the checkout directory or the memory directory on the owner's machine.
- Any behavior change, refactor or cleanup noticed along the way; note it in the report instead.
- Windows and macOS runs; CI covers them after merge.
