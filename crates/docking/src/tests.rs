//! Headless GPUI tests: GPUI's test platform (no GPU, no compositor) with real
//! GPUI mouse event dispatch, so drag and drop, hover and clicks go through
//! the same code paths as on a desktop. Each interaction must reach the layout
//! through the command bus; the audit log proves it.

use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::Duration;

use eludite_commands::{CommandRegistry, view};
use eludite_ui::Theme;
use gpui::{
    AnyElement, AppContext as _, Context, Entity, IntoElement, Modifiers, MouseButton,
    ParentElement, Render, Styled, TestAppContext, VisualTestContext, Window, div, point, px, size,
};
use serde_json::json;

use crate::controller::DockController;
use crate::model::{DockLayout, DockSide, Place, ToolWindowRegistry, ids};
use crate::persist::{LayoutStore, LayoutWriter, read_layout};
use crate::view::{DockHost, Persistence};

struct Root(Entity<DockHost>);

impl Render for Root {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().flex().flex_col().size_full().child(self.0.clone())
    }
}

struct Harness {
    host: Entity<DockHost>,
    vcx: VisualTestContext,
    commands: Arc<CommandRegistry>,
    controller: DockController,
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
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            let host = cx.new(|cx| {
                DockHost::new(
                    controller.clone(),
                    commands.clone(),
                    Theme::vs_dark(),
                    Rc::new(body),
                    Rc::new(|d, _| div().child(d.title.clone()).into_any_element()),
                    persistence,
                    cx,
                )
            });
            host_out = Some(host.clone());
            cx.new(|_| Root(host))
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
    DockLayout::default_vs(&ToolWindowRegistry::vs_default())
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
    assert_eq!(g.tabs, [ids::ERROR_LIST, ids::OUTPUT, ids::PROPERTIES]);
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
        [ids::ERROR_LIST, ids::OUTPUT]
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
    h.click("hide-solution_explorer");
    assert_eq!(
        h.place(ids::SOLUTION_EXPLORER),
        Some(Place::AutoHidden {
            side: DockSide::Right
        })
    );
    assert!(h.vcx.debug_bounds("flyout").is_none());
    // Hovering the strip tab slides it out.
    h.hover("strip-solution_explorer");
    assert_eq!(
        h.controller.snapshot().flyout.as_deref(),
        Some(ids::SOLUTION_EXPLORER)
    );
    assert!(h.vcx.debug_bounds("flyout").is_some());
    // Clicking in the document area slides it back in.
    h.click("documents");
    assert!(h.controller.snapshot().flyout.is_none());
    assert!(h.vcx.debug_bounds("flyout").is_none());
    // Clicking the strip tab slides it out, and clicking again keeps it out
    // (the hover that precedes a real click has usually opened it already).
    h.click("strip-solution_explorer");
    assert!(h.vcx.debug_bounds("flyout").is_some());
    h.click("strip-solution_explorer");
    assert!(h.vcx.debug_bounds("flyout").is_some());
    // Pin docks it.
    h.click("flyout-pin");
    assert_eq!(h.side(ids::SOLUTION_EXPLORER), Some(DockSide::Right));
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
    assert_eq!(h.layout(), default_layout());
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
    let (layout, _) = store.load(Some(&solution), &registry);
    let persistence = Persistence {
        path: path.clone(),
        writer: writer.clone(),
    };
    let mut h = open(cx, layout, Some(persistence.clone()));
    h.drag("tab-output", "guide-left");
    h.drag("head-properties", "group-error_list");
    h.drag("tab-git_changes", "documents");
    h.click("hide-solution_explorer");
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
