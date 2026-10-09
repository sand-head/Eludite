//! Headless GPUI tests of rename, code actions and completion's additional edits in the shell (brief 0015) against the
//! in-process fake `eludite-host`: Ctrl+R, Ctrl+R opens the Rename dialog after prepareRename, the preview lists files
//! and lines, Enter applies through `eludite.editor.rename` as one undo step per document, Escape cancels; a
//! prepareRename `null` rejects; the light bulb appears after the caret rests and Ctrl+. opens its menu, grouped, from
//! which an action is resolved lazily and applied; nested actions expand, commands are unsupported, Fix All is not
//! offered; completion items' `additionalTextEdits` apply on accept, inline or after a resolve; stale answers are
//! dropped; agents drive all of it on the bus.

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eludite_commands::workspace;
use eludite_editor::{EditorView, LightbulbKind};
use eludite_lsp::fake::FakeReply;
use gpui::{Entity, Modifiers};
use serde_json::{Value, json};

use super::code_actions::LIGHTBULB_DEBOUNCE;
use super::documents::path_to_uri;
use super::rename::{NAME_REFUSED, NOT_RENAMEABLE, PreviewView, RENAME_PREVIEW_DEBOUNCE};
use super::tests::{PROGRAM, T, Ws, setup_with};
use super::workspace_edit_tests::text_edit;

/// A preview file as (name, open, [(line, text after)]).
type FileLines = (String, bool, Vec<(u32, String)>);

/// Order.cs for these tests: it calls `Program.Main`, so a rename of `Main` touches a closed file too.
const ORDER: &str = "class Order\n{\n    void M() { Program.Main(); }\n}\n";

fn capabilities() -> Value {
    json!({"completionProvider": {"triggerCharacters": ["."], "resolveProvider": true},
           "renameProvider": {"prepareProvider": true},
           "codeActionProvider": {"resolveProvider": true},
           "hoverProvider": true})
}

fn main_offset() -> usize {
    PROGRAM.find("Main").unwrap()
}

fn range(line: u32, start: u32, end: u32) -> Value {
    json!({"start": {"line": line, "character": start}, "end": {"line": line, "character": end}})
}

/// The rename the fake answers: `Main` in Program.cs (open) and in Order.cs (closed), renamed to the new name.
fn rename_edit(program: &Path, order: &Path, name: &str) -> Value {
    json!({"documentChanges": [
        {"textDocument": {"uri": path_to_uri(program), "version": null}, "edits": [text_edit(2, 16, 20, name)]},
        {"textDocument": {"uri": path_to_uri(order), "version": null}, "edits": [text_edit(2, 23, 27, name)]}
    ]})
}

impl Ws {
    fn order_with_call(&self) -> PathBuf {
        let order = self.path("src/App/Models/Order.cs");
        std::fs::write(&order, ORDER).unwrap();
        order
    }

    fn put_caret(&mut self, view: &Entity<EditorView>, offset: usize) {
        view.update(&mut self.vcx, |v, cx| {
            v.update_editor(cx, |e| e.set_caret(offset))
        });
        self.vcx.run_until_parked();
    }

    fn rename_state(&self) -> Value {
        self.shell.read_with(&self.vcx, |s, _| {
            serde_json::to_value(s.rename_output()).unwrap()
        })
    }

    fn dialog_open(&self) -> bool {
        self.shell
            .read_with(&self.vcx, |s, _| s.rename_dialog().is_some())
    }

