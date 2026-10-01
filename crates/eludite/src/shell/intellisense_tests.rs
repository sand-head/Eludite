//! Headless GPUI tests of IntelliSense in the shell (brief 0013) against the in-process fake `eludite-host`: the
//! completion list opens on a trigger, filters as the user types, cancels the previous request, drops a stale
//! answer, commits with Tab through `eludite.editor.accept_completion`, resolves documentation lazily, falls back to
//! the syntax tree's identifiers while the solution loads and swaps to the server's items; Quick Info appears after
//! the hover delay and dismisses; Parameter Info tracks the active parameter; agents drive all of it on the bus.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use eludite_commands::workspace;
use eludite_editor::intellisense::HOVER_DELAY;
use eludite_editor::{CompletionSource, EditorView};
use eludite_lsp::fake::FakeReply;
use gpui::{Entity, Modifiers, point, px};
use serde_json::{Value, json};

use super::tests::{PROGRAM, T, Ws, setup_with};

/// Roslyn-like capabilities: `.` triggers completion, items resolve, `(` and `,` trigger Parameter Info.
fn capabilities() -> Value {
    json!({"completionProvider": {"triggerCharacters": [".", "<"], "resolveProvider": true},
           "signatureHelpProvider": {"triggerCharacters": ["(", ","], "retriggerCharacters": [")"]},
           "hoverProvider": true})
}

/// A completion list for `labels` whose items replace `[start, position)` on the request's line.
fn list(params: &Value, start: u32, labels: &[(&str, u32)]) -> Value {
    let line = params["position"]["line"].clone();
    let end = params["position"]["character"].clone();
    let items: Vec<Value> = labels
        .iter()
        .map(|(label, kind)| {
            json!({"label": label, "kind": kind, "sortText": label,
                   "textEdit": {"range": {"start": {"line": line, "character": start},
                                          "end": {"line": line, "character": end}},
                                "newText": label}})
        })
        .collect();
    json!({"isIncomplete": false, "items": items})
}

const MEMBERS: [(&str, u32); 4] = [("WriteLine", 2), ("Write", 2), ("Out", 10), ("ReadLine", 2)];

/// Program.cs's `Main` body, after `{ `.
fn body() -> usize {
    PROGRAM.find("{ }").unwrap() + 2
}

impl Ws {
    fn completion(&self, view: &Entity<EditorView>) -> Option<eludite_editor::CompletionSnapshot> {
        view.read_with(&self.vcx, |v, _| v.completion())
    }

    fn labels(&self, view: &Entity<EditorView>) -> Vec<String> {
        self.completion(view)
            .filter(|c| c.visible)
            .map(|c| c.items.into_iter().map(|i| i.0).collect())
            .unwrap_or_default()
    }

    fn insert_at(&mut self, view: &Entity<EditorView>, offset: usize, text: &str) {
        view.update(&mut self.vcx, |v, cx| {
            v.update_editor(cx, |e| {
                e.set_caret(offset);
                e.insert(text);
            })
        });
        self.vcx.run_until_parked();
    }

    /// Run a command from the UI thread, as a key or menu item does.
    fn run_cmd(&mut self, command: &str, args: Value) -> Value {
        self.shell
            .update_in(&mut self.vcx, |s, window, cx| {
                s.invoke(command, args, window, cx)
            })
            .unwrap()
    }

    fn requests(&self, method: &str) -> usize {
        self.fake.received_params(method).len()
    }

    /// Open Program.cs with a loaded solution and `Console` typed into `Main`, caret after it.
    fn program_with_console(&mut self) -> Entity<EditorView> {
        let (_, view) = self.open_program();
        self.insert_at(&view, body(), "Console");
        view
    }
}

