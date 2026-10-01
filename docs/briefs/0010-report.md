# Brief 0010 report: rename the project from Niello to Eludite

Date: 2026-10-01. Brief: [0010-rename-to-eludite.md](0010-rename-to-eludite.md). Linux only; Windows and macOS are covered by CI after merge.

## 1. Summary

- Every crate, binary, namespace, protocol method, schema id, environment variable, config path, MCP server name, fixture, script and document now uses Eludite. No behavior changed apart from the names.
- The proving grep returns only the allowed lines (section 4).
- Every check in the brief's proving test passes on Linux (section 5).
- The work is four commits: the moves, the case-aware substitution, the hand edits, and this report.

## 2. Moves (`git mv`)

| From | To |
|---|---|
| `crates/niello/` | `crates/eludite/` (the app crate; the generic crate directories keep their names) |
| `dotnet/Niello.slnx` | `dotnet/Eludite.slnx` |
| `dotnet/src/Niello.Host/` and `Niello.Host.csproj` | `dotnet/src/Eludite.Host/` and `Eludite.Host.csproj` |
| `dotnet/src/Niello.TestBridge/` and `Niello.TestBridge.csproj` | `dotnet/src/Eludite.TestBridge/` and `Eludite.TestBridge.csproj` |
| `dotnet/src/Niello.Wcf/` and `Niello.Wcf.csproj` | `dotnet/src/Eludite.Wcf/` and `Eludite.Wcf.csproj` |
| `dotnet/src/Niello.Web/` and `Niello.Web.csproj` | `dotnet/src/Eludite.Web/` and `Eludite.Web.csproj` |
| `dotnet/tests/Niello.Host.Tests/` and its csproj | `dotnet/tests/Eludite.Host.Tests/` and `Eludite.Host.Tests.csproj` |
| `dotnet/tests/Niello.TestBridge.Tests/` and its csproj | `dotnet/tests/Eludite.TestBridge.Tests/` and `Eludite.TestBridge.Tests.csproj` |
| `dotnet/tests/Niello.Wcf.Tests/` and its csproj | `dotnet/tests/Eludite.Wcf.Tests/` and `Eludite.Wcf.Tests.csproj` |
| `dotnet/tests/Niello.Web.Tests/` and its csproj | `dotnet/tests/Eludite.Web.Tests/` and `Eludite.Web.Tests.csproj` |
| `docs/briefs/0002-niello-host-roslyn-spike.md` | `docs/briefs/0002-eludite-host-roslyn-spike.md` |

No spike package, script, screenshot or fixture had the old name in its path. The spike packages were already named `spike-gpui-shell` and `spike-acp-panel`.

## 3. Substitution and the edits it could not make

**Mechanical.** `Niello`→`Eludite`, `niello`→`eludite` and `NIELLO`→`ELUDITE` in 255 tracked text files, which is every match outside `.git`, `target` and `vendor/` plus `vendor/README.md`. Only those three casings occur in the tree. The four `Cargo.lock` files were then normalized with `cargo metadata --offline`. That re-sorts the renamed packages, and no package or version changed: the sorted name and version pairs are identical before and after.

**Hand edits** (commit 3):

1. **Repository URL.** The substitution would have produced `github.com/sand-head/eludite-ide`. It is now `https://github.com/sand-head/Eludite` in the root `Cargo.toml` `repository`, every schema `$id` (12 in `protocol/schemas/`, 12 in `protocol/schemas/host/`), the `$id` check in `protocol/rust/src/schema_tests.rs`, and the clone URL in `docs/briefs/windows-checklist.md`. The repository-tree root in PLAN.md section 12 is now `Eludite/` instead of `niello-ide/`.
2. **Column alignment.** `Eludite` is one character longer than `Niello`, so aligned text moved. I fixed:
   - the architecture diagram boxes in PLAN.md D2;
   - the layout trees in PLAN.md section 12 and README.md;
   - the `ELUDITE_CONFIG_DIR` line in `crates/eludite/src/args.rs` (`--help` text; one space removed so it lines up with the other options);
   - the doc-comment example in `crates/editor/src/lib.rs`;
   - the `#` comments in the reproduce blocks of the 0002, 0003 and 0008 reports.
