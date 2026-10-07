//! Headless GPUI tests of brief 0052's CodeLens against the in-process fake `eludite-host` and the scripted fake
//! generic server: lenses appear above members with their reference counts, the References popup lists the locations
//! grouped by file and navigates (Enter, Escape, the keyboard menu), the count matches Find All References, Run Test
//! from a lens discovers first ("Discovering…") then runs through the Test Explorer and the lens shows the outcome's
//! glyph and duration, Debug Test starts the session, `workspace/codeLens/refresh` asks again, an edit drops a stale
//! answer, the settings hide references or tests or all with per-language overrides, a rust-analyzer-style "Run Test"
//! lens from the fake generic server runs the Cargo test, a server without lenses contributes nothing, and typing
//! stays under the keystroke budget with 200 lens rows on screen.

use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};

use eludite_commands::settings::SET;
use eludite_commands::test as cmds;
use eludite_commands::workspace;
use eludite_editor::EditorView;
use eludite_editor::intellisense::CODE_LENS_DEBOUNCE;
use eludite_lsp::fake::{FakeHost, FakeReply};
use eludite_lsp::fake_server::{FakeServer, analyzer_capabilities};
use eludite_ui::TestGlyph;
use gpui::{Entity, Modifiers, TestAppContext, px, size};
use serde_json::{Value, json};

use super::codelens::{DISCOVERING, PopupRow, PopupState};
use super::debug::state::Mode;
use super::test_runs::{Phase, RunState};
use super::test_runs_tests::{self as tr, Tw};
use super::tests::{Ws, setup_services, setup_with};

/// The capabilities a server that serves lenses advertises.
fn caps() -> Value {
    json!({"codeLensProvider": {"resolveProvider": true}, "referencesProvider": true,
           "definitionProvider": true, "hoverProvider": true})
}

fn range(line: u32, from: u32, to: u32) -> Value {
    json!({"start": {"line": line, "character": from}, "end": {"line": line, "character": to}})
}

fn pos(line: u32, character: u32) -> Value {
    json!({"line": line, "character": character})
}

/// The lenses an editor shows: (row, title, resolved, glyph).
fn lenses(w: &Ws, view: &Entity<EditorView>) -> Vec<(u32, String, bool, Option<TestGlyph>)> {
    view.read_with(&w.vcx, |v, _| {
        v.code_lenses()
            .into_iter()
            .map(|l| (l.row, l.title, l.resolved, l.glyph))
            .collect()
    })
}

fn titles(w: &Ws, view: &Entity<EditorView>) -> Vec<(u32, String)> {
    view.read_with(&w.vcx, |v, _| {
        v.code_lenses()
            .into_iter()
            .map(|l| (l.row, l.title))
            .collect()
    })
}

/// The id of the lens on `row` whose title starts with `title` (a test lens adds its last duration).
fn lens_id(w: &Ws, view: &Entity<EditorView>, row: u32, title: &str) -> u64 {
    view.read_with(&w.vcx, |v, _| {
        v.code_lenses()
            .into_iter()
            .find(|l| l.row == row && l.title.starts_with(title))
            .unwrap_or_else(|| panic!("no lens {title} on row {row}: {:?}", v.code_lenses()))
            .id
    })
}

fn click_lens(w: &mut Ws, view: &Entity<EditorView>, id: u64) {
    w.vcx.run_until_parked();
    let bounds = view
        .read_with(&w.vcx, |v, _| v.code_lens_bounds(id))
        .expect("the lens is painted");
    w.vcx.simulate_click(bounds.center(), Modifiers::none());
    w.vcx.run_until_parked();
}

/// Every title each lens showed, recorded as the editor changes.
fn record_titles(w: &mut Ws, view: &Entity<EditorView>) -> Rc<RefCell<Vec<String>>> {
    let seen: Rc<RefCell<Vec<String>>> = Rc::default();
    let sink = seen.clone();
    w.vcx.update(|_, cx| {
        cx.observe(view, move |v, cx| {
            for l in v.read(cx).code_lenses() {
                let mut s = sink.borrow_mut();
                if !s.contains(&l.title) {
                    s.push(l.title);
                }
            }
        })
        .detach()
    });
    seen
}

