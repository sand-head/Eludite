//! Headless GPUI tests: GPUI's test platform (no GPU, no compositor) with real
//! GPUI mouse event dispatch, so drag and drop, hover and clicks go through
//! the same code paths as on a desktop. Each interaction must reach the layout
//! through the command bus; the audit log proves it.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use eludite_commands::{CommandRegistry, view};
use eludite_ui::vertical_text::{RotatedLabelCache, rotated_label};
use eludite_ui::{RunCommand, Theme};
use gpui::{
    AnyElement, AppContext as _, Context, Entity, InteractiveElement, IntoElement, Modifiers,
    MouseButton, ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div,
    point, px, size,
};
use serde_json::json;

use crate::controller::DockController;
use crate::model::{DockLayout, DockSide, Place, ToolWindowRegistry, ids};
use crate::persist::{LayoutStore, LayoutWriter, read_layout};
use crate::view::{DockHost, Persistence};

/// The window root: hosts the dock and records the `RunCommand` actions that bubble up to it.
struct Root(
    Entity<DockHost>,
    Rc<RefCell<Vec<RunCommand>>>,
    gpui::FocusHandle,
);

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .track_focus(&self.2)
            .on_action(
                cx.listener(|this, a: &RunCommand, _, _| this.1.borrow_mut().push(a.clone())),
            )
            .flex()
            .flex_col()
            .size_full()
            .child(self.0.clone())
    }
}

struct Harness {
    host: Entity<DockHost>,
    vcx: VisualTestContext,
    commands: Arc<CommandRegistry>,
    controller: DockController,
    actions: Rc<RefCell<Vec<RunCommand>>>,
}

fn body(id: &str, _: &Theme) -> AnyElement {
    div().child(format!("{id} body")).into_any_element()
}

fn open(cx: &mut TestAppContext, layout: DockLayout, persistence: Option<Persistence>) -> Harness {
    let registry = ToolWindowRegistry::vs_default();
    let controller = DockController::new(layout, registry);
    let mut commands = eludite_commands::builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let commands = Arc::new(commands);
    let mut host_out = None;
    let actions = Rc::new(RefCell::new(Vec::new()));
    let recorded = actions.clone();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let host = cx.new(|cx| {
                DockHost::new(
                    controller.clone(),
                    commands.clone(),
                    Theme::dark(),
                    Rc::new(body),
                    Rc::new(|d, _| div().child(d.title.clone()).into_any_element()),
                    persistence,
                    cx,
                )
            });
            host_out = Some(host.clone());
            let root = cx.new(|cx| Root(host, recorded, cx.focus_handle()));
            root.read(cx).2.clone().focus(window, cx);
            root
        })
        .unwrap()
    });
    let host = host_out.unwrap();
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(1400.), px(900.)));
    vcx.run_until_parked();
    vcx.update(|_, cx| DockHost::sync_floating_windows(&host, cx));
    vcx.run_until_parked();
    Harness {
        host,
        vcx,
        commands,
        controller,
        actions,
    }
}

impl Harness {
    fn layout(&mut self) -> DockLayout {
        self.host
            .read_with(&self.vcx, |h, _| h.snapshot().layout.clone())
    }

    fn place(&mut self, id: &str) -> Option<Place> {
        self.layout().find(id)
    }

    fn side(&mut self, id: &str) -> Option<DockSide> {
        match self.place(id)? {
            Place::Docked { side, .. } => Some(side),
            _ => None,
        }
    }

    fn bounds(&mut self, sel: &str) -> gpui::Bounds<gpui::Pixels> {
        let sel: &'static str = Box::leak(sel.to_owned().into_boxed_str());
        self.vcx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"))
    }

