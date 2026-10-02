//! Headless GPUI tests of navigation in the shell (brief 0014) against the in-process fake `eludite-host`: Go To
//! Definition (F12 and Ctrl+click) opens the target and pushes the history; a metadata-as-source target opens a
//! read-only `[from metadata]` tab with the file's text; several targets show a picker; Find All References fills
//! its tool window grouped by project and file, and a double-click navigates; Ctrl+- and Ctrl+Shift+- walk the
//! history; each Error List filter hides and shows the right rows with correct counts; stale answers are dropped;
//! agents drive all of it on the bus.

use std::path::{Path, PathBuf};
use std::time::Duration;

use eludite_commands::diagnostics::DIAGNOSTICS_LIST;
use eludite_commands::workspace;
use eludite_docking::ids;
use eludite_editor::EditorView;
use eludite_lsp::fake::FakeReply;
use gpui::{
    Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent, point, px,
};
use serde_json::{Value, json};

use super::documents::path_to_uri;
use super::error_list;
use super::navigation::{self, picker_row_selector};
use super::references::{RefRow, row_selector};
use super::tests::{PROGRAM, T, Ws, setup_with};

const ORDER: &str = "class Order { }\n";

fn capabilities() -> Value {
    json!({"definitionProvider": true, "referencesProvider": true, "hoverProvider": true})
}