/// Open a file as File > Open does (the test thread is the UI thread).
fn open_file(w: &mut Ws, path: &std::path::Path) -> Entity<EditorView> {
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::FILE_OPEN,
                json!({"path": path.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    w.editor(path)
}

fn set(w: &mut Ws, key: &str, value: Value) {
    w.commands
        .invoke(SET, json!({"key": key, "value": value}))
        .unwrap();
}

/// Program.cs's lenses as the host answers them: an unresolved references lens on `class Program` (line 0) and on
/// `Main` (line 2), resolved to "2 references" and "1 reference"; references found in Order.cs and Program.cs.
fn script_program(fake: &FakeHost) {
    fake.set_language_server("running", Some(caps()));
    fake.respond("textDocument/codeLens", |p| {
        let uri = p["textDocument"]["uri"].clone();
        FakeReply::Result(json!([
            {"range": range(0, 6, 13), "data": {"i": 0, "uri": uri}},
            {"range": range(2, 16, 20), "data": {"i": 1, "uri": uri}}
        ]))
    });
    fake.respond("codeLens/resolve", |p| {
        let uri = p["data"]["uri"].as_str().unwrap().to_owned();
        let path = super::documents::uri_to_path(&uri).unwrap();
        let (title, at) = if p["data"]["i"] == 0 {
            ("2 references", pos(0, 6))
        } else {
            ("1 reference", pos(2, 16))
        };
        FakeReply::Result(json!({"range": p["range"], "data": p["data"], "command": {
            "title": title, "command": "eludite.editor.find_references",
            "arguments": [{"uri": uri, "path": path, "position": at}]}}))
    });
    fake.respond("textDocument/references", |p| {
        let uri = p["textDocument"]["uri"].as_str().unwrap();
        let order = uri.replace("Program.cs", "Models/Order.cs");
        let mut found = vec![
            json!({"uri": order, "range": range(0, 6, 11)}),
            json!({"uri": uri, "range": range(2, 16, 20)}),
        ];
        if p["context"]["includeDeclaration"] == true {
            found.push(json!({"uri": uri, "range": range(0, 6, 13)}));
        }
        FakeReply::Result(Value::Array(found))
    });
}

#[gpui::test]
fn lenses_appear_above_members_and_the_references_popup_lists_and_navigates(
    cx: &mut TestAppContext,
) {
    let mut w = setup_with(cx, script_program);
    let (program, view) = w.open_program();
    w.wait("the resolved lenses", |w| {
        lenses(w, &view).iter().filter(|l| l.2).count() == 2
    });
    assert_eq!(
        titles(&w, &view),
        [
            (0, "2 references".to_owned()),
            (2, "1 reference".to_owned())
        ]
    );
    // The request went to the host after the document was opened, under the current generation; one resolve each.
    let received: Vec<String> = w.fake.received().into_iter().map(|r| r.method).collect();
    let open = received
        .iter()
        .position(|m| m == "textDocument/didOpen")
        .unwrap();
    let lens = received
        .iter()
        .position(|m| m == "textDocument/codeLens")
        .unwrap();
    assert!(open < lens, "{received:?}");
    assert_eq!(
        w.fake.received_params("textDocument/codeLens")[0]["eluditeGeneration"],
        w.fake.generation()
    );
    assert_eq!(w.fake.received_params("codeLens/resolve").len(), 2);
    // The rows take layout height: rows 0 and 2 have a lens row above them; the caret stays in the text.
    let (rows, top0, lens_h) = view.read_with(&w.vcx, |v, _| {
        (
            v.vertical_layout().lens_rows().to_vec(),
            v.row_top(0),
            v.lens_height(),
        )
    });
    assert_eq!(rows, [0, 2]);
    assert_eq!(top0, lens_h);
    assert_eq!(w.caret(&view), 0);

    // A click on "2 references" opens the popup: the references grouped by file, each with its line.
    let class_lens = lens_id(&w, &view, 0, "2 references");
    let clicked = Instant::now();
    click_lens(&mut w, &view, class_lens);
    // The popup is on screen in the next frame (its rows follow the server's answer).
    w.vcx.update(|window, cx| {
        window.refresh();
        let _ = window.draw(cx);
    });
    let opened = clicked.elapsed();
    w.bounds("code-lens-popup-header");
    let popup = w
        .shell
        .read_with(&w.vcx, |s, _| s.code_lens().popup.clone())
        .expect("the popup is open");
    w.wait("the popup's rows", |w| {
        popup.read_with(&w.vcx, |p, _| *p.state() == PopupState::Done)
    });
    let (header, rows, refs) = popup.read_with(&w.vcx, |p, _| {
        (p.header(), p.rows().to_vec(), p.references().to_vec())
    });
    assert_eq!(header, "'Program': 2 references");
    assert_eq!(refs.len(), 2, "the count the lens shows");
    assert_eq!(
        rows,
        [
            PopupRow::File {
                title: "Order.cs".into(),
                count: 1
            },
            PopupRow::Reference(0),
            PopupRow::File {
                title: "Program.cs".into(),
                count: 1
            },
            PopupRow::Reference(1),
        ]
    );
    assert_eq!(refs[0].text, "class Order { }");
    assert_eq!((refs[1].line, refs[1].column), (3, 17));
    assert_eq!(
        w.fake.received_params("textDocument/references")[0]["context"]["includeDeclaration"],
        false
    );
    w.bounds("code-lens-popup");
    w.bounds(&super::codelens::popup_row_selector(3));
    // It opened within 50 ms of the click.
    let (created, filled) = w.shell.read_with(&w.vcx, |s, _| {
        let t = s.code_lens().popup_timings.last().unwrap().clone();
        (
            t.opened.unwrap() - t.activated.unwrap(),
            t.filled.unwrap() - t.activated.unwrap(),
        )
    });
    eprintln!(
        "timing: lens click to the References popup drawn {:.2} ms (created {:.2} ms; rows in {:.2} ms, the fake host's answer included)",
        opened.as_secs_f64() * 1e3,
        created.as_secs_f64() * 1e3,
        filled.as_secs_f64() * 1e3
    );
    super::tests::assert_budget("the References popup", opened, Duration::from_millis(50));
    // Down selects the next reference, Enter navigates there (the history remembers where the caret was).
    assert_eq!(
        popup.read_with(&w.vcx, |p, _| p.selected_reference()),
        Some(0)
    );
    w.vcx.simulate_keystrokes("down");
    assert_eq!(
        popup.read_with(&w.vcx, |p, _| p.selected_reference()),
        Some(1)
    );
    w.vcx.simulate_keystrokes("up enter");
    let order = w.path("src/App/Models/Order.cs");
    let order_view = w.editor(&order);
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.code_lens().popup.is_none())
    );
    assert_eq!(w.caret(&order_view), 6);
    assert!(w.audit().contains(&workspace::FILE_OPEN.to_owned()));
    w.vcx.simulate_keystrokes("ctrl--");
    let program_id = super::documents::normalize_path(&program)
        .to_string_lossy()
        .into_owned();
    w.wait("back in Program.cs", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.controller.active_document().as_deref() == Some(program_id.as_str())
        })
    });

    // Ctrl+K, Ctrl+Q opens the first lens of the member at the caret; Escape closes the popup.
    let view = w.editor(&program);
    view.update_in(&mut w.vcx, |v, window, cx| {
        v.update_editor(cx, |e| e.set_caret(40));
        window.focus(&gpui::Focusable::focus_handle(v, cx), cx);
    });
    w.vcx.run_until_parked();
    w.vcx.simulate_keystrokes("ctrl-k ctrl-q");
    w.wait("the keyboard's popup", |w| {
        w.shell.read_with(&w.vcx, |s, cx| {
            s.code_lens()
                .popup
                .as_ref()
                .is_some_and(|p| p.read(cx).header().starts_with("'Main'"))
        })
    });
    w.vcx.simulate_keystrokes("escape");
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.code_lens().popup.is_none())
    );

    // The count matches Find All References for the same symbol, which also lists the declaration.
    let c = w.commands.clone();
    let path = program.to_string_lossy().into_owned();
    let h = std::thread::spawn(move || {
        c.invoke(
            workspace::EDITOR_FIND_REFERENCES,
            json!({"path": path, "line": 1, "column": 7}),
        )
    });
    w.wait("Find All References", |_| h.is_finished());
    let out = h.join().unwrap().unwrap();
    assert_eq!(out["total"], 3, "{out}");
}