    /// Press on `from`, move past the drag threshold (guides appear), move
    /// onto `to`, release.
    fn drag(&mut self, from: &str, to: &str) {
        let start = self.bounds(from).center();
        self.vcx
            .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
        self.vcx.simulate_mouse_move(
            start + point(px(30.), px(-30.)),
            MouseButton::Left,
            Modifiers::none(),
        );
        self.vcx.run_until_parked();
        assert!(
            self.vcx.debug_bounds("guide-left").is_some(),
            "guides are drawn during a drag"
        );
        let dst = self.bounds(to).center();
        self.vcx
            .simulate_mouse_move(dst, MouseButton::Left, Modifiers::none());
        self.vcx.run_until_parked();
        self.vcx
            .simulate_mouse_up(dst, MouseButton::Left, Modifiers::none());
        self.vcx.run_until_parked();
        assert!(
            self.vcx.debug_bounds("guide-left").is_none(),
            "guides go away after the drop"
        );
    }

    fn click(&mut self, sel: &str) {
        let c = self.bounds(sel).center();
        self.vcx.simulate_click(c, Modifiers::none());
        self.vcx.run_until_parked();
    }

    fn hover(&mut self, sel: &str) {
        let c = self.bounds(sel).center();
        self.vcx.simulate_mouse_move(c, None, Modifiers::none());
        self.vcx.run_until_parked();
    }

    /// Commands invoked so far, in order.
    fn audit(&self) -> Vec<String> {
        self.commands
            .audit_log()
            .entries()
            .into_iter()
            .map(|e| e.command)
            .collect()
    }

    fn floating_count(&mut self) -> usize {
        self.host
            .read_with(&self.vcx, |h, _| h.floating_window_count())
    }
}

fn default_layout() -> DockLayout {
    DockLayout::fixture(&ToolWindowRegistry::vs_default())
}

#[gpui::test]
fn dock_to_each_side_by_dragging_onto_guides(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    // Output is a tab in the bottom group; drag it to each guide.
    h.drag("tab-output", "guide-left");
    assert_eq!(h.side(ids::OUTPUT), Some(DockSide::Left));
    h.drag("head-output", "guide-right");
    assert_eq!(h.side(ids::OUTPUT), Some(DockSide::Right));
    h.drag("head-output", "guide-bottom");
    assert_eq!(h.side(ids::OUTPUT), Some(DockSide::Bottom));
    assert_eq!(h.layout().bottom.groups.len(), 2);
    assert_eq!(h.audit(), [view::DOCK, view::DOCK, view::DOCK]);
}

#[gpui::test]
fn tab_and_untab_by_dragging(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    // Properties' title bar onto the Error List group: tabbed together.
    h.drag("head-properties", "group-error_list");
    let l = h.layout();
    let g = l.group_of(ids::PROPERTIES).unwrap();
    assert_eq!(
        g.tabs,
        [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL, ids::PROPERTIES]
    );
    assert_eq!(g.active_id(), Some(ids::PROPERTIES));
    assert_eq!(l.right.groups.len(), 1, "the emptied group is gone");
    // Clicking a tab activates it, through the bus.
    h.click("tab-output");
    assert_eq!(
        h.layout().group_of(ids::OUTPUT).unwrap().active_id(),
        Some(ids::OUTPUT)
    );
    // Untab: drag the Properties tab back out to the right guide.
    h.drag("tab-properties", "guide-right");
    assert_eq!(h.side(ids::PROPERTIES), Some(DockSide::Right));
    assert_eq!(
        h.layout().group_of(ids::OUTPUT).unwrap().tabs,
        [ids::ERROR_LIST, ids::OUTPUT, ids::TERMINAL]
    );
    assert_eq!(h.audit(), [view::DOCK, view::SHOW, view::DOCK]);
}