fn location(path: &Path, line: u32, start: u32, end: u32) -> Value {
    json!({"uri": path_to_uri(path),
           "range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}}})
}

/// A metadata-as-source file as the pinned Roslyn writes it (host-rpc.md), removed on drop.
struct MetadataFile {
    dir: PathBuf,
    path: PathBuf,
}

const JSON_RPC: &str = "#region Assembly StreamJsonRpc, Version=2.25.0.0\n// Decompiled with ICSharpCode.Decompiler\n#endregion\n\nnamespace StreamJsonRpc;\n\npublic class JsonRpc : IDisposable\n{\n}\n";

impl MetadataFile {
    fn new() -> Self {
        let dir = navigation::metadata_root().join(format!(
            "eludite-test-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let path = dir
            .join("DecompilationMetadataAsSourceFileProvider")
            .join("c19c8186")
            .join("JsonRpc.cs");
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, JSON_RPC).unwrap();
        Self { dir, path }
    }
}

impl Drop for MetadataFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Ws {
    fn invoke(&mut self, command: &str, args: Value) -> Value {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| {
                s.invoke(command, args, window, cx)
            })
            .unwrap()
    }

    fn active(&self) -> Option<String> {
        self.controller.active_document()
    }

    fn caret_line_column(&self, view: &Entity<EditorView>) -> (u32, u32) {
        view.read_with(&self.vcx, |v, _| {
            let b = v.editor().buffer();
            let p = b.offset_to_point(v.editor().primary_selection().head);
            (p.row + 1, p.column + 1)
        })
    }

    fn set_caret(&mut self, view: &Entity<EditorView>, offset: usize) {
        view.update(&mut self.vcx, |v, cx| {
            v.update_editor(cx, |e| e.set_caret(offset))
        });
        self.vcx.run_until_parked();
    }

    fn definition_state(&self) -> Value {
        self.shell.read_with(&self.vcx, |s, _| {
            serde_json::to_value(s.definition_output().state).unwrap()
        })
    }

    fn back_forward(&self) -> (usize, usize) {
        self.shell.read_with(&self.vcx, |s, _| {
            let h = &s.navigation.history;
            (h.back_len(), h.forward_len())
        })
    }

    fn reference_rows(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            let w = s.references_window().read(cx);
            w.rows()
                .iter()
                .map(|r| match r {
                    RefRow::Project { name, count, .. } => format!("{name} ({count})"),
                    RefRow::File { title, count, .. } => format!("  {title} ({count})"),
                    RefRow::Reference(i) => {
                        let r = &w.references()[*i];
                        format!(
                            "    {} [{}] ({}, {})",
                            r.text,
                            &r.text[r.highlight.clone()],
                            r.line,
                            r.column
                        )
                    }
                })
                .collect()
        })
    }

    fn references_header(&self) -> String {
        self.shell
            .read_with(&self.vcx, |s, cx| s.references_window().read(cx).header())
    }

    fn shown_errors(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.error_list()
                .read(cx)
                .visible_rows()
                .map(|r| r.code.clone())
                .collect()
        })
    }

    fn error_counts(&self) -> (usize, usize, usize) {
        self.shell.read_with(&self.vcx, |s, cx| {
            let c = s.error_list().read(cx).counts();
            (c.errors, c.warnings, c.messages)
        })
    }

    fn ctrl_click(&mut self, view: &Entity<EditorView>, offset: usize) {
        let p = view
            .read_with(&self.vcx, |v, _| v.pixel_position_for_offset(offset))
            .expect("row painted");
        let position = point(p.x + px(1.), p.y + px(5.));
        self.vcx.simulate_event(MouseDownEvent {
            position,
            modifiers: Modifiers::control(),
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        self.vcx.simulate_event(MouseUpEvent {
            position,
            modifiers: Modifiers::control(),
            button: MouseButton::Left,
            click_count: 1,
        });
        self.vcx.run_until_parked();
    }
}

/// Offset of `Main` in Program.cs.
fn main_offset() -> usize {
    PROGRAM.find("Main").unwrap()
}

#[gpui::test]
fn definition_in_a_file_opens_positions_pushes_history_and_back_and_forward_walk_it(
    cx: &mut gpui::TestAppContext,
) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let order = w.path("src/App/Models/Order.cs");
    let target = location(&order, 0, 6, 11);
    w.fake.respond("textDocument/definition", move |_| {
        FakeReply::Result(json!([target]))
    });
    let (program, view) = w.open_program();
    let program_id = program.to_string_lossy().into_owned();
    let order_id = order.to_string_lossy().into_owned();
    w.set_caret(&view, main_offset() + 1);

    // F12 runs the command; the request is the caret's position.
    w.vcx.simulate_keystrokes("f12");
    let order_view = w.editor(&order);
    w.wait("the caret at the definition", |w| {
        w.active().as_deref() == Some(order_id.as_str()) && w.caret(&order_view) == 6
    });
    let sent = w.fake.received_params("textDocument/definition");
    assert_eq!(sent.len(), 1);
    assert_eq!(sent[0]["position"], json!({"line": 2, "character": 17}));
    assert_eq!(sent[0]["textDocument"]["uri"], path_to_uri(&program));
    assert_eq!(w.text(&order_view), ORDER);
    let audit = w.audit();
    let def = audit
        .iter()
        .position(|c| c == workspace::EDITOR_GO_TO_DEFINITION)
        .expect("F12 ran the command");
    assert!(
        audit[def..].contains(&workspace::FILE_OPEN.to_owned()),
        "the target opened through eludite.file.open"
    );
    assert_eq!(w.definition_state(), "navigated");
    // The position the request was made at was pushed first.
    assert_eq!(w.back_forward(), (1, 0));

    // Ctrl+- goes back to it; Ctrl+Shift+- forward again.
    w.vcx.simulate_keystrokes("ctrl--");
    w.wait("back in Program.cs", |w| {
        w.active().as_deref() == Some(program_id.as_str())
    });
    assert_eq!(w.caret(&view), main_offset() + 1);
    assert_eq!(w.back_forward(), (0, 1));
    assert_eq!(
        w.audit().last().map(String::as_str),
        Some(workspace::NAVIGATION_BACK)
    );
    w.vcx.simulate_keystrokes("ctrl-shift--");
    w.wait("forward in Order.cs", |w| {
        w.active().as_deref() == Some(order_id.as_str())
    });
    assert_eq!(w.caret(&order_view), 6);
    assert_eq!(w.back_forward(), (1, 0));
    // Nowhere further to go.
    let out = w.invoke(workspace::NAVIGATION_FORWARD, json!({}));
    assert_eq!(out, json!({"navigated": false, "back": 1, "forward": 0}));
    let out = w.invoke(workspace::NAVIGATION_BACK, json!({}));
    assert_eq!(out["navigated"], true);
    assert_eq!(out["path"], program_id);
    assert_eq!(
        (out["line"].clone(), out["column"].clone()),
        (json!(3), json!(18))
    );

    // A document that was closed reopens when the history returns to it.
    w.invoke(
        workspace::FILE_CLOSE,
        json!({"path": order_id.clone(), "save": "discard"}),
    );
    // Give the editor the keyboard and draw the frame the keys dispatch through.
    w.vcx
        .update(|window, cx| view.focus_handle(cx).focus(window, cx));
    w.vcx.run_until_parked();
    w.vcx.simulate_keystrokes("ctrl-shift--");
    let reopened = w.editor(&order);
    w.wait("Order.cs reopened at the definition", |w| {
        w.active().as_deref() == Some(order_id.as_str()) && w.caret(&reopened) == 6
    });

    // Ctrl+click on a symbol is Go To Definition at the clicked position.
    w.vcx.simulate_keystrokes("ctrl--");
    w.wait("back in Program.cs", |w| {
        w.active().as_deref() == Some(program_id.as_str())
    });
    let class = PROGRAM.find("Program").unwrap() + 2;
    w.ctrl_click(&view, class);
    w.wait("a second definition request", |w| {
        w.fake.received_params("textDocument/definition").len() == 2
    });
    assert_eq!(
        w.fake.received_params("textDocument/definition")[1]["position"],
        json!({"line": 0, "character": 8})
    );
    w.wait("navigated by Ctrl+click", |w| {
        w.active().as_deref() == Some(order_id.as_str())
    });
}