#[gpui::test]
fn a_server_without_lenses_contributes_nothing(cx: &mut TestAppContext) {
    // The fake host advertises no codeLensProvider: no lens traffic, no rows, everything else as before.
    let mut w = setup_with(cx, |fake| {
        fake.respond("textDocument/codeLens", |_| {
            FakeReply::Result(json!([{"range": range(0, 6, 13)}]))
        });
    });
    let (_, view) = w.open_program();
    w.vcx.run_until_parked();
    view.update(&mut w.vcx, |v, cx| v.refresh_code_lenses(cx));
    w.vcx.run_until_parked();
    assert!(view.read_with(&w.vcx, |v, _| v.code_lens_enabled()));
    assert!(lenses(&w, &view).is_empty());
    assert!(w.fake.received_params("textDocument/codeLens").is_empty());
}

/// CalculatorTests.cs's lenses as the host answers them after its mapping: Run Test and Debug Test above `Adds` (line
/// 5) and `Subtracts` (line 11), Run All Tests and Debug All Tests above the class (line 2).
fn script_tests(fake: &FakeHost) {
    fake.set_language_server("running", Some(caps()));
    fake.respond("textDocument/codeLens", |p| {
        let uri = p["textDocument"]["uri"].as_str().unwrap().to_owned();
        let path = super::documents::uri_to_path(&uri).unwrap();
        let lens = |line: u32, from: u32, to: u32, member: &str, title: &str, debug: bool| {
            json!({"range": range(line, from, to), "command": {
                "title": title,
                "command": if debug { "eludite.test.debug" } else { "eludite.test.run" },
                "arguments": [{"uri": uri, "path": path, "range": range(line, from, to), "member": member}]}})
        };
        FakeReply::Result(json!([
            lens(5, 20, 24, "Adds", "Run Test", false),
            lens(5, 20, 24, "Adds", "Debug Test", true),
            lens(11, 20, 29, "Subtracts", "Run Test", false),
            lens(11, 20, 29, "Subtracts", "Debug Test", true),
            lens(2, 17, 32, "CalculatorTests", "Run All Tests", false),
            lens(2, 17, 32, "CalculatorTests", "Debug All Tests", true),
            {"range": range(4, 0, 1), "command": {"title": "Something else", "command": "custom.unknown"}}
        ]))
    });
}

