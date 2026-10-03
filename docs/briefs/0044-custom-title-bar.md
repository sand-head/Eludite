# Brief 0044: Eludite draws the main window's title bar

Status: done (2026-10-03, Linux; macOS and Windows by CI and the unit tests of their chrome)
Phase: 2
Plan reference: PLAN.md sections 2 (principles 1, 5), 8 (Chrome)
Related ADRs: ADR-0001, ADR-0010 (new)

## Goal

The main window's title bar is Eludite's, laid out as Visual Studio 2022's: the menu bar in the title bar, the window's
title in a box beside it, the build toolbar (configuration and platform), and the caption buttons, in one 32-pixel
row where there were two. macOS keeps its window buttons at the left; Windows' caption buttons behave natively (snap
layouts, the system menu); on Linux, where the compositor gives client-side decorations, Eludite draws the frame and
the buttons, and where it does not the platform's title bar stays over the old menu row.

## Files in scope

- `crates/ui/src/title_bar.rs` (new: `chrome`, `TitleBar`, `client_frame`, the caption glyphs), `crates/ui/src/lib.rs`.
- `crates/eludite/src/app.rs` (the window options), `crates/eludite/src/shell.rs` (the title, the row, the frame),
  `crates/eludite/src/shell/folder.rs` (the title), `crates/eludite/src/shell/tests.rs`.
- `docs/adr/0010-own-title-bar.md`, `docs/adr/README.md`, `docs/briefs/README.md`, this file.

## Contract

- The main window opens with `TitlebarOptions::appears_transparent`, macOS's buttons at `MAC_BUTTONS_POSITION`, and
  `WindowDecorations::Client`.
- `title_bar::chrome(platform, decorations, fullscreen)` decides, on every frame: macOS draws the row after 78 pixels
  for its buttons (none when full screen) and no caption buttons; Windows draws the caption buttons as
  `WindowControlArea::{Min, Max, Close}` and the drag area as `WindowControlArea::Drag`, which Windows hit-tests;
  Linux with client-side decorations draws and handles the buttons (Minimize, Maximize or Restore, Close, which quits
  as the platform's does), moves the window on a drag of the title area, maximizes on its double-click, opens the
  window menu on its right-click, and draws the frame: a 10-pixel margin with the shadow and the eight resize handles
  on each untiled side, a one-pixel border (accent while active); Linux with server-side decorations keeps the
  platform's title bar and the old menu row.
- The title bar shows the window's title (`Shell::set_title` sets both).
- Floating tool windows and dialogs keep the platform's title bar.

## Proving test

- `eludite-ui` `title_bar` tests: the chrome per platform and decoration (full screen, tiled), the resize edges skip
  tiled sides, the glyphs stay in their 10 pixels.
- `eludite` `shell::tests::the_title_bar_holds_the_menu_the_title_and_the_caption_buttons`: the test window
  (server-side decorations) keeps the menu row; with Linux client-side chrome the title bar is 32 pixels and holds the
  menu bar, then the title, then Minimize, Maximize and Close in order to the row's end, the frame is inset 10 pixels
  (none when tiled), the File menu opens from the title bar; Windows' chrome draws the buttons without a frame; macOS's
  leaves its 78 pixels and draws none.
- On Xvfb with picom (and the root properties a window manager sets, `_NET_SUPPORTING_WM_CHECK` and
  `_GTK_FRAME_EXTENTS` in `_NET_SUPPORTED`), the real window drew the row, the frame, Close's red hover and the File
  menu from the title bar. Without a compositor it fell back to the old row.

## Out of scope

- A title bar for floating tool windows and dialogs; the search box Visual Studio puts in its title bar; the app icon
  at the row's left.
- Runs on a macOS and a Windows desktop beyond CI.