#[gpui::test]
fn completion_opens_on_trigger_filters_and_commits_with_tab(cx: &mut gpui::TestAppContext) {
    // Items replace the word after `Console.`, wherever the caret is in it.
    let after_dot = (body() - PROGRAM.find("    static").unwrap() + "Console.".len()) as u32;
    let mut w = setup_with(cx, move |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/completion", move |p| {
            FakeReply::Result(list(p, after_dot, &MEMBERS))
        });
    });
    let view = w.program_with_console();
    let dot = body() + "Console".len();

    // `.` triggers a request (trigger kind 2: the server lists `.`), sent after the text it needs.
    w.vcx.simulate_input(".");
    w.wait("the completion list", |w| !w.labels(&view).is_empty());
    let request = w
        .fake
        .wait_for("textDocument/completion", T, |_| true)
        .unwrap();
    assert_eq!(
        request.params["context"],
        json!({"triggerKind": 2, "triggerCharacter": "."})
    );
    let line_start = PROGRAM.find("    static").unwrap();
    assert_eq!(
        request.params["position"],
        json!({"line": 2, "character": dot + 1 - line_start})
    );
    let change = w
        .fake
        .received()
        .into_iter()
        .rposition(|r| r.method == "textDocument/didChange")
        .unwrap();
    let asked = w
        .fake
        .received()
        .into_iter()
        .position(|r| r.method == "textDocument/completion")
        .unwrap();
    assert!(change < asked, "didChange went first");
    assert_eq!(w.labels(&view), ["Out", "ReadLine", "Write", "WriteLine"]);
    let audit = w.audit();
    assert!(
        audit.contains(&workspace::EDITOR_COMPLETE.to_owned()),
        "typing ran the command"
    );

    // Typing filters (fuzzy) without asking again: the list is complete.
    w.vcx.simulate_input("Wr");
    w.wait("the filter", |w| w.labels(&view) == ["Write", "WriteLine"]);
    w.vcx.simulate_input("L");
    w.wait("the filter", |w| w.labels(&view) == ["WriteLine"]);
    assert_eq!(w.requests("textDocument/completion"), 1);
    // The bus reports what is shown.
    let shown = w.run_cmd(workspace::EDITOR_COMPLETE, json!({}));
    assert_eq!(
        shown["state"], "open",
        "the list stays while it is asked again"
    );
    assert_eq!(shown["filter"], "WrL");
    assert_eq!(shown["selected"], "WriteLine");
    assert_eq!(
        shown["items"],
        json!([{"label": "WriteLine", "kind": "method"}])
    );
    w.wait("the new answer", |w| {
        w.requests("textDocument/completion") == 2
    });

    // Tab commits through eludite.editor.accept_completion with the item's edit.
    w.vcx.simulate_keystrokes("tab");
    assert!(
        w.text(&view).contains("{ Console.WriteLine}"),
        "{}",
        w.text(&view)
    );
    assert_eq!(
        w.audit().last().map(String::as_str),
        Some(workspace::EDITOR_ACCEPT_COMPLETION)
    );
    assert!(w.completion(&view).is_none());

    // Ctrl+Space runs the command; Escape dismisses.
    w.vcx.simulate_keystrokes("ctrl-space");
    w.wait("the list", |w| !w.labels(&view).is_empty());
    assert_eq!(
        w.audit().last().map(String::as_str),
        Some(workspace::EDITOR_COMPLETE)
    );
    w.vcx.simulate_keystrokes("escape");
    assert!(w.completion(&view).is_none());
}