#[gpui::test]
fn float_and_redock(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    // Dropping on the document area (no guide, no group) floats.
    h.drag("tab-output", "documents");
    assert!(matches!(h.place(ids::OUTPUT), Some(Place::Floating { .. })));
    assert_eq!(h.floating_count(), 1);
    // The Float button floats too.
    h.click("float-properties");
    assert!(matches!(
        h.place(ids::PROPERTIES),
        Some(Place::Floating { .. })
    ));
    assert_eq!(h.floating_count(), 2);

    // Dock button in the floating OS window re-docks it home.
    let windows = h.host.read_with(&h.vcx, |h, _| h.floating_windows());
    let mut fvcx = VisualTestContext::from_window(windows[0].1, &h.vcx.cx);
    fvcx.run_until_parked();
    let dock = fvcx
        .debug_bounds("dock-output")
        .expect("Dock button in floating window");
    fvcx.simulate_click(dock.center(), Modifiers::none());
    h.vcx.run_until_parked();
    assert_eq!(h.side(ids::OUTPUT), Some(DockSide::Bottom));
    assert_eq!(h.floating_count(), 1);

    // Closing the other floating OS window closes (hides) its tool window.
    let windows = h.host.read_with(&h.vcx, |h, _| h.floating_windows());
    let mut fvcx = VisualTestContext::from_window(windows[0].1, &h.vcx.cx);
    assert!(fvcx.simulate_close());
    h.vcx.run_until_parked();
    assert_eq!(
        h.place(ids::PROPERTIES),
        Some(Place::Hidden {
            side: DockSide::Right
        })
    );
    assert_eq!(h.floating_count(), 0);
    assert_eq!(
        h.audit(),
        [view::FLOAT, view::FLOAT, view::DOCK, view::HIDE]
    );
}

#[gpui::test]
fn auto_hide_fly_out_and_pin(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    h.click("hide-workspace");
    assert_eq!(
        h.place(ids::WORKSPACE),
        Some(Place::AutoHidden {
            side: DockSide::Right
        })
    );
    assert!(h.vcx.debug_bounds("flyout").is_none());
    // Hovering the strip tab slides it out.
    h.hover("strip-workspace");
    assert_eq!(
        h.controller.snapshot().flyout.as_deref(),
        Some(ids::WORKSPACE)
    );
    assert!(h.vcx.debug_bounds("flyout").is_some());
    // Clicking in the document area slides it back in.
    h.click("documents");
    assert!(h.controller.snapshot().flyout.is_none());
    assert!(h.vcx.debug_bounds("flyout").is_none());
    // Clicking the strip tab slides it out, and clicking again keeps it out
    // (the hover that precedes a real click has usually opened it already).
    h.click("strip-workspace");
    assert!(h.vcx.debug_bounds("flyout").is_some());
    h.click("strip-workspace");
    assert!(h.vcx.debug_bounds("flyout").is_some());
    // Pin docks it.
    h.click("flyout-pin");
    assert_eq!(h.side(ids::WORKSPACE), Some(DockSide::Right));
    assert!(h.vcx.debug_bounds("flyout").is_none());
    // Toolbox starts auto-hidden on the left (VS default).
    h.click("strip-toolbox");
    assert_eq!(
        h.controller.snapshot().flyout.as_deref(),
        Some(ids::TOOLBOX)
    );
    let audit = h.audit();
    assert_eq!(audit.first().map(String::as_str), Some(view::AUTO_HIDE));
    assert!(audit.contains(&view::DOCK.to_owned()));
    assert!(audit.iter().all(|c| c.starts_with("eludite.view.")));
}