#[gpui::test]
fn definition_into_metadata_opens_a_read_only_tab_with_its_text(cx: &mut gpui::TestAppContext) {
    let meta = MetadataFile::new();
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let target = location(&meta.path, 6, 13, 20);
    w.fake.respond("textDocument/definition", move |_| {
        FakeReply::Result(json!([target]))
    });
    w.fake.respond("textDocument/hover", |p| {
        FakeReply::Result(json!({"contents": {"kind": "markdown",
            "value": format!("class StreamJsonRpc.JsonRpc at {}", p["position"]["line"])}}))
    });
    let (_, view) = w.open_program();
    w.set_caret(&view, main_offset());
    let out = w.invoke(workspace::EDITOR_GO_TO_DEFINITION, json!({}));
    assert_eq!(out["state"], "loading", "the UI thread does not wait");
    let id = meta.path.to_string_lossy().into_owned();
    let meta_view = w.editor(&meta.path);
    w.wait("the caret in the metadata", |w| {
        w.active().as_deref() == Some(id.as_str()) && w.caret_line_column(&meta_view) == (7, 14)
    });
    // Visual Studio's title, the language server's text, read-only.
    assert_eq!(
        w.controller.layout().documents.get(&id).unwrap().title,
        "JsonRpc [from metadata]"
    );
    assert_eq!(w.text(&meta_view), JSON_RPC);
    assert!(meta_view.read_with(&w.vcx, |v, _| v.is_read_only()));
    let out = w
        .shell
        .read_with(&w.vcx, |s, _| s.definition_output())
        .clone();
    let out = serde_json::to_value(out).unwrap();
    assert_eq!(out["state"], "navigated");
    assert_eq!(out["targets"][0]["metadata"], true);
    assert_eq!(out["navigated"]["title"], "JsonRpc [from metadata]");
    w.vcx.simulate_input("x");
    w.vcx.simulate_keystrokes("enter backspace");
    assert_eq!(w.text(&meta_view), JSON_RPC, "typing does nothing");
    assert!(!w.dirty(&id));
    let saved = w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.invoke(workspace::EDITOR_SAVE, json!({}), window, cx)
    });
    assert!(saved.is_err(), "a metadata document cannot be saved");
    // No document notifications for it; requests made inside it still go to the server.
    let uri = path_to_uri(&meta.path);
    assert!(
        !w.fake
            .received_params("textDocument/didOpen")
            .iter()
            .any(|p| p["textDocument"]["uri"] == uri)
    );
    let hover = w.invoke(
        workspace::EDITOR_HOVER,
        json!({"path": id.clone(), "line": 7, "column": 15}),
    );
    assert_eq!(hover["state"], "loading");
    w.wait("Quick Info in the metadata", |w| {
        meta_view.read_with(&w.vcx, |v, _| v.hover().and_then(|h| h.text))
            == Some("class StreamJsonRpc.JsonRpc at 6".into())
    });
    w.invoke(workspace::FILE_CLOSE, json!({"path": id.clone()}));
    w.vcx.run_until_parked();
    assert!(
        !w.fake
            .received_params("textDocument/didClose")
            .iter()
            .any(|p| p["textDocument"]["uri"] == uri)
    );
    // Back returns to Program.cs.
    w.vcx.simulate_keystrokes("ctrl--");
    w.wait("back in Program.cs", |w| {
        w.active().is_some_and(|a| a.ends_with("Program.cs"))
    });
}

