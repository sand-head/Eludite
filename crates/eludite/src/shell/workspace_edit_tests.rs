//! Headless GPUI tests of the workspace-edit applier (brief 0015) against the in-process fake `eludite-host`: edits to
//! an open buffer are one undo step and reach the host as `didChange`; edits to closed files are written on disk with
//! `workspace/didChangeWatchedFiles`; mixed edits; a stale document version or solution generation refuses the whole
//! edit; create, rename and delete file operations, with an open tab following a rename; the language server's
//! `workspace/applyEdit` through the host; and a summary that is the same each time the same edit is applied.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;
use std::time::Duration;

use eludite_lsp::lsp;
use serde_json::{Value, json};

use super::documents::{normalize_path, path_to_uri};
use super::tests::{PROGRAM, T, Ws, setup};
use super::workspace_edit::{ApplyOptions, ApplySummary};

impl Ws {
    /// Apply `edit` through the applier as the shell's features do, and wait for the summary.
    pub(super) fn apply_edit(&mut self, edit: Value, options: ApplyOptions) -> ApplySummary {
        let edit: lsp::WorkspaceEdit = serde_json::from_value(edit).unwrap();
        let slot: Rc<RefCell<Option<ApplySummary>>> = Rc::default();
        let out = slot.clone();
        self.shell.update_in(&mut self.vcx, |s, window, cx| {
            s.apply_workspace_edit(
                &edit,
                options,
                window,
                cx,
                Box::new(move |_, summary, _, _| *out.borrow_mut() = Some(summary)),
            )
        });
        self.wait("the applier", |_| slot.borrow().is_some());
        slot.take().unwrap()
    }

    pub(super) fn undo(&mut self, path: &Path) -> Value {
        self.bus(
            eludite_commands::workspace::EDITOR_UNDO,
            json!({"path": normalize_path(path).to_string_lossy()}),
        )
    }

    /// Invoke a command from the UI thread, as keys and menus do.
    pub(super) fn bus(&mut self, command: &str, args: Value) -> Value {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| {
                s.invoke(command, args, window, cx)
            })
            .unwrap_or_else(|e| panic!("{command}: {e}"))
    }
}

pub(super) fn text_edit(line: u32, start: u32, end: u32, text: &str) -> Value {
    json!({"range": {"start": {"line": line, "character": start}, "end": {"line": line, "character": end}},
           "newText": text})
}

fn doc_edit(path: &Path, version: Option<i32>, edits: Vec<Value>) -> Value {
    json!({"textDocument": {"uri": path_to_uri(path), "version": version}, "edits": edits})
}

#[gpui::test]
fn open_buffers_closed_files_and_mixed_edits(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let uri = path_to_uri(&program);
    // Unsent typing: the edit's ranges are in the text the server saw, so they still land on `Main`.
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.insert("// typed\n"))
    });
    w.vcx.run_until_parked();

    // `Main` is line 2, characters 16 to 20; `Order` in Order.cs is line 0, 6 to 11.
    let summary = w.apply_edit(
        json!({"documentChanges": [
            doc_edit(&program, Some(1), vec![text_edit(2, 16, 20, "Start"), text_edit(0, 6, 13, "Entry")]),
            doc_edit(&order, None, vec![text_edit(0, 6, 11, "Purchase")]),
        ]}),
        ApplyOptions {
            label: Some("Rename".into()),
            ..Default::default()
        },
    );
    assert!(summary.applied, "{summary:?}");
    assert_eq!(
        (
            summary.files,
            summary.edits,
            summary.open_documents,
            summary.files_on_disk
        ),
        (2, 3, 1, 1)
    );
    let program_path = normalize_path(&program).to_string_lossy().into_owned();
    let order_path = normalize_path(&order).to_string_lossy().into_owned();
    let mut paths = vec![program_path.clone(), order_path.clone()];
    paths.sort();
    assert_eq!(summary.paths, paths);
    assert_eq!(
        w.text(&view),
        "// typed\nclass Entry\n{\n    static void Start() { }\n}\n"
    );
    // The buffer is not saved; the closed file is, and the host hears about both.
    assert_eq!(std::fs::read_to_string(&program).unwrap(), PROGRAM);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        "class Purchase { }\n"
    );
    let change = w
        .fake
        .wait_for("textDocument/didChange", T, |p| {
            p["textDocument"]["uri"] == uri
        })
        .expect("didChange at once, without the debounce");
    assert_eq!(change.params["textDocument"]["version"], 2);
    let watched = w
        .fake
        .wait_for("workspace/didChangeWatchedFiles", T, |_| true)
        .expect("didChangeWatchedFiles");
    assert_eq!(
        watched.params["changes"],
        json!([{"uri": path_to_uri(&order), "type": 2}])
    );
    assert!(w.dirty(&program_path));
    assert!(w.status_text().starts_with("Rename: 3 edits in 2 files"));

    // One undo step for the whole edit in the buffer; the typing before it is another.
    let undone = w.undo(&program);
    assert_eq!(undone["applied"], true);
    assert_eq!(w.text(&view), format!("// typed\n{PROGRAM}"));
}