    fn dialog_preview(&self) -> Option<PreviewView> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.rename_dialog().map(|d| d.read(cx).preview().clone())
        })
    }

    fn bulb(&self, view: &Entity<EditorView>) -> Option<(u32, LightbulbKind)> {
        view.read_with(&self.vcx, |v, _| v.lightbulb())
    }

    fn menu_titles(&self) -> Vec<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.code_action_menu()
                .map(|m| {
                    let m = m.read(cx);
                    m.visible()
                        .into_iter()
                        .map(|i| m.list().actions[i].title.clone())
                        .collect()
                })
                .unwrap_or_default()
        })
    }

    fn menu_selected(&self) -> Option<String> {
        self.shell.read_with(&self.vcx, |s, cx| {
            s.code_action_menu().map(|m| {
                let m = m.read(cx);
                m.list().actions[m.selected()].title.clone()
            })
        })
    }

    /// Run an agent's commands on another thread while the UI runs.
    pub(super) fn agent<R: Send + 'static>(&mut self, f: impl FnOnce() -> R + Send + 'static) -> R {
        let agent = std::thread::spawn(f);
        let deadline = Instant::now() + eludite_test_support::hang_bound(T);
        while !agent.is_finished() {
            assert!(Instant::now() < deadline, "the agent timed out");
            self.vcx.run_until_parked();
            std::thread::sleep(Duration::from_millis(5));
        }
        agent.join().unwrap()
    }
}

#[gpui::test]
fn rename_opens_the_dialog_previews_and_applies_as_one_undo_step(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/prepareRename", |_| {
            FakeReply::Result(range(2, 16, 20))
        });
    });
    let order = w.order_with_call();
    let (program, view) = w.open_program();
    let (p, o) = (program.clone(), order.clone());
    w.fake.respond("textDocument/rename", move |params| {
        FakeReply::Result(rename_edit(&p, &o, params["newName"].as_str().unwrap()))
    });
    w.put_caret(&view, main_offset() + 2);

    // Ctrl+R, Ctrl+R runs the command: prepareRename at the caret, then the dialog with the symbol's name.
    w.vcx.simulate_keystrokes("ctrl-r ctrl-r");
    w.wait("the Rename dialog", |w| w.dialog_open());
    assert!(w.audit().contains(&workspace::EDITOR_RENAME.to_owned()));
    let prepare = w.fake.received_params("textDocument/prepareRename");
    assert_eq!(prepare[0]["position"], json!({"line": 2, "character": 18}));
    assert_eq!(w.rename_state()["state"], "dialog");
    assert_eq!(w.rename_state()["symbol"], "Main");
    assert_eq!(w.dialog_preview(), Some(PreviewView::Empty));

    // Typing replaces the selected name; the preview comes after the debounce.
    w.vcx.simulate_keystrokes("shift-s t a r t");
    assert!(w.fake.received_params("textDocument/rename").is_empty());
    w.vcx.executor().advance_clock(RENAME_PREVIEW_DEBOUNCE);
    w.wait("the preview", |w| {
        matches!(w.dialog_preview(), Some(PreviewView::Ready { .. }))
    });
    let renames = w.fake.received_params("textDocument/rename");
    assert_eq!(renames.len(), 1, "one request after the debounce");
    assert_eq!(renames[0]["newName"], "Start");
    assert_eq!(renames[0]["position"], json!({"line": 2, "character": 16}));
    let Some(PreviewView::Ready { files, total }) = w.dialog_preview() else {
        unreachable!()
    };
    assert_eq!(total, 2);
    let summary: Vec<FileLines> = files
        .iter()
        .map(|f| {
            (
                f.path.file_name().unwrap().to_string_lossy().into_owned(),
                f.open,
                f.changes
                    .iter()
                    .map(|c| (c.line, c.after.clone()))
                    .collect(),
            )
        })
        .collect();
    assert_eq!(
        summary,
        [
            (
                "Order.cs".to_owned(),
                false,
                vec![(3, "void M() { Program.Start(); }".to_owned())]
            ),
            (
                "Program.cs".to_owned(),
                true,
                vec![(3, "static void Start() { }".to_owned())]
            ),
        ]
    );
    let out = w.rename_state();
    assert_eq!(out["state"], "preview");
    assert_eq!(out["total_edits"], 2);
    assert_eq!(
        out["files"][1]["changes"][0]["before"],
        "static void Main() { }"
    );

    // Enter applies through the bus: the open buffer (unsaved) and the closed file on disk.
    w.vcx.simulate_keystrokes("enter");
    w.wait("the rename", |w| w.rename_state()["state"] == "applied");
    assert!(!w.dialog_open());
    assert_eq!(
        w.text(&view),
        PROGRAM.replace("Main", "Start"),
        "the buffer"
    );
    assert_eq!(std::fs::read_to_string(&program).unwrap(), PROGRAM);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        ORDER.replace("Main", "Start")
    );
    let out = w.rename_state();
    assert_eq!(out["summary"]["files"], 2);
    assert_eq!(out["summary"]["open_documents"], 1);
    assert_eq!(out["summary"]["files_on_disk"], 1);
    let renames = w
        .audit()
        .into_iter()
        .filter(|c| c == workspace::EDITOR_RENAME)
        .count();
    assert_eq!(renames, 2, "Ctrl+R, Ctrl+R and the dialog's Apply");
    // One undo step for the rename in the buffer.
    w.undo(&program);
    assert_eq!(w.text(&view), PROGRAM);

    // F2 opens the dialog too; Escape cancels without a rename request.
    w.put_caret(&view, main_offset());
    w.vcx.simulate_keystrokes("f2");
    w.wait("the Rename dialog", |w| w.dialog_open());
    w.vcx.simulate_keystrokes("x escape");
    w.vcx.executor().advance_clock(RENAME_PREVIEW_DEBOUNCE * 2);
    w.vcx.run_until_parked();
    assert!(!w.dialog_open());
    assert_eq!(w.fake.received_params("textDocument/rename").len(), 1);
    assert_eq!(w.text(&view), PROGRAM);
}

