# CLAUDE.md

Instructions for every agent working in this repository. Read this file, then the ADRs in `docs/adr/` before proposing any structural change.

## Purpose

Eludite is a native, cross-platform, agent-first IDE, .NET-first but also first-class for the web stack and Rust (PLAN.md section 7), built by one person directing many agents. The master plan is [docs/PLAN.md](docs/PLAN.md). This file restates what an agent must check on every task. If this file and PLAN.md disagree, PLAN.md wins and this file has a bug to fix.

## Build and test commands

Run from the repo root. Toolchains are pinned (`rust-toolchain.toml` = 1.98.1, `global.json` = SDK 10.0.302).

```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
dotnet build dotnet/Eludite.slnx
dotnet test dotnet/Eludite.slnx
```

Linux needs, on Debian/Ubuntu: `libwayland-dev libxkbcommon-x11-dev libvulkan-dev libfontconfig1-dev libssl-dev libgit2-dev pkg-config cmake clang`. On Arch: `wayland libxkbcommon vulkan-icd-loader fontconfig openssl libgit2 pkgconf cmake clang`. Optional: Mono (`mono-devel` on Debian/Ubuntu, `mono` on Arch) to debug .NET Framework programs with `eludite-dbg-mono`; without it that adapter's tests skip. Optional: lldb-dap (`lldb-18` on Debian/Ubuntu, `lldb` on Arch) to debug Rust (Cargo packages); without it `crates/dap/tests/lldb.rs` skips. Optional: `corpus/tests/build.sh` (`build.ps1` on Windows) builds the Test Explorer's corpus in place; without it the real-host and shell Test Explorer tests that use it skip. Optional: CEF for the embedded browser engine: `export CEF_PATH="$(tools/cef/fetch.sh)"`, then add `--features eludite-chromium/cef` to the cargo commands; without it the engine tests skip.

## Crate and project map