fn open_calculator(t: &mut Tw) -> Entity<EditorView> {
    let source = t.w.path("tests/Corpus.Tests/CalculatorTests.cs");
    t.cmd(
        workspace::FILE_OPEN,
        json!({"path": source.to_string_lossy()}),
    )
    .unwrap();
    let view = t.w.editor(&source);
    t.w.wait("the test lenses", |w| lenses(w, &view).len() == 6);
    view
}

/// The id of the Test Explorer's last run.
fn last_run(t: &Tw) -> u64 {
    t.w.shell
        .read_with(&t.w.vcx, |s, _| s.test_runs().last_run().unwrap().id)
}

/// Wait for a run after run `after` and return its id.
fn wait_next_run(t: &mut Tw, after: u64) -> u64 {
    t.w.wait("the next run", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.test_runs().last_run().is_some_and(|r| r.id > after)
        })
    });
    last_run(t)
}

#[gpui::test]
fn run_test_from_a_lens_discovers_first_runs_through_the_test_explorer_and_shows_the_outcome(
    cx: &mut TestAppContext,
) {
    let mut t = tr::setup_scripted(cx, script_tests);
    t.w.fake.set_test_outcomes(json!({
        "u-adds": {"outcome": "passed", "durationMs": 12.0},
        "u-subtracts": {"outcome": "failed", "durationMs": 3.0, "message": "Assert.Equal() Failure"}
    }));
    t.w.open_solution();
    let view = open_calculator(&mut t);
    // Rows in order (an unknown command shows nothing): the class, then each test method, above its `[Fact]`.
    assert_eq!(
        titles(&t.w, &view),
        [
            (2, "Run All Tests".to_owned()),
            (2, "Debug All Tests".to_owned()),
            (4, "Run Test".to_owned()),
            (4, "Debug Test".to_owned()),
            (10, "Run Test".to_owned()),
            (10, "Debug Test".to_owned()),
        ]
    );
    let seen = record_titles(&mut t.w, &view);
    // Nothing is discovered yet: the lens reads "Discovering…", the Test Explorer discovers, then runs the test.
    assert_eq!(
        t.w.shell.read_with(&t.w.vcx, |s, _| s.test_runs().phase),
        Phase::Idle
    );
    let adds = lens_id(&t.w, &view, 4, "Run Test");
    click_lens(&mut t.w, &view, adds);
    // The discovery builds first (the Test Explorer's build gate): the fake host's build succeeds.
    t.w.wait("the build before the discovery", |w| {
        w.fake.running_build().is_some()
    });
    assert!(seen.borrow().contains(&DISCOVERING.to_owned()), "{seen:?}");
    t.w.fake.finish_build("succeeded", json!([]));
    t.w.wait("the run of Adds", |w| {
        !w.fake.received_params("eludite/test/run").is_empty()
    });
    let audit = t.w.audit();
    let discover = audit.iter().position(|c| c == cmds::DISCOVER).unwrap();
    let run = audit.iter().rposition(|c| c == cmds::RUN).unwrap();
    assert!(discover < run, "{audit:?}");
    let params = t.run_params().pop().unwrap();
    assert_ne!(params["debug"], true);
    assert_eq!(
        params["containers"],
        json!([{"id": t.mtp, "tests": ["u-adds"]}])
    );
    let first = last_run(&t);
    assert_eq!(t.wait_run_done(first), RunState::Passed);
    t.w.wait("the outcome on the lens", |w| {
        lenses(w, &view).contains(&(4, "Run Test (12 ms)".into(), true, Some(TestGlyph::Passed)))
    });
    // The tree is current now: Run Test on Subtracts runs at once, and its failure shows.
    let subtracts = lens_id(&t.w, &view, 10, "Run Test");
    click_lens(&mut t.w, &view, subtracts);
    let second = wait_next_run(&mut t, first);
    assert_eq!(t.wait_run_done(second), RunState::Failed);
    assert_eq!(
        t.run_params().pop().unwrap()["containers"],
        json!([{"id": t.mtp, "tests": ["u-subtracts"]}])
    );
    t.w.wait("the failure on the lens", |w| {
        lenses(w, &view).contains(&(10, "Run Test (3 ms)".into(), true, Some(TestGlyph::Failed)))
    });
    // The failure is an Error List row, as a run from the Test Explorer makes.
    assert!(
        t.w.error_rows()
            .iter()
            .any(|r| r.message.contains("Subtracts")),
        "{:?}",
        t.w.error_rows()
    );
    // Run All Tests above the class runs both, and the class lens aggregates them.
    let all = lens_id(&t.w, &view, 2, "Run All Tests");
    click_lens(&mut t.w, &view, all);
    let third = wait_next_run(&mut t, second);
    t.wait_run_done(third);
    assert_eq!(
        t.run_params().pop().unwrap()["containers"],
        json!([{"id": t.mtp, "tests": ["u-adds", "u-subtracts"]}])
    );
    t.w.wait("the class's outcome", |w| {
        lenses(w, &view)
            .iter()
            .any(|l| l.0 == 2 && l.3 == Some(TestGlyph::Failed) && l.1 == "Run All Tests (15 ms)")
    });

    // Debug Test starts a session, as brief 0035's does: the fake adapter breaks in Adds.
    let debug = lens_id(&t.w, &view, 4, "Debug Test");
    click_lens(&mut t.w, &view, debug);
    t.w.wait("the debugged test", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.debugger().model.mode == Mode::Break)
    });
    let params = t.run_params().pop().unwrap();
    assert_eq!(params["debug"], true);
    assert_eq!(
        params["containers"],
        json!([{"id": t.mtp, "tests": ["u-adds"]}])
    );
    assert!(t.w.audit().contains(&cmds::DEBUG.to_owned()));
}

