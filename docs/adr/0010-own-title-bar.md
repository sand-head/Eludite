# ADR-0010: Eludite draws the main window's title bar

Status: Accepted, 2026-10-03; amended 2026-10-06 (the mark at the row's left, the toolbar in its own row)
Plan reference: PLAN.md sections 2 (principles 1, 5), 8 (Chrome); ADR-0001

## Context

The main window used the platform's title bar: a strip with the window's title and the caption buttons, and under it
Eludite's menu bar with the build toolbar at its right. Visual Studio 2022 draws its own: the menu bar sits in the
title bar beside the solution's name, with the caption buttons at the right, so the window spends one row on chrome
where ours spent two. PLAN.md 8 asks that Eludite be recognizable to a Visual Studio user in the first five seconds.

GPUI at the pinned rev supports this on every platform: `TitlebarOptions::appears_transparent` puts the content under
the title bar on macOS (the window buttons stay, placed by `traffic_light_position`) and on Windows (the app draws
the caption buttons and marks them, and the drag area, with `WindowControlArea`, so Windows hit-tests them natively:
snap layouts, double-click to maximize, the system menu). On Linux, `WindowDecorations::Client` asks the compositor
for client-side decorations; the app then draws the frame and calls `start_window_move`, `start_window_resize`,
`show_window_menu`, `zoom_window` and `minimize_window`. A compositor may refuse (X11 without a compositor, some
Wayland compositors), which `window_decorations()` reports.

## Decision

- Draw the main window's title bar in `crates/ui::title_bar`: Eludite's mark (the 3D cube of `crates/ui::crystal`,
  which spins while building or while a debuggee runs), the menu bar, the window's title in a box on the part of the
  row that drags the window, and the caption buttons, 32 pixels high.
- Draw the toolbar in its own row under the title bar (amended 2026-10-06; it was at the title bar's right): Visual
  Studio's Standard and Debug toolbars, with the configuration, platform, target framework and launch profile lists
  and Start in one control. It had outgrown the space beside the title.
- Open the main window with `appears_transparent` and `WindowDecorations::Client`.
- Decide what to draw on every frame from the platform and the decorations the window got (`title_bar::chrome`):
  macOS leaves room for its buttons; Windows draws the caption buttons as `WindowControlArea`s and lets the platform
  press them; Linux with client-side decorations draws and handles them and draws the frame (a 10-pixel margin for
  the shadow and the resize edges on each untiled side, and a one-pixel border, accent while active, as Visual
  Studio's); Linux with server-side decorations keeps the platform's title bar and the old menu row.
- Draw the caption glyphs as paths, not from an icon font, so they need no font the machine may lack.
- Keep the platform's title bar on floating tool windows and dialogs.

## Alternatives considered

- Keep the platform's title bar: two rows of chrome and a window that does not look like Visual Studio's.
- Linux only through server-side decorations: GNOME's Wayland session offers no server-side decorations, so the
  window would have no title bar there at all unless GPUI's fallback is used anyway.
- Zed's `platform_title_bar` and `workspace` code: Zed UI crates are excluded by invariant 7.
- Segoe Fluent Icons or MDL2 glyphs for the buttons: present on Windows only, and only on some versions.

## Consequences

Positive:
- One row of chrome; the menu bar and the title where a Visual Studio user expects them.
- Windows keeps its native caption behavior (snap layouts, Aero Snap, the system menu).

Negative:
- On Linux, Eludite owns the frame: resize edges, the shadow, tiling, and the fallback when the compositor declines.
- Screenshots taken before this change show the platform's title bar.
- The caption buttons are window-manager actions, not commands: an agent cannot minimize the IDE (none needs to).

## Revisit when

- GPUI gains a native title bar with app-provided content, or drops one of the APIs above.
- A platform's caption behavior (snap layouts, accessibility of the caption buttons) is found missing in use.
