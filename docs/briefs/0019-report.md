# Brief 0019 report: Rust through the generic paths, so Eludite can build Eludite

Status: done on Linux. Windows and macOS: not run (no machines). CI: not run (nothing pushed).
Branch: `brief/0019-rust-workspace`, rebased onto `origin/main` at `833960f` (brief 0018 landed during the work; the
measurements below ran before the rebase, and a confirmation drive ran after it). Date: 2026-10-02.
Brief: [0019-rust-workspace.md](0019-rust-workspace.md).

## Summary

- **Eludite edited and built Eludite.** `eludite --folder <this repository>` (File > Open Folder, Ctrl+Shift+Alt+O,
  runs the same command) shows `Solution 'Eludite' (8 of 8 projects)` and `Cargo workspace '…' (16 members)` side by
  side under the folder root. `crates/editor/src/buffer.rs` gets rust-analyzer's completion and diagnostics; a type
  error typed into `Buffer::len` becomes a live Error List row; Ctrl+S, Ctrl+Shift+B runs `cargo build`, the Output
  window streams it, the status bar says `Build failed: 1 error, 0 warnings`, and the Error List shows one row from
  `Build + IntelliSense` (cargo's row deduplicated with rust-analyzer's); a double-click puts the caret on the error.
  The file was reverted afterwards (the run script restores it from git).
- **One abstraction for "a language server for these documents".** `eludite-lsp` now has a connection core,
  `Connection`, that both peers use: `HostClient` (the `eludite-host` bridge) and the new `ServerClient` (plain LSP
  to a server the shell launches). In the shell, every document holds the `ServerSession` of its server, and the
  editor features of briefs 0013 to 0015 send their requests through it without knowing which kind it is.
- **Registration is data.** `crates/lsp/src/servers.json` lists `roslyn` (via `eludite-host`, `*.cs`) and
  `rust-analyzer` (`*.rs`, root marker `Cargo.toml`, executable discovery, initialization options, settings).
- **Cargo** is modelled from `cargo metadata --no-deps --offline` (no network) and built with
  `cargo build --message-format=json-diagnostic-rendered-ansi` through the `eludite/build/*` shapes, so the brief 0017
  Output, Error List, status bar and cancel serve it unchanged.
- **Budgets hold** except keystroke cost in one of three runs taken at load 11 while rust-analyzer's start-up
  `cargo check` ran (below).

## rust-analyzer

- **Used:** the rustup component of the pinned toolchain, `rust-analyzer 1.98.1 (48a229c 2026-09-01)`, found on `PATH`
  through the rustup proxy (`rustup component add rust-analyzer` was run once on this machine).