#[gpui::test]
fn refresh_asks_again_and_an_edit_drops_a_stale_answer(cx: &mut TestAppContext) {
    let calls = Arc::new(AtomicU32::new(0));
    let c = calls.clone();
    let mut w = setup_with(cx, move |fake| {
        fake.set_language_server("running", Some(caps()));
        fake.respond("textDocument/codeLens", move |p| {
            let n = c.fetch_add(1, Ordering::SeqCst) + 1;
            let uri = p["textDocument"]["uri"].clone();
            let answer = json!([{"range": range(0, 6, 13), "command": {
                "title": format!("{n} references"), "command": "eludite.editor.find_references",
                "arguments": [{"uri": uri, "path": "/x", "position": pos(0, 6)}]}}]);
            // The third answer is slow: an edit makes it stale before it arrives.
            if n == 3 {
                FakeReply::After(Duration::from_millis(400), answer)
            } else {
                FakeReply::Result(answer)
            }
        });
    });
    let (_, view) = w.open_program();
    w.wait("the first answer", |w| {
        titles(w, &view) == [(0, "1 references".into())]
    });
    let seen = record_titles(&mut w, &view);
    // The host relays Roslyn's workspace/codeLens/refresh: the lenses are asked for again.
    w.fake.notify(
        "eludite/codeLens/refresh",
        json!({"eluditeGeneration": w.fake.generation()}),
    );
    w.wait("the refreshed answer", |w| {
        titles(w, &view) == [(0, "2 references".into())]
    });
    // A refresh for an older generation is ignored.
    w.fake
        .notify("eludite/codeLens/refresh", json!({"eluditeGeneration": 0}));
    w.vcx.run_until_parked();
    std::thread::sleep(Duration::from_millis(50));
    w.vcx.run_until_parked();
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    // A third request is in flight (slow); typing moves the document on, so its answer is dropped when it comes
    // (the host delivers it even though it was canceled), and the debounce asks for a fourth.
    w.fake.set_ignore_cancel(true);
    view.update(&mut w.vcx, |v, cx| v.refresh_code_lenses(cx));
    w.wait("the third request", |_| calls.load(Ordering::SeqCst) == 3);
    view.update_in(&mut w.vcx, |v, window, cx| {
        v.update_editor(cx, |e| e.set_caret(e.buffer().len()));
        window.focus(&gpui::Focusable::focus_handle(v, cx), cx);
    });
    w.vcx.simulate_input("x");
    std::thread::sleep(Duration::from_millis(500));
    w.vcx.run_until_parked();
    assert_eq!(titles(&w, &view), [(0, "2 references".into())]);
    w.vcx.executor().advance_clock(CODE_LENS_DEBOUNCE);
    w.wait("the fourth answer", |w| {
        titles(w, &view) == [(0, "4 references".into())]
    });
    assert!(
        !seen.borrow().contains(&"3 references".to_owned()),
        "{seen:?}"
    );
    // The fourth request came after the edit reached the host.
    let received: Vec<String> = w.fake.received().into_iter().map(|r| r.method).collect();
    let change = received
        .iter()
        .rposition(|m| m == "textDocument/didChange")
        .unwrap();
    let last = received
        .iter()
        .rposition(|m| m == "textDocument/codeLens")
        .unwrap();
    assert!(change < last, "{received:?}");
}