| Path | Role | PLAN.md |
|---|---|---|
| `crates/eludite` | App binary: entry, window, layout | 3 (D1), 8 |
| `crates/docking` | Tool windows, document tabs, layouts (layout schema v3 with migrations) | 8 |
| `crates/ui` | Widgets, themes, keymaps, icons | 8 |
| `crates/editor` | Buffer, view, input | 4.1 |
| `crates/commands` | Command bus, schemas, audit | 5.1 |
| `crates/workspace` | Solution and project model, client side | 4.2 |
| `crates/lsp` | LSP client | 3 (D3), 4.3 |
| `crates/dap` | DAP client and transports | 3 (D7), 4.5 |
| `crates/acp` | ACP client | 5.2 |
| `crates/mcp` | MCP server over the command bus | 5.1, 5.2 |
| `crates/browser` | Browser automation (`eludite.browser.*`): the engine trait, the external Chrome over CDP, tabs, refs, console and network rings (brief 0023); `EmbeddedChromium`, the engine over `eludite-chromium` with frames from shared memory (brief 0031); `record`, `devtools`, `dialog`, the engine selection (`browser.engine`) and the key table of the Web Browser window (brief 0032) | 4.9, 5.8 |
| `crates/git` | libgit2 wrapper | 4.8 |
| `crates/terminal` | Integrated terminal | 4.11 |
| `crates/extensions` | wasmtime extension host | 3 (D6) |
| `protocol/` | MIT schemas and generated bindings (crate `eludite-protocol`) | 3 (D3), 11 |
| `protocol/cdp/` | The pinned Chrome DevTools Protocol JSON and `eludite-cdp-generator` (MIT), which writes `protocol/rust/src/cdp/` | 4.9, 11 |
| `extension-sdk/` | MIT WASM extension API (crate `eludite-extension-sdk`) | 3 (D6) |
| `agents/claude-acp/` | `eludite-claude-acp`: MIT, standalone ACP adapter driving the `claude` binary, no Node (brief 0006) | 5.2 |
| `debuggers/netfx` | `eludite-dbg-netfx`, ICorDebug DAP server; Windows at runtime, compiles everywhere | 4.5, 13 |
| `browsers/chromium` | `eludite-chromium`: CEF's browser process (GPL), windowless tabs rendered into a shared-memory frame ring, JSON-RPC control on stdio (`protocol/schemas/browser-rpc/`); the real engine needs the `cef` feature and `CEF_PATH` from `tools/cef/fetch.sh`, else it is a stub and its tests skip; Linux only so far (brief 0031); popups, dialogs, permissions, downloads, DevTools as a tab and the privacy switches (brief 0032) | 4.9, 12 |
| `debuggers/mono` | `eludite-dbg-mono`, Mono soft-debugger DAP server (C#, net472 on Mono.Debugging.Soft, runs under the located Mono): .NET Framework debugging on Linux and macOS; built by `dotnet build dotnet/Eludite.slnx` (brief 0022) | 4.5 |
| `dotnet/` | `eludite-host`: Roslyn LSP embedding, project system, NuGet, EnC; tests discovered and run out of process over Microsoft.Testing.Platform's server mode and VSTest's translation layer (`Eludite.TestBridge`, brief 0035) | 3 (D2, D4), 4.3, 4.6 |
| `vendor/` | Pinned Zed crates, each with `WHY.md` | 3 (D1) |
| `tools/` | Pinned external tools located at run time, never vendored: `roslyn-pin/` (language server build), `netcoredbg/` (debugger fetch), `rust-analyzer/` (fetch), `chrome/` (Chrome for Testing fetch), `cef/` (CEF minimal distribution fetch, brief 0031), `lldb-dap/` (install notes only: distributions carry it; brief 0029), `legacy-load/` (brief 0003 runner); also `dap-corpus/` (re-records `corpus/dap/`, brief 0033) | 3 (D3), 4.3, 4.5, 7 |

GPUI is a git dependency on zed-industries/zed at rev `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8`, as the `gpui` and `gpui_platform` crates (both Apache-2.0; `gpui_platform` holds the window backends at this rev). Do not bump it without an ADR note.

## Invariants

These come from PLAN.md section 2. Violating one is a defect even if tests pass.

1. Never block the UI thread. Anything that can take more than a frame runs off-thread or out-of-process, is cancelable, and renders a partial result first.
2. Keep Roslyn, MSBuild, debuggers, test runners and agents out of the shell process.
3. Every user-visible action is a command with a stable ID, JSON schemas and a typed result. Agents and the UI call the same command. Never add an agent-only API.
4. Every cross-process boundary has a schema in `protocol/` checked in before the code on either side. Generate bindings; never hand-edit them.
5. Use Visual Studio names, layout and shortcuts by default. Do not rename old concepts.
6. Speak protocols (LSP, DAP, MTP/VSTest, ACP, MCP) rather than adding bespoke plugin hooks.
7. Do not depend on Zed UI crates (`editor`, `workspace`, `ui`, `theme`, `project`, `terminal_view`, agent panel). Only GPUI and audited low-level text crates may be vendored.
8. Do not use or reference `vsdbg`. It is license-restricted to Microsoft products.
9. Do not bundle, link or redistribute Visual Studio binaries. Build Tools MSBuild is located on the user's machine, never shipped.
10. The host's stdout carries protocol messages only. Logs go to stderr or a file.
11. Do not use Electron, Tauri, WebViews or a VS Code extension runtime.
12. Message handlers carry cancellation and a solution generation number. Drop stale results, do not render them.

## Performance budgets

Enforced in CI on a reference machine. A shell-touching PR that regresses any benchmark by more than 5 percent does not merge.

| Metric | Budget |
|---|---|
| Cold start to interactive window | < 300 ms |
| 100-project solution to editable text with syntax highlighting | < 1 s (semantic features stream in after) |
| Keystroke to frame submitted (input, layout, render; excludes waiting for the display's next refresh) | < 8 ms at p99 |
| Keystroke to pixel, end to end | < one refresh interval + 8 ms at p99 (24.7 ms at 60 Hz, 14.1 ms at 165 Hz) |
| Scrolling a 50k-line file | sustained monitor refresh rate |
| Shell resident memory, 100-project solution, 20 tabs | < 400 MB |
| Completion popup after trigger | < 50 ms p95 from host; tree-sitter fallback immediately |
| Ctrl+Shift+B to first Output line | < 100 ms |

## Definition of done

A PR is done when all of these hold:

- [ ] CI is green on Linux, Windows and macOS (Rust job) and on Linux and Windows (.NET job).
- [ ] Every behavior change has a test. A PR without one is rejected by policy.
- [ ] Benchmarks have not regressed by more than 5 percent.
- [ ] A structural decision has an ADR in `docs/adr/` (new, or a status change on an existing one).
- [ ] README.md and this file still match the repo (layout, commands, crate map).
- [ ] Every commit message is one plain, direct, active-voice line: no body, no trailers, no co-author lines.
- [ ] Any new dependency has its SPDX license id in the PR description.

## How work is issued

- Work arrives as a brief in `docs/briefs/NNNN-name.md`: goal, files in scope, contract, proving test, budget, exit criterion, out of scope. See `docs/briefs/README.md`.
- A feature that PLAN.md names but does not detail, or does not name, is designed first as a proposal in `docs/proposals/NNNN-name.md` (scope, command surface, schemas, process model, briefs). A proposal binds nothing until the owner accepts it and PLAN.md is updated. See `docs/proposals/README.md`.
- One brief per git worktree. Do not work on two briefs in one tree.
- Each worktree builds into its own `target/`. Never point `CARGO_TARGET_DIR` at another checkout's target directory: workspace crates with the same name from different trees overwrite each other's artifacts there, and a test binary can link against a library built from a different branch. The merge check on `main` is the authoritative one.
- The brief declares which files you own. Do not edit files outside that list. If you need a change elsewhere, stop and say so in the PR.
- If a brief is ambiguous or cannot be satisfied as written, report that instead of guessing. Imprecise briefs go back to design.
- Run `git fetch origin` before starting and rebase if `main` has moved.

## Code conventions

Rust:
- Edition 2024. Clippy clean with `-D warnings`. `cargo fmt` clean.
- Every crate has `//!` crate-level docs stating its purpose and its public API boundary.
- Keep public APIs narrow so one agent can own a crate without reading the others.

C#:
- `<Nullable>enable</Nullable>`, warnings as errors.
- Central Package Management (`Directory.Packages.props`); no versions in project files.
- Target the SDK pinned in `global.json`.

Per-crate `CLAUDE.md` files exist only where rules are non-obvious (editor core, debugger, ASPX generator). They add to this file, never override it.

## What not to do

- No telemetry, ever, by default.
- No network calls at startup. Everything works offline except model calls the user chooses to make.
- No new dependency without a license check; put the SPDX id in the PR. It must be compatible with GPL-3.0-or-later, and with MIT for `protocol/` and `extension-sdk/`.
- Do not edit `vendor/` by hand. Changes go through `vendor/sync.sh`, which re-fetches at the pinned Zed commit and reports drift; each vendored crate keeps a `WHY.md`. `vendor/` is its own cargo workspace under Zed's lint rules, used by path from the root workspace.
- Do not edit generated protocol bindings by hand.
- Do not edit recordings in `corpus/dap/` (or their golden files) by hand; re-record them from the real adapters with `tools/dap-corpus/record.sh` (brief 0033). A recording is only ever produced by the recorder.
- Do not add features absent from PLAN.md. Propose them through an ADR or a brief.
- Do not modify `docs/PLAN.md` unless a brief says so.