#[gpui::test]
fn side_strip_tabs_draw_titles_rotated_clockwise(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    h.click("hide-workspace");
    // Toolbox on the left (VS default) and Workspace on the right: each tab
    // is a tall, narrow box holding the rotated title.
    for sel in ["strip-toolbox", "strip-workspace"] {
        let b = h.bounds(sel);
        assert!(
            b.size.height > b.size.width * 2.,
            "{sel} is a vertical tab: {b:?}"
        );
    }
    let small = Theme::dark().typography.small;
    let built = h.vcx.update(|window, cx| {
        let font = window.text_style().font();
        let cache = cx.global::<RotatedLabelCache>();
        let built = cache.len();
        let label = cache
            .get("Toolbox", &font, small)
            .expect("the Toolbox tab built its rotated label");
        assert!(label.size.height > label.size.width, "{:?}", label.size);
        // Same inputs, same label: no second build.
        let again = rotated_label(&"Toolbox".into(), &font, small, window, cx);
        assert!(Arc::ptr_eq(&label, &again));
        assert_eq!(cx.global::<RotatedLabelCache>().len(), built);
        // Rasterized the way the sprite atlas does it, at the window's scale
        // factor: non-empty, taller than wide, and the ink runs top to bottom.
        let image = cx
            .svg_renderer()
            .render_single_frame(&label.svg, window.scale_factor())
            .expect("the label SVG rasterizes");
        let (w, h) = (
            image.size(0).width.0 as usize,
            image.size(0).height.0 as usize,
        );
        assert!(w > 0 && h > w, "image is {w}x{h}");
        let px = image.as_bytes(0).unwrap();
        let (mut x0, mut x1, mut y0, mut y1) = (w, 0, h, 0);
        for (i, p) in px.as_chunks::<4>().0.iter().enumerate() {
            if p[3] > 0 {
                let (x, y) = (i % w, i / w);
                (x0, x1, y0, y1) = (x0.min(x), x1.max(x), y0.min(y), y1.max(y));
            }
        }
        assert!(x1 >= x0, "the label has ink");
        assert!(
            y1 - y0 > 2 * (x1 - x0),
            "ink is taller than wide: x {x0}..{x1}, y {y0}..{y1}"
        );
        built
    });
    // Later frames reuse the cache.
    h.hover("strip-toolbox");
    h.click("documents");
    let after = h.vcx.update(|_, cx| cx.global::<RotatedLabelCache>().len());
    assert_eq!(after, built);
}

#[gpui::test]
fn close_then_show_and_reset(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    h.click("close-properties");
    assert_eq!(
        h.place(ids::PROPERTIES),
        Some(Place::Hidden {
            side: DockSide::Right
        })
    );
    assert!(h.vcx.debug_bounds("head-properties").is_none());
    h.commands
        .invoke(view::SHOW, json!({"id": "properties"}))
        .unwrap();
    h.vcx.run_until_parked();
    assert_eq!(h.side(ids::PROPERTIES), Some(DockSide::Right));
    assert!(h.vcx.debug_bounds("head-properties").is_some());

    h.drag("tab-output", "guide-left");
    h.drag("head-error_list", "documents");
    assert_eq!(h.floating_count(), 1);
    h.commands.invoke(view::RESET_LAYOUT, json!({})).unwrap();
    h.vcx.run_until_parked();
    assert_eq!(
        h.layout(),
        DockLayout::default_vs(&ToolWindowRegistry::vs_default())
    );
    assert!(
        h.vcx.debug_bounds("head-properties").is_none(),
        "the default has no Properties window"
    );
    assert_eq!(h.floating_count(), 0, "reset closes floating windows");
}

#[gpui::test]
fn commands_not_from_the_view_rerender_it(cx: &mut TestAppContext) {
    // An agent (MCP) invokes the bus directly, never through the view. GPUI's
    // test scheduler forbids wakeups from other threads, so this runs on the
    // test thread; `controller::tests` covers the cross-thread wakeup.
    let mut h = open(cx, default_layout(), None);
    h.commands
        .invoke(view::DOCK, json!({"id": "toolbox", "side": "bottom"}))
        .unwrap();
    h.vcx.run_until_parked();
    assert_eq!(h.side(ids::TOOLBOX), Some(DockSide::Bottom));
    assert!(h.vcx.debug_bounds("head-toolbox").is_some(), "drawn");
}