#[gpui::test]
fn rename_rejections_are_reported(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/prepareRename", |p| {
            // Only `Main` can be renamed.
            if p["position"]["line"] == 2 {
                FakeReply::Result(range(2, 16, 20))
            } else {
                FakeReply::Result(Value::Null)
            }
        });
        fake.respond("textDocument/rename", |_| FakeReply::Result(Value::Null));
    });
    let (_, view) = w.open_program();
    // On `{` (line 2): prepareRename answers null; no dialog.
    w.put_caret(&view, PROGRAM.find('{').unwrap());
    w.vcx.simulate_keystrokes("ctrl-r ctrl-r");
    w.wait("the rejection", |w| w.rename_state()["state"] == "rejected");
    assert!(!w.dialog_open());
    assert_eq!(w.rename_state()["message"], NOT_RENAMEABLE);
    assert_eq!(w.status_text(), NOT_RENAMEABLE);
    assert!(w.fake.received_params("textDocument/rename").is_empty());

    // The server refuses the name: the dialog shows why and nothing changes.
    w.put_caret(&view, main_offset());
    w.vcx.simulate_keystrokes("f2");
    w.wait("the Rename dialog", |w| w.dialog_open());
    w.vcx.simulate_keystrokes("c l a s s");
    w.vcx.executor().advance_clock(RENAME_PREVIEW_DEBOUNCE);
    w.wait("the refusal", |w| {
        matches!(w.dialog_preview(), Some(PreviewView::Error(_)))
    });
    assert_eq!(
        w.dialog_preview(),
        Some(PreviewView::Error(NAME_REFUSED.into()))
    );
    assert_eq!(w.rename_state()["state"], "rejected");
    w.vcx.simulate_keystrokes("enter");
    w.vcx.run_until_parked();
    assert_eq!(w.text(&view), PROGRAM);
}

/// Roslyn-like code actions at `Main`: a refactoring, the preferred fix, its Fix All, a nested action and a command.
fn code_actions_answer() -> Value {
    let data = json!({"UniqueIdentifier": "u", "CodeActionPath": ["u"]});
    json!([
        {"title": "Extract method", "kind": "refactor.extract", "data": data},
        {"title": "Use primary constructor", "kind": "quickfix", "isPreferred": true, "data": data},
        {"title": "Fix All: Use primary constructor", "kind": "quickfix",
         "command": {"title": "Fix All", "command": "roslyn.client.fixAllCodeAction", "arguments": [data]}, "data": data},
        {"title": "Suppress or configure issues", "kind": "quickfix",
         "command": {"title": "Suppress", "command": "roslyn.client.nestedCodeAction",
                     "arguments": [{"NestedCodeActions": [{"title": "Suppress IDE0290", "kind": "quickfix", "data": data}]}]},
         "data": data},
        {"title": "Organize usings", "command": "x.organizeUsings"}
    ])
}

