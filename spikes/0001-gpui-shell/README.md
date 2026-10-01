# Spike 0001: GPUI shell and docking prototype

Throwaway code for [brief 0001](../../docs/briefs/0001-gpui-shell-and-docking-spike.md). The findings are in [docs/briefs/0001-report.md](../../docs/briefs/0001-report.md). Nothing here is production code; production docking lands in `crates/docking` through a later brief.

This directory is its own Cargo workspace (empty `[workspace]` table), so the production workspace does not build it. Run every command from this directory. It uses the same GPUI pin as the root `Cargo.toml` and the toolchain from the root `rust-toolchain.toml` (1.98.1). Linux needs the system packages listed in the root README.

```
cargo run --release                      # the prototype
cargo test                               # headless GPUI tests (layout round-trip, docking by mouse, typing)
cargo run --release -- --bench-scroll    # frame times, JSON on stdout
cargo run --release -- --bench-keys 500  # keystroke to present, JSON on stdout
cargo run --release -- --bench-start     # startup timings, JSON on stdout
python3 tools/bench_all.py --label NAME --refresh-hz HZ   # the full measurement set, written to results/NAME.json
python3 tools/inject_keys.py --count 300 # Linux only: real keystrokes through /dev/uinput (refuses to run while the session is locked)
```

From the repository root, `cargo run --manifest-path spikes/0001-gpui-shell/Cargo.toml --release` works too. `cargo run -p spike-gpui-shell` from the root does not, because the spike is deliberately not a member of the root workspace.

## What the prototype does

- One window: document tabs (a generated 100,000-line C#-like file, and a Welcome page), Solution Explorer and Git Changes tabbed over Properties on the right, Error List and Output tabbed at the bottom, Toolbox auto-hidden on the left.
- Drag a tool window's title bar or tab. Guides appear on the left, right and bottom of the document area: drop on one to dock there. Drop on another tool window group to tab into it. Drop anywhere else to float it in its own OS window.
- Group header buttons: Float, Auto Hide. Auto-hidden windows sit on the edge strip; click to slide out, Pin to dock again. A floating window has Dock; closing it also docks it back.
- The layout is saved as JSON on every change to `$XDG_CONFIG_HOME/niello-spike/layout.json` (or `%APPDATA%\niello-spike\layout.json`, or `--layout PATH`) and restored at startup, including floating windows. `--reset-layout` starts from the default.
- The text view types, deletes, splits and joins lines, and moves the cursor with the arrow keys, Home, End, Page Up and Page Down.

Benchmark modes use the default layout and never write the layout file.

Tests run with `cargo test` on GPUI's test platform (no GPU or display needed). `SPIKE_FONT` overrides the monospace font (default Noto Sans Mono on Linux, Consolas on Windows, Menlo on macOS). `ZED_DEVICE_ID=0x7480` (any PCI device id) makes GPUI pick a specific GPU.

## Licenses of the dependencies added

- `gpui`, `gpui_platform`: Apache-2.0 (git, pinned rev, same as the root workspace)
- `serde`, `serde_json`, `log`: MIT OR Apache-2.0