#[gpui::test]
fn layout_save_and_load_round_trip(cx: &mut TestAppContext) {
    let dir = tempfile::tempdir().unwrap();
    let store = LayoutStore::new(dir.path());
    let solution = PathBuf::from("/work/Shop/Shop.sln");
    let path = store.path_for(Some(&solution));
    let writer = LayoutWriter::spawn(Duration::from_millis(20), None);
    let registry = ToolWindowRegistry::vs_default();
    // Saved and restored below; it starts from the fixture so Properties can be moved.
    let layout = DockLayout::fixture(&registry);
    let persistence = Persistence {
        path: path.clone(),
        writer: writer.clone(),
    };
    let mut h = open(cx, layout, Some(persistence.clone()));
    h.drag("tab-output", "guide-left");
    h.drag("head-properties", "group-error_list");
    h.drag("tab-git_changes", "documents");
    h.click("hide-workspace");
    h.click("close-properties");
    let saved = h.layout();
    let flushed = h.host.read_with(&h.vcx, |h, _| h.flush()).unwrap();
    futures::executor::block_on(flushed).unwrap();
    assert_eq!(read_layout(&path).unwrap(), saved);
    assert!(
        !store.default_path().exists(),
        "a solution's layout does not overwrite the default"
    );

    // "Restart": load from disk into a new window.
    let (restored, source) = store.load(Some(&solution), &registry);
    assert_eq!(source, crate::persist::LayoutSource::Solution(path.clone()));
    assert_eq!(restored, saved);
    let mut h2 = open(cx, restored, Some(persistence));
    assert_eq!(h2.layout(), saved);
    assert_eq!(h2.floating_count(), 1, "floating window reopened");
    assert!(matches!(
        h2.place(ids::GIT_CHANGES),
        Some(Place::Floating { .. })
    ));
    assert_eq!(h2.side(ids::OUTPUT), Some(DockSide::Left));
}

#[gpui::test]
fn document_tabs_show_dirty_markers_and_close_through_the_bus(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    h.controller.open_document("/src/A.cs", "A.cs");
    h.controller.open_document("/src/B.cs", "B.cs");
    h.vcx.run_until_parked();
    assert!(h.vcx.debug_bounds("doc-tab-/src/A.cs").is_some());
    assert_eq!(h.controller.active_document().as_deref(), Some("/src/B.cs"));

    // The dirty marker is view state: drawn, not persisted.
    h.controller.set_document_dirty("/src/A.cs", true);
    h.vcx.run_until_parked();
    let snap = h.host.read_with(&h.vcx, |host, _| host.snapshot().clone());
    assert!(snap.dirty.contains("/src/A.cs"));
    assert!(!snap.layout.to_json().contains("dirty"));

    // Clicking a tab activates it through eludite.view.show.
    h.click("doc-tab-/src/A.cs");
    assert_eq!(h.audit(), [view::SHOW]);
    assert_eq!(h.controller.active_document().as_deref(), Some("/src/A.cs"));

    // The close button asks the shell to run eludite.file.close; it does not close the tab itself.
    h.click("doc-close-/src/A.cs");
    let actions = h.actions.borrow().clone();
    assert_eq!(actions.len(), 1);
    assert_eq!(actions[0].command.as_ref(), "eludite.file.close");
    assert_eq!(actions[0].args, json!({"path": "/src/A.cs"}));
    assert_eq!(
        h.audit(),
        [view::SHOW],
        "the close click did not activate the tab"
    );
    assert!(h.controller.layout().documents.get("/src/A.cs").is_some());

    assert!(h.controller.close_document("/src/A.cs"));
    assert!(!h.controller.close_document("/src/A.cs"));
    h.vcx.run_until_parked();
    assert!(h.vcx.debug_bounds("doc-tab-/src/A.cs").is_none());
    assert!(!h.host.read_with(&h.vcx, |host, _| {
        host.snapshot().dirty.contains("/src/A.cs")
    }));
}

#[gpui::test]
fn reset_keeps_documents_and_retain_drops_stale_tabs(cx: &mut TestAppContext) {
    let h = open(cx, default_layout(), None);
    h.controller.open_document("/src/A.cs", "A.cs");
    h.commands
        .invoke(view::DOCK, json!({"id": "output", "side": "left"}))
        .unwrap();
    h.commands.invoke(view::RESET_LAYOUT, json!({})).unwrap();
    let layout = h.controller.layout();
    assert!(layout.documents.get("/src/A.cs").is_some());
    assert_eq!(layout.documents.active.as_deref(), Some("/src/A.cs"));
    assert!(matches!(
        layout.find(ids::OUTPUT),
        Some(Place::Docked {
            side: DockSide::Bottom,
            ..
        })
    ));
    h.controller.retain_documents(|id| id == "welcome");
    let ids: Vec<String> = h
        .controller
        .layout()
        .documents
        .tabs
        .iter()
        .map(|t| t.id.clone())
        .collect();
    assert_eq!(ids, ["welcome"]);
}