#[gpui::test]
fn several_definitions_show_a_picker(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let program = w.path("src/App/Program.cs");
    let order = w.path("src/App/Models/Order.cs");
    let targets = json!([location(&program, 0, 6, 13), location(&order, 0, 6, 11)]);
    w.fake.respond("textDocument/definition", move |_| {
        FakeReply::Result(targets.clone())
    });
    let (_, view) = w.open_program();
    w.set_caret(&view, main_offset());
    w.vcx.simulate_keystrokes("f12");
    w.wait("the picker", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.definition_picker().is_some())
    });
    assert_eq!(w.definition_state(), "choose");
    assert_eq!(w.back_forward(), (0, 0), "nothing pushed until a choice");
    let titles: Vec<String> = w.shell.read_with(&w.vcx, |s, cx| {
        s.definition_picker()
            .unwrap()
            .read(cx)
            .targets()
            .iter()
            .map(|t| t.title())
            .collect()
    });
    assert_eq!(titles, ["Program.cs", "Order.cs"]);
    assert!(w.vcx.debug_bounds("definition-picker").is_some(), "drawn");

    // Down, Enter: the second target, through the command with `target`.
    w.vcx.simulate_keystrokes("down enter");
    let order_id = order.to_string_lossy().into_owned();
    w.wait("Order.cs", |w| {
        w.active().as_deref() == Some(order_id.as_str())
    });
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.definition_picker().is_none())
    );
    assert_eq!(w.back_forward(), (1, 0));
    assert!(
        w.audit()
            .iter()
            .filter(|c| *c == workspace::EDITOR_GO_TO_DEFINITION)
            .count()
            >= 2
    );

    // A click chooses too; Escape dismisses without navigating.
    w.vcx.simulate_keystrokes("ctrl--");
    w.wait("Program.cs", |w| {
        w.active().is_some_and(|a| a.ends_with("Program.cs"))
    });
    w.vcx.simulate_keystrokes("f12");
    w.wait("the picker", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.definition_picker().is_some())
    });
    w.vcx.simulate_keystrokes("escape");
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.definition_picker().is_none())
    );
    assert!(w.active().is_some_and(|a| a.ends_with("Program.cs")));
    w.vcx.simulate_keystrokes("f12");
    w.wait("the picker", |w| {
        w.shell
            .read_with(&w.vcx, |s, _| s.definition_picker().is_some())
    });
    w.click(&picker_row_selector(1));
    w.wait("Order.cs", |w| {
        w.active().as_deref() == Some(order_id.as_str())
    });
    // Choosing with no picker open is an error.
    let err = w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.invoke(
            workspace::EDITOR_GO_TO_DEFINITION,
            json!({"target": 0}),
            window,
            cx,
        )
    });
    assert!(err.is_err());
}

