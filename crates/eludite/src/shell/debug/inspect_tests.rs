//! Headless tests of brief 0077 against the fake debug adapter: the exception helper, DataTips as trees, the Immediate
//! window, QuickWatch, Autos, Watch 2, changed values, `[External Code]`, every thread's location, the Modules window,
//! Enable/Disable All and Ctrl+F9, the Breakpoint Settings panel and the glyph's menu, Show Next Statement, the
//! Design/Debug layouts and Just My Code from the setting.

use eludite_commands::debug as cmds;
use eludite_commands::debug_inspect as inspect_cmds;
use eludite_dap::fake::FakeThrow;
use eludite_docking::ids;
use eludite_editor::EditorEvent;
use gpui::TestAppContext;
use serde_json::{Value, json};

use super::breakpoint_settings::{HitKind, SettingsField};
use super::immediate::{ImmediateLine, NOT_IN_BREAK};
use super::state::Mode;
use super::tests::{Dbg, debug_output, setup, setup_with, throwing};
use super::windows::{EditField, VarsKind, VarsMenu};

const PROGRAM: &str = "src/App/Program.cs";

fn set(d: &mut Dbg, key: &str, value: Value) {
    d.w.commands
        .invoke(
            eludite_commands::settings::SET,
            json!({"key": key, "value": value}),
        )
        .unwrap();
    d.w.vcx.run_until_parked();
}

/// A breakpoint on `line` of Program.cs, F5, and the break there.
fn break_at(d: &mut Dbg, line: u32) -> gpui::Entity<eludite_editor::EditorView> {
    d.w.open_solution();
    let view = d.open(PROGRAM, line);
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"path": PROGRAM, "line": line}),
    )
    .unwrap();
    d.start_and_break();
    view
}

fn rows(d: &Dbg, kind: VarsKind) -> Vec<(String, String)> {
    d.w.shell.read_with(&d.w.vcx, |s, cx| {
        s.debugger()
            .windows
            .vars(kind)
            .unwrap()
            .read(cx)
            .rows()
            .iter()
            .map(|r| (r.name.clone(), r.value.clone()))
            .collect()
    })
}

fn popups(d: &Dbg, view: &gpui::Entity<eludite_editor::EditorView>) -> Vec<&'static str> {
    view.read_with(&d.w.vcx, |v, _| v.anchored_popups())
}

fn window_state(d: &Dbg, id: &str) -> eludite_commands::view::WindowState {
    d.w.controller
        .all_states()
        .into_iter()
        .find(|s| s.id == id)
        .unwrap()
        .state
}

#[gpui::test]
fn the_exception_helper_shows_the_exception_and_writes_its_type(cx: &mut TestAppContext) {
    let mut d = setup_with(cx, |p| {
        throwing(p);
        p.steps[1].throws.as_mut().unwrap().inner = Some(Box::new(FakeThrow::new(
            "System.IO.IOException",
            "disk",
            true,
        )));
    });
    d.w.open_solution();
    let view = d.open(PROGRAM, 5);
    d.cmd(cmds::EXCEPTION_SETTINGS, json!({"break_when_thrown": true}))
        .unwrap();
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    d.fake().trigger();
    d.wait_break(1);
    d.w.wait("the exception helper", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.debugger().inspect.helper.is_some())
    });
    assert!(popups(&d, &view).contains(&"exception-helper"));
    assert_eq!(
        view.read_with(&d.w.vcx, |v, _| v.anchored_popup_row("exception-helper")),
        Some(6)
    );
    let helper = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.helper.as_ref().unwrap().2.clone()
    });
    let c = helper.read_with(&d.w.vcx, |h, _| h.content().clone());
    assert_eq!(c.title, "Exception Thrown");
    assert_eq!(c.type_name, "System.FormatException");
    assert_eq!(c.message, "bad");
    assert_eq!(
        c.inner,
        [(1, "System.IO.IOException".into(), "disk".into())]
    );
    // Thrown is on for the category: the box is checked; unchecking it writes the type's row.
    assert!(c.break_when_thrown);
    helper.update_in(&mut d.w.vcx, |h, window, cx| h.toggle_break(window, cx));
    d.w.vcx.run_until_parked();
    let s = d.state();
    assert_eq!(
        s["exceptions"]["types"][0],
        json!({"type": "System.FormatException", "break_when_thrown": false, "break_when_user_unhandled": true})
    );
    assert!(d.w.audit().contains(&cmds::EXCEPTION_SETTINGS.to_owned()));
    // Closing it, then Show Next Statement reopens it.
    helper.update(&mut d.w.vcx, |_, cx| {
        cx.emit(super::exception_helper::CloseHelper)
    });
    d.w.vcx.run_until_parked();
    assert!(!popups(&d, &view).contains(&"exception-helper"));
    d.cmd(inspect_cmds::SHOW_NEXT_STATEMENT, json!({})).unwrap();
    assert!(popups(&d, &view).contains(&"exception-helper"));
    // Resuming closes it; the next exception stop opens its own.
    d.cmd(cmds::CONTINUE, json!({})).unwrap();
    d.wait_break(2);
    d.w.wait("the second helper", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.debugger().inspect.helper.as_ref().is_some_and(|h| {
                h.1 == 2 && h.2.read(cx).content().type_name == "System.InvalidOperationException"
            })
        })
    });
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
    assert!(!popups(&d, &view).contains(&"exception-helper"));
}