3. **rustfmt.** The renamed `eludite_*` imports now sort before `gpui`. `cargo fmt --all` reordered them in the root workspace (docking, editor, eludite) and in `spikes/0005-acp-panel`, and re-wrapped one longer `format!` in `crates/lsp/src/client.rs`.
4. **Articles.** "a Eludite" and "a `eludite-…`" became "an …" in 10 places: `crates/editor/src/syntax/theme.rs`, `crates/mcp/src/server.rs` (the MCP `instructions` string, which no fixture records), `protocol/schemas/host-rpc.md`, briefs 0002, 0005 and 0006, the 0002 report (twice, one of them across a line break) and the 0003 report.
5. **History.**
   - README.md gets the line "Eludite was called Niello until 2026-10-02."
   - PLAN.md section 14 row 6 is restored to its original text ("Niello. …"), and row 10 is appended: "Name (v0.4) | Eludite, replacing Niello, 2026-10-02. Repository sand-head/Eludite."
   - This brief's file is restored to its original wording and only its Status line changes.
   - The briefs index row is titled "Rename the project to Eludite" so that the index does not carry the old name.
6. **`vendor/` notes.** The brief says `vendor/` is untouched except its README, but the proving grep must also be clean. Four files outside the upstream sources named Niello: the `vendor/Cargo.toml` comments (two lines) and `vendor/{rope,text,fuzzy}/WHY.md`. I renamed them. No upstream source changed, and `vendor/sync.sh` still reports no drift at the pin.

**Fixtures.** The recorded fixtures are JSONL, one message per line:
- `crates/acp/tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl`
- `agents/claude-acp/tests/fixtures/claude-2.1.287-session.jsonl`
- `agents/claude-acp/tests/fixtures/claude-2.1.287-session.acp.jsonl`

None of them embeds a Content-Length, a byte offset or a hash (the only numeric `size` field is the model context size). Framing is computed when they are replayed, so the substitution leaves them consistent and they were not re-recorded. `mcp__niello__diagnostics-list` and server name `niello` are now `mcp__eludite__diagnostics-list` and `eludite` throughout. The conformance and replay tests pass.

## 4. Proving grep

`grep -ri niello --exclude-dir=.git --exclude-dir=target .` returns:

- `README.md`: "Eludite was called Niello until 2026-10-02."
- `docs/PLAN.md`: section 14 rows 6 and 10
- `docs/briefs/0010-rename-to-eludite.md`: the brief, 10 lines
- `docs/briefs/0010-report.md`: this report

In this worktree only, it also returns `./.git`. That is the worktree's `gitdir:` pointer file, whose path contains the checkout directory `niello-ide`. `--exclude-dir` does not skip a file, and in a normal clone `.git` is a directory, so the line does not appear there.

## 5. Checks (Linux, 2026-10-01)

| Check | Result |
|---|---|
| root: `cargo fmt --all --check` | pass |
| root: `cargo clippy --workspace --all-targets -- -D warnings` | pass |
| root: `cargo test --workspace` | pass: 194 passed, 0 failed, 1 ignored (the `ignore` doctest in `crates/editor/src/lib.rs`, as before), 35 suites. `crates/lsp/tests/real_host.rs` ran against the real `eludite-host.dll`. |
| `agents/claude-acp`: fmt, clippy `-D warnings`, test | pass: 18 tests, 5 suites |
| `vendor`: `cargo fmt --all --check` | pass |
| `vendor`: clippy and test | pass with `--all-features`: 81 tests (fuzzy 9, rope 24, sum_tree 10, text 38). Without `--all-features`, `clock`'s tests fail to compile (no `parking_lot`). That was already the case and is unrelated to the rename; the 0009 report and `vendor/README.md` give the `--all-features` command. |
| `vendor/sync.sh --from <cargo checkout>` | no drift at `20d29fc6` |
| `spikes/0001-gpui-shell`: `cargo check --all-targets` | pass |
| `spikes/0005-acp-panel`: `cargo check --all-targets` | pass |
| `dotnet build dotnet/Eludite.slnx` | pass, 0 warnings, 0 errors |
| `dotnet test dotnet/Eludite.slnx` | pass: 113 total, 113 succeeded, 0 skipped (corpus fetched, bench solution prepared, Roslyn LS and user-space Mono present) |
| `tools/legacy-load/run.sh --prepare-only` (fetches the corpus, builds `tools/legacy-load/runner`) | pass, 0 warnings |
| `bench/roslyn-200/run.sh --prepare-only` (builds `eludite-host` Release and `bench/roslyn-200/driver`) | pass, 0 warnings |
| `eludite` window title | pass |
| `eludite-claude-acp` | pass |