#[gpui::test]
fn the_settings_hide_references_or_tests_or_all_and_per_language_overrides_win(
    cx: &mut TestAppContext,
) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(caps()));
        fake.respond("textDocument/codeLens", |p| {
            let uri = p["textDocument"]["uri"].as_str().unwrap().to_owned();
            let refs = |line: u32, from: u32, title: &str| {
                json!({"range": range(line, from, from + 4), "command": {"title": title,
                    "command": "eludite.editor.find_references",
                    "arguments": [{"uri": uri, "path": "/x", "position": pos(line, from)}]}})
            };
            let test = |title: &str, debug: bool| {
                json!({"range": range(2, 16, 20), "command": {"title": title,
                    "command": if debug { "eludite.test.debug" } else { "eludite.test.run" },
                    "arguments": [{"uri": uri, "path": "/x", "range": range(2, 16, 20), "member": "Main"}]}})
            };
            FakeReply::Result(json!([
                refs(0, 6, "3 references"),
                refs(2, 16, "1 reference"),
                test("Run Test", false),
                test("Debug Test", true)
            ]))
        });
    });
    let (_, view) = w.open_program();
    let all = vec![
        (0, "3 references".to_owned()),
        (2, "1 reference".to_owned()),
        (2, "Run Test".to_owned()),
        (2, "Debug Test".to_owned()),
    ];
    w.wait("every lens", |w| titles(w, &view) == all);
    set(&mut w, "editor.codeLens.tests", json!(false));
    w.wait("references only", |w| {
        titles(w, &view) == [(0, "3 references".into()), (2, "1 reference".into())]
    });
    set(&mut w, "editor.codeLens.tests", json!(true));
    set(&mut w, "editor.codeLens.references", json!(false));
    w.wait("tests only", |w| {
        titles(w, &view) == [(2, "Run Test".into()), (2, "Debug Test".into())]
    });
    set(&mut w, "editor.codeLens.references", json!(true));
    w.wait("every lens again", |w| titles(w, &view) == all);
    // Off: the rows go, and the editor stops asking.
    let asked = w.fake.received_params("textDocument/codeLens").len();
    set(&mut w, "editor.codeLens", json!(false));
    w.wait("no lenses", |w| {
        lenses(w, &view).is_empty() && !view.read_with(&w.vcx, |v, _| v.code_lens_enabled())
    });
    assert_eq!(
        view.read_with(&w.vcx, |v, _| v.vertical_layout().lens_rows().len()),
        0
    );
    assert_eq!(w.fake.received_params("textDocument/codeLens").len(), asked);
    // A C# override wins over the switch.
    set(
        &mut w,
        "editor.languages.csharp.codeLens",
        json!("references"),
    );
    w.wait("C#'s references", |w| {
        titles(w, &view) == [(0, "3 references".into()), (2, "1 reference".into())]
    });
    // Another language's override does not apply to C#.
    set(&mut w, "editor.languages.csharp.codeLens", json!("default"));
    set(&mut w, "editor.languages.rust.codeLens", json!("on"));
    w.wait("C# follows the switch again", |w| {
        lenses(w, &view).is_empty()
    });
    let get = w
        .commands
        .invoke(
            eludite_commands::settings::GET,
            json!({"key": "editor.languages.rust.codeLens"}),
        )
        .unwrap();
    assert_eq!(get["settings"][0]["value"], "on", "{get}");
}

