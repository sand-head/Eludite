//! Headless GPUI tests (GPUI's test platform: no GPU, no compositor).

use gpui::{AppContext as _, TestAppContext, VisualTestContext};

use crate::buffer::Buffer;
use crate::layout::{Layout, Place, Side};
use crate::shell::Shell;
use crate::text_view::TextView;

fn open_shell(
    cx: &mut TestAppContext,
    layout: Layout,
    path: Option<std::path::PathBuf>,
) -> (gpui::WindowHandle<Shell>, VisualTestContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let tv = cx.new(|cx| TextView::new(Buffer::generate(1_000), cx));
            cx.new(|cx| Shell::new(layout, path, tv, cx))
        })
        .unwrap()
    });
    let vcx = VisualTestContext::from_window(window.into(), cx);
    (window, vcx)
}

/// Drive the shell through dock, tab, float and auto-hide, let it save the
/// layout to JSON, then restore that file into a fresh window and check the
/// layout (and the floating OS windows) come back identical.
#[gpui::test]
fn layout_save_restore_roundtrip(cx: &mut TestAppContext) {
    let dir = std::env::temp_dir().join(format!("eludite-spike-test-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("layout.json");
    let _ = std::fs::remove_file(&path);

    let (window, mut vcx) = open_shell(cx, Layout::default_vs(), Some(path.clone()));
    let shell = window.root(&mut vcx).unwrap();
    shell.update(&mut vcx, |s, cx| {
        s.mutate(cx, |l| l.dock_to("output", Side::Left));
        let gid = s.layout.right.groups[1].id;
        s.mutate(cx, |l| {
            l.tab_into("error_list", gid);
        });
        s.mutate(cx, |l| {
            l.float("properties", [50., 60., 300., 200.]);
        });
        s.mutate(cx, |l| {
            l.auto_hide("solution_explorer");
        });
    });
    vcx.run_until_parked();
    let saved = shell.read_with(&vcx, |s, _| s.layout.clone());
    assert!(saved.is_consistent());
    assert!(matches!(
        saved.find("output"),
        Some(Place::Docked {
            side: Side::Left,
            ..
        })
    ));
    assert!(matches!(
        saved.find("properties"),
        Some(Place::Floating { .. })
    ));
    assert!(matches!(
        saved.find("solution_explorer"),
        Some(Place::AutoHidden { side: Side::Right })
    ));
    assert_eq!(shell.read_with(&vcx, |s, _| s.floating_window_count()), 1);

    // Restore into a new window from the file the shell wrote.
    let restored = Layout::load(&path).expect("layout file written");
    assert_eq!(restored, saved);
    let (window2, mut vcx2) = open_shell(cx, restored, Some(path.clone()));
    let shell2 = window2.root(&mut vcx2).unwrap();
    vcx2.update(|_, cx| Shell::sync_floating_windows(&shell2, cx));
    vcx2.run_until_parked();
    assert_eq!(shell2.read_with(&vcx2, |s, _| s.layout.clone()), saved);
    assert_eq!(shell2.read_with(&vcx2, |s, _| s.floating_window_count()), 1);

    // Docking the floating group back closes its window and persists.
    let fid = saved.floating[0].group.id;
    shell2.update(&mut vcx2, |s, cx| s.mutate(cx, |l| l.dock_floating(fid)));
    vcx2.run_until_parked();
    assert_eq!(shell2.read_with(&vcx2, |s, _| s.floating_window_count()), 0);
    let reloaded = Layout::load(&path).unwrap();
    assert!(reloaded.floating.is_empty());
    assert!(matches!(
        reloaded.find("properties"),
        Some(Place::Docked {
            side: Side::Right,
            ..
        })
    ));

    let _ = std::fs::remove_dir_all(&dir);
}

fn center(b: gpui::Bounds<gpui::Pixels>) -> gpui::Point<gpui::Pixels> {
    b.center()
}

/// Drag `from` (a debug selector) and drop it on `to`, through real GPUI mouse events.
fn drag(vcx: &mut VisualTestContext, from: &'static str, to: &'static str) {
    use gpui::{Modifiers, MouseButton, point, px};
    let src = vcx
        .debug_bounds(from)
        .unwrap_or_else(|| panic!("no element {from}"));
    let start = center(src);
    vcx.simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    // Exceed the drag threshold so GPUI starts a drag and the guides render.
    vcx.simulate_mouse_move(
        start + point(px(30.), px(30.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    vcx.run_until_parked();
    let dst = vcx
        .debug_bounds(to)
        .unwrap_or_else(|| panic!("no drop target {to}"));
    vcx.simulate_mouse_move(center(dst), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
    vcx.simulate_mouse_up(center(dst), MouseButton::Left, Modifiers::none());
    vcx.run_until_parked();
}

fn click(vcx: &mut VisualTestContext, sel: &'static str) {
    let b = vcx
        .debug_bounds(sel)
        .unwrap_or_else(|| panic!("no element {sel}"));
    vcx.simulate_click(center(b), gpui::Modifiers::none());
    vcx.run_until_parked();
}

/// The docking interactions through simulated mouse input: drag to a dock
/// guide, drag onto a group to tab, drag onto the document area to float,
/// auto-hide, slide out and pin.
#[gpui::test]
fn docking_by_mouse(cx: &mut TestAppContext) {
    let (window, mut vcx) = open_shell(cx, Layout::default_vs(), None);
    vcx.simulate_resize(gpui::size(gpui::px(1400.), gpui::px(900.)));
    vcx.run_until_parked();
    let shell = window.root(&mut vcx).unwrap();
    let layout = |vcx: &mut VisualTestContext| shell.read_with(vcx, |s, _| s.layout.clone());

    // Drag the Output tab to the left dock guide.
    drag(&mut vcx, "tab-output", "guide-Dock Left");
    assert!(matches!(
        layout(&mut vcx).find("output"),
        Some(Place::Docked {
            side: Side::Left,
            ..
        })
    ));

    // Drag Properties' title bar onto the Error List group: tabbed together.
    drag(&mut vcx, "head-properties", "group-error_list");
    let l = layout(&mut vcx);
    let Some(Place::Docked {
        side: Side::Bottom,
        group,
    }) = l.find("properties")
    else {
        panic!("properties not tabbed into bottom group: {l:?}")
    };
    assert_eq!(l.group(group).unwrap().tabs, ["error_list", "properties"]);

    // Drag Output onto the document area (no guide, no group): floats.
    drag(&mut vcx, "head-output", "documents");
    assert!(matches!(
        layout(&mut vcx).find("output"),
        Some(Place::Floating { .. })
    ));
    assert_eq!(shell.read_with(&vcx, |s, _| s.floating_window_count()), 1);

    // Auto-hide Solution Explorer, slide it out from the edge strip, pin it back.
    click(&mut vcx, "hide-solution_explorer");
    assert!(matches!(
        layout(&mut vcx).find("solution_explorer"),
        Some(Place::AutoHidden { side: Side::Right })
    ));
    click(&mut vcx, "strip-solution_explorer");
    assert_eq!(
        shell.read_with(&vcx, |s, _| s.flyout.clone()).as_deref(),
        Some("solution_explorer")
    );
    click(&mut vcx, "flyout-pin");
    assert!(matches!(
        layout(&mut vcx).find("solution_explorer"),
        Some(Place::Docked {
            side: Side::Right,
            ..
        })
    ));
    assert!(layout(&mut vcx).is_consistent());
}

/// Typing into the 100k-line buffer through the key dispatch path.
#[gpui::test]
fn typing_edits_buffer(cx: &mut TestAppContext) {
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let tv = cx.new(|cx| TextView::new(Buffer::generate(100_000), cx));
            gpui::Focusable::focus_handle(tv.read(cx), cx).focus(window, cx);
            tv
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let tv = window.root(&mut vcx).unwrap();
    tv.update(&mut vcx, |tv, _| tv.place_cursor(50_000));
    vcx.simulate_keystrokes("h i enter backspace");
    let line = tv.read_with(&vcx, |tv, _| tv.buffer.lines[50_000].text.clone());
    assert!(line.starts_with("hi"), "{line}");
    assert_eq!(tv.read_with(&vcx, |tv, _| tv.buffer.len()), 100_000);
}
