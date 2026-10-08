# ADR-0001: Shell is Rust on GPUI with our own component layer

Status: Accepted, 2026-10-01
Plan reference: PLAN.md section 3 (D1), section 8

## Context

The shell owns the window, rendering, editor core, panels and the command bus. It must hit the budgets in PLAN.md section 9 (under 8 ms keystroke to pixel at p99, under 300 ms cold start, a 50k-line file scrolling at monitor refresh rate) on Windows, Linux and macOS.

The reference failure mode is Visual Studio: a garbage-collection pause or a synchronous Roslyn call on the UI thread. The shell must make that class of bug structurally hard.

Two further constraints:
- The product is built by one person directing agents, so the framework must already exist and be proven at editor scale. Writing a renderer is out of budget.
- The product must look like Visual Studio, not like Zed (PLAN.md section 8). Docking, tool windows, menus and status bar are all things Zed does not have in the same form.

Zed's GPL-3.0 crates became legally usable once the product was licensed GPL (ADR-0005). Version 0.1 of the plan had ruled them out.

## Decision

- Write the shell in Rust on GPUI: Metal on macOS, Vulkan over Wayland/X11 on Linux, DirectX on Windows.
- Pin GPUI as a git dependency on zed-industries/zed at an exact revision. Bump it deliberately.
- Take from Zed, after a per-crate license audit: GPUI and the low-level text infrastructure (rope and sum-tree, buffer and anchor model, fuzzy matcher, tree-sitter glue). Take them by git at the GPUI pin (see the note of 2026-10-08); copy a crate into the repo, with a `WHY.md` listing local changes, only when we must carry a change to it.
- Do not take any crate that decides what the user sees: `editor`, `workspace`, `ui`, `theme`, `project`, `terminal_view`, the agent panel. Write those fresh in `crates/` against the Visual Studio model (docking, tool windows, menus, status bar).
- Which crates pass the audit is a Phase 0 output (brief 0001), not decided here.

## Alternatives considered

- All-C# with Avalonia: one language and in-process Roslyn, but GC pauses and synchronous Roslyn calls on the UI thread are the failure mode we are avoiding. Kept as the documented fallback with NativeAOT. ADR-0002 means switching shells does not touch hosts, protocols or the debugger.
- C++ with Qt or Skia: Qt's licensing fits a GPL product better than a permissive one, but velocity and memory safety favor Rust for agent-written code.
- Other Rust frameworks (iced, Slint, egui, Makepad): none has shipped an editor at Zed's scale.
- Electron, Tauri or a WebView: excluded by the non-goals in PLAN.md section 1.
- Take Zed's UI crates wholesale: fastest start, but the result would look like Zed and inherit its fixed-dock interaction model.

## Consequences

Positive:
- GPU rendering and a proven text stack from day one.
- Rust gives the compiler as a first reviewer of agent output.
- The VS-style chrome is original code, so the product cannot look like Zed by construction.
- Out-of-process hosts keep the shell small and the choice reversible.

Negative:
- GPUI's Windows backend is the youngest. Windows is a first-class target, so this is the top risk (PLAN.md section 13, risk 1).
- GPUI is tracked by git revision, not a stable release, so upgrades can break us.
- Zed's crates move under us with every GPUI bump. We accept that cost and review what each bump brings.
- We write docking, tool windows and menus ourselves, which is substantial work.

## Revisit when

- The Phase 0 GPUI spike (brief 0001) fails its exit criteria on Windows. Then evaluate Avalonia with NativeAOT.
- The vendoring audit shows no low-level text crate is license-compatible or maintainable.
- GPUI is published as a stable release, at which point replace the git revision with a version.
- Keystroke-to-pixel p99 cannot be brought under 8 ms with the editor core on GPUI.

Note, 2026-10-01 (brief 0001, [report](../briefs/0001-report.md) section 3.4): GPUI draws only when the platform asks for a frame, so measured keystroke-to-present includes a wait of up to one refresh interval (16.7 ms at 60 Hz). Read the keystroke trigger above as input-to-frame-submitted *excluding* that wait (measured at 3.4 to 4.9 ms p99 on Linux), or restate the budget against a named reference refresh rate. Until that is decided, a keystroke result over 8 ms does not by itself trigger this ADR.

Note, 2026-10-01 (Phase 0 close on Linux): brief 0001 is GO on Linux Wayland and XWayland (provisional pending an unlocked-session re-run; [report](../briefs/0001-report.md) section 8). The keystroke budget was split into frame cost and end to end (PLAN.md section 9) and every Linux number passes. The vendoring audit approved `sum_tree`, `rope`, `text`, `clock` and `fuzzy`; brief 0009 vendors them. Windows and macOS remain undetermined until the runs happen; the first Windows run is planned on the owner's work laptop.

Note, 2026-10-08 (GPUI bump): the pin moves from `20d29fc6bc2fc2b58d1fff8d8e0503b9ba7f41d8` (2026-10-01) to `b47a4ca595d3a3fba116b9e78ef01a46e2e43f3e` (Zed's main on 2026-10-08), at the owner's request. It brings a fix for UTF-8 boundary crashes in `rope` (zed#65273), a Linux fallback-font fix in `gpui_wgpu` (zed#65269), element ids hashed once (zed#64209), display tracking per window (zed#65059), a headless windowing API (zed#64969) and Zed's smaller dependency footprint (zed#65176: `rope` and `text` no longer depend on `util`; about 800 lines leave `Cargo.lock`). No Eludite code changed.

Note, 2026-10-08 (git dependencies, owner's decision): `sum_tree`, `rope`, `text`, `clock` and `fuzzy` are no longer copied into `vendor/`; they are git dependencies at the GPUI pin, like GPUI's own support crates. The copies carried no local changes, so they bought nothing but a re-sync on every bump. With them go `vendor/sync.sh`, the separate vendor workspace and the `[patch]` that pointed GPUI's `sum_tree` at our copy: there is still one `sum_tree` in the build, GPUI's. We grow outward when we need to: the first change we must carry to a Zed crate brings that one crate back into the repo, with its `WHY.md`.