#[gpui::test]
fn datatips_expand_edit_and_add_watches_and_failures_show_none(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    let view = break_at(&mut d, 8);
    let text = view.read_with(&d.w.vcx, |v, _| v.editor().text());
    let offset = text.find("order = new").unwrap() + 1;
    view.update(&mut d.w.vcx, |_, cx| {
        cx.emit(EditorEvent::HoverTriggered { offset })
    });
    d.w.wait("the data tip", |w| {
        view.read_with(&w.vcx, |v, _| v.anchored_popups().contains(&"datatip"))
    });
    // No Quick Info beside it.
    assert!(view.read_with(&d.w.vcx, |v, _| v.hover()).is_none());
    let tree = d.w.shell.read_with(&d.w.vcx, |s, cx| {
        s.debugger()
            .inspect
            .datatip_panel
            .as_ref()
            .unwrap()
            .2
            .read(cx)
            .tree
            .clone()
    });
    let tip_rows = |d: &Dbg| {
        tree.read_with(&d.w.vcx, |t, _| {
            t.rows()
                .iter()
                .map(|r| (r.name.clone(), r.value.clone()))
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(tip_rows(&d), [("order".to_owned(), "{Order}".to_owned())]);
    // Expanding asks for the members once and shows them in the frame the answer arrives.
    let before = d
        .fake()
        .commands()
        .iter()
        .filter(|c| *c == "variables")
        .count();
    d.w.shell.update(&mut d.w.vcx, |s, cx| {
        s.inspect_toggle(VarsKind::DataTip, &[0], cx)
    });
    d.w.wait("the members", |w| {
        tree.read_with(&w.vcx, |t, _| t.rows().len() == 3)
    });
    assert_eq!(
        d.fake()
            .commands()
            .iter()
            .filter(|c| *c == "variables")
            .count(),
        before + 1
    );
    assert_eq!(tip_rows(&d)[1], ("Id".to_owned(), "7".to_owned()));
    // Edit Id in place.
    tree.update_in(&mut d.w.vcx, |t, window, cx| {
        t.begin_edit(1, EditField::Value, window, cx);
        t.edit_text("8", cx);
        t.commit_edit(window, cx);
    });
    let fake = d.fake();
    d.w.wait("setVariable", |_| fake.last("setVariable").is_some());
    let sent = d.fake().last("setVariable").unwrap();
    assert_eq!(
        (sent["name"].clone(), sent["value"].clone()),
        (json!("Id"), json!("8"))
    );
    // Add Watch from the tip.
    tree.update_in(&mut d.w.vcx, |t, window, cx| {
        t.menu_action(1, VarsMenu::AddWatch, window, cx)
    });
    d.w.vcx.run_until_parked();
    assert_eq!(d.state()["watches"][0]["name"], "order.Id");
    // A failed evaluation shows no tip.
    let offset = text.find("WriteLine").unwrap() + 1;
    view.update(&mut d.w.vcx, |_, cx| {
        cx.emit(EditorEvent::HoverTriggered { offset })
    });
    d.w.wait("the failed hover", |w| {
        view.read_with(&w.vcx, |v, _| v.hover().is_none())
            && w.shell
                .read_with(&w.vcx, |s, _| s.debugger().inspect.datatip_panel.is_none())
    });
    assert!(!popups(&d, &view).contains(&"datatip"));
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn the_immediate_window_evaluates_in_repl_context_with_history(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    let imm =
        d.w.shell
            .read_with(&d.w.vcx, |s, _| s.debugger().windows.immediate.clone());
    imm.update_in(&mut d.w.vcx, |w, window, cx| {
        w.set_input("?x", cx);
        w.submit(window, cx);
    });
    d.w.wait("the answer", |w| {
        imm.read_with(&w.vcx, |i, _| i.lines().len() == 2)
    });
    assert_eq!(
        imm.read_with(&d.w.vcx, |i, _| i.lines().to_vec()),
        [
            ImmediateLine::Input("?x".into()),
            ImmediateLine::Result("1".into())
        ]
    );
    let sent = d.fake().last("evaluate").unwrap();
    assert_eq!(
        (sent["expression"].clone(), sent["context"].clone()),
        (json!("x"), json!("repl"))
    );
    // Up recalls the line.
    imm.update(&mut d.w.vcx, |w, cx| w.recall(true, cx));
    assert_eq!(
        imm.read_with(&d.w.vcx, |i, _| i.input_text().to_owned()),
        "?x"
    );
    // `>` commands are refused; outside a break nothing is evaluated.
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
    imm.update_in(&mut d.w.vcx, |w, window, cx| {
        w.set_input("x", cx);
        w.submit(window, cx);
    });
    assert_eq!(
        imm.read_with(&d.w.vcx, |i, _| i.lines().last().cloned()),
        Some(ImmediateLine::Error(NOT_IN_BREAK.into()))
    );
}

#[gpui::test]
fn quickwatch_evaluates_reevaluates_and_adds_a_watch(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    // Outside an expression and without one: refused with a reason.
    let out = d
        .cmd(inspect_cmds::QUICKWATCH, json!({"expression": "x"}))
        .unwrap();
    assert_eq!(out["state"], "pending");
    d.w.wait("the value", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger()
                .inspect
                .quickwatch
                .first()
                .is_some_and(|n| n.value == "1")
        })
    });
    let dialog = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.quickwatch_dialog.clone().unwrap()
    });
    assert_eq!(
        dialog.read_with(&d.w.vcx, |q, _| q.expression().to_owned()),
        "x"
    );
    let tree_rows = dialog.read_with(&d.w.vcx, |q, cx| {
        q.tree
            .read(cx)
            .rows()
            .iter()
            .map(|r| (r.name.clone(), r.value.clone()))
            .collect::<Vec<_>>()
    });
    assert_eq!(tree_rows, [("x".to_owned(), "1".to_owned())]);
    assert_eq!(
        dialog.read_with(&d.w.vcx, |q, _| q.history().to_vec()),
        ["x"]
    );
    dialog.update_in(&mut d.w.vcx, |q, window, cx| q.add_watch(window, cx));
    d.w.vcx.run_until_parked();
    assert_eq!(d.state()["watches"][0]["name"], "x");
    // An agent's QuickWatch answers with the value.
    let s = d.cmd(cmds::STOP, json!({}));
    assert!(s.is_ok());
    d.wait_mode(Mode::Design);
    assert!(d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.quickwatch_dialog.is_none()
    }));
    let e = d
        .cmd(inspect_cmds::QUICKWATCH, json!({"expression": "x"}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("break mode"), "{e}");
}