#[gpui::test]
fn a_rust_analyzer_run_test_lens_from_the_generic_server_runs_the_cargo_test(
    cx: &mut TestAppContext,
) {
    let ra = FakeServer::new();
    let mut lens_caps = analyzer_capabilities();
    lens_caps["codeLensProvider"] = json!({"resolveProvider": true});
    ra.set_capabilities(lens_caps);
    ra.respond("textDocument/codeLens", |p| {
        let uri = p["textDocument"]["uri"].clone();
        let runnable = json!({"label": "test tests::adds", "kind": "cargo",
            "location": {"targetUri": uri, "targetRange": range(16, 4, 20), "targetSelectionRange": range(16, 7, 11)},
            "args": {"workspaceRoot": "/", "cargoArgs": ["test", "--package", "corpus-tests", "--lib"],
                     "executableArgs": ["tests::adds", "--exact", "--nocapture"]}});
        FakeReply::Result(json!([
            {"range": range(16, 7, 11), "command": {"title": "\u{25b6}\u{fe0e} Run Test", "command": "rust-analyzer.runSingle", "arguments": [runnable]}},
            {"range": range(16, 7, 11), "command": {"title": "Debug", "command": "rust-analyzer.debugSingle", "arguments": [runnable]}},
            {"range": range(3, 7, 10), "data": {"impls": "add"}}
        ]))
    });
    ra.respond("codeLens/resolve", |p| {
        let uri = p["data"]["impls"].as_str().map(|_| "file:///nowhere/b.rs");
        FakeReply::Result(json!({"range": p["range"], "data": p["data"], "command": {
            "title": "1 implementation", "command": "rust-analyzer.showReferences",
            "arguments": [uri, pos(3, 7), [{"uri": uri, "range": range(0, 0, 3)}]]}}))
    });
    let connector = ra.connector();
    let debug = Some(super::debug::DebugSetup {
        connect: None,
        search: eludite_dap::discovery::AdapterSearch::default(),
        mono: eludite_dap::discovery::MonoSearch::default(),
        mono_adapter: eludite_dap::discovery::MonoAdapterSearch::default(),
        platform: eludite_dap::launch::Platform::current(),
        store_dir: None,
        dotnet: "dotnet".into(),
        js: Default::default(),
    });
    let mut w = setup_services(
        cx,
        |_| {},
        None,
        debug,
        move |s| {
            s.launches
                .in_process
                .insert("rust-analyzer".into(), connector);
        },
    );
    let rs = w.path("rs");
    tr::copy_rust_corpus(&rs);
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::WORKSPACE_OPEN_FOLDER,
                json!({"path": rs.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    w.wait("the Cargo workspace", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.cargo_workspace().is_some())
    });
    let lib = rs.join("src/lib.rs");
    let view = open_file(&mut w, &lib);
    w.wait("rust-analyzer's lenses", |w| lenses(w, &view).len() == 3);
    w.wait("the implementations lens resolved", |w| {
        lenses(w, &view).iter().all(|l| l.2)
    });
    // rust-analyzer's titles in Visual Studio's words; the implementations lens resolved with its locations.
    assert_eq!(
        titles(&w, &view),
        [
            (3, "1 implementation".to_owned()),
            (15, "Run Test".to_owned()),
            (15, "Debug Test".to_owned()),
        ]
    );
    assert!(
        ra.received_params("textDocument/codeLens")[0]
            .get("eluditeGeneration")
            .is_none()
    );
    // The implementations lens opens the popup with the locations it carries, without asking the server again.
    let imps = lens_id(&w, &view, 3, "1 implementation");
    click_lens(&mut w, &view, imps);
    let popup = w
        .shell
        .read_with(&w.vcx, |s, _| s.code_lens().popup.clone())
        .unwrap();
    w.wait("the implementations", |w| {
        popup.read_with(&w.vcx, |p, _| *p.state() == PopupState::Done)
    });
    assert_eq!(popup.read_with(&w.vcx, |p, _| p.references().len()), 1);
    assert!(ra.received_params("textDocument/references").is_empty());
    w.vcx.simulate_keystrokes("escape");
    // Run Test: the Cargo tests are discovered (the real cargo), then tests::adds runs, and its outcome shows.
    let run = lens_id(&w, &view, 15, "Run Test");
    click_lens(&mut w, &view, run);
    tr::wait_long(&mut w, "the Rust run", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.test_runs().last_run().is_some_and(|r| r.state.done())
        })
    });
    let (tests, state) = w.shell.read_with(&w.vcx, |s, _| {
        let r = s.test_runs().last_run().unwrap();
        (r.tests.clone(), r.state)
    });
    assert_eq!(tests.len(), 1, "{tests:?}");
    assert!(tests[0].ends_with("|tests::adds"), "{tests:?}");
    assert_eq!(state, RunState::Passed);
    w.wait("the outcome on the lens", |w| {
        lenses(w, &view)
            .iter()
            .any(|l| l.0 == 15 && l.1.starts_with("Run Test (") && l.3 == Some(TestGlyph::Passed))
    });
}

/// Type `keys` keys into `view` (at its caret), each followed by a frame; the time from the key to the frame drawn.
fn keystroke_frames(w: &mut Ws, keys: usize) -> Vec<Duration> {
    let mut frames = Vec::new();
    // Ten keys first, not counted: the first frames after a resize lay everything out anew.
    for i in 0..keys + 10 {
        let key = if i % 2 == 0 { "x" } else { "y" };
        let took = w.vcx.update(|window, cx| {
            let t = Instant::now();
            window.dispatch_keystroke(gpui::Keystroke::parse(key).unwrap(), cx);
            window.refresh();
            let _ = window.draw(cx);
            t.elapsed()
        });
        w.vcx.run_until_parked();
        if i % 10 == 9 {
            // Past the debounce: a lens request goes out (and is held).
            w.vcx.executor().advance_clock(CODE_LENS_DEBOUNCE);
            w.vcx.run_until_parked();
        }
        if i >= 10 {
            frames.push(took);
        }
    }
    frames.sort();
    frames
}

