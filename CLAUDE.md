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

Linux needs, on Debian/Ubuntu: `libwayland-dev libxkbcommon-x11-dev libvulkan-dev libfontconfig1-dev libssl-dev libgit2-dev pkg-config cmake clang`. On Arch: `wayland libxkbcommon vulkan-icd-loader fontconfig openssl libgit2 pkgconf cmake clang`. Optional: Mono (`mono-devel` on Debian/Ubuntu, `mono` on Arch) to debug .NET Framework programs with `eludite-dbg-mono`; without it that adapter's tests skip. Optional: lldb-dap (`lldb-18` on Debian/Ubuntu, `lldb` on Arch) to debug Rust (Cargo packages); without it `crates/dap/tests/lldb.rs` skips. Optional: `corpus/tests/build.sh` (`build.ps1` on Windows) builds the Test Explorer's corpus in place; without it the real-host and shell Test Explorer tests that use it skip. Optional: CEF for the embedded browser engine: `export CEF_PATH="$(tools/cef/fetch.sh)"`, then add `--features eludite-chromium/cef` to the cargo commands; without it the engine tests skip. Optional: vscode-js-debug and Node.js 18 or later to debug pages' JavaScript: `export ELUDITE_JS_DEBUG="$(tools/js-debug/fetch.sh)"`; without it the real js-debug tests skip (the recorded `corpus/dap/js-debug/` replays everywhere). Optional: the web language servers and formatters with Node.js 20 or later and npm: `export ELUDITE_WEB_SERVERS="$(tools/web-servers/fetch.sh)"`; without it the real web-server tests skip. Optional: an OpenAI-compatible server (a llama.cpp `llama-server`) for `agents/openai-acp`'s real-server test: `ELUDITE_LLAMA_SERVER=http://localhost:8080/v1` (`ELUDITE_LLAMA_MODEL` picks the model); without it that test skips. `agents/openai-acp` and `agents/claude-acp` are their own Cargo workspaces: run `cargo test` and `cargo clippy --all-targets -- -D warnings` inside them. Optional: FsAutoComplete (the F# language server, MIT) on the machine's .NET SDK: `export ELUDITE_FSAUTOCOMPLETE="$(tools/fsautocomplete/fetch.sh)"`; without it `crates/lsp/tests/real_fsautocomplete.rs` skips. Optional: the pinned Roslyn language server built from source with Visual Basic composed in: `tools/roslyn-pin/build.sh`; without it the host tests against the real Roslyn skip.

## Crate and project map

| Path | Role | PLAN.md |
|---|---|---|
| `crates/eludite` | App binary: entry, window, layout | 3 (D1), 8 |
| `crates/docking` | Tool windows, document tabs, layouts (layout schema v5 with migrations) | 8 |
| `crates/ui` | Widgets, themes (Eludite Dark, the default, Eludite Light and Classic; with the `warning`, `success` and `panel_raised` tokens, brief 0059, and the `icon_*` tints, brief 0062), keymaps; `icons` (brief 0062): Eludite's own 16 by 16 SVG icon set in `crates/ui/icons/` (solution, workspace, Cargo workspace and package, folders and solution folders, the C# project variants, the Dependencies node, Cargo targets, files by type, the search box), embedded and served by `icons::Assets` (the shell's `Application::with_assets`), drawn by `icon()` (monochrome `currentColor` icons tinted by the theme, colored ones as images; the drawing rules are the module docs), and tree rows led by an icon with search matches in bold (`tree_row_with_icon`); `fonts`: the embedded Instrument Sans (UI) and JetBrains Mono (code) faces in `crates/ui/fonts/` (OFL-1.1, Fontsource's Latin and Latin Extended subsets merged; the license texts ship in the packages' `licenses/fonts/`), registered at startup; `crystal`: Eludite's mark as a 3D cube under a fixed light, drawn at the title bar's left and on the Welcome page, with its motion (the Welcome open, the spin while building or a debuggee runs, the eased stop); the Agents window's transcript widgets (`transcript`: prompts, Markdown blocks, thoughts, tool call cards with the kind glyphs and the spinner, plans, the usage strip and the status line of a running turn, brief 0059) | 8 |
| `crates/editor` | Buffer, view, input; `TextInput`, a text box over the editor core with wrapped rows, selection, the clipboard, undo, IME and 2 to 8 rows then scrolling (the Agents window's prompt box, brief 0057); tree-sitter highlighting for C#, Rust and (brief 0050) TypeScript, TSX, JavaScript, HTML with its `<script>` and `<style>` injected, CSS, JSON and JSON with comments, and (brief 0056) Razor (`.razor`, `.cshtml`) on `grammars/razor` with the C# query inside code and its `<script>` and `<style>` injected, and (brief 0063) Visual Basic (`tree-sitter-vb-dotnet`) and F# with its signature files (`tree-sitter-fsharp`), both from crates.io with Eludite's queries; Emmet on Tab | 4.1 |
| `crates/commands` | Command bus, schemas, audit | 5.1 |
| `crates/workspace` | Solution and project model, client side | 4.2 |
| `crates/lsp` | LSP client: the host bridge and the generic client over one connection core; `servers.json` registrations (brief 0019; the web entries `typescript`, `eslint`, `html`, `css`, `json` and the formatters `prettier` and `biome`, brief 0050), several servers per file with the fan-out and merge rules (`fanout`), npm servers found in the project's `node_modules` first, and the Node.js search shared with `crates/dap` (`node`); (brief 0063) `*.vb` to the host with the `vb` language id, and `fsautocomplete` for F# as a `process` server found as a .NET global tool (beside `eludite`, `ELUDITE_FSAUTOCOMPLETE`, the pinned cache of `tools/fsautocomplete/`, `~/.dotnet/tools`, `PATH`), with root markers by glob (`*.fsproj`) | 3 (D3), 4.3 |
| `crates/dap` | DAP client and transports; vscode-js-debug's discovery, TCP server, `startDebugging` and source-mapped frames (brief 0038; its Node.js search now lives in `crates/lsp`'s `node`, re-exported) | 3 (D7), 4.5 |
| `crates/acp` | ACP client | 5.2 |
| `crates/mcp` | MCP server over the command bus | 5.1, 5.2 |
| `crates/browser` | Browser automation (`eludite.browser.*`): the engine trait, the external Chrome over CDP, tabs, refs, console and network rings (brief 0023); `EmbeddedChromium`, the engine over `eludite-chromium` with frames from shared memory (brief 0031); `record`, `devtools`, `dialog`, the engine selection (`browser.engine`) and the key table of the Web Browser window (brief 0032); a tab's debugging endpoint and target id, and `input`'s `paused` for a page stopped in the debugger (brief 0038); `EngineSearch` and `CefSearch`, the engine and CEF found beside the executable first, on first use only (brief 0039) | 4.9, 5.8 |
| `crates/git` | `eludite-git`, libgit2 with no `git` process (brief 0040): status with renames and the Workspace glyphs, stage, unstage, discard, commit with the identity rule, diff texts, log with graph lanes, branches, checkout, merge, rebase, cherry-pick, reset, stash, fetch, pull and push with the credential callback, worktrees, blame, and the status cache with its polling watcher; the shell's Git Changes and Git Repository windows and `eludite.git.*` sit on it | 4.8 |
| `crates/terminal` | `eludite-terminal` (brief 0041): a shell on a real PTY (`alacritty_terminal::tty`: Unix PTYs, ConPTY on Windows) read by its own I/O thread, `alacritty_terminal`'s emulator (from crates.io, never vendored), the plain-text transcript and marks agents read and wait on, shell integration scripts (OSC 133 and OSC 7 for bash, zsh, fish and PowerShell, passed through each shell's startup options), profiles (Developer PowerShell on Windows), the located tools first on PATH, links, and our own GPUI view (never Zed's `terminal_view`); the shell's Terminal window and `eludite.terminal.*` sit on it | 4.11 |
| `crates/search` | `eludite-search` (brief 0042): Find in Files and Replace in Files on ripgrep's engines (`ignore`'s parallel walker with `.gitignore`, `search.excludes` and File types; `grep-regex` and `grep-searcher`, multiline off, binary files skipped, UTF-8 and UTF-16 by BOM), streaming per file with cancellation and a cap, the open-documents overlay, Visual Studio's whole-word rule, and the replacement engine with `$1` groups; the shell's Find in Files dialog, the Find Results 1 and 2 windows and `eludite.search.*` sit on it | 4.11 |
| `crates/forge` | `eludite-forge` (brief 0046): one `Forge` trait with a capabilities table over GitHub (REST and GraphQL), GitLab, Azure DevOps, Forgejo and Gitea, and Tangled (AT Protocol lexicons); detection from the remote url and `forge.hosts` (version probes for unknown hosts), the HTTP client on `ureq` and `rustls` with conditional requests, rate limits, timeouts and cancellation, the stale-while-refreshing cache under the workspace's `.eludite/forge/` (never a token), the credential store through `keyring` with a consented 0600 file fallback, the device flows, and the replay transport and loopback fixture server its tests and the shell's run on; the shell's Pull Requests and Issues windows, the pull request document and `eludite.forge.*` sit on it | 4.8 |
| `crates/update` | `eludite-update` (brief 0055, ADR-0011): self-update from GitHub releases by channel (`unstable` first): `build.json` beside the executable, the release list with conditional requests, the archive streamed and verified against `SHA256SUMS`, staged in `.eludite-update/` inside the install folder, unpacked (`tar` and `flate2`, a small zip reader), swapped in on restart by a copy of the new executable (`eludite --apply-update`) with the old build kept in `.eludite-previous/` until the next start; the shell's Help > Check for Updates, the status bar's update slot, the Output window's Updates source, the first-start question and `eludite.update.*` sit on it; the release contract is `tools/package/RELEASE.md` | 10 (Phase 2) |
| `crates/extensions` | wasmtime extension host | 3 (D6) |
| `grammars/razor` | `tree-sitter-razor`: the owner's Razor tree-sitter grammar in-repo (MIT, extending tree-sitter-c-sharp), its parser generated at build time from the checked-in `src/grammar.json` by `tree-sitter-generate` at the CLI's pinned version (`PIN`) and cached by content under `~/.cache/eludite/grammars/` (ADR-0012; `grammar.json` written only by `grammars/razor/generate.sh`), its corpus replayed by `cargo test` with no CLI (brief 0056) | 4.9, 7 |
| `protocol/` | MIT schemas and generated bindings (crate `eludite-protocol`); `protocol/forge/` holds the pinned forge API descriptions and Tangled's lexicons, reference only, nothing generated (brief 0046) | 3 (D3), 11 |
| `protocol/cdp/` | The pinned Chrome DevTools Protocol JSON and `eludite-cdp-generator` (MIT), which writes `protocol/rust/src/cdp/` | 4.9, 11 |
| `extension-sdk/` | MIT WASM extension API (crate `eludite-extension-sdk`) | 3 (D6) |
| `agents/claude-acp/` | `eludite-claude-acp`: MIT, standalone ACP adapter driving the `claude` binary, no Node (brief 0006) | 5.2 |
| `agents/openai-acp/` | `eludite-openai-acp` (brief 0060, ADR-0013): MIT, standalone ACP agent over any server that speaks the OpenAI Chat Completions API (llama.cpp, Ollama, vLLM, LM Studio, OpenRouter, OpenAI and the rest) on `ureq` and `rustls`, whose only tools are the MCP servers the client passes (the IDE's endpoint; the core set plus `eludite-tools`); models from `GET /models` as the `model` config option, streaming with the servers' quirks, compaction, `/compact` and `/clear`; the key only from `ELUDITE_OPENAI_API_KEY`, never logged. The servers are `agents.json`'s `providers` (`crates/acp`'s `ProviderSettings`, source `provider`), their keys in the credential store under `provider:<name>`, edited through `eludite.agents.provider_*`; `eludite.file.read` and `eludite.file.edit` (`crates/commands`' `files`) serve it and every other agent | 5.2 |
| `debuggers/netfx` | `eludite-dbg-netfx`, ICorDebug DAP server; Windows at runtime, compiles everywhere | 4.5, 13 |
| `browsers/chromium` | `eludite-chromium`: CEF's browser process (GPL), windowless tabs rendered into a shared-memory frame ring, JSON-RPC control on stdio (`protocol/schemas/browser-rpc/`); the real engine needs the `cef` feature and `CEF_PATH` from `tools/cef/fetch.sh`, else it is a stub and its tests skip; Linux only so far (brief 0031); popups, dialogs, permissions, downloads, DevTools as a tab and the privacy switches (brief 0032); the remote debugging port on loopback, announced in `engine/ready` (brief 0038); the sandbox rule (user namespaces, else the setuid helper, else a refusal; `--no-sandbox` only with `--allow-no-sandbox`) and CEF in `cef/` beside it (brief 0039) | 4.9, 12 |
| `debuggers/mono` | `eludite-dbg-mono`, Mono soft-debugger DAP server (C#, net472 on Mono.Debugging.Soft, runs under the located Mono): .NET Framework debugging on Linux and macOS; built by `dotnet build dotnet/Eludite.slnx` (brief 0022) | 4.5 |
| `dotnet/` | `eludite-host`: Roslyn LSP embedding, project system, NuGet, EnC; tests discovered and run out of process over Microsoft.Testing.Platform's server mode and VSTest's translation layer (`Eludite.TestBridge`, brief 0035); the NuGet client on the NuGet.Client packages (`NuGet/`, brief 0048): the `NuGet.config` chain, search, installed sets from the assets files, updates, `PackageReference` and `Directory.Packages.props` edits that keep formatting, `dotnet restore` out of process with the lock-file rule, credential providers then the shell's prompt, vulnerability and deprecation data, and the tree's Dependencies node; `eludite/nuget/*`, the Manage NuGet Packages window and `eludite.nuget.*` sit on it; the project property catalog, evaluation per configuration and the formatting-preserving project edit, launch profiles and solution configurations (`Projects/`, brief 0049); `.csproj`, `.vbproj` and `.fsproj` alike in the project list, the tree, tests and properties, an `.fsproj` never handed to Roslyn (brief 0063) | 3 (D2, D4), 4.3, 4.6, 4.7 |
| `vendor/` | Pinned Zed crates, each with `WHY.md` | 3 (D1) |
| `tools/package/` | `linux.sh` (brief 0039): the release build of `eludite` and `eludite-chromium` with CEF from `tools/cef/fetch.sh`'s cache, laid out as `eludite-<version>-linux-<arch>/` (`eludite`, `eludite-chromium`, `cef/` with CEF's runtime files, the sandbox helper and licenses, `eludite.desktop`, `icons/`, `README`, `LICENSE`, `THIRD-PARTY-CRATES.txt`) and its tarball; its smoke test is `browsers/chromium/tests/package.rs`. `companions.sh` adds the .NET host (`eludite-host/`, a framework-dependent publish), `eludite-dbg-mono/` and `eludite-claude-acp` beside the shell, where it finds them (`linux.sh --with-companions`). `shell.sh`: the same layout without the engine for Windows (`.zip`) and macOS (`.tar.gz`), or Linux without CEF. CI's `package` job uploads all three archives plus the Windows MSI as artifacts `eludite-<version>-<os>-<arch>`. `windows.ps1` builds the unsigned, machine-wide MSI from `shell.sh`'s Windows layout without `build.json` (MSI upgrades replace archive self-update); the engine's Windows CEF packaging and macOS nested bundle are written up, not built | 4.9, 13 |
| `tools/` | Pinned external tools located at run time, never vendored: `roslyn-pin/` (language server build; `vb.patch` composes Visual Basic in, brief 0063), `fsautocomplete/` (the F# language server as a pinned .NET tool, brief 0063), `netcoredbg/` (debugger fetch), `rust-analyzer/` (fetch), `chrome/` (Chrome for Testing fetch), `cef/` (CEF minimal distribution fetch, brief 0031), `js-debug/` (vscode-js-debug release fetch by checksum, run on the machine's Node.js; brief 0038), `web-servers/` (the web language servers and formatters as pinned npm packages with a checked-in lockfile, `npm ci` into `~/.cache/eludite/web-servers/<pin>/`, and the SchemaStore schemas by checksum; brief 0050), `lldb-dap/` (install notes only: distributions carry it; brief 0029), `legacy-load/` (brief 0003 runner); also `dap-corpus/` (re-records `corpus/dap/`, brief 0033) and `forge-corpus/` (re-records `crates/forge/testdata/` from the real forges, scrubbed, and re-synthesizes the rest; brief 0046) | 3 (D3), 4.3, 4.5, 7 |

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

The budget tests assert these on a developer machine; under CI (`CI` set) they print their numbers instead, because the hosted runners are not a reference machine. A shell-touching PR that regresses any benchmark by more than 5 percent does not merge.

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
- Do not create any git commit, on any branch, and do not push to the remote on weekdays between 8 am and 5 pm Central time (America/Chicago) unless the owner overrides it for that push. Work in a worktree can continue uncommitted; the commits, the rebase, the merge and the push wait for the window to close. Check `TZ=America/Chicago date` before each one.

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