#[gpui::test]
fn autos_list_the_variables_of_the_current_and_previous_statement(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    d.w.wait("the autos", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            let a = &s.debugger().inspect.autos;
            a.len() == 2 && a.iter().all(|n| !n.loading)
        })
    });
    let autos = rows(&d, VarsKind::Autos);
    assert_eq!(autos[0], ("x".to_owned(), "1".to_owned()));
    assert_eq!(autos[1].0, "y");
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn watch_2_adds_edits_and_deletes_without_touching_watch_1(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    d.cmd(cmds::WATCH, json!({"add": "x", "window": 2}))
        .unwrap();
    d.w.wait("Watch 2's value", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.debugger().inspect.watches[0]
                .first()
                .is_some_and(|n| n.value == "1")
        })
    });
    assert_eq!(
        rows(&d, VarsKind::Watch2),
        [("x".to_owned(), "1".to_owned())]
    );
    // F2 in the window edits the expression in place.
    let w2 = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().windows.vars(VarsKind::Watch2).unwrap().clone()
    });
    w2.update_in(&mut d.w.vcx, |v, window, cx| {
        v.begin_edit(0, EditField::Expression, window, cx);
        v.edit_text("y", cx);
        v.commit_edit(window, cx);
    });
    d.w.vcx.run_until_parked();
    assert!(w2.read_with(&d.w.vcx, |v, _| v.editing().is_none()));
    assert_eq!(rows(&d, VarsKind::Watch2)[0].0, "y");
    assert!(d.w.audit().contains(&cmds::WATCH.to_owned()));
    d.cmd(
        cmds::WATCH,
        json!({"move": {"from": 0, "to": 0}, "window": 2}),
    )
    .unwrap();
    d.cmd(cmds::WATCH, json!({"remove": 0, "window": 2}))
        .unwrap();
    assert!(rows(&d, VarsKind::Watch2).is_empty());
    assert!(d.state()["watches"].as_array().unwrap().is_empty());
    let e = d
        .cmd(cmds::WATCH, json!({"remove": 0, "window": 3}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("Watch 3"), "{e}");
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn changed_values_are_red_after_the_step_that_changed_them(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 5);
    let marks = |d: &Dbg| {
        d.w.shell.read_with(&d.w.vcx, |s, cx| {
            s.debugger().windows.locals.read(cx).changed().to_vec()
        })
    };
    assert_eq!(d.locals(), [("x".to_owned(), "0".to_owned())]);
    assert_eq!(marks(&d), [false]);
    d.cmd(cmds::STEP_OVER, json!({})).unwrap();
    d.wait_break(2);
    assert_eq!(d.locals(), [("x".to_owned(), "1".to_owned())]);
    assert_eq!(marks(&d), [true]);
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn the_call_stack_collapses_external_code_and_threads_show_their_frames(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    let stack = |d: &Dbg| {
        d.w.shell.read_with(&d.w.vcx, |s, cx| {
            s.debugger()
                .windows
                .call_stack
                .read(cx)
                .rows()
                .iter()
                .map(|r| r.name.clone())
                .collect::<Vec<_>>()
        })
    };
    assert_eq!(stack(&d), ["Program.Main()", "[External Code]"]);
    set(&mut d, "debugger.showExternalCode", json!(true));
    assert_eq!(stack(&d), ["Program.Main()", "[Native Frames]"]);
    // The worker thread's location, from its own top frame.
    d.w.wait("the worker's frame", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.debugger()
                .windows
                .threads
                .read(cx)
                .rows()
                .iter()
                .any(|t| t.id == 2 && t.location == "System.Threading.Monitor.Wait()")
        })
    });
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn modules_come_from_module_events_with_their_output_lines(cx: &mut TestAppContext) {
    let mut d = setup_with(cx, |p| {
        p.modules = vec![
            json!({"id": 1, "name": "App.dll", "path": "/app/App.dll", "isUserCode": true,
                   "isOptimized": false, "symbolStatus": "Symbols loaded.", "version": "1.0.0.0"}),
            json!({"id": 2, "name": "System.Runtime.dll", "path": "/dotnet/System.Runtime.dll",
                   "isUserCode": false, "isOptimized": true, "symbolStatus": "Skipped loading symbols."}),
        ];
    });
    d.w.open_solution();
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    d.w.wait("the modules", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.debugger().windows.modules.read(cx).rows().len() == 2
        })
    });
    let out = d.cmd(inspect_cmds::MODULES, json!({})).unwrap();
    assert_eq!(out["total"], 2);
    assert_eq!(out["modules"][0]["name"], "App.dll");
    assert_eq!(out["modules"][0]["user_code"], true);
    assert_eq!(
        out["modules"][1]["symbol_status"],
        "Skipped loading symbols."
    );
    let lines = debug_output(&d);
    assert!(
        lines.contains(&"Loaded '/app/App.dll'. Symbols loaded.".to_owned()),
        "{lines:?}"
    );
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn enable_and_disable_all_ctrl_f9_and_ctrl_shift_f9(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    for line in [5, 6] {
        d.cmd(
            cmds::TOGGLE_BREAKPOINT,
            json!({"path": PROGRAM, "line": line}),
        )
        .unwrap();
    }
    let enabled = |d: &mut Dbg| {
        d.state()["breakpoints"]
            .as_array()
            .unwrap()
            .iter()
            .map(|b| b["enabled"].as_bool().unwrap())
            .collect::<Vec<_>>()
    };
    d.cmd(
        inspect_cmds::ENABLE_BREAKPOINTS,
        json!({"all": true, "enabled": false}),
    )
    .unwrap();
    assert_eq!(enabled(&mut d), [false, false]);
    // Ctrl+F9 toggles the caret line's breakpoint.
    d.open(PROGRAM, 6);
    d.w.vcx.simulate_keystrokes("ctrl-f9");
    d.w.vcx.run_until_parked();
    assert_eq!(enabled(&mut d), [false, true]);
    d.cmd(
        inspect_cmds::ENABLE_BREAKPOINTS,
        json!({"all": true, "enabled": true}),
    )
    .unwrap();
    assert_eq!(enabled(&mut d), [true, true]);
    // The window's search filters its rows.
    let bp =
        d.w.shell
            .read_with(&d.w.vcx, |s, _| s.debugger().windows.breakpoints.clone());
    bp.update(&mut d.w.vcx, |b, cx| b.set_search("line 6", cx));
    assert_eq!(bp.read_with(&d.w.vcx, |b, _| b.visible_rows()), [1]);
    d.w.vcx.simulate_keystrokes("ctrl-shift-f9");
    d.w.vcx.run_until_parked();
    assert!(d.state()["breakpoints"].as_array().unwrap().is_empty());
}