#[gpui::test]
fn references_fill_the_window_grouped_and_double_click_navigates(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let program = w.path("src/App/Program.cs");
    let order = w.path("src/App/Models/Order.cs");
    // A file outside every project.
    let loose = w.path("Loose.cs");
    std::fs::write(&loose, "// Main\n\tvar m = Main;\n").unwrap();
    let refs = json!([
        location(&loose, 1, 9, 13),
        location(&program, 2, 16, 20),
        location(&order, 0, 6, 11)
    ]);
    w.fake.respond("textDocument/references", move |p| {
        assert_eq!(p["context"]["includeDeclaration"], true);
        FakeReply::Result(refs.clone())
    });
    let (_, view) = w.open_program();
    w.set_caret(&view, main_offset() + 2);
    w.vcx.simulate_keystrokes("shift-f12");
    // The window shows at once, beside the Error List, searching.
    let snapshot = w.controller.layout();
    let group = snapshot.group_of(ids::FIND_ALL_REFERENCES).expect("docked");
    assert_eq!(group.active_id(), Some(ids::FIND_ALL_REFERENCES));
    assert!(group.tabs.contains(&ids::ERROR_LIST.to_owned()));
    w.wait("the results", |w| {
        w.references_header() == "'Main' references: 3 results"
    });
    assert_eq!(
        w.reference_rows(),
        [
            "App (2)",
            "  Order.cs (1)",
            "    class Order { } [Order] (1, 7)",
            "  Program.cs (1)",
            "    static void Main() { } [Main] (3, 17)",
            "Miscellaneous Files (1)",
            "  Loose.cs (1)",
            "    var m = Main; [Main] (2, 10)",
        ]
    );
    assert!(
        w.audit()
            .contains(&workspace::EDITOR_FIND_REFERENCES.to_owned())
    );

    // Collapsing a group hides its rows.
    w.click(&format!("{}-toggle", row_selector(0)));
    assert_eq!(
        w.reference_rows(),
        [
            "App (2)",
            "Miscellaneous Files (1)",
            "  Loose.cs (1)",
            "    var m = Main; [Main] (2, 10)"
        ]
    );
    w.click(&format!("{}-toggle", row_selector(0)));

    // Double-click navigates, after pushing where the caret was.
    w.double_click(&row_selector(2));
    let order_view = w.editor(&order);
    let order_id = order.to_string_lossy().into_owned();
    w.wait("Order.cs at the reference", |w| {
        w.active().as_deref() == Some(order_id.as_str()) && w.caret(&order_view) == 6
    });
    assert_eq!(w.back_forward(), (1, 0));
    // Collapse App so the last group's reference is in view (row 3).
    w.click(&format!("{}-toggle", row_selector(0)));
    w.double_click(&row_selector(3));
    let loose_view = w.editor(&loose);
    w.wait("Loose.cs at the reference", |w| {
        w.caret_line_column(&loose_view) == (2, 10)
    });
    assert_eq!(w.back_forward(), (2, 0));

    // A new search replaces the results.
    w.fake
        .respond("textDocument/references", |_| FakeReply::Result(json!([])));
    let out = w.invoke(
        workspace::EDITOR_FIND_REFERENCES,
        json!({"path": program.to_string_lossy(), "line": 1, "column": 8}),
    );
    assert_eq!(out["state"], "loading");
    assert_eq!(out["symbol"], "Program");
    w.wait("the new results", |w| {
        w.references_header() == "'Program' references: 0 results"
    });
    assert!(w.reference_rows().is_empty());
}

#[gpui::test]
fn stale_navigation_answers_are_dropped(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.set_ignore_cancel(true);
    });
    let program = w.path("src/App/Program.cs");
    let order = w.path("src/App/Models/Order.cs");
    let (slow, fast) = (location(&order, 0, 6, 11), location(&program, 0, 6, 13));
    let calls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counter = calls.clone();
    w.fake.respond("textDocument/definition", move |_| {
        match counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst) {
            0 => FakeReply::After(Duration::from_millis(300), json!([slow])),
            _ => FakeReply::Result(json!([fast])),
        }
    });
    let (_, view) = w.open_program();
    w.set_caret(&view, main_offset());
    // The first request is superseded by the second; its late answer (the host ignores the cancel) is never shown.
    w.vcx.simulate_keystrokes("f12");
    w.wait("the first request", |w| {
        w.fake.received_params("textDocument/definition").len() == 1
    });
    w.vcx.simulate_keystrokes("f12");
    w.wait("the second answer", |w| w.caret(&view) == 6);
    assert_eq!(
        w.fake.received_params("$/cancelRequest").len(),
        1,
        "the first was canceled"
    );
    std::thread::sleep(Duration::from_millis(400));
    w.vcx.run_until_parked();
    let order_id = order.to_string_lossy();
    assert!(
        !w.shell.read_with(&w.vcx, |s, _| s
            .editor(Path::new(order_id.as_ref()))
            .is_some()),
        "the late answer did not navigate"
    );

    // An answer for a document version that changed meanwhile is dropped too.
    w.fake.respond("textDocument/definition", {
        let target = location(&order, 0, 6, 11);
        move |_| FakeReply::After(Duration::from_millis(200), json!([target]))
    });
    w.vcx.simulate_keystrokes("f12");
    w.wait("the request", |w| {
        w.fake.received_params("textDocument/definition").len() == 3
    });
    w.vcx.simulate_input("x");
    w.shell.update(&mut w.vcx, |s, cx| {
        let id = program.to_string_lossy().into_owned();
        s.flush_change(&id, cx)
    });
    std::thread::sleep(Duration::from_millis(300));
    w.wait("the answer dropped", |w| w.definition_state() == "failed");
    assert!(!w.shell.read_with(&w.vcx, |s, _| {
        s.editor(Path::new(order_id.as_ref())).is_some()
    }));

    // References: an answer computed under the old generation never reaches the window.
    w.fake.respond("textDocument/references", {
        let r = location(&order, 0, 6, 11);
        move |_| FakeReply::After(Duration::from_millis(300), json!([r]))
    });
    w.invoke(workspace::EDITOR_FIND_REFERENCES, json!({}));
    w.wait("the request", |w| {
        !w.fake.received_params("textDocument/references").is_empty()
    });
    let sln = w.path("App.slnx");
    w.commands
        .invoke(
            workspace::SOLUTION_OPEN,
            json!({"path": sln.to_string_lossy()}),
        )
        .unwrap();
    w.wait("generation 2", |w| w.fake.generation() == 2);
    std::thread::sleep(Duration::from_millis(400));
    w.vcx.run_until_parked();
    assert!(w.reference_rows().is_empty(), "{:?}", w.reference_rows());
    assert!(
        w.references_header().contains("changed"),
        "{}",
        w.references_header()
    );
}

