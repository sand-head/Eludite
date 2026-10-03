# Eludite

Eludite is a native, cross-platform, agent-first IDE. It is .NET-first, not .NET-only: the first-class workloads are .NET in all its languages (C#, F#, VB.NET) including .NET Framework, WebForms and WCF; modern web development (TypeScript, JavaScript and the front-end stack); and Rust. The goal is feature parity with Visual Studio Community and JetBrains Rider for the .NET workload, with the responsiveness and restraint of Zed, a layout and keymap a Visual Studio user recognizes on day one, and agents as a peer of the human at every surface of the product.

Eludite was called Niello until 2026-10-02.

It is built by one person directing many agents. The shell is Rust on GPUI and draws its own UI. Roslyn, MSBuild, debuggers, test runners and agents each run in their own process, so a hung analyzer cannot freeze the editor. Every action is a command on one bus that both the UI and agents call, and the shell speaks open protocols (LSP, DAP, MTP, ACP, MCP) instead of hosting a VS Code extension runtime.

## Status

Pre-alpha scaffold. Nothing is usable yet. The repository currently holds the plan, the architecture decisions, the crate skeletons and the Phase 0 briefs.

Phase 0 spikes (each has a brief in [docs/briefs/](docs/briefs/)):

- [ ] 0001: GPUI shell and docking prototype on three OSes, plus the Zed crate vendoring audit
- [ ] 0002: `eludite-host` embedding the Roslyn language server, time-to-IntelliSense on a 200-project solution
- [ ] 0003: legacy project load with Mono MSBuild (Linux) and Build Tools (Windows), WebForms code-behind IntelliSense
- [ ] 0004: ICorDebug proof on Windows from Rust over a TCP DAP transport
- [ ] 0005: Claude Code hosted via ACP in a GPUI panel, calling one Eludite MCP tool

## Build

Toolchains are pinned: Rust 1.98.1 by `rust-toolchain.toml`, .NET SDK 10.0.302 by `global.json`.

Shell (Rust workspace):

```
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
```

Host (.NET):

```
dotnet build dotnet/Eludite.slnx
dotnet test dotnet/Eludite.slnx
```

GPUI is a git dependency on zed-industries/zed at a pinned revision, so the first build downloads it.

Linux system packages for the GPUI build:

- Debian/Ubuntu: `libwayland-dev libxkbcommon-x11-dev libvulkan-dev libfontconfig1-dev libssl-dev libgit2-dev pkg-config cmake clang`
- Arch: `wayland libxkbcommon vulkan-icd-loader fontconfig openssl libgit2 pkgconf cmake clang`

Optional, to debug .NET Framework programs under Mono with `eludite-dbg-mono` (and to run its tests, which skip without it): `mono-devel` on Debian/Ubuntu, `mono` on Arch, the Mono framework package on macOS.

Optional, to debug Rust (Cargo packages) with lldb-dap (and to run `crates/dap/tests/lldb.rs`, which skips without it): `lldb-18` on Debian/Ubuntu, `lldb` on Fedora and Arch, Xcode on macOS, LLVM's installer on Windows ([tools/lldb-dap/README.md](tools/lldb-dap/README.md)).

Optional, the embedded browser engine `eludite-chromium` (brief 0031; Linux so far): fetch CEF (326 MB, unpacked to 558 MB in `~/.cache/eludite/cef/`) and build the engine with its `cef` feature; without it a workspace build downloads nothing and the engine's tests skip ([tools/cef/README.md](tools/cef/README.md)):

```
export CEF_PATH="$(tools/cef/fetch.sh)"
cargo build --workspace --features eludite-chromium/cef
```

Windows and macOS need no extra system packages beyond the Rust and .NET toolchains. `debuggers/netfx` compiles on every OS but only runs on Windows; `debuggers/mono` builds everywhere and runs under Mono on Linux and macOS.

## Repository layout

```
crates/            Rust workspace (shell)
  eludite/         binary: entry, window, layout
  docking/ ui/ editor/ commands/ workspace/
  lsp/ dap/ acp/ mcp/ browser/ git/ terminal/ extensions/
vendor/            pinned Zed text crates (sum_tree, rope, text, clock, fuzzy), each with WHY.md
dotnet/            .NET solution (hosts)
debuggers/netfx/   eludite-dbg-netfx (Rust, Windows)
debuggers/mono/    eludite-dbg-mono (C# on Mono.Debugging.Soft, runs under Mono: .NET Framework on Linux and macOS)
browsers/chromium/ eludite-chromium: CEF's browser process, tabs rendered off screen into shared memory (brief 0031)
protocol/          MIT: schemas, generated bindings
  cdp/             the pinned Chrome DevTools Protocol and the generator of its Rust types
  schemas/browser-rpc/  the shell-to-engine control protocol and frame ring of eludite-chromium
extension-sdk/     MIT: WASM extension API
agents/claude-acp/ MIT: native ACP adapter for Claude Code (no Node)
corpus/  bench/    golden-test solutions, performance suite (READMEs only until Phase 0 reports)
tools/             scripts that build or fetch pinned external tools (Roslyn LS, netcoredbg, rust-analyzer, Chrome for Testing, CEF); lldb-dap's install notes
docs/              PLAN.md, adr/, briefs/
```

## Documents

- [docs/PLAN.md](docs/PLAN.md): the master plan, source of truth for scope and phases
- [docs/adr/](docs/adr/): architecture decision records, one per decision D1 to D7
- [docs/briefs/](docs/briefs/): units of delegable work, starting with the Phase 0 spikes
- [CLAUDE.md](CLAUDE.md): rules for every agent working in this repo
- [CONTRIBUTING.md](CONTRIBUTING.md): how to contribute

## License

- Product (shell, hosts, debuggers, web tooling): GPL-3.0-or-later, see [LICENSE](LICENSE).
- `protocol/` and `extension-sdk/`: MIT, so anyone can write an agent, extension or alternative host against Eludite under any license.
- Contributions are accepted under the Developer Certificate of Origin. There is no CLA.