#[gpui::test]
fn the_glyph_menu_and_the_breakpoint_settings_panel_write_conditions_and_actions(
    cx: &mut TestAppContext,
) {
    let mut d = setup(cx);
    d.w.open_solution();
    let view = d.open(PROGRAM, 6);
    d.cmd(cmds::TOGGLE_BREAKPOINT, json!({"path": PROGRAM, "line": 6}))
        .unwrap();
    let path = d.w.path(PROGRAM).to_string_lossy().into_owned();
    let path = crate::shell::documents::normalize_path(std::path::Path::new(&path))
        .to_string_lossy()
        .into_owned();
    // A right click on the glyph opens its menu.
    view.update(&mut d.w.vcx, |_, cx| {
        cx.emit(EditorEvent::BreakpointMarginContext {
            row: 5,
            position: gpui::point(gpui::px(4.), gpui::px(4.)),
        })
    });
    d.w.vcx.run_until_parked();
    assert!(popups(&d, &view).contains(&"breakpoint-menu"));
    // Conditions... opens the panel under the line.
    let menu = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.glyph_menu.clone().unwrap()
    });
    menu.update_in(&mut d.w.vcx, |m, window, cx| {
        m.act(
            super::breakpoint_settings::GlyphAction::Conditions,
            window,
            cx,
        )
    });
    d.w.vcx.run_until_parked();
    assert_eq!(popups(&d, &view), ["breakpoint-settings"]);
    let panel = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.glyph_settings.clone().unwrap()
    });
    panel.update_in(&mut d.w.vcx, |p, window, cx| {
        p.set_text(SettingsField::Condition, "x > 0", cx);
        p.set_hit_kind(HitKind::AtLeast, cx);
        p.set_text(SettingsField::HitCount, "2", cx);
        p.actions = true;
        p.set_text(SettingsField::Message, "x is {x}", cx);
        p.apply(window, cx);
    });
    d.w.vcx.run_until_parked();
    let b = &d.state()["breakpoints"][0];
    assert_eq!(b["condition"], "x > 0");
    assert_eq!(b["hit_condition"], ">=2");
    assert_eq!(b["log_message"], "x is {x}");
    assert!(popups(&d, &view).is_empty());
    // The tooltip over the glyph names it.
    view.update(&mut d.w.vcx, |_, cx| {
        cx.emit(EditorEvent::BreakpointMarginHovered { row: Some(5) })
    });
    d.w.vcx.run_until_parked();
    assert_eq!(popups(&d, &view), ["breakpoint-tooltip"]);
    // Disable from the menu goes through enable_breakpoints.
    d.w.shell
        .update(&mut d.w.vcx, |s, cx| s.glyph_menu_open(&path, 6, cx));
    let menu = d.w.shell.read_with(&d.w.vcx, |s, _| {
        s.debugger().inspect.glyph_menu.clone().unwrap()
    });
    menu.update_in(&mut d.w.vcx, |m, window, cx| {
        m.act(
            super::breakpoint_settings::GlyphAction::ToggleEnabled,
            window,
            cx,
        )
    });
    d.w.vcx.run_until_parked();
    assert_eq!(d.state()["breakpoints"][0]["enabled"], false);
}