#[gpui::test]
fn error_list_filters_hide_and_show_rows_and_keep_counts(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let (program, _) = w.open_program();
    let loose = w.path("Loose.cs");
    let d = |line: u32, severity: u8, code: &str, message: &str| {
        json!({"range": {"start": {"line": line, "character": 0}, "end": {"line": line, "character": 1}},
               "severity": severity, "code": code, "message": message})
    };
    w.fake.publish_diagnostics(
        &path_to_uri(&program),
        1,
        json!([
            d(
                0,
                1,
                "CS0103",
                "The name 'x' does not exist in the current context"
            ),
            d(1, 1, "CS1002", "; expected"),
            d(
                2,
                2,
                "CS0168",
                "The variable 'e' is declared but never used"
            ),
            d(2, 3, "IDE0290", "Use primary constructor"),
            d(2, 4, "IDE0001", "A hint is never listed")
        ]),
    );
    w.fake.publish_diagnostics(
        &path_to_uri(&loose),
        1,
        json!([d(
            0,
            2,
            "CS0219",
            "The variable 'm' is assigned but its value is never used"
        )]),
    );
    w.wait("the rows", |w| w.shown_errors().len() == 5);
    assert_eq!(
        w.shown_errors(),
        ["CS0103", "CS1002", "CS0219", "CS0168", "IDE0290"]
    );
    assert_eq!(w.error_counts(), (2, 2, 1));

    // The Errors toggle hides errors; the counts stay (Visual Studio's buttons show them either way).
    w.click(error_list::ERRORS_BUTTON);
    assert_eq!(w.shown_errors(), ["CS0219", "CS0168", "IDE0290"]);
    assert_eq!(w.error_counts(), (2, 2, 1));
    assert_eq!(
        w.audit().last().map(String::as_str),
        Some(workspace::ERROR_LIST_FILTER)
    );
    w.click(error_list::WARNINGS_BUTTON);
    assert_eq!(w.shown_errors(), ["IDE0290"]);
    w.click(error_list::MESSAGES_BUTTON);
    assert!(w.shown_errors().is_empty());
    w.click(error_list::ERRORS_BUTTON);
    w.click(error_list::WARNINGS_BUTTON);
    w.click(error_list::MESSAGES_BUTTON);
    assert_eq!(w.shown_errors().len(), 5);

    // The project dropdown: App only (Loose.cs is in no project); the counts follow it.
    w.click(error_list::PROJECT_BUTTON);
    w.click(&error_list::project_item_selector(1));
    assert_eq!(w.shown_errors(), ["CS0103", "CS1002", "CS0168", "IDE0290"]);
    assert_eq!(w.error_counts(), (2, 1, 1));
    w.click(error_list::PROJECT_BUTTON);
    w.click(&error_list::project_item_selector(0));
    assert_eq!(w.shown_errors().len(), 5);

    // The search box filters code and description as the user types; the counts follow it.
    w.click(error_list::SEARCH_BOX);
    w.vcx.simulate_keystrokes("v a r");
    assert_eq!(w.shown_errors(), ["CS0219", "CS0168"]);
    assert_eq!(w.error_counts(), (0, 2, 0));
    w.vcx
        .simulate_keystrokes("backspace backspace backspace c s 1");
    assert_eq!(w.shown_errors(), ["CS1002"]);
    w.vcx.simulate_keystrokes("escape");
    assert_eq!(w.shown_errors().len(), 5);

    // The same through the bus, as an agent would; diagnostics.list is unaffected by the window's filters.
    let out = w.invoke(
        workspace::ERROR_LIST_FILTER,
        json!({"errors": false, "messages": false, "text": "variable"}),
    );
    assert_eq!(
        out,
        json!({"errors": false, "warnings": true, "messages": false, "text": "variable",
               "counts": {"errors": 0, "warnings": 2, "messages": 0}, "shown": 2, "total": 5})
    );
    let all = w.commands.invoke(DIAGNOSTICS_LIST, json!({})).unwrap();
    assert_eq!(all.as_array().unwrap().len(), 5);
    let errors = w
        .commands
        .invoke(DIAGNOSTICS_LIST, json!({"severity": "error"}))
        .unwrap();
    assert_eq!(errors.as_array().unwrap().len(), 2);
    // New diagnostics are filtered as they arrive.
    w.fake.publish_diagnostics(
        &path_to_uri(&program),
        1,
        json!([d(
            0,
            2,
            "CS0168",
            "The variable 'y' is declared but never used"
        )]),
    );
    w.wait("the new rows", |w| w.error_counts() == (0, 2, 0));
    assert_eq!(w.shown_errors(), ["CS0219", "CS0168"]);
}