- **Pinned for fetching:** release `2026-08-31` (the component's source), SPDX `MIT OR Apache-2.0`, with SHA-256 per
  platform in `tools/rust-analyzer/PIN`. `tools/rust-analyzer/fetch.sh` downloads, verifies and installs it
  (checked here: `rust-analyzer 0.3.3033-standalone`); `fetch.ps1` does the same on Windows (not run).
- **Discovery** (`ServerRegistration::locate`): beside the `eludite` executable, then `ELUDITE_RUST_ANALYZER`, then
  `PATH`, then `rustup which rust-analyzer`. Each candidate must answer `--version`, so a rustup proxy without the
  component is skipped instead of launched. Discovery runs on the session's worker thread, never the UI thread.
- **Diagnostics are pull and push in the pinned version.** It computes its native diagnostics (syntax and semantic:
  `E0107`, `E0063`, `E0308`, ...) only on `textDocument/diagnostic` (`diagnosticProvider`), and pushes `cargo check`
  results (`checkOnSave`). The generic client therefore pulls itself, as the host's warming does for Roslyn: on
  `didOpen`, 150 ms after the last `didChange`, and for every open document when `experimental/serverStatus` turns
  quiescent or the server sends `workspace/diagnostic/refresh`; it delivers one merged list per document (a pushed
  and a pulled diagnostic with the same range and code are one). Documented in host-rpc.md's generic-client section.
- **LSP extension used:** `experimental/serverStatus` (`health`, `quiescent`), enabled with the client capability
  `experimental.serverStatusNotification`; it and `$/progress` drive the status bar slot
  (`rust-analyzer: Indexing 120/300 (core) 40%` → `rust-analyzer: ready (1.98.1 …)`).
- **Settings:** `cargo.targetDir: true` (rust-analyzer's `cargo check` uses `target/rust-analyzer`, so it never holds
  the lock of the user's `cargo build`), `checkOnSave: true`, `cachePriming.enable: true`.
- **Gotcha found:** rust-analyzer does not load files under hidden folders, so the real-server test uses a temporary
  folder whose name does not start with `.`.

## What was built, by step

1. **Schemas first** (`e54e000`): `eludite.workspace.open_folder` (a folder or a `Cargo.toml`; this is the brief's
   `eludite/workspace/open`: no process boundary is crossed, the shell computes the folder and Cargo models),
   `eludite.workspace.tree` (`eludite/workspace/tree`: `kind` per project `csproj`/`cargo`/`folder`, Cargo targets,
   dependencies), `system` on `eludite/build/start` (`msbuild` or `cargo`, toolchain kind `cargo`) and on the build
   commands (`msbuild`, `cargo`, `all`), a `language_servers` Output source, and host-rpc.md's "Generic language
   servers and Cargo" section. The host is not touched: it builds `msbuild` only and the shell never sends it `cargo`.
2. **Generic client** (`ad8074f`): `connection.rs` (supervision, framing, correlation, `$/cancelRequest`,
   generation pinning, document notifications, restarts) with a `Dialect` per peer; `client.rs` (host dialect,
   public API unchanged); `server.rs` (LSP dialect: `initialize` with the host's client capabilities plus
   `serverStatusNotification` and diagnostics refresh, `initialized`, `shutdown`/`exit`, `workspace/configuration`
   from the registration, `$/progress` and `serverStatus` as events, a generation per (re)start); `pull.rs`
   (pull-and-merge diagnostics); `registry.rs` + `servers.json`; `fake_server.rs` (`FakeServer`).
3. **Models** (`60754ff`): `eludite-workspace::cargo` (members, targets with kinds, dependencies), `::folder`
   (listing without `target/`, `bin/`, `obj/`, `node_modules/` and hidden folders; the solution at the root or one
   folder down, `3ce5cd9`; `Cargo.toml`), `SolutionModel::compose` (folder root; solution and Cargo workspace as
   siblings; members as `name (bin)`, each with a `Targets` folder and its files; then `Cargo.toml`, `Cargo.lock`;
   then the folder's other files).
4. **Shell** (`2ea05a5`): File > Open Folder (menu, Ctrl+Shift+Alt+O, `--folder`), the composed Workspace tree,
   `eludite.workspace.tree`, `ServerSession` (the old `HostSession` generalized: the same worker for host and generic
   servers), per-document server routing (`servers.rs`), per-server generation, capabilities and IntelliSense
   provider, a status bar slot per generic server, the Language Servers Output source, the workspace-edit applier
   checking the generation of the server that computed an edit, `didChangeWatchedFiles` to every server.
5. **Cargo build** (`48696e9`): `cargo_build.rs` (runner, JSON parsing, chunked output, progress, process-group
   kill), Ctrl+Shift+B choosing the active system (the active document's, else every system, MSBuild then cargo, as
   one build), Build Project building the active document's Cargo package (`-p`), Release as `--release`,
   Rebuild/Clean through `cargo clean`.
6. **Tests** (`e695de5` and the ones in each step), **fetch scripts** (`f758bb8`), **harness** (`619c131`: folder
   and generic-server timings in `--timings-out`, the generic servers' state in `--bench-type`, readiness for
   `--bench-complete` and `--bench-build`), **merge fix** (`66c9385`), **manual run** (`1b521e1`, `d8e8747`).

## What remained Roslyn-specific

- **Host-only features:** solution open/close, the MSBuild tree, MSBuild builds, solution generations, the solution
  load status and its `ELUDITE000x` diagnostics, metadata-as-source (`<temp>/MetadataAsSource/`), Roslyn's
  client-side commands in code actions (`roslyn.client.nestedCodeAction` submenus, `fixAllCodeAction`), and the host's
  own warming. These live in the host session or in host-specific branches; generic documents never reach them.
- **IntelliSense provider rules** differ by server kind in one function (`Shell::provider`): the host's combines
  the language server state with the solution load; a generic server's uses its own state and quiescence.
- **`workspace/executeCommand`** is still not sent to any server (rust-analyzer's code actions apply through edits;
  its runnables/`rust-analyzer.*` client commands are not offered).
- **Status text** for the host stays `C#: running` in its slot; generic servers get one slot each.

## Measurements

Machine: AMD Ryzen 9 7940HS (16 threads), 30 GiB, CachyOS, nested virtual KWin 60 Hz, release build. Another agent
was building in a sibling worktree; the 1-minute load average is given with each step.

**Open this repository** (`--timings-out`, Wayland; `cold` = `target/rust-analyzer` removed first):

| Run (load) | Open to editable | Folder listed (589 files) | `cargo metadata` | Workspace tree (host) | Solution loaded | rust-analyzer running | first diagnostics for buffer.rs | rust-analyzer ready (quiescent) |
|---|---|---|---|---|---|---|---|---|
| cold (6.3) | 25 ms | 4 ms | 29 ms | 562 ms | 4.4 s | 56 ms | 56 ms | **49.3 s** |
| warm 1 (13.5) | 32 ms | 6 ms | 80 ms | 1.29 s | 3.5 s | 128 ms | 128 ms | 7.8 s |
| warm 2 (13.4) | 24 ms | 6 ms | 31 ms | 560 ms | 2.7 s | 67 ms | 67 ms | 6.7 s |
| warm 3 (12.8) | 192 ms | 19 ms | 89 ms | 980 ms | 3.1 s | 177 ms | 177 ms | 7.1 s |

Editable text is under 1 s in every run (budget met). The first diagnostics for buffer.rs are the empty list of a
clean file (the first pull answers at once); rust-analyzer is ready, with full semantic diagnostics, at quiescence:
49 s cold (within the brief's expected 10 to 60 s), 7 to 8 s warm. In the drive, a type error typed after
quiescence was a live squiggle and Error List row 355 ms after the edit (150 ms debounce + pull). Shell RSS with the
folder open: 99 MB.

**Keystroke frame cost while rust-analyzer indexes** (`--bench-type 500` in buffer.rs; typing starts when the file's
first diagnostics arrive, with rust-analyzer still `Fetching cargo metadata`; frame cost = key handler + render to
end of present):

| Run (load) | p50 | p95 | p99 | rust-analyzer at the end |
|---|---|---|---|---|
| 1 (11.2) | 4.8 ms | 10.4 ms | **14.2 ms** | still busy (`cargo check swash (lib)`, its start-up check) |
| 2 (5.9) | 2.9 ms | 5.2 ms | 6.1 ms | ready |
| 3 (4.2) | 2.9 ms | 5.1 ms | 5.8 ms | ready |
| no server, 3 runs (12.1, 5.1, 4.0) | 2.7 ms | 3.9 to 4.0 ms | 5.4 to 5.9 ms | (unavailable) |

The 8 ms p99 budget holds in two of three runs; run 1 missed it at load 11 with rust-analyzer's start-up
`cargo check` compiling the workspace's dependencies on every core. The shell does no rust-analyzer work on the UI
thread (all of it is on the session worker, the reader and pull threads); this needs a quiet reference machine to
tell machine contention from shell cost.

**Completion** (`--bench-complete 100` after `self.` in buffer.rs, after rust-analyzer is quiescent):

| Run (load) | rust-analyzer (request written to reply read) p50 / p95 | UI latency p50 / p95 | trigger to visible p95 | keystroke cost while filtering p99 |
|---|---|---|---|---|
| 1 (8.8) | 75.9 / 87.2 ms | 3.1 / 5.1 ms | 111 ms | 6.8 ms |
| 2 (3.8) | 73.8 / 77.8 ms | 3.1 / 4.8 ms | 101 ms | 6.6 ms |
| 3 (3.2) | 71.7 / 75.6 ms | 3.2 / 5.4 ms | 114 ms | 6.3 ms |

The brief's budget, the popup under 50 ms p95 after rust-analyzer answers, holds: the shell's own part is 5 ms p95.
rust-analyzer itself takes 72 to 87 ms (it lists 140 to 200 members for `self.` with documentation resolved
lazily). No timeouts in 300 triggers. RSS rose 105 → 110 MB over 100 cycles.

**Cargo build through the brief 0017 path** (`--bench-build 5`, Ctrl+Shift+B with buffer.rs active, nothing to
rebuild): first Output line 2.4 to 4.3 ms p50 (5.0 ms max; budget 100 ms), presented 14 to 19 ms p50; finished to
Error List rows 0.3 to 0.4 ms p50; key to finished 317 to 357 ms p50 (one 6.7 s outlier at load 6); frame cost while
building p99 3.7 to 4.8 ms. In the drive, the failing build (type error in eludite-editor) took 0.6 s key to
finished; the confirmation drive after the rebase 1.7 s.

## Tests

`cargo test --workspace`: **433 passed, 0 failed** (40 new). `cargo fmt --all --check` and
`cargo clippy --workspace --all-targets -- -D warnings` are clean. `.NET`: not touched (built only, 0 warnings, for
the manual run).

| Behavior (Proving test) | Test |
|---|---|
| Registration as data, discovery order, dead rustup proxy skipped | `eludite-lsp` `registry::tests` (5) |
| Launch: LSP handshake with the registration's options, `initialized`, shutdown/exit | `generic_server::handshake_sends_lsp_initialize_with_registration_options_then_initialized` |
| Document notifications | `generic_server::documents_diagnostics_and_completion_through_the_shared_connection`; shell `rust_documents_go_to_the_generic_server_and_its_state_reaches_the_status_bar` |
| Progress to the status bar | `generic_server::progress_and_server_status_arrive_as_events`; shell test above (`Indexing 1/2 (core) 50%`, then `ready`) |
| Diagnostics (push, pull, merge, stale versions) | `generic_server::pulled_and_pushed_diagnostics_merge_per_document`; shell `generic_diagnostics_become_squiggles_and_live_error_list_rows` |
| Completion through the shared popup, and Quick Info, Go To Definition, rename, code actions | shell `completion_quick_info_definition_rename_and_code_actions_use_the_shared_paths` |
| Crash restart (new generation, documents replayed), restart budget | `generic_server::a_crash_restarts_with_a_new_generation_and_drops_old_results`, `the_restart_budget_gives_up`; shell `a_crash_restarts_the_generic_server_and_replays_its_documents` |
| Cancellation, `workspace/configuration`, `workspace/applyEdit` from the server | `generic_server` (2) |
| Cargo model from recorded `cargo metadata` (this repository) | `eludite-workspace` `cargo::tests` (5) |
| Folder model and the composed tree (mixed repository, loading parts) | `folder::tests` (2), `explorer::tests` (2), shell `open_folder_shows_the_solution_and_the_cargo_workspace_side_by_side` |
| Cargo JSON parsing from recorded output | `cargo_build::tests` (3) |
| Recorded JSON into Error List rows, dedup with live rows, click-through | shell `cargo_rows_from_recorded_json_dedup_with_live_rows_and_click_through` (Unix) |
| Cancel kills cargo | shell `cancel_kills_cargo` (Unix) |
| Real `cargo build`, MSBuild then cargo as one build | shell `a_real_cargo_build_streams_into_output_and_both_systems_build_with_no_active_document` |
| Real rust-analyzer (skips when absent) | `real_analyzer::real_rust_analyzer_diagnoses_and_completes` |
| `eludite.workspace.tree`, `eludite.workspace.open_folder`, `system` parsing | `eludite-commands` (3) |
| Status bar text for each server state | shell `servers::tests` |

## Manual run

`crates/eludite/tools/rust-linux.sh OUT_DIR` (nested KWin; X11 with XTest for the drive, `tools/rust.py`; Wayland
for timings and benches). Screenshots:

- `crates/eludite/screenshots/linux-workspace-side-by-side.png`: the folder root with `Solution 'Eludite' (8 of 8
  projects)` and `Cargo workspace '0019-rust-workspace' (16 members)` as siblings, members labelled `eludite (bin)`,
  `eludite-acp (lib, bin)`; status bar `C#: running`, `rust-analyzer: ready (1.98.1 …)`.
- `crates/eludite/screenshots/linux-rust-completion.png`: rust-analyzer's list after `self.text.` in `Buffer::len`
  with the signature of the selected method (key to reply 431 ms, the first request of a fresh file).
- `crates/eludite/screenshots/linux-rust-build-error.png`: after Ctrl+Shift+B, `Build failed: 1 error, 0 warnings`,
  one `E0308` row in eludite-editor's buffer.rs 217:25 from `Build + IntelliSense`, the caret on it after a
  double-click.
- Also `crates/eludite/screenshots/linux-rust-diagnostics.png`: the squiggle and the live row before the build.

## Gaps and follow-ups

- **Windows and macOS not run.** Paths go through `normalize_path` and URIs, the server name gets `EXE_SUFFIX`,
  cancel uses `taskkill /T /F` on Windows (untested), `fetch.ps1` is untested. Two shell tests are Unix-only (they use
  a shell-script `cargo`).
- **Keystroke p99 at high load** (above). Re-measure on the quiet reference machine.
- **rust-analyzer's start-up `cargo check`** compiles the workspace once into `target/rust-analyzer` (minutes of CPU
  on a cold cache). It is rust-analyzer's default and its results are the live `cargo check` rows; turning it off is a
  one-line registration change if the owner prefers native diagnostics only.
- **Error List rows show multi-line messages on one line** (overflowing into the next row) when a server sends them;
  rustc's `cargo check` messages are multi-line. Pre-existing rendering; now visible with Rust.
- **Open Folder's dialog** was not exercised in the nested session (the drive uses `--folder`, the same command);
  the menu item and shortcut are covered by the keymap and menu tables.
- **One generic server per registration and root;** a `.rs` file outside the open Cargo workspace starts another
  rust-analyzer at its nearest `Cargo.toml`. Opening another folder shuts the old folder's servers down.
- **No file watching:** files changed on disk outside the editor (other than by the applier) are not reported with
  `didChangeWatchedFiles` (rust-analyzer watches the workspace itself, with `notify`).
- **Not in scope and not done:** debugging Rust, `cargo test` in Test Explorer, crates.io UI, `Cargo.toml` completion.
- **Out of my file scope, so not updated:** `README.md`/`CLAUDE.md` (the new `tools/rust-analyzer/` and the
  rust-analyzer requirement) and `docs/briefs/README.md`'s index row.
- New dependencies: none (crates already in the workspace: `serde_json` for `eludite-workspace`, `tempfile` as a
  dev-dependency of `eludite-lsp` and `eludite-workspace`). rust-analyzer is an external tool: `MIT OR Apache-2.0`.

## Sizing the next briefs

- **TypeScript and the web stack through the same path: medium (about one agent-week).** Add registrations
  (`typescript-language-server --stdio` for `*.ts`/`*.tsx`/`*.js`, `vscode-css/html-language-server`) to
  `servers.json`, discovery through `node_modules/.bin` and an npm global (one new discovery source in
  `CommandSpec`), a `package.json` workspace model beside `cargo` (scripts as targets), and an `npm run build` runner
  in the build-system plan with `tsc`'s problem matcher instead of cargo's JSON. The client, routing, status bar,
  merge and IntelliSense paths need no change; the editor needs tree-sitter TS/JS grammars registered.
- **F# and VB.NET through the host: medium to large.** VB: Roslyn already serves VB; route `*.vb` to the host
  (`servers.json`: `via: eluditeHost`, language `vb`), add the VB grammar and include `.vbproj` items in the tree
  (about three days). F#: FsAutoComplete is a separate server; running it inside the host (invariant 2 keeps it out
  of the shell) means a second upstream in the host's bridge with its own document routing and warming, about two
  agent-weeks.
- **Phase 1 close-out list:** a quiet-machine benchmark pass for all budgets (this brief's keystroke p99 included);
  the Windows pass of briefs 0007 to 0019 (`windows-checklist.md`); multi-line Error List rows; file watching into
  `didChangeWatchedFiles`; `workspace/executeCommand` for server commands; the host restarting Roslyn on its own;
  README/CLAUDE.md updates for the generic-server path and `tools/rust-analyzer/`.
