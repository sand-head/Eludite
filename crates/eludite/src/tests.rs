//! Headless GPUI tests of the shell: key bindings and menu items dispatch
//! `eludite.view.*` (and other) commands through the command bus; disabled menu
//! items dispatch nothing.

use std::sync::Arc;

use eludite_commands::{CommandRegistry, builtins, view};
use eludite_docking::{DockController, DockLayout, DockSide, Place, ToolWindowRegistry, ids};
use eludite_ui::{Theme, bind_keymap, slots, vs_keymap};
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, TestAppContext, VisualTestContext, px, size,
};

use crate::shell::Shell;
use crate::shell::session::HostLaunch;

struct Harness {
    shell: Entity<Shell>,
    vcx: VisualTestContext,
    commands: Arc<CommandRegistry>,
    controller: DockController,
}

fn open(cx: &mut TestAppContext) -> Harness {
    let tools = ToolWindowRegistry::vs_default();
    let controller = DockController::new(DockLayout::default_vs(&tools), tools);
    let mut commands = builtins::default_registry();
    view::register(&mut commands, Arc::new(controller.clone())).unwrap();
    let services = crate::shell::register_workspace(
        &mut commands,
        HostLaunch::Missing("no host in these tests".into()),
        crate::settings::SettingsSetup::isolated(None),
    );
    let mut services = Some(services);
    let commands = Arc::new(commands);
    let window = cx.update(|cx| {
        bind_keymap(cx, &vs_keymap());
        cx.open_window(Default::default(), |window, cx| {
            let shell = cx.new(|cx| {
                Shell::new(
                    commands.clone(),
                    controller.clone(),
                    Theme::vs_dark(),
                    None,
                    services.take().unwrap(),
                    window,
                    cx,
                )
            });
            shell.focus_handle(cx).focus(window, cx);
            shell
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    vcx.simulate_resize(size(px(1280.), px(800.)));
    vcx.run_until_parked();
    let shell = window.root(&mut vcx).unwrap();
    Harness {
        shell,
        vcx,
        commands,
        controller,
    }
}

impl Harness {
    fn audit(&self) -> Vec<String> {
        self.commands
            .audit_log()
            .entries()
            .into_iter()
            .map(|e| e.command)
            .collect()
    }

    fn keys(&mut self, keys: &str) {
        self.vcx.simulate_keystrokes(keys);
        self.vcx.run_until_parked();
    }

    fn click(&mut self, sel: &'static str) {
        let b = self
            .vcx
            .debug_bounds(sel)
            .unwrap_or_else(|| panic!("no element {sel}"));
        self.vcx.simulate_click(b.center(), Modifiers::none());
        self.vcx.run_until_parked();
    }

    fn place(&self, id: &str) -> Option<Place> {
        self.controller.layout().find(id)
    }

    fn active_in_group(&self, id: &str) -> bool {
        self.controller
            .layout()
            .group_of(id)
            .is_some_and(|g| g.active_id() == Some(id))
    }
}

#[gpui::test]
fn key_bindings_dispatch_view_commands(cx: &mut TestAppContext) {
    let mut h = open(cx);
    // Close Workspace, then Ctrl+Alt+L brings it back.
    h.commands
        .invoke(view::HIDE, serde_json::json!({"id": "workspace"}))
        .unwrap();
    h.vcx.run_until_parked();
    assert!(matches!(
        h.place(ids::WORKSPACE),
        Some(Place::Hidden { .. })
    ));
    let before = h.audit().len();
    h.keys("ctrl-alt-l");
    assert_eq!(h.audit()[before..], [view::SHOW]);
    assert!(matches!(
        h.place(ids::WORKSPACE),
        Some(Place::Docked {
            side: DockSide::Right,
            ..
        })
    ));
    assert!(h.active_in_group(ids::WORKSPACE));

    // Ctrl+Alt+O activates Output (a background tab of the bottom group).
    assert!(!h.active_in_group(ids::OUTPUT));
    h.keys("ctrl-alt-o");
    assert!(h.active_in_group(ids::OUTPUT));

    // Ctrl+\, Ctrl+E (a chord) activates the Error List.
    h.keys("ctrl-\\ ctrl-e");
    assert!(h.active_in_group(ids::ERROR_LIST));

    // Ctrl+Alt+X slides out the auto-hidden Toolbox.
    h.keys("ctrl-alt-x");
    assert_eq!(
        h.controller.snapshot().flyout.as_deref(),
        Some(ids::TOOLBOX)
    );

    assert_eq!(h.audit()[before..], [view::SHOW; 4]);
    let status = h.shell.read_with(&h.vcx, |s, _| {
        s.status().get(slots::STATE).map(str::to_owned)
    });
    assert_eq!(status.as_deref(), Some("Ready"));
}

#[gpui::test]
fn menu_items_dispatch_commands(cx: &mut TestAppContext) {
    let mut h = open(cx);
    // View > Output.
    h.click("menu-View");
    let open = h.shell.read_with(&h.vcx, |s, cx| {
        s.menu().read(cx).open_menu().map(str::to_owned)
    });
    assert_eq!(open.as_deref(), Some("View"));
    h.click("menu-item-View-Output");
    assert_eq!(h.audit(), [builtins::ABOUT, view::SHOW]);
    assert!(h.active_in_group(ids::OUTPUT));
    let open = h.shell.read_with(&h.vcx, |s, cx| {
        s.menu().read(cx).open_menu().map(str::to_owned)
    });
    assert!(open.is_none(), "choosing an item closes the menu");

    // Window > Float acts on the active tool window (Output, just shown).
    h.click("menu-Window");
    h.click("menu-item-Window-Float");
    assert!(matches!(h.place(ids::OUTPUT), Some(Place::Floating { .. })));
    // Window > Reset Window Layout.
    h.click("menu-Window");
    h.click("menu-item-Window-Reset Window Layout");
    let tools = ToolWindowRegistry::vs_default();
    assert_eq!(h.controller.layout(), DockLayout::default_vs(&tools));

    // Help > About Eludite shows the version in the status bar.
    h.click("menu-Help");
    h.click("menu-item-Help-About Eludite");
    let status = h.shell.read_with(&h.vcx, |s, _| {
        s.status().get(slots::STATE).map(str::to_owned)
    });
    assert_eq!(status, Some(format!("Eludite {}", builtins::VERSION)));
    assert_eq!(
        h.audit(),
        [
            builtins::ABOUT,
            view::SHOW,
            view::FLOAT,
            view::RESET_LAYOUT,
            builtins::ABOUT
        ]
    );
}

#[gpui::test]
fn disabled_menu_items_do_nothing(cx: &mut TestAppContext) {
    let mut h = open(cx);
    let enabled = h.shell.read_with(&h.vcx, |s, cx| {
        let m = s.menu().read(cx);
        (
            m.is_item_enabled("Analyze", "Code Cleanup"),
            m.is_item_enabled("View", "Output"),
            m.enabled_labels("Analyze"),
            m.enabled_labels("Window"),
            m.enabled_labels("Build"),
        )
    });
    assert_eq!(enabled.0, Some(false), "no analysis command exists yet");
    assert_eq!(enabled.1, Some(true));
    assert!(enabled.2.is_empty());
    assert_eq!(
        enabled.3,
        ["Float", "Dock", "Auto Hide", "Hide", "Reset Window Layout"]
    );
    // Brief 0017: the build commands exist; Cancel is disabled while no build runs.
    assert_eq!(
        enabled.4,
        [
            "Build Solution",
            "Rebuild Solution",
            "Clean Solution",
            "Build Project",
            "Rebuild Project",
            "Clean Project"
        ]
    );

    let before = h.audit();
    h.click("menu-Build");
    h.click("menu-item-Build-Cancel");
    h.click("menu-Analyze");
    h.click("menu-item-Analyze-Code Cleanup");
    h.click("menu-item-Analyze-Run Code Analysis");
    assert_eq!(h.audit(), before, "disabled items invoke nothing");
    let open = h.shell.read_with(&h.vcx, |s, cx| {
        s.menu().read(cx).open_menu().map(str::to_owned)
    });
    assert_eq!(
        open.as_deref(),
        Some("Analyze"),
        "and do not close the menu"
    );
    // Brief 0035: the Test menu's items exist (Run All Tests and Debug All Tests wait while a test run goes).
    let test = h
        .shell
        .read_with(&h.vcx, |s, cx| s.menu().read(cx).enabled_labels("Test"));
    assert_eq!(
        test,
        [
            "Run All Tests",
            "Debug All Tests",
            "Run Failed Tests",
            "Repeat Last Run",
            "Test Explorer"
        ]
    );
    // Clicking outside closes it.
    h.click("menu-backdrop");
    let open = h.shell.read_with(&h.vcx, |s, cx| {
        s.menu().read(cx).open_menu().map(str::to_owned)
    });
    assert!(open.is_none());
    assert_eq!(h.audit(), before);
}

#[gpui::test]
fn every_menu_opens(cx: &mut TestAppContext) {
    let mut h = open(cx);
    for title in eludite_ui::MENU_TITLES {
        let sel: &'static str = Box::leak(format!("menu-{title}").into_boxed_str());
        h.click(sel);
        let open = h.shell.read_with(&h.vcx, |s, cx| {
            s.menu().read(cx).open_menu().map(str::to_owned)
        });
        assert_eq!(open.as_deref(), Some(title));
        h.click(sel);
    }
}