#[gpui::test]
fn the_light_bulb_appears_and_the_menu_applies_a_resolved_action(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/codeAction", |_| {
            FakeReply::Result(code_actions_answer())
        });
    });
    let (program, view) = w.open_program();
    let uri = path_to_uri(&program);
    w.fake.respond("codeAction/resolve", move |p| {
        let mut a = p.clone();
        a.as_object_mut().unwrap().remove("eluditeGeneration");
        a["edit"] = json!({"documentChanges": [{"textDocument": {"uri": uri, "version": null},
                            "edits": [text_edit(0, 13, 13, "(int x)")]}]});
        FakeReply::Result(a)
    });
    // A warning on Main's line goes with the request.
    w.fake.publish_diagnostics(
        &path_to_uri(&program),
        1,
        json!([{"range": range(2, 16, 20), "severity": 2, "code": "IDE0290", "message": "Use primary constructor"},
               {"range": range(0, 6, 13), "severity": 2, "code": "X", "message": "elsewhere"}]),
    );
    w.wait("the diagnostics", |w| !w.error_rows().is_empty());

    // The caret rests on `Main`: after the debounce, one request, and the bulb on its line (yellow: there are fixes).
    w.put_caret(&view, main_offset());
    assert!(w.fake.received_params("textDocument/codeAction").is_empty());
    w.vcx.executor().advance_clock(LIGHTBULB_DEBOUNCE);
    w.wait("the light bulb", |w| w.bulb(&view).is_some());
    assert_eq!(w.bulb(&view), Some((2, LightbulbKind::Fix)));
    let requests = w.fake.received_params("textDocument/codeAction");
    assert_eq!(requests.len(), 1);
    assert_eq!(requests[0]["context"]["triggerKind"], 2);
    assert_eq!(requests[0]["range"], range(2, 16, 16));
    assert_eq!(
        requests[0]["context"]["diagnostics"]
            .as_array()
            .unwrap()
            .iter()
            .map(|d| d["code"].clone())
            .collect::<Vec<_>>(),
        [json!("IDE0290")]
    );
    w.vcx.run_until_parked();
    assert!(w.shell.read_with(&w.vcx, |s, _| {
        s.lightbulb_timings().last().unwrap().shown.is_some()
    }));

    // Ctrl+. opens the menu from the bulb's answer: fixes, then refactorings, then the rest; no Fix All.
    w.vcx.simulate_keystrokes("ctrl-.");
    w.wait("the menu", |w| !w.menu_titles().is_empty());
    assert!(
        w.audit()
            .contains(&workspace::EDITOR_CODE_ACTIONS.to_owned())
    );
    assert_eq!(w.fake.received_params("textDocument/codeAction").len(), 1);
    assert_eq!(
        w.menu_titles(),
        [
            "Use primary constructor",
            "Suppress or configure issues",
            "Extract method",
            "Organize usings"
        ]
    );
    assert_eq!(
        w.menu_selected().as_deref(),
        Some("Use primary constructor")
    );
    let listed = w.bus(workspace::EDITOR_CODE_ACTIONS, json!({}));
    assert_eq!(listed["state"], "open");
    assert_eq!(listed["actions"][0]["group"], "fix");
    assert_eq!(listed["actions"][0]["preferred"], true);
    assert_eq!(listed["actions"][2]["title"], "Suppress IDE0290");
    assert_eq!(listed["actions"][2]["parent"], 2);

    // Right shows the nested action; Left goes back; Enter on the preferred fix resolves and applies it.
    w.vcx.simulate_keystrokes("down right");
    assert_eq!(w.menu_selected().as_deref(), Some("Suppress IDE0290"));
    assert!(w.menu_titles().contains(&"Suppress IDE0290".to_owned()));
    w.vcx.simulate_keystrokes("left up enter");
    w.wait("the applied action", |w| {
        w.text(&view).starts_with("class Program(int x)")
    });
    let resolved = w.fake.received_params("codeAction/resolve");
    assert_eq!(resolved.len(), 1);
    assert_eq!(resolved[0]["title"], "Use primary constructor");
    assert_eq!(resolved[0]["data"]["UniqueIdentifier"], "u");
    assert!(
        w.audit()
            .contains(&workspace::EDITOR_APPLY_CODE_ACTION.to_owned())
    );
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.code_action_menu().is_none())
    );
    // The list is gone once applied.
    let again = w.shell.update_in(&mut w.vcx, |s, window, cx| {
        s.invoke(
            workspace::EDITOR_APPLY_CODE_ACTION,
            json!({"index": 1}),
            window,
            cx,
        )
    });
    assert!(again.is_err());
    // One undo step.
    w.undo(&program);
    assert_eq!(w.text(&view), PROGRAM);

    // A click on the bulb opens the menu; a command-only action is unsupported; a nested one expands.
    w.put_caret(&view, main_offset() + 1);
    w.vcx.executor().advance_clock(LIGHTBULB_DEBOUNCE);
    w.wait("the light bulb", |w| w.bulb(&view).is_some());
    w.vcx.run_until_parked();
    let bulb = view
        .read_with(&w.vcx, |v, _| v.lightbulb_bounds())
        .expect("painted");
    w.vcx.simulate_click(bulb.center(), Modifiers::none());
    w.wait("the menu", |w| !w.menu_titles().is_empty());
    let unsupported = w.bus(
        workspace::EDITOR_APPLY_CODE_ACTION,
        json!({"title": "Organize usings"}),
    );
    assert_eq!(unsupported["state"], "unsupported");
    assert!(
        unsupported["message"]
            .as_str()
            .unwrap()
            .contains("x.organizeUsings")
    );
    let expanded = w.bus(
        workspace::EDITOR_APPLY_CODE_ACTION,
        json!({"title": "Suppress or configure issues"}),
    );
    assert_eq!(expanded["state"], "expanded");
    w.vcx.simulate_keystrokes("escape");
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.code_action_menu().is_none())
    );
    assert_eq!(w.text(&view), PROGRAM);
    // An action with an edit already is applied without a resolve; a disabled one is refused.
    let calls = w.fake.received_params("codeAction/resolve").len();
    w.fake.respond("textDocument/codeAction", |_| {
        FakeReply::Result(json!([
            {"title": "Add braces", "kind": "quickfix", "edit": {"changes": {}}},
            {"title": "Inline", "kind": "refactor.inline", "disabled": {"reason": "Not here."}}
        ]))
    });
    w.put_caret(&view, main_offset() + 2);
    w.vcx.executor().advance_clock(LIGHTBULB_DEBOUNCE);
    w.wait("the new list", |w| {
        w.shell.read_with(&w.vcx, |s, _| {
            s.code_actions
                .list
                .as_ref()
                .is_some_and(|l| l.actions[0].title == "Add braces")
        })
    });
    let applied = w.bus(
        workspace::EDITOR_APPLY_CODE_ACTION,
        json!({"title": "Add braces"}),
    );
    assert_eq!(applied["state"], "applied", "{applied}");
    assert_eq!(w.fake.received_params("codeAction/resolve").len(), calls);
}