#[gpui::test]
fn each_keystroke_cancels_the_previous_request_and_stale_answers_are_dropped(
    cx: &mut gpui::TestAppContext,
) {
    let calls = Arc::new(AtomicUsize::new(0));
    let counter = calls.clone();
    let mut w = setup_with(cx, move |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/completion", move |p| {
            let start = p["position"]["character"].as_u64().unwrap() as u32;
            match counter.fetch_add(1, Ordering::SeqCst) {
                // The first request is slow; the second answers at once with other items.
                0 => FakeReply::After(Duration::from_millis(300), list(p, start, &[("Stale", 1)])),
                _ => FakeReply::Result(list(p, start - 1, &[("WriteLine", 2), ("Write", 2)])),
            }
        });
    });
    let view = w.program_with_console();
    // The host has already sent the first answer when the cancel arrives (it ignores cancels).
    w.fake.set_ignore_cancel(true);
    w.vcx.simulate_input(".");
    w.fake
        .wait_for("textDocument/completion", T, |_| true)
        .unwrap();
    // A keystroke while the request is in flight cancels it and asks again.
    w.vcx.simulate_input("W");
    let second = w
        .fake
        .wait_for("textDocument/completion", T, |p| {
            p["context"]["triggerKind"] == 1
        })
        .unwrap();
    let first = &w
        .fake
        .received()
        .into_iter()
        .find(|r| r.method == "textDocument/completion")
        .unwrap();
    let cancel = w
        .fake
        .wait_for("$/cancelRequest", T, |_| true)
        .expect("the first request was canceled");
    assert_eq!(Some(&cancel.params["id"]), first.id.as_ref());
    assert_ne!(first.id, second.id);
    w.wait("the second answer", |w| {
        w.labels(&view) == ["Write", "WriteLine"]
    });
    // The first answer arrives late, for an older version: it is never shown.
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        w.vcx.run_until_parked();
        assert!(
            !w.labels(&view).contains(&"Stale".to_owned()),
            "a stale answer was shown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(w.fake.inflight(), 0, "the late answer was sent anyway");

    // A new solution generation drops an answer computed under the old one.
    calls.store(0, Ordering::SeqCst);
    w.vcx.simulate_keystrokes("escape");
    w.vcx.simulate_input(".");
    w.fake
        .wait_for("textDocument/completion", T, |p| {
            p["eluditeGeneration"] == 1
        })
        .unwrap();
    let sln = w.path("App.slnx");
    w.commands
        .invoke(
            workspace::SOLUTION_OPEN,
            json!({"path": sln.to_string_lossy()}),
        )
        .unwrap();
    w.wait("generation 2", |w| w.fake.generation() == 2);
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline {
        w.vcx.run_until_parked();
        assert!(
            !w.labels(&view).contains(&"Stale".to_owned()),
            "an answer from generation 1 was shown"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[gpui::test]
fn the_selected_item_resolves_its_documentation_lazily(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/completion", |p| {
            let start = p["position"]["character"].as_u64().unwrap() as u32;
            let mut l = list(p, start, &MEMBERS);
            for item in l["items"].as_array_mut().unwrap() {
                item["data"] = json!({"label": item["label"]});
            }
            FakeReply::Result(l)
        });
        fake.respond("completionItem/resolve", |item| {
            let mut out = item.clone();
            out.as_object_mut().unwrap().remove("eluditeGeneration");
            out["documentation"] = json!({"kind": "markdown", "value": format!("Docs for `{}`.", item["label"].as_str().unwrap())});
            out["detail"] = json!(format!("void Console.{}()", item["label"].as_str().unwrap()));
            FakeReply::Result(out)
        });
    });
    let view = w.program_with_console();
    w.vcx.simulate_input(".");
    w.wait("the list", |w| !w.labels(&view).is_empty());
    let resolved = w
        .fake
        .wait_for("completionItem/resolve", T, |_| true)
        .expect("the selected item is resolved");
    assert_eq!(resolved.params["label"], "Out");
    assert_eq!(resolved.params["data"], json!({"label": "Out"}));
    w.wait("the documentation", |w| {
        w.completion(&view)
            .and_then(|c| c.items.first().cloned())
            .is_some_and(|i| i.2.as_deref() == Some("void Console.Out()"))
    });
    // Only the selected item was resolved; moving the selection resolves the next one.
    assert_eq!(w.requests("completionItem/resolve"), 1);
    w.vcx.simulate_keystrokes("down");
    w.fake
        .wait_for("completionItem/resolve", T, |p| p["label"] == "ReadLine")
        .expect("the next item is resolved");
    assert_eq!(w.requests("completionItem/resolve"), 2);
    assert!(w.vcx.debug_bounds("completion-list").is_some());
}

#[gpui::test]
fn falls_back_to_syntax_identifiers_while_loading_and_swaps_to_server_items(
    cx: &mut gpui::TestAppContext,
) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("starting", None);
        fake.set_hold_load(true);
        fake.respond("textDocument/completion", |p| {
            let start = p["position"]["character"].as_u64().unwrap() as u32 - 1;
            FakeReply::After(
                Duration::from_millis(400),
                list(p, start, &[("Main", 2), ("Math", 7)]),
            )
        });
    });
    w.open_solution();
    let project = w.path("src/App/App.csproj");
    w.click(&format!(
        "{}-toggle",
        super::explorer::row_selector(&project.to_string_lossy())
    ));
    let program = w.path("src/App/Program.cs");
    w.double_click(&super::explorer::row_selector(&format!(
        "{}|Program.cs",
        project.to_string_lossy()
    )));
    let view = w.editor(&program);
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(body()))
    });
    // Typing an identifier character: the syntax tree's identifiers at once, marked as such.
    let t0 = Instant::now();
    w.vcx.simulate_input("M");
    w.wait("the fallback list", |w| !w.labels(&view).is_empty());
    let fallback = w.completion(&view).unwrap();
    assert_eq!(fallback.source, Some(CompletionSource::Syntax));
    assert_eq!(w.labels(&view)[0], "Main");
    assert!(
        t0.elapsed() < Duration::from_millis(400),
        "before the server answered"
    );
    let out = w.run_cmd(workspace::EDITOR_COMPLETE, json!({}));
    assert_eq!(out["source"], "syntax");
    // The server was asked as well; its items replace the fallback when they arrive.
    w.wait("the server's items", |w| {
        w.completion(&view)
            .is_some_and(|c| c.source == Some(CompletionSource::LanguageServer))
    });
    let labels = w.labels(&view);
    assert!(
        labels.contains(&"Math".to_owned()) && labels.contains(&"Main".to_owned()),
        "{labels:?}"
    );
    w.fake.finish_load();
}