#[gpui::test]
fn agents_go_to_definition_and_find_references_on_the_bus(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
    });
    let program = w.path("src/App/Program.cs");
    let order = w.path("src/App/Models/Order.cs");
    let target = location(&order, 0, 6, 11);
    w.fake.respond("textDocument/definition", move |_| {
        FakeReply::After(Duration::from_millis(50), json!([target]))
    });
    let refs = json!([location(&program, 2, 16, 20)]);
    w.fake.respond("textDocument/references", move |_| {
        FakeReply::After(Duration::from_millis(50), refs.clone())
    });
    w.open_program();
    let commands = w.commands.clone();
    let path = program.to_string_lossy().into_owned();
    let agent = std::thread::spawn(move || {
        let refs = commands
            .invoke(
                workspace::EDITOR_FIND_REFERENCES,
                json!({"path": path, "line": 3, "column": 18}),
            )
            .unwrap();
        let def = commands
            .invoke(
                workspace::EDITOR_GO_TO_DEFINITION,
                json!({"path": path, "line": 3, "column": 18}),
            )
            .unwrap();
        let back = commands
            .invoke(workspace::NAVIGATION_BACK, json!({}))
            .unwrap();
        (refs, def, back)
    });
    let deadline = std::time::Instant::now() + T;
    while !agent.is_finished() {
        assert!(std::time::Instant::now() < deadline, "the agent timed out");
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (refs, def, back) = agent.join().unwrap();
    assert_eq!(refs["state"], "done");
    assert_eq!(refs["symbol"], "Main");
    assert_eq!(refs["total"], 1);
    assert_eq!(
        refs["references"][0],
        json!({"project": "App", "path": program.to_string_lossy(), "line": 3, "column": 17,
               "text": "static void Main() { }"})
    );
    assert_eq!(def["state"], "navigated");
    assert_eq!(def["navigated"]["path"], order.to_string_lossy().as_ref());
    assert_eq!(def["navigated"]["line"], 1);
    assert_eq!(def["navigated"]["column"], 7);
    assert_eq!(def["navigated"]["metadata"], false);
    assert_eq!(back["navigated"], true);
    assert_eq!(back["path"], program.to_string_lossy().as_ref());
    assert_eq!(
        (back["line"].clone(), back["column"].clone()),
        (json!(3), json!(18))
    );
}
