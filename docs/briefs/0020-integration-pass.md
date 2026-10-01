# Brief 0020: Integration pass after build and debug

Status: open
Phase: 1
Plan reference: PLAN.md sections 4.4, 4.5, 4.12, 8, 9
Depends on: briefs 0017 (build), 0018 (debug), 0019 (Rust); runs after 0019 merges

## Goal

Close the seams the build and debug reports listed so the daily loop feels like Visual Studio: F5 builds the startup project first and only launches on success; the debuggee's output goes to a "Debug" source in the Output window instead of a separate console; a real settings store replaces the environment-variable toggles (build on save, configured agents, netcoredbg and rust-analyzer paths, keymap preset) with a VS-style Options dialog for the few settings that exist; the host gains `eludite/build/status` so a reconnecting shell recovers a running build; the startup project is settable from the Workspace window's context menu (the first context menu: Set as Startup Project, Build, Rebuild, Clean, Open Containing Folder); and the Rust debugging path is sized, not built.

## Files in scope

- `protocol/schemas/` first and alone: `eludite/build/status`, command schemas for `eludite.workspace.set_startup_project`, `eludite.settings.get`, `eludite.settings.set`, the settings file schema
- `protocol/rust/**`, `crates/lsp/**`, `dotnet/src/Eludite.Host/**` and tests for the status query
- `crates/eludite/**`: F5 build-then-launch with the build's result gating the launch and its errors shown, the Debug output source and the Debug Console retired, the settings store (layered: user then solution, JSON, live-reloaded, documented paths per OS), the Options dialog, the Workspace context menu, the startup project persisted per solution
- `crates/commands/src/**`, `crates/ui/**`, `crates/docking/**` as needed
- `docs/briefs/0020-report.md` (new)

## Contract

- F5 with a dirty or stale build runs `eludite.build.project` for the startup project (or the active system's equivalent), streams to Output, and launches only on success; on failure the Error List comes forward and the status bar says why; Ctrl+F5 does the same without the debugger.
- The Output window gains a Debug source fed by the adapter's output events; the standalone Debug Console tool window is removed from the registry with a layout migration step.
- Settings: `settings.json` under the documented config directory and `.eludite/settings.json` beside a solution, merged user then solution; the shell watches both and applies changes without restart; every setting has a schema entry with a default and a description, and the Options dialog is generated from that schema (text, bool, enum, path pickers) grouped by section; `eludite.settings.*` commands let agents read and set them under the edit permission class.
- Build on save, the agents registry entries, adapter and server paths, and the keymap preset move to settings; the environment variables remain as overrides and are documented.
- `eludite/build/status` returns the running build (if any) with its output so far, so the shell can replay it after a host restart.
- Workspace context menu with the five items above; Set as Startup Project is persisted and shown in bold as VS does.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers.

## Proving test

- Headless tests: F5 builds then launches, F5 on a failing build stops with the Error List forward, Debug source receives adapter output, settings load and merge and live-reload, the Options dialog edits a setting through the bus, the context menu sets the startup project and it persists, build status replay after a simulated host restart.
- Manual, recorded with two screenshots: the Options dialog; F5 on `dotnet/Eludite.slnx` with an introduced error showing the build failing before launch, then fixed and launching. Revert afterwards.

## Budget

- F5 to launch adds no more than the build time plus 50 ms.
- Settings reload applies within 200 ms of the file change.

## Exit criterion

1. The manual flow works with screenshots.
2. All tests green; workspace fmt, clippy, tests green; `dotnet build` zero warnings and `dotnet test` green.
3. The report sizes the Phase 1 close-out: the remaining PLAN.md Phase 1 items (Test Explorer via MTP, git basics beyond status, terminal), the Windows pass, and Rust debugging via CodeLLDB.

## Out of scope

- Test Explorer, git UI, integrated terminal (next briefs).
- Themes and the theme system (Phase 2).
- Windows and macOS runs.