#[gpui::test]
fn quick_info_appears_after_the_delay_and_dismisses(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/hover", |p| {
            let at = p["position"].clone();
            FakeReply::Result(json!({"contents": {"kind": "markdown",
                "value": "```csharp\nclass Program\n```\n\nThe program\\'s `entry` class."},
                "range": {"start": at, "end": {"line": at["line"], "character": at["character"].as_u64().unwrap() + 7}}}))
        });
    });
    let (_, view) = w.open_program();
    let word = PROGRAM.find("Program").unwrap();
    let at = view
        .read_with(&w.vcx, |v, _| v.pixel_position_for_offset(word + 3))
        .unwrap();
    w.vcx
        .simulate_mouse_move(point(at.x + px(2.), at.y + px(5.)), None, Modifiers::none());
    w.vcx
        .executor()
        .advance_clock(HOVER_DELAY - Duration::from_millis(1));
    w.vcx.run_until_parked();
    assert_eq!(
        w.requests("textDocument/hover"),
        0,
        "nothing before the delay"
    );
    w.vcx.executor().advance_clock(Duration::from_millis(1));
    w.wait("Quick Info", |w| {
        view.read_with(&w.vcx, |v, _| v.hover())
            .is_some_and(|h| h.visible)
    });
    let hover = view.read_with(&w.vcx, |v, _| v.hover()).unwrap();
    assert_eq!(
        hover.text.as_deref(),
        Some("class Program\n\nThe program's entry class.")
    );
    assert_eq!(
        w.fake.received_params("textDocument/hover")[0]["position"],
        json!({"line": 0, "character": 6})
    );
    assert!(w.audit().contains(&workspace::EDITOR_HOVER.to_owned()));
    assert!(w.vcx.debug_bounds("quick-info").is_some());
    // The mouse leaving the word dismisses it.
    let away = view
        .read_with(&w.vcx, |v, _| {
            v.pixel_position_for_offset(PROGRAM.find("static").unwrap())
        })
        .unwrap();
    w.vcx.simulate_mouse_move(
        point(away.x + px(2.), away.y + px(5.)),
        None,
        Modifiers::none(),
    );
    assert!(view.read_with(&w.vcx, |v, _| v.hover()).is_none());

    // Ctrl+K, Ctrl+I at the caret; typing dismisses it. An agent gets the text.
    view.update(&mut w.vcx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(word + 1))
    });
    w.vcx.simulate_keystrokes("ctrl-k ctrl-i");
    w.wait("Quick Info", |w| {
        view.read_with(&w.vcx, |v, _| v.hover())
            .is_some_and(|h| h.visible)
    });
    w.vcx.simulate_input("x");
    assert!(view.read_with(&w.vcx, |v, _| v.hover()).is_none());
    let commands = w.commands.clone();
    let path = w.path("src/App/Program.cs");
    let agent = std::thread::spawn(move || {
        commands.invoke(
            workspace::EDITOR_HOVER,
            json!({"path": path.to_string_lossy(), "line": 1, "column": 8}),
        )
    });
    let deadline = Instant::now() + T;
    while !agent.is_finished() {
        assert!(Instant::now() < deadline);
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    let out = agent.join().unwrap().unwrap();
    assert_eq!(out["state"], "open");
    assert_eq!(out["text"], "class Program\n\nThe program's entry class.");
}