#[gpui::test]
fn additional_text_edits_apply_on_completion_accept(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/completion", |p| {
            let line = p["position"]["line"].clone();
            let end = p["position"]["character"].as_u64().unwrap();
            let edit = |label: &str| {
                json!({"range": {"start": {"line": line, "character": end - 2}, "end": {"line": line, "character": end}},
                       "newText": label})
            };
            FakeReply::Result(json!({"isIncomplete": false, "items": [
                {"label": "List", "kind": 7, "sortText": "a", "textEdit": edit("List"),
                 "additionalTextEdits": [text_edit(0, 0, 0, "using System.Collections.Generic;\n")]},
                {"label": "Stopwatch", "kind": 7, "sortText": "b", "textEdit": edit("Stopwatch"), "data": {"id": 2}}
            ]}))
        });
        fake.respond("completionItem/resolve", |p| {
            let mut item = p.clone();
            item.as_object_mut().unwrap().remove("eluditeGeneration");
            item["additionalTextEdits"] =
                json!([text_edit(0, 0, 0, "using System.Diagnostics;\n")]);
            FakeReply::After(Duration::from_millis(20), item)
        });
    });
    let (program, view) = w.open_program();
    let body = PROGRAM.find("{ }").unwrap() + 2;
    let complete = |w: &mut Ws, typed: &str| {
        view.update(&mut w.vcx, |v, cx| {
            v.update_editor(cx, |e| {
                e.set_caret(body);
                e.insert(typed);
            })
        });
        w.vcx.run_until_parked();
        let out = w.bus(workspace::EDITOR_COMPLETE, json!({}));
        assert_eq!(out["state"], "loading");
        w.wait("the list", |w| {
            view.read_with(&w.vcx, |v, _| v.completion().is_some_and(|c| c.visible))
        });
    };

    // The item carries its edits: applied with the commit, one undo step for both.
    complete(&mut w, "Li");
    let accepted = w.bus(
        workspace::EDITOR_ACCEPT_COMPLETION,
        json!({"label": "List"}),
    );
    assert_eq!(accepted["accepted"], true);
    assert_eq!(
        w.text(&view),
        format!(
            "using System.Collections.Generic;\n{}",
            PROGRAM.replace("{ }", "{ List}")
        )
    );
    // The using joined the commit's undo step (which also holds the typing just before it).
    w.undo(&program);
    let text = w.text(&view);
    assert!(!text.contains("using") && !text.contains("List"), "{text}");

    // The item is not resolved yet: committing resolves it, then its using is added, joining the commit's undo step.
    complete(&mut w, "St");
    w.bus(
        workspace::EDITOR_ACCEPT_COMPLETION,
        json!({"label": "Stopwatch"}),
    );
    w.wait("the using", |w| {
        w.text(&view).starts_with("using System.Diagnostics;")
    });
    assert_eq!(
        w.text(&view),
        format!(
            "using System.Diagnostics;\n{}",
            PROGRAM.replace("{ }", "{ Stopwatch}")
        )
    );
    assert!(!w.fake.received_params("completionItem/resolve").is_empty());
    w.undo(&program);
    let text = w.text(&view);
    assert!(
        !text.contains("using") && !text.contains("Stopwatch"),
        "{text}"
    );
}

