//! Headless GPUI tests of brief 0069 in the editor: snippet sessions (Tab and Shift+Tab between stops, linked stops
//! typed together, Enter and Escape, leaving the stop), Tab-Tab, and completion commit characters (commit then insert,
//! space only on a hard selection, never on a soft one).

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use eludite_editor::{
    Buffer, CompletionEdit, CompletionItem, CompletionKind, CompletionSource, CompletionTrigger,
    EditorEvent, EditorView, key_bindings,
};
use gpui::{AppContext as _, Entity, Focusable as _, TestAppContext, VisualTestContext};

struct Ed {
    view: Entity<EditorView>,
    cx: VisualTestContext,
    events: Rc<RefCell<Vec<EditorEvent>>>,
}

fn open(cx: &mut TestAppContext, text: &str) -> Ed {
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let text = text.to_owned();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let view = cx.new(|cx| {
                let mut buffer = Buffer::new(&text);
                buffer.set_group_interval(Duration::ZERO);
                EditorView::new(buffer, None, cx)
            });
            window.focus(&view.focus_handle(cx), cx);
            view
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let view = window.root(&mut vcx).unwrap();
    let events: Rc<RefCell<Vec<EditorEvent>>> = Rc::default();
    let sink = events.clone();
    vcx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &EditorEvent, _| {
            sink.borrow_mut().push(e.clone())
        })
        .detach()
    });
    vcx.run_until_parked();
    Ed {
        view,
        cx: vcx,
        events,
    }
}

impl Ed {
    fn text(&self) -> String {
        self.view.read_with(&self.cx, |v, _| v.editor().text())
    }

    fn selections(&self) -> Vec<String> {
        self.view.read_with(&self.cx, |v, _| {
            v.editor()
                .selections()
                .iter()
                .map(|s| v.editor().buffer().text_for_range(s.range()))
                .collect()
        })
    }

    fn caret(&self) -> usize {
        self.view
            .read_with(&self.cx, |v, _| v.editor().primary_selection().head)
    }

    fn in_session(&self) -> bool {
        self.view
            .read_with(&self.cx, |v, _| v.snippet_session().is_some())
    }

    fn insert(&mut self, at: usize, snippet: &str) {
        self.view.update(&mut self.cx, |v, cx| {
            v.insert_snippet(at..at, snippet, &[], cx);
        });
        self.cx.run_until_parked();
    }
}

#[gpui::test]
fn a_session_moves_between_stops_and_types_linked_stops_together(cx: &mut TestAppContext) {
    let mut ed = open(cx, "    \n");
    ed.insert(4, "private ${1:int} ${2:field};\npublic $1 Value => $2;$0");
    // The second line is indented like the first; stop 1 is selected at both of its places.
    assert_eq!(
        ed.text(),
        "    private int field;\n    public int Value => field;\n"
    );
    assert_eq!(ed.selections(), ["int", "int"]);
    assert!(ed.in_session());
    ed.cx.simulate_input("long");
    assert_eq!(
        ed.text(),
        "    private long field;\n    public long Value => field;\n"
    );
    ed.cx.simulate_keystrokes("tab");
    assert_eq!(ed.selections(), ["field", "field"]);
    ed.cx.simulate_input("_v");
    assert_eq!(
        ed.text(),
        "    private long _v;\n    public long Value => _v;\n"
    );
    ed.cx.simulate_keystrokes("shift-tab");
    assert_eq!(ed.selections(), ["long", "long"]);
    // Tab past the last field ends the session at $0.
    ed.cx.simulate_keystrokes("tab tab");
    assert!(!ed.in_session());
    assert_eq!(ed.caret(), ed.text().len() - 1);
}

#[gpui::test]
fn enter_escape_and_leaving_the_stop_end_the_session(cx: &mut TestAppContext) {
    let mut ed = open(cx, "\n");
    ed.insert(0, "if (${1:true})\n{\n    $0\n}");
    assert_eq!(ed.selections(), ["true"]);
    // Enter: the caret goes to $0 and no line break is typed.
    ed.cx.simulate_keystrokes("enter");
    assert!(!ed.in_session());
    assert_eq!(ed.caret(), "if (true)\n{\n    ".len());
    assert_eq!(ed.text(), "if (true)\n{\n    \n}\n");

    let mut ed = open(cx, "\n");
    ed.insert(0, "M(${1:a}, ${2:b})");
    ed.cx.simulate_keystrokes("escape");
    assert!(!ed.in_session());
    assert_eq!(ed.selections(), ["a"], "Escape leaves the selection");

    let mut ed = open(cx, "x\n");
    ed.insert(0, "M(${1:a}, ${2:b})");
    ed.cx.simulate_keystrokes("ctrl-end");
    assert!(!ed.in_session(), "the caret left the stop");
    // A snippet with only $0 places the caret, no session.
    let mut ed = open(cx, "\n");
    ed.insert(0, "Console.WriteLine($0);");
    assert!(!ed.in_session());
    assert_eq!(ed.caret(), "Console.WriteLine(".len());
}