#[gpui::test]
fn parameter_info_tracks_the_active_parameter(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/signatureHelp", |p| {
            // The argument index from the position: the call starts after `Foo(` on line 2.
            let ch = p["position"]["character"].as_u64().unwrap();
            // `Foo(` ends at column 29 of line 2; after `1,` the caret is at 31.
            let active = if ch >= 31 { 1 } else { 0 };
            FakeReply::Result(json!({"signatures": [
                {"label": "void Foo(int count, string name)",
                 "documentation": "Does foo.",
                 "parameters": [{"label": "int count"}, {"label": "string name"}]}],
                "activeSignature": 0, "activeParameter": active}))
        });
        fake.respond("textDocument/completion", |_| {
            FakeReply::Result(Value::Null)
        });
    });
    let (_, view) = w.open_program();
    w.insert_at(&view, body(), "Foo");
    w.vcx.simulate_input("(");
    w.wait("Parameter Info", |w| {
        view.read_with(&w.vcx, |v, _| v.signature_help())
            .is_some_and(|s| s.visible)
    });
    let request = w
        .fake
        .wait_for("textDocument/signatureHelp", T, |_| true)
        .unwrap();
    assert_eq!(request.params["context"]["triggerKind"], 2);
    assert_eq!(request.params["context"]["triggerCharacter"], "(");
    let s = view.read_with(&w.vcx, |v, _| v.signature_help()).unwrap();
    assert_eq!(s.active_parameter, Some(0));
    assert!(w.vcx.debug_bounds("signature-help").is_some());
    let out = w.run_cmd(workspace::EDITOR_SIGNATURE_HELP, json!({}));
    assert_eq!(
        out["signatures"][0]["parameters"],
        json!(["int count", "string name"])
    );

    // `,` moves to the second argument; the server's answer agrees.
    w.vcx.simulate_input("1,");
    let s = view.read_with(&w.vcx, |v, _| v.signature_help()).unwrap();
    assert_eq!(s.active_parameter, Some(1), "at once, from the text");
    w.fake
        .wait_for("textDocument/signatureHelp", T, |p| {
            p["context"]["isRetrigger"] == true && p["context"]["triggerCharacter"] == ","
        })
        .expect("a retrigger with the active help");
    w.wait("the server's answer", |w| {
        view.read_with(&w.vcx, |v, _| v.signature_help())
            .is_some_and(|s| !s.loading && s.active_parameter == Some(1))
    });
    // Moving the caret back into the first argument follows it.
    w.vcx.simulate_keystrokes("left");
    w.wait("the first argument again", |w| {
        view.read_with(&w.vcx, |v, _| v.signature_help())
            .is_some_and(|s| !s.loading && s.active_parameter == Some(0))
    });
    w.vcx.simulate_keystrokes("right");
    // `)` closes it.
    w.vcx.simulate_input("x)");
    assert!(view.read_with(&w.vcx, |v, _| v.signature_help()).is_none());
}

#[gpui::test]
fn agents_complete_and_commit_on_the_bus(cx: &mut gpui::TestAppContext) {
    let mut w = setup_with(cx, |fake| {
        fake.set_language_server("running", Some(capabilities()));
        fake.respond("textDocument/completion", |p| {
            let start = p["position"]["character"].as_u64().unwrap() as u32;
            FakeReply::After(Duration::from_millis(30), list(p, start, &MEMBERS))
        });
    });
    let view = w.program_with_console();
    w.insert_at(&view, body() + "Console".len(), ".");
    let commands = w.commands.clone();
    let path = w.path("src/App/Program.cs").to_string_lossy().into_owned();
    let column = body() - PROGRAM.find("    static").unwrap() + "Console.".len() + 1;
    let agent = std::thread::spawn(move || {
        let listed = commands
            .invoke(
                workspace::EDITOR_COMPLETE,
                json!({"path": path, "line": 3, "column": column}),
            )
            .unwrap();
        let committed = commands
            .invoke(
                workspace::EDITOR_ACCEPT_COMPLETION,
                json!({"path": path, "label": "ReadLine"}),
            )
            .unwrap();
        (listed, committed)
    });
    let deadline = Instant::now() + T;
    while !agent.is_finished() {
        assert!(Instant::now() < deadline);
        w.vcx.run_until_parked();
        std::thread::sleep(Duration::from_millis(5));
    }
    let (listed, committed) = agent.join().unwrap();
    assert_eq!(listed["state"], "open", "{listed}");
    assert_eq!(listed["source"], "languageServer");
    assert_eq!(listed["total"], 4);
    assert_eq!(
        listed["items"][0],
        json!({"label": "Out", "kind": "property"})
    );
    assert_eq!(committed["accepted"], true);
    assert_eq!(committed["text"], "ReadLine");
    assert!(w.text(&view).contains("{ Console.ReadLine}"));
}