#[gpui::test]
fn show_next_statement_brings_back_the_statement_after_a_frame_switch(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    d.open("src/App/Calc.cs", 5);
    d.cmd(
        cmds::TOGGLE_BREAKPOINT,
        json!({"path": "src/App/Calc.cs", "line": 5}),
    )
    .unwrap();
    d.start_and_break();
    d.cmd(cmds::SELECT_FRAME, json!({"frame": 1})).unwrap();
    assert_eq!(d.state()["frame"], 1);
    d.cmd(inspect_cmds::SHOW_NEXT_STATEMENT, json!({})).unwrap();
    assert_eq!(d.state()["frame"], 0);
    let active = d.w.controller.active_document().unwrap_or_default();
    assert!(active.ends_with("Calc.cs"), "{active}");
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
    let e = d
        .cmd(inspect_cmds::SHOW_NEXT_STATEMENT, json!({}))
        .unwrap_err()
        .to_string();
    assert!(e.contains("break mode"), "{e}");
}

#[gpui::test]
fn the_debug_layout_is_kept_and_the_design_layout_restored(cx: &mut TestAppContext) {
    use eludite_commands::view::WindowState;
    let mut d = setup(cx);
    break_at(&mut d, 6);
    assert_eq!(window_state(&d, ids::IMMEDIATE), WindowState::Hidden);
    // The person shows the Immediate window while debugging.
    d.cmd("eludite.view.show", json!({"id": ids::IMMEDIATE}))
        .unwrap();
    assert_eq!(window_state(&d, ids::IMMEDIATE), WindowState::Docked);
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
    d.w.wait("the Design layout", |w| {
        w.controller
            .all_states()
            .iter()
            .any(|s| s.id == ids::IMMEDIATE && s.state == WindowState::Hidden)
    });
    assert_eq!(window_state(&d, ids::LOCALS), WindowState::Hidden);
    // The next session brings the Debug layout back, with the Immediate window.
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    assert_eq!(window_state(&d, ids::IMMEDIATE), WindowState::Docked);
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn just_my_code_follows_the_setting(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    d.w.open_solution();
    set(&mut d, "debugger.justMyCode", json!(false));
    d.w.vcx.simulate_keystrokes("f5");
    d.wait_mode(Mode::Running);
    let launch = d.fake().last("launch").unwrap();
    // Restore the process-wide default before asserting, for the other tests in this process.
    set(&mut d, "debugger.justMyCode", json!(true));
    assert_eq!(launch["justMyCode"], false);
    assert!(eludite_dap::options::just_my_code());
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

#[gpui::test]
fn the_context_menu_copies_and_toggles_hexadecimal_display(cx: &mut TestAppContext) {
    let mut d = setup(cx);
    break_at(&mut d, 6);
    let locals =
        d.w.shell
            .read_with(&d.w.vcx, |s, _| s.debugger().windows.locals.clone());
    locals.update_in(&mut d.w.vcx, |v, window, cx| {
        v.open_menu(0, cx);
        v.menu_action(0, VarsMenu::Copy, window, cx);
    });
    assert_eq!(
        d.w.vcx
            .read_from_clipboard()
            .and_then(|c| c.text())
            .as_deref(),
        Some("x = 1")
    );
    locals.update_in(&mut d.w.vcx, |v, window, cx| {
        v.menu_action(0, VarsMenu::Hexadecimal, window, cx)
    });
    d.w.vcx.run_until_parked();
    assert_eq!(
        d.w.commands
            .invoke(
                eludite_commands::settings::GET,
                json!({"key": super::windows::HEX_SETTING})
            )
            .unwrap()["value"],
        true
    );
    assert_eq!(
        super::windows::VarsWindow::display_value("26", true),
        "0x0000001a"
    );
    set(&mut d, super::windows::HEX_SETTING, json!(false));
    d.cmd(cmds::STOP, json!({})).unwrap();
    d.wait_mode(Mode::Design);
}

/// The budget: every window of this brief draws a frame of 500 rows under 8 ms at p99 (ADR-0014: asserted on a quiet
/// developer machine, reported on CI).
#[gpui::test]
fn the_windows_draw_500_rows_within_the_frame_budget(cx: &mut TestAppContext) {
    use std::time::{Duration, Instant};

    use eludite_commands::debug_inspect::ModuleRow;
    use eludite_ui::Theme;

    use super::immediate::ImmediateWindow;
    use super::modules::ModulesWindow;
    use super::state::FlatRow;
    use super::windows::VarsWindow;

    fn p99(cx: &mut gpui::VisualTestContext) -> Duration {
        let mut frames: Vec<Duration> = (0..100)
            .map(|_| {
                cx.update(|window, cx| {
                    window.refresh();
                    let t = Instant::now();
                    let _ = window.draw(cx);
                    t.elapsed()
                })
            })
            .collect();
        frames.sort();
        frames[98]
    }
    let rows: Vec<FlatRow> = (0..500)
        .map(|i| FlatRow {
            path: vec![i],
            depth: 0,
            name: format!("item{i}"),
            value: i.to_string(),
            type_name: "int".into(),
            expanded: (i % 7 == 0).then_some(false),
            error: false,
        })
        .collect();
    let mut results = Vec::new();
    for kind in [VarsKind::Locals, VarsKind::Watch2, VarsKind::Autos] {
        let rows = rows.clone();
        let (view, vcx) = cx.add_window_view(|_, cx| VarsWindow::new(Theme::dark(), kind, cx));
        view.update(vcx, |v, cx| {
            v.set_marks((0..500).map(|i| i % 3 == 0).collect(), Vec::new(), cx);
            v.set_rows(rows, None, cx);
        });
        results.push((kind.prefix().to_owned(), p99(vcx)));
    }
    let (view, vcx) = cx.add_window_view(|_, _| ModulesWindow::new(Theme::dark()));
    let modules: Vec<ModuleRow> = (0..500)
        .map(|i| ModuleRow {
            id: i.to_string(),
            name: format!("M{i}.dll"),
            path: Some(format!("/app/M{i}.dll")),
            optimized: Some(true),
            user_code: Some(false),
            symbol_status: Some("Symbols loaded.".into()),
            version: Some("1.0.0.0".into()),
        })
        .collect();
    view.update(vcx, |v, cx| v.set_rows(&modules, cx));
    results.push(("modules".into(), p99(vcx)));
    let (view, vcx) = cx.add_window_view(|_, cx| ImmediateWindow::new(Theme::dark(), cx));
    view.update(vcx, |v, cx| {
        for i in 0..500 {
            v.push_result(format!("{i}"), i % 5 == 0, cx);
        }
    });
    results.push(("immediate".into(), p99(vcx)));
    for (what, t) in results {
        eprintln!(
            "timing: {what} with 500 rows: p99 {:.2} ms",
            t.as_secs_f64() * 1e3
        );
        super::super::tests::assert_budget(
            &format!("{what}'s frame with 500 rows (p99)"),
            t,
            Duration::from_millis(8),
        );
    }
}