#[gpui::test]
fn stale_versions_and_generations_refuse_the_whole_edit(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let both = |version: Option<i32>| {
        json!({"documentChanges": [
            doc_edit(&order, None, vec![text_edit(0, 6, 11, "Purchase")]),
            doc_edit(&program, version, vec![text_edit(2, 16, 20, "Start")]),
        ]})
    };
    // The open document is at version 1; an edit for version 7 is refused, and the closed file is not touched.
    let summary = w.apply_edit(both(Some(7)), ApplyOptions::default());
    assert!(!summary.applied);
    assert!(
        summary.message.as_deref().unwrap().contains("version 7"),
        "{summary:?}"
    );
    assert_eq!(summary.edits, 0);
    assert_eq!(w.text(&view), PROGRAM);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        "class Order { }\n"
    );
    // The version a request was made at counts too.
    let id = normalize_path(&program).to_string_lossy().into_owned();
    let summary = w.apply_edit(
        both(None),
        ApplyOptions {
            versions: [(id.clone(), 0)].into(),
            ..Default::default()
        },
    );
    assert!(!summary.applied);
    // An edit computed under another solution generation.
    let summary = w.apply_edit(
        both(None),
        ApplyOptions {
            generation: Some(0),
            ..Default::default()
        },
    );
    assert!(!summary.applied);
    assert!(summary.message.unwrap().contains("generation 0"));
    // A range past the end of the open document.
    let summary = w.apply_edit(
        json!({"changes": {path_to_uri(&program): [text_edit(40, 0, 0, "x")]}}),
        ApplyOptions::default(),
    );
    assert!(!summary.applied);
    assert_eq!(w.text(&view), PROGRAM);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        "class Order { }\n"
    );
    // The current version applies.
    let summary = w.apply_edit(
        both(Some(1)),
        ApplyOptions {
            versions: [(id, 1)].into(),
            generation: Some(1),
            ..Default::default()
        },
    );
    assert!(summary.applied, "{summary:?}");
}

#[gpui::test]
fn file_operations_and_open_tabs_follow_them(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    let new = w.path("src/App/Models/Customer.cs");
    let renamed = w.path("src/App/Main.cs");
    let aspx = w.path("src/App/Default.aspx");

    // A dirty open document cannot be renamed.
    view.update(&mut w.vcx, |v, cx| v.update_editor(cx, |e| e.insert(" ")));
    w.vcx.run_until_parked();
    let rename = json!({"documentChanges": [
        {"kind": "rename", "oldUri": path_to_uri(&program), "newUri": path_to_uri(&renamed)}]});
    let summary = w.apply_edit(rename.clone(), ApplyOptions::default());
    assert!(!summary.applied);
    assert!(summary.message.unwrap().contains("unsaved changes"));
    assert!(program.exists() && !renamed.exists());
    w.undo(&program);
    w.bus(
        eludite_commands::workspace::EDITOR_SAVE,
        json!({"path": normalize_path(&program).to_string_lossy()}),
    );

    // Create, edit the new file, rename the open (saved) document, delete a file.
    let summary = w.apply_edit(
        json!({"documentChanges": [
            {"kind": "create", "uri": path_to_uri(&new)},
            doc_edit(&new, None, vec![text_edit(0, 0, 0, "class Customer { }\n")]),
            {"kind": "rename", "oldUri": path_to_uri(&program), "newUri": path_to_uri(&renamed)},
            {"kind": "delete", "uri": path_to_uri(&aspx)},
            doc_edit(&order, None, vec![text_edit(0, 6, 11, "Purchase")]),
        ]}),
        ApplyOptions::default(),
    );
    assert!(summary.applied, "{summary:?}");
    assert_eq!(
        (
            summary.created,
            summary.renamed,
            summary.deleted,
            summary.edits
        ),
        (1, 1, 1, 2)
    );
    assert_eq!(summary.files, 5);
    assert_eq!(
        std::fs::read_to_string(&new).unwrap(),
        "class Customer { }\n"
    );
    assert_eq!(std::fs::read_to_string(&renamed).unwrap(), PROGRAM);
    assert!(!program.exists() && !aspx.exists());
    let watched = w
        .fake
        .wait_for("workspace/didChangeWatchedFiles", T, |_| true)
        .unwrap();
    let mut events: Vec<(String, i64)> = watched.params["changes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| {
            (
                c["uri"].as_str().unwrap().to_owned(),
                c["type"].as_i64().unwrap(),
            )
        })
        .collect();
    events.sort();
    let mut expected = vec![
        (path_to_uri(&new), 1),
        (path_to_uri(&renamed), 1),
        (path_to_uri(&order), 2),
        (path_to_uri(&program), 3),
        (path_to_uri(&aspx), 3),
    ];
    expected.sort();
    assert_eq!(events, expected);
    // The renamed document's tab followed it: the old one closed (didClose), the new one opened (didOpen).
    let renamed_view = w.editor(&renamed);
    assert_eq!(w.text(&renamed_view), PROGRAM);
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.editor(&program).is_none())
    );
    w.fake
        .wait_for("textDocument/didClose", T, |p| {
            p["textDocument"]["uri"] == path_to_uri(&program)
        })
        .expect("didClose for the old path");
    w.fake
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["uri"] == path_to_uri(&renamed)
        })
        .expect("didOpen for the new path");
    // A failing operation changes nothing: the file exists already.
    let summary = w.apply_edit(
        json!({"documentChanges": [
            doc_edit(&order, None, vec![text_edit(0, 0, 5, "struct")]),
            {"kind": "create", "uri": path_to_uri(&new)}]}),
        ApplyOptions::default(),
    );
    assert!(!summary.applied);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        "class Purchase { }\n"
    );
}