#[gpui::test]
fn stale_rename_and_code_action_answers_are_dropped(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.set_ignore_cancel(true);
        fake.respond("textDocument/codeAction", |_| {
            FakeReply::After(Duration::from_millis(150), code_actions_answer())
        });
        fake.respond("textDocument/prepareRename", |_| {
            FakeReply::After(Duration::from_millis(150), range(2, 16, 20))
        });
    });
    let (_, view) = w.open_program();

    // The light bulb's request is answered after the document changed: dropped, no bulb.
    w.put_caret(&view, main_offset());
    w.vcx.executor().advance_clock(LIGHTBULB_DEBOUNCE);
    w.wait("the request", |w| {
        !w.fake.received_params("textDocument/codeAction").is_empty()
    });
    view.update(&mut w.vcx, |v, cx| v.update_editor(cx, |e| e.insert("x")));
    w.shell.update(&mut w.vcx, |s, cx| {
        let id = s.active_document().unwrap();
        s.flush_change(&id, cx)
    });
    // The edit canceled the request; its late answer (the fake ignores cancels) is never shown.
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(w.bulb(&view), None);
    assert!(
        w.shell
            .read_with(&w.vcx, |s, _| s.code_actions.list.is_none())
    );
    let cancels = w.fake.received_params("$/cancelRequest");
    assert!(!cancels.is_empty(), "the request was canceled");

    // A prepareRename answered after the solution reopened (a new generation): no dialog, the rename failed.
    w.put_caret(&view, main_offset());
    w.vcx.simulate_keystrokes("f2");
    w.wait("the request", |w| {
        !w.fake
            .received_params("textDocument/prepareRename")
            .is_empty()
    });
    w.open_solution();
    let deadline = Instant::now() + Duration::from_millis(400);
    while Instant::now() < deadline {
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(!w.dialog_open());
    assert_eq!(w.rename_state()["state"], "failed");
}