**Window title.** I ran the debug `eludite` binary in a nested KWin, using `spikes/0005-acp-panel/tools/nested.sh` under `dbus-run-session` on the X11 backend. `xprop` reported `WM_NAME = "Eludite"`, `_NET_WM_NAME = "Eludite"` and `WM_CLASS = "eludite"`. The status bar reads "Eludite 0.1.0". Screenshot: [`crates/eludite/screenshots/linux-window-title.png`](../../crates/eludite/screenshots/linux-window-title.png).

**Claude adapter.**
- `cargo run --bin eludite-claude-acp -- --help` prints "eludite-claude-acp: Agent Client Protocol (stdio) adapter for Claude Code." with `$ELUDITE_CLAUDE_PATH` and `$ELUDITE_CLAUDE_ACP_LOG`.
- `--version` prints `eludite-claude-acp 0.1.0`.
- An ACP `initialize` returns `agentInfo.name = "eludite-claude-acp"`.

The brief's `cargo run -p eludite-claude-acp -- --help` needs `--bin`, because the package has two binaries and no `default-run`. That was already the case.

One `dotnet test` run, made while the cargo test build was loading all 16 cores, failed `HostProcessTests.Stdout_IsProtocolOnly_AndRenamedLifecycleExitsCleanly` once. The test passed alone and in two further full runs on an idle machine (113/113 each). I read it as a timing flake under load, not a rename defect.

## 6. External paths and what the owner should know

- **Cache.** The code defaults are renamed with everything else: `ROSLYN_SRC_DIR` defaults to `~/.cache/eludite/roslyn`, and the design-time cache, Mono config and reference assemblies live under `$XDG_CACHE_HOME/eludite`. The existing build on this machine is still at `~/.cache/niello`, which is outside the repository and not renamed. I ran the checks with `ROSLYN_SRC_DIR=~/.cache/niello/roslyn ELUDITE_CACHE_DIR=~/.cache/niello`. To use the new defaults, run `mv ~/.cache/niello ~/.cache/eludite`, or keep setting those two variables. Without either, the Roslyn and legacy tests skip cleanly as before.
- **Saved layouts.** They now live in `<config dir>/eludite/layouts`. Layouts saved under `<config dir>/niello` are not migrated (a migration would be a behavior change).
- **Images.** The pixels of the screenshots committed before this brief show the old name in title bars and status bars:
  - `crates/eludite/screenshots/linux-default-layout.png`
  - `crates/eludite/screenshots/linux-customized-after-restart.png`
  - `spikes/0005-acp-panel/screenshots/*.png`
  - `agents/claude-acp/screenshots/*.png`

  They are historical captures. Their captions in the reports are renamed. Re-taking them would need the original scenarios, including live Claude sessions.

## 7. Noticed, not changed (out of scope)

- `agents/claude-acp/Cargo.toml` has no `default-run`, so `cargo run -p eludite-claude-acp` needs `--bin eludite-claude-acp`.
- The brief's vendor check, plain `cargo test` in `vendor/`, needs `--all-features` (clock's `test-support` feature).
- `HostProcessTests.Stdout_IsProtocolOnly_AndRenamedLifecycleExitsCleanly` may be timing-sensitive under heavy CPU load (section 5).
- PLAN.md's trademark line (section 3 and ADR-0005) now says "Eludite". Decision 6's open item (check crates.io, the GitHub org name and a domain) may need repeating for the new name.
