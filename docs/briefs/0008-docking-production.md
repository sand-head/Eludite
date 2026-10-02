# Brief 0008: Docking and tool windows in the production shell

Status: done on Linux ([report](0008-report.md)); Windows and macOS not run
Phase: 1
Plan reference: PLAN.md sections 2 (principle 1), 3 (D1), 4.2, 4.12, 8, 9
Related ADR: ADR-0001
Depends on: brief 0001 (report and `spikes/0001-gpui-shell`)

## Goal

Replace the static layout in `crates/niello` with real Visual Studio-style docking in `crates/docking` and `crates/ui`, carried over from the brief 0001 prototype and made production-grade: drag to dock with docking guides, tab with another window, float, auto-hide, pin, per-solution layout persistence, named layouts, and the menu bar and status bar as reusable components. After this brief the app binary is a working, empty IDE frame that any later brief can add tool windows to by registering them.

## Files in scope

- `crates/docking/**`
- `crates/ui/**`
- `crates/niello/**` (wiring only: register the default tool windows and document area; keep `main.rs` small)
- `crates/commands/src/**` only to add `niello.view.*` commands for show, hide, float, auto-hide and reset layout, with schemas added to `protocol/schemas/` first in their own commit
- `protocol/schemas/` for those command schemas only
- `docs/briefs/0008-report.md` (new)

Do not touch `crates/editor`, `vendor/**`, `spikes/**`, `docs/adr/**`, the host, or the root `Cargo.toml` beyond adding a dependency line if a crate truly needs one (prefer per-crate `Cargo.toml`).

## Contract

- Read `docs/briefs/0001-report.md` and the prototype in `spikes/0001-gpui-shell` first. Port the model and interactions; do not copy the throwaway rendering code wholesale. Keep the known limit documented there: GPUI drag-and-drop stays within a window, so a floated window re-docks through a Dock command or button, not by dragging across windows.
- Model (`crates/docking`): `DockLayout` with left, right, bottom and document areas; tool windows have id, title, dock side, tabbed group, auto-hide and float state; the document area has tabs with pinned and preview states. Serialize to JSON; a stable schema with a version field; migration path documented.
- Interactions: drag a tool window tab to a side guide or onto another group (docking guides drawn during drag), float to an OS window, auto-hide to a side strip that flies out on hover and click, pin back, close, and reset to the default layout. Keyboard: Ctrl+Alt+L, Ctrl+Alt+X, Ctrl+Alt+O, Ctrl+\ Ctrl+E map to Solution Explorer, Toolbox, Output and Error List as in VS; use the keymap mechanism from `crates/ui` and document the table.
- Persistence: layout per solution path under the user's config directory (document the path per OS), plus a default; load on open, save on change (debounced) and on exit.
- `crates/ui`: `Theme::vs_dark()` stays the default; add `vs_light()` and `vs_blue()` token sets; menu bar with real menus (File, Edit, View, Git, Project, Build, Debug, Test, Analyze, Tools, Extensions, Window, Help) whose items dispatch command-bus commands, with stubs for commands that do not exist yet shown disabled; status bar with slots.
- Every user-visible action is a command (PLAN.md 5.1): menus and keys dispatch through `crates/commands`, never call functions directly.
- UI thread rule: nothing in this brief may block the UI thread; persistence I/O runs off-thread.
- Commit messages: one plain, direct, active-voice line each; no body, no trailers, no co-author or sign-off lines.

## Proving test

- Headless GPUI tests (as the prototype did) for: dock to each side, tab and untab, float and re-dock, auto-hide and pin, layout save and load round trip, reset, keyboard bindings dispatch the right command, menu items dispatch commands and disabled items do not.
- `cargo test --workspace`, `cargo clippy --workspace --all-targets -- -D warnings`, `cargo fmt --all --check` green.
- Manual: run `niello`, exercise each interaction on Linux, and record two screenshots (default layout; a customized layout after restart). Use the nested-compositor method from `spikes/0005-acp-panel/tools/nested.sh` if the session is locked.

## Budget

- Cold start to interactive window under 300 ms release (PLAN.md section 9); report the number.
- Frame cost under 8 ms p99 while dragging a tool window with the docking guides visible; measure with a harness flag like brief 0001's and report.
- Layout save never on the UI thread.

## Exit criterion

1. The app binary opens with the default VS layout and every interaction in the Contract works on Linux, with the headless tests proving each.
2. Layouts persist across restart per solution path.
3. Menus and keys dispatch through the command bus; the schemas for the new commands are in `protocol/` and committed before the code.
4. Cold start and drag frame-cost numbers in the report, within budget or explained.
5. The report lists GPUI limitations hit and sizes the follow-up (for example, cross-window drag if GPUI gains it).

## Out of scope

- Any editor content: the document area shows placeholder tabs only.
- Real contents of tool windows (Solution Explorer is a titled empty panel).
- Themes beyond the three token sets; no theme loading from files (that is the Phase 2 theme system).
- Settings UI. Keymap presets other than VS.
- Windows and macOS runs: write platform-neutral code and report them as not run.