fn p99(frames: &[Duration]) -> Duration {
    frames[(frames.len() * 99).div_ceil(100) - 1]
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

#[gpui::test]
fn typing_stays_under_the_keystroke_budget_with_200_lens_rows_on_screen(cx: &mut TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(caps()));
        fake.respond("textDocument/codeLens", |p| {
            let uri = p["textDocument"]["uri"].clone();
            let lenses: Vec<Value> = (0..200)
                .map(|i| {
                    json!({"range": range(i * 2 + 2, 16, 20), "command": {"title": format!("{i} references"),
                        "command": "eludite.editor.find_references",
                        "arguments": [{"uri": uri, "path": "/x", "position": pos(i * 2 + 2, 16)}]}})
                })
                .collect();
            FakeReply::Result(Value::Array(lenses))
        });
    });
    // 400 members' worth of lines: a lens above every other line.
    let mut text = String::from("class Big\n{\n");
    for i in 0..200 {
        text.push_str(&format!("    void M{i:03}() {{ }}\n    // {i}\n"));
    }
    text.push_str("}\n");
    let big = w.path("src/App/Big.cs");
    std::fs::write(&big, &text).unwrap();
    w.open_solution();
    let view = open_file(&mut w, &big);
    w.wait("200 lens rows", |w| lenses(w, &view).len() == 200);
    view.update_in(&mut w.vcx, |v, window, cx| {
        v.update_editor(cx, |e| e.set_caret(text.find("M100").unwrap()));
        window.focus(&gpui::Focusable::focus_handle(v, cx), cx);
    });
    w.vcx.run_until_parked();
    // The lens requests made while typing are held by the host: the UI never waits on them.
    w.fake.respond("textDocument/codeLens", |_| FakeReply::Hold);

    // The editor's window (1280 x 800): the budget, with 200 lens rows in the document. The best of up to three runs
    // of 200 keys, so another process's burst on a loaded machine is not counted as the editor's.
    let mut window = keystroke_frames(&mut w, 200);
    for _ in 0..2 {
        if p99(&window) < Duration::from_millis(8) {
            break;
        }
        let again = keystroke_frames(&mut w, 200);
        if p99(&again) < p99(&window) {
            window = again;
        }
    }
    // A window tall enough for all 200 lens rows (and their 400 lines) at once, with the lenses and without.
    w.vcx.simulate_resize(size(px(1280.), px(11000.)));
    view.update(&mut w.vcx, |v, cx| v.scroll_to_row(0, cx));
    w.vcx.run_until_parked();
    let painted = view.read_with(&w.vcx, |v, _| {
        v.code_lenses()
            .iter()
            .filter(|l| v.code_lens_bounds(l.id).is_some())
            .count()
    });
    assert_eq!(painted, 200, "every lens row on screen");
    let tall = keystroke_frames(&mut w, 60);
    let tall_again = keystroke_frames(&mut w, 60);
    let tall = if tall_again[30] < tall[30] {
        tall_again
    } else {
        tall
    };
    assert!(w.text(&view).contains("M10"), "the keys reached the editor");
    assert_eq!(lenses(&w, &view).len(), 200, "the rows stay while typing");
    view.update(&mut w.vcx, |v, cx| v.set_code_lens_enabled(false, cx));
    w.vcx.run_until_parked();
    let without = keystroke_frames(&mut w, 60);
    let without_again = keystroke_frames(&mut w, 60);
    let without = if without_again[30] < without[30] {
        without_again
    } else {
        without
    };
    eprintln!(
        "timing: keystroke to frame with 200 lens rows in the document: median {:.2} ms, p99 {:.2} ms; \
         with all 200 on a 11000 px screen: median {:.2} ms, p99 {:.2} ms (the same screen without lenses: median \
         {:.2} ms, p99 {:.2} ms)",
        ms(window[window.len() / 2]),
        ms(p99(&window)),
        ms(tall[tall.len() / 2]),
        ms(p99(&tall)),
        ms(without[without.len() / 2]),
        ms(p99(&without)),
    );
    super::tests::assert_budget(
        "the keystroke frame with 200 lens rows",
        p99(&window),
        Duration::from_millis(8),
    );
    // On the tall screen the 200 lens rows cost no more than the 400 text lines under them do, line for line.
    let lines = without[without.len() / 2];
    super::tests::assert_budget(
        "the lens rows' share of a frame with all 200 on screen",
        tall[tall.len() / 2].saturating_sub(lines),
        (lines * 3 / 4).max(Duration::from_millis(3)),
    );
}