#[gpui::test]
fn resize_a_dock_by_dragging_its_splitter(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    let before = h.layout().right.size;
    assert!(
        h.vcx.debug_bounds("splitter-left").is_none(),
        "no splitter for an empty dock"
    );
    // Press on the right dock's splitter and pull it 100 px to the left.
    let start = h.bounds("splitter-right").center();
    h.vcx
        .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    // The first move past GPUI's threshold starts the drag; later moves preview.
    h.vcx.simulate_mouse_move(
        start - point(px(10.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.simulate_mouse_move(
        start - point(px(100.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.run_until_parked();
    assert!(
        h.vcx.debug_bounds("guide-left").is_none(),
        "a splitter drag shows no docking guides"
    );
    assert_eq!(h.layout().right.size, before, "the drag only previews");
    let previewed = h.bounds("group-workspace").size.width;
    assert!(
        (f32::from(previewed) - (before + 100.)).abs() < 12.,
        "the preview widened the dock: {previewed:?}"
    );
    h.vcx.simulate_mouse_up(
        start - point(px(100.), px(0.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.run_until_parked();
    let after = h.layout().right.size;
    assert!((after - (before + 100.)).abs() < 1., "{before} -> {after}");
    assert_eq!(h.audit(), [view::RESIZE]);

    // The bottom dock, pulled up by 60 px.
    let before = h.layout().bottom.size;
    let start = h.bounds("splitter-bottom").center();
    h.vcx
        .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    h.vcx.simulate_mouse_move(
        start - point(px(0.), px(10.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.simulate_mouse_move(
        start - point(px(0.), px(60.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.simulate_mouse_up(
        start - point(px(0.), px(60.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.run_until_parked();
    let after = h.layout().bottom.size;
    assert!((after - (before + 60.)).abs() < 1., "{before} -> {after}");

    // A press without a move commits nothing.
    let start = h.bounds("splitter-bottom").center();
    h.vcx
        .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    h.vcx
        .simulate_mouse_up(start, MouseButton::Left, Modifiers::none());
    h.vcx.run_until_parked();
    assert_eq!(h.audit(), [view::RESIZE, view::RESIZE]);
    assert_eq!(h.layout().bottom.size, after);
}

#[gpui::test]
fn resize_groups_by_dragging_the_splitter_between_them(cx: &mut TestAppContext) {
    let mut h = open(cx, default_layout(), None);
    // The right dock stacks the Workspace group over Properties, equal by default.
    assert_eq!(h.layout().right.shares(), [0.5, 0.5]);
    let workspace_before = h.bounds("group-workspace").size.height;
    let start = h.bounds("splitter-right-0").center();
    h.vcx
        .simulate_mouse_down(start, MouseButton::Left, Modifiers::none());
    h.vcx.simulate_mouse_move(
        start + point(px(0.), px(10.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.simulate_mouse_move(
        start + point(px(0.), px(120.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.run_until_parked();
    let previewed = h.bounds("group-workspace").size.height;
    assert!(
        f32::from(previewed - workspace_before) > 100.,
        "the preview grew Workspace: {workspace_before:?} -> {previewed:?}"
    );
    h.vcx.simulate_mouse_up(
        start + point(px(0.), px(120.)),
        MouseButton::Left,
        Modifiers::none(),
    );
    h.vcx.run_until_parked();
    let shares = h.layout().right.shares();
    assert!(shares[0] > 0.6 && shares[0] < 0.9, "{shares:?}");
    assert!((shares.iter().sum::<f32>() - 1.).abs() < 1e-5);
    assert_eq!(h.audit(), [view::RESIZE]);
    // Workspace's group keeps the share after a redraw from the committed layout.
    let committed = h.bounds("group-workspace").size.height;
    assert!(
        (f32::from(committed - previewed)).abs() < 2.,
        "{previewed:?} vs {committed:?}"
    );
}