#[gpui::test]
fn tab_expands_a_shortcut_before_the_caret(cx: &mut TestAppContext) {
    let mut ed = open(cx, "class Foo\n{\n    ctor\n}\n");
    ed.view.update(&mut ed.cx, |v, _| {
        v.set_snippet_expander(Some(Rc::new(|word: &str, before: &str| {
            (word == "ctor").then(|| {
                let class = before.split_whitespace().nth(1).unwrap_or("C").to_owned();
                format!("public {class}()\n{{\n    $0\n}}")
            })
        })))
    });
    let at = ed.text().find("ctor").unwrap() + 4;
    ed.view
        .update(&mut ed.cx, |v, cx| v.update_editor(cx, |e| e.set_caret(at)));
    ed.cx.simulate_keystrokes("tab");
    assert_eq!(
        ed.text(),
        "class Foo\n{\n    public Foo()\n    {\n        \n    }\n}\n"
    );
    // Not a shortcut: Tab indents.
    let mut ed = open(cx, "nope\n");
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_snippet_expander(Some(Rc::new(|_: &str, _: &str| None)));
        v.update_editor(cx, |e| e.set_caret(4))
    });
    ed.cx.simulate_keystrokes("tab");
    assert_eq!(ed.text(), "nope    \n");
}

fn open_list(ed: &mut Ed, word_start: usize, items: Vec<CompletionItem>) -> u64 {
    let request = ed.view.update(&mut ed.cx, |v, cx| {
        v.open_completion(Some(CompletionTrigger::Character('.')), cx)
    });
    assert_eq!(request.word_start, word_start);
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id,
            items,
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    ed.cx.run_until_parked();
    request.id
}

fn item(label: &str, commit: Option<&[char]>) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind: CompletionKind::Method,
        resolved: true,
        commit_characters: commit.map(|c| std::sync::Arc::new(c.to_vec())),
        ..Default::default()
    }
}

#[gpui::test]
fn commit_characters_commit_the_item_then_insert_the_character(cx: &mut TestAppContext) {
    let mut ed = open(cx, "Console.\n");
    ed.view
        .update(&mut ed.cx, |v, cx| v.update_editor(cx, |e| e.set_caret(8)));
    open_list(
        &mut ed,
        8,
        vec![item("WriteLine", None), item("Write", Some(&[]))],
    );
    ed.events.borrow_mut().clear();
    // Nothing typed yet: `(` does not commit (no hard selection).
    ed.cx.simulate_input("WriteL");
    ed.cx.run_until_parked();
    ed.cx.simulate_input("(");
    ed.cx.run_until_parked();
    assert_eq!(ed.text(), "Console.WriteLine(\n");
    assert!(
        ed.events
            .borrow()
            .iter()
            .any(|e| matches!(e, EditorEvent::CompletionCommitted(a) if a.label == "WriteLine")),
        "the owner is told, to apply the item's additional edits"
    );

    // An item whose characters are empty commits only with Tab or Enter.
    let mut ed = open(cx, "Console.\n");
    ed.view
        .update(&mut ed.cx, |v, cx| v.update_editor(cx, |e| e.set_caret(8)));
    open_list(&mut ed, 8, vec![item("Write", Some(&[]))]);
    ed.cx.simulate_input("Wr");
    ed.cx.run_until_parked();
    ed.cx.simulate_input(";");
    assert_eq!(ed.text(), "Console.Wr;\n");

    // Space commits a hard selection, never in suggestion mode.
    let mut ed = open(cx, "Console.\n");
    ed.view
        .update(&mut ed.cx, |v, cx| v.update_editor(cx, |e| e.set_caret(8)));
    let id = open_list(&mut ed, 8, vec![item("Write", None)]);
    ed.view.update(&mut ed.cx, |v, _| {
        v.set_completion_suggestion_mode(id, true)
    });
    ed.cx.simulate_input("Wr");
    ed.cx.run_until_parked();
    ed.cx.simulate_input(" ");
    assert_eq!(ed.text(), "Console.Wr \n");
}

#[gpui::test]
fn a_snippet_item_starts_a_session_on_its_placeholders(cx: &mut TestAppContext) {
    let mut ed = open(cx, "Console.\n");
    ed.view
        .update(&mut ed.cx, |v, cx| v.update_editor(cx, |e| e.set_caret(8)));
    let range = ed.view.read_with(&ed.cx, |v, _| {
        let b = v.editor().buffer();
        b.anchor_before(8)..b.anchor_after(8)
    });
    open_list(
        &mut ed,
        8,
        vec![CompletionItem {
            label: "WriteLine".into(),
            kind: CompletionKind::Method,
            resolved: true,
            snippet: true,
            edit: Some(CompletionEdit {
                range,
                new_text: "WriteLine(${1:value})$0".into(),
            }),
            ..Default::default()
        }],
    );
    let accepted = ed
        .view
        .update(&mut ed.cx, |v, cx| v.accept_completion(None, cx))
        .unwrap();
    assert_eq!(accepted.text, "WriteLine(value)");
    assert_eq!(ed.text(), "Console.WriteLine(value)\n");
    assert_eq!(ed.selections(), ["value"]);
    ed.cx.simulate_input("x");
    ed.cx.simulate_keystrokes("tab");
    assert_eq!(ed.text(), "Console.WriteLine(x)\n");
    assert_eq!(ed.caret(), "Console.WriteLine(x)".len());
}
