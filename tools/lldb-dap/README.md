# lldb-dap: the debugger for Rust (Cargo packages)

Eludite debugs a Cargo package's executable with `lldb-dap`, LLVM's LLDB debug adapter (Apache-2.0 WITH
LLVM-exception), located on the machine and never bundled (brief 0029; `protocol/schemas/dap-lldb.md` is what Eludite
sends it). There is no fetch script: every platform's package manager or toolchain carries it. CodeLLDB's adapter
(MIT) is accepted instead when lldb-dap is not found.

## Install

| Platform | Command | Executable |
|---|---|---|
| Debian, Ubuntu | `sudo apt install lldb-18` (or another `lldb-NN` from 18 on; apt.llvm.org has newer ones) | `/usr/bin/lldb-dap-18`, `/usr/lib/llvm-18/bin/lldb-dap` |
| Fedora | `sudo dnf install lldb` | `/usr/bin/lldb-dap` |
| Arch | `sudo pacman -S lldb` | `/usr/bin/lldb-dap` |
| macOS | Xcode, or its Command Line Tools: `xcode-select --install` | `xcrun --find lldb-dap` |
| Windows | LLVM's installer from https://github.com/llvm/llvm-project/releases (`LLVM-<version>-win64.exe`) | `C:\Program Files\LLVM\bin\lldb-dap.exe` (on `PATH` when the installer adds it) |

LLVM 18 renamed the adapter from `lldb-vscode` to `lldb-dap`. Eludite looks for `lldb-dap` names only (the search
covers `lldb-dap-15` to `-17` as the brief asks, though those releases install `lldb-vscode-NN`); with LLVM 15 to 17,
set `debugger.lldbDapPath` to their `lldb-vscode` (not tried). The Rust formatters need LLDB's Python scripting, which
the packages above include (`lldb-18` depends on `python3-lldb-18` on Ubuntu).

CodeLLDB, the alternative: download the VSIX for your platform from https://github.com/vadimcn/codelldb/releases,
unzip it, and point `ELUDITE_CODELLDB` at the unzipped `extension` folder (or put it beside the eludite executable as
`codelldb/`); its adapter is `adapter/codelldb`. It was not run for brief 0029 (this machine cannot download from
GitHub).

## How Eludite finds it

In this order (`crates/dap/src/discovery.rs`, `LldbSearch`):

1. the setting `debugger.lldbDapPath` (Tools > Options > Debugging > General), or `ELUDITE_LLDB_DAP`: the executable or
   its folder;
2. `lldb-dap` on `PATH`, then `lldb-dap-22` down to `lldb-dap-15`;
3. `/usr/lib/llvm-22/bin/lldb-dap` down to `/usr/lib/llvm-15/bin/lldb-dap`;
4. on macOS, `xcrun --find lldb-dap`;
5. CodeLLDB: `ELUDITE_CODELLDB`, then `codelldb/adapter/codelldb` beside the eludite executable.

When none is found, F5 on a Cargo package says where it looked and which package to install. `tools/run-dev.sh`
passes `ELUDITE_LLDB_DAP` through when it is set.

## The Rust formatters

With `debugger.rustFormatters` on (the default), Eludite loads the toolchain's own LLDB formatters,
`<sysroot>/lib/rustlib/etc/lldb_lookup.py` (`rustc --print sysroot` in the Cargo workspace, so `rust-toolchain.toml`
picks the toolchain), as `rust-lldb` does. Every rustup toolchain ships them. For the standard library's sources in
the Call Stack, install the `rust-src` component (`rustup component add rust-src`); without it those frames are
external code.

## Check

```
lldb-dap-18 --help | head -3                         # or lldb-dap, per the table
cargo test -p eludite-dap --test lldb -- --nocapture  # the real adapter against a small Cargo program
```