#[gpui::test]
fn agents_rename_and_apply_code_actions_on_the_bus(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/prepareRename", |_| {
            FakeReply::After(Duration::from_millis(20), range(2, 16, 20))
        });
        fake.respond("textDocument/codeAction", |_| {
            FakeReply::After(Duration::from_millis(20), code_actions_answer())
        });
    });
    let order = w.order_with_call();
    let (program, view) = w.open_program();
    let (p, o) = (program.clone(), order.clone());
    w.fake.respond("textDocument/rename", move |params| {
        FakeReply::After(
            Duration::from_millis(20),
            rename_edit(&p, &o, params["newName"].as_str().unwrap()),
        )
    });
    let uri = path_to_uri(&program);
    w.fake.respond("codeAction/resolve", move |p| {
        let mut a = p.clone();
        a.as_object_mut().unwrap().remove("eluditeGeneration");
        a["edit"] = json!({"changes": {uri.clone(): [text_edit(0, 13, 13, "(int x)")]}});
        FakeReply::After(Duration::from_millis(20), a)
    });
    let commands = w.commands.clone();
    let path = program.to_string_lossy().into_owned();
    let (preview, renamed, listed, applied, edited) = w.agent(move || {
        let preview = commands
            .invoke(
                workspace::EDITOR_RENAME,
                json!({"path": path, "line": 3, "column": 17, "new_name": "Run", "apply": false}),
            )
            .unwrap();
        let renamed = commands
            .invoke(
                workspace::EDITOR_RENAME,
                json!({"path": path, "line": 3, "column": 17, "new_name": "Start"}),
            )
            .unwrap();
        let listed = commands
            .invoke(
                workspace::EDITOR_CODE_ACTIONS,
                json!({"path": path, "line": 3, "column": 17}),
            )
            .unwrap();
        let applied = commands
            .invoke(
                workspace::EDITOR_APPLY_CODE_ACTION,
                json!({"title": "Use primary constructor"}),
            )
            .unwrap();
        let edited = commands
            .invoke(
                workspace::WORKSPACE_APPLY_EDIT,
                json!({"label": "Agent edit", "edit": {"changes": {path_to_uri(Path::new(&path)): [
                    {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// agent\n"}]}}}),
            )
            .unwrap();
        (preview, renamed, listed, applied, edited)
    });
    assert_eq!(preview["state"], "preview", "{preview}");
    assert_eq!(preview["new_name"], "Run");
    assert_eq!(preview["files"].as_array().unwrap().len(), 2);
    assert_eq!(renamed["state"], "applied", "{renamed}");
    assert_eq!(renamed["summary"]["edits"], 2);
    assert_eq!(
        std::fs::read_to_string(&order).unwrap(),
        ORDER.replace("Main", "Start")
    );
    assert_eq!(listed["state"], "open", "{listed}");
    assert_eq!(listed["actions"][0]["title"], "Use primary constructor");
    assert_eq!(applied["state"], "applied", "{applied}");
    assert_eq!(applied["summary"]["edits"], 1);
    assert_eq!(edited["state"], "applied", "{edited}");
    assert_eq!(
        w.text(&view),
        format!(
            "// agent\n{}",
            PROGRAM
                .replace("Main", "Start")
                .replace("class Program", "class Program(int x)")
        )
    );
    // No dialog or menu was left open for the agent.
    assert!(!w.dialog_open());
}