#[gpui::test]
fn host_initiated_apply_edit_goes_through_the_applier(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let edit = json!({"changes": {path_to_uri(&program): [text_edit(0, 0, 0, "using System;\n")]}});
    let fake = w.fake.clone();
    let answer =
        std::thread::spawn(move || fake.apply_edit(json!({"label": "Add using", "edit": edit}), T));
    w.wait("the edit", |w| w.text(&view).starts_with("using System;"));
    let response = answer.join().unwrap().expect("an answer");
    assert_eq!(response["result"], json!({"applied": true}));
    assert_eq!(w.text(&view), format!("using System;\n{PROGRAM}"));
    assert!(w.status_text().starts_with("Add using: 1 edit in 1 file"));

    // Computed under an older generation (the solution reopened since): refused, nothing applied.
    let stale = json!({"changes": {path_to_uri(&program): [text_edit(0, 0, 0, "// no\n")]}});
    let fake = w.fake.clone();
    let answer = std::thread::spawn(move || {
        fake.apply_edit(json!({"edit": stale, "eluditeGeneration": 0}), T)
    });
    let response = loop {
        w.vcx.run_until_parked();
        if answer.is_finished() {
            break answer.join().unwrap().expect("an answer");
        }
        std::thread::sleep(Duration::from_millis(5));
    };
    assert_eq!(response["result"]["applied"], false);
    assert!(
        response["result"]["failureReason"]
            .as_str()
            .unwrap()
            .contains("generation 0")
    );
    assert!(!w.text(&view).contains("// no"));
}

#[gpui::test]
fn the_same_edit_gives_the_same_summary(cx: &mut gpui::TestAppContext) {
    let mut w = setup(cx);
    let (program, view) = w.open_program();
    let order = w.path("src/App/Models/Order.cs");
    // Replace `class` with `class` in both: applying it again finds the same text and gives the same summary.
    let edit = json!({"changes": {
        path_to_uri(&program): [text_edit(0, 0, 5, "class")],
        path_to_uri(&order): [text_edit(0, 0, 5, "class")]
    }});
    let first = w.apply_edit(edit.clone(), ApplyOptions::default());
    let second = w.apply_edit(edit, ApplyOptions::default());
    assert!(first.applied);
    assert_eq!(first, second);
    assert_eq!(w.text(&view), PROGRAM);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        "class Order { }\n"
    );
    // An empty edit applies nothing and says so.
    let empty = w.apply_edit(json!({}), ApplyOptions::default());
    assert!(empty.applied);
    assert_eq!((empty.files, empty.edits), (0, 0));
}

impl Ws {
    pub(super) fn status_text(&self) -> String {
        self.shell.read_with(&self.vcx, |s, _| {
            s.status()
                .get(eludite_ui::slots::STATE)
                .unwrap_or_default()
                .to_owned()
        })
    }
}
