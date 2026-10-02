//! Headless GPUI tests of the editor's IntelliSense layers (brief 0013), without a language server: the triggers
//! the view reports, the completion list (filtering, navigation, commit, stale answers, the syntax-tree fallback and
//! the swap to server items), Quick Info after the hover delay, and Parameter Info following the arguments.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use eludite_editor::intellisense::HOVER_DELAY;
use eludite_editor::syntax::LanguageRegistry;
use eludite_editor::{
    Buffer, CompletionEdit, CompletionItem, CompletionKind, CompletionSource, CompletionTrigger,
    EditorEvent, EditorView, SignatureHelpData, SignatureInfo, SignatureTrigger, key_bindings,
};
use gpui::{
    AppContext as _, Entity, Focusable as _, Point, TestAppContext, VisualTestContext, point, px,
};

struct Ed {
    view: Entity<EditorView>,
    cx: VisualTestContext,
    events: Rc<RefCell<Vec<EditorEvent>>>,
}

fn open(cx: &mut TestAppContext, text: &str, language: Option<&str>) -> Ed {
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let language = language.and_then(|id| LanguageRegistry::with_builtins().by_id(id));
    let text = text.to_owned();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let view = cx.new(|cx| {
                let mut buffer = Buffer::new(&text);
                buffer.set_group_interval(Duration::ZERO);
                EditorView::new(buffer, language, cx)
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
    vcx.executor().allow_parking();
    let mut ed = Ed {
        view,
        cx: vcx,
        events,
    };
    ed.settle();
    ed
}

impl Ed {
    /// Run until idle, waiting for the syntax thread (a real OS thread) when highlighting is pending.
    fn settle(&mut self) {
        for _ in 0..2000 {
            self.cx.run_until_parked();
            let done = self.view.read_with(&self.cx, |v, _| {
                v.language().is_none() || v.highlights_complete()
            });
            if done {
                return;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        panic!("highlighting did not complete");
    }

    fn take_events(&self) -> Vec<EditorEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    fn text(&self) -> String {
        self.view.read_with(&self.cx, |v, _| v.editor().text())
    }

    fn set_caret(&mut self, offset: usize) {
        self.view.update(&mut self.cx, |v, cx| {
            v.update_editor(cx, |e| e.set_caret(offset))
        });
        self.take_events();
    }

    fn labels(&self) -> Vec<String> {
        self.view.read_with(&self.cx, |v, _| {
            v.completion()
                .map(|c| c.items.into_iter().map(|i| i.0).collect())
                .unwrap_or_default()
        })
    }

    fn selected(&self) -> Option<String> {
        self.view.read_with(&self.cx, |v, _| {
            let c = v.completion()?;
            c.selected.map(|i| c.items[i].0.clone())
        })
    }

    fn position_of(&mut self, offset: usize) -> Point<gpui::Pixels> {
        let p = self
            .view
            .read_with(&self.cx, |v, _| v.pixel_position_for_offset(offset))
            .expect("row painted");
        point(p.x + px(2.), p.y + px(5.))
    }
}

fn item(label: &str, kind: CompletionKind) -> CompletionItem {
    CompletionItem {
        label: label.into(),
        kind,
        ..Default::default()
    }
}

#[gpui::test]
fn typing_reports_completion_and_signature_triggers(cx: &mut TestAppContext) {
    let mut ed = open(
        cx,
        "class A\n{\n    // note\n    void M() { }\n}\n",
        Some("csharp"),
    );
    let body = ed.text().find("{ }").unwrap() + 2;
    ed.set_caret(body);
    ed.cx.simulate_input("x");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CompletionTriggered(CompletionTrigger::Typing(
            'x'
        ))]
    );
    // Inside a word (no list open): no new trigger.
    ed.cx.simulate_input("y");
    assert!(ed.take_events().is_empty());
    ed.cx.simulate_input(".");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CompletionTriggered(
            CompletionTrigger::Character('.')
        )]
    );
    ed.cx.simulate_input("(");
    let events = ed.take_events();
    assert!(events.contains(&EditorEvent::CompletionTriggered(
        CompletionTrigger::Character('(')
    )));
    assert!(events.contains(&EditorEvent::SignatureHelpTriggered(
        SignatureTrigger::Character('(')
    )));
    // Digits do not start a completion; neither does typing in a comment.
    ed.cx.simulate_input(" 1");
    assert!(
        ed.take_events()
            .iter()
            .all(|e| !matches!(e, EditorEvent::CompletionTriggered(_)))
    );
    let comment = ed.text().find("note").unwrap() + 4;
    ed.set_caret(comment);
    ed.cx.simulate_input(" a.");
    assert!(
        ed.take_events()
            .iter()
            .all(|e| !matches!(e, EditorEvent::CompletionTriggered(_)))
    );
    // The explicit keys.
    ed.cx.simulate_keystrokes("ctrl-space");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CompletionTriggered(CompletionTrigger::Invoked)]
    );
    ed.cx.simulate_keystrokes("ctrl-shift-space");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::SignatureHelpTriggered(
            SignatureTrigger::Invoked
        )]
    );
    ed.cx.simulate_keystrokes("ctrl-k ctrl-i");
    assert!(matches!(
        ed.take_events()[..],
        [EditorEvent::HoverTriggered { .. }]
    ));
}

#[gpui::test]
fn completion_list_filters_navigates_commits_and_drops_stale_answers(cx: &mut TestAppContext) {
    let mut ed = open(cx, "Console.\n", None);
    ed.set_caret(8);
    let request = ed.view.update(&mut ed.cx, |v, cx| {
        v.open_completion(Some(CompletionTrigger::Character('.')), cx)
    });
    assert_eq!((request.offset, request.word_start), (8, 8));
    let loading = ed.view.read_with(&ed.cx, |v, _| v.completion().unwrap());
    assert!(loading.loading && !loading.visible);
    // An answer for another request is ignored.
    let stale = ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id + 7,
            vec![item("Stale", CompletionKind::Text)],
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    assert!(!stale);
    let items = vec![
        item("WriteLine", CompletionKind::Method),
        item("Write", CompletionKind::Method),
        item("Out", CompletionKind::Property),
        CompletionItem {
            label: "ReadLine".into(),
            kind: CompletionKind::Method,
            sort_text: Some("0".into()),
            ..Default::default()
        },
    ];
    ed.view.update(&mut ed.cx, |v, cx| {
        assert!(v.set_completions(
            request.id,
            items,
            CompletionSource::LanguageServer,
            false,
            cx
        ))
    });
    ed.cx.run_until_parked();
    // Empty filter: sort text order (ReadLine sorts first by its sort text), first item selected.
    assert_eq!(ed.labels(), ["ReadLine", "Out", "Write", "WriteLine"]);
    assert_eq!(ed.selected().as_deref(), Some("ReadLine"));
    assert!(
        ed.cx.debug_bounds("completion-list").is_some(),
        "the list is drawn"
    );
    // The first selected item has no documentation: resolve it lazily.
    assert!(ed.take_events().contains(&EditorEvent::ResolveCompletion {
        id: request.id,
        index: 3
    }));

    // Typing filters (fuzzy) without a new request: the list is complete and nothing is in flight.
    ed.cx.simulate_input("w");
    ed.cx.run_until_parked();
    assert_eq!(ed.labels(), ["Write", "WriteLine"]);
    assert!(
        ed.take_events()
            .iter()
            .all(|e| !matches!(e, EditorEvent::CompletionTriggered(_)))
    );
    ed.cx.simulate_input("rl");
    ed.cx.run_until_parked();
    assert_eq!(ed.labels(), ["WriteLine"]);
    ed.cx.simulate_keystrokes("backspace backspace");
    ed.cx.run_until_parked();
    assert_eq!(ed.labels(), ["Write", "WriteLine"]);
    ed.cx.simulate_keystrokes("down");
    assert_eq!(ed.selected().as_deref(), Some("WriteLine"));
    ed.cx.simulate_keystrokes("up up");
    assert_eq!(ed.selected().as_deref(), Some("Write"));
    ed.cx.simulate_keystrokes("down");
    // Tab commits the selection over the typed word.
    ed.cx.simulate_keystrokes("tab");
    assert_eq!(ed.text(), "Console.WriteLine\n");
    assert!(ed.view.read_with(&ed.cx, |v, _| v.completion().is_none()));
    assert!(ed.take_events().contains(&EditorEvent::CompletionClosed));

    // Escape closes; a typed character that cannot continue the word closes too.
    let request = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_completion(None, cx));
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id,
            vec![item("WriteLine", CompletionKind::Method)],
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    ed.cx.run_until_parked();
    assert_eq!(request.word_start, 8, "the word typed so far is replaced");
    ed.cx.simulate_keystrokes("escape");
    assert!(ed.view.read_with(&ed.cx, |v, _| v.completion().is_none()));
    assert_eq!(ed.text(), "Console.WriteLine\n");
}

#[gpui::test]
fn commit_applies_the_text_edit_and_enter_respects_soft_selection(cx: &mut TestAppContext) {
    let mut ed = open(cx, "x.Wr\n", None);
    ed.set_caret(4);
    let request = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_completion(None, cx));
    assert_eq!(request.word_start, 2);
    ed.view.update(&mut ed.cx, |v, cx| {
        let b = v.editor().buffer();
        let edit = CompletionEdit {
            range: b.anchor_before(2)..b.anchor_after(4),
            new_text: "WriteLine()".into(),
        };
        let items = vec![CompletionItem {
            label: "WriteLine".into(),
            kind: CompletionKind::Method,
            edit: Some(edit),
            ..Default::default()
        }];
        v.set_completions(
            request.id,
            items,
            CompletionSource::LanguageServer,
            false,
            cx,
        );
    });
    ed.cx.run_until_parked();
    // Text typed after the request: the edit extends to the caret.
    ed.cx.simulate_input("i");
    ed.cx.run_until_parked();
    assert_eq!(ed.labels(), ["WriteLine"]);
    ed.cx.simulate_keystrokes("enter");
    assert_eq!(ed.text(), "x.WriteLine()\n");

    // Opened by `(` with nothing typed: the selection is soft, Enter is a new line.
    let end = ed.text().len() - 1;
    ed.set_caret(end);
    let request = ed.view.update(&mut ed.cx, |v, cx| {
        v.open_completion(Some(CompletionTrigger::Character('(')), cx)
    });
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id,
            vec![item("value", CompletionKind::Variable)],
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    ed.cx.run_until_parked();
    assert_eq!(ed.labels(), ["value"]);
    ed.cx.simulate_keystrokes("enter");
    assert_eq!(ed.text(), "x.WriteLine()\n\n");
    // Accepting by label (the command's form).
    let request = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_completion(None, cx));
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id,
            vec![
                item("alpha", CompletionKind::Field),
                item("beta", CompletionKind::Field),
            ],
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    ed.cx.run_until_parked();
    let accepted = ed
        .view
        .update(&mut ed.cx, |v, cx| v.accept_completion(Some("beta"), cx))
        .unwrap();
    assert_eq!(
        (accepted.label.as_str(), accepted.text.as_str()),
        ("beta", "beta")
    );
    assert!(ed.text().ends_with("\nbeta\n"));
}

#[gpui::test]
fn fallback_lists_syntax_identifiers_then_swaps_to_server_items(cx: &mut TestAppContext) {
    let src = "class Order\n{\n    int quantity;\n    void Ship(int count) { }\n}\n";
    let mut ed = open(cx, src, Some("csharp"));
    let at = src.find("{ }").unwrap() + 2;
    ed.set_caret(at);
    ed.cx.simulate_input("q");
    let request = ed.view.update(&mut ed.cx, |v, cx| {
        v.open_completion(Some(CompletionTrigger::Typing('q')), cx)
    });
    ed.view
        .update(&mut ed.cx, |v, cx| v.complete_from_syntax(request.id, cx));
    for _ in 0..1000 {
        ed.cx.run_until_parked();
        if !ed.labels().is_empty() {
            break;
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let snapshot = ed.view.read_with(&ed.cx, |v, _| v.completion().unwrap());
    assert_eq!(snapshot.source, Some(CompletionSource::Syntax));
    assert_eq!(snapshot.filter, "q");
    assert_eq!(ed.labels(), ["quantity"]);
    assert!(ed.cx.debug_bounds("completion-list").is_some());
    // Backspace to an empty word shows every identifier but the one being typed.
    ed.cx.simulate_keystrokes("backspace");
    ed.cx.run_until_parked();
    let all = ed.labels();
    for name in ["Order", "Ship", "count", "quantity"] {
        assert!(all.contains(&name.to_owned()), "{all:?}");
    }
    ed.cx.simulate_input("q");
    ed.cx.run_until_parked();
    // The server answers: its items replace the fallback at once.
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_completions(
            request.id,
            vec![
                item("quantity", CompletionKind::Field),
                item("Queue", CompletionKind::Class),
            ],
            CompletionSource::LanguageServer,
            false,
            cx,
        )
    });
    ed.cx.run_until_parked();
    let snapshot = ed.view.read_with(&ed.cx, |v, _| v.completion().unwrap());
    assert_eq!(snapshot.source, Some(CompletionSource::LanguageServer));
    assert!(
        snapshot
            .items
            .contains(&("quantity".into(), CompletionKind::Field, None))
    );
    assert!(
        snapshot
            .items
            .contains(&("Queue".into(), CompletionKind::Class, None))
    );
    // A late syntax answer does not replace the server's.
    ed.view
        .update(&mut ed.cx, |v, cx| v.complete_from_syntax(request.id, cx));
    ed.cx.run_until_parked();
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.completion().unwrap().source),
        Some(CompletionSource::LanguageServer)
    );
}

#[gpui::test]
fn quick_info_after_the_delay_and_dismissal(cx: &mut TestAppContext) {
    let src = "var rpc = new JsonRpc(stream);\nvar other = 1;\n";
    let mut ed = open(cx, src, None);
    let word = src.find("JsonRpc").unwrap();
    let at = ed.position_of(word + 2);
    ed.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    ed.cx
        .executor()
        .advance_clock(HOVER_DELAY - Duration::from_millis(10));
    ed.cx.run_until_parked();
    assert!(ed.take_events().is_empty(), "nothing before the delay");
    ed.cx.executor().advance_clock(Duration::from_millis(10));
    ed.cx.run_until_parked();
    let events = ed.take_events();
    assert_eq!(events, [EditorEvent::HoverTriggered { offset: word }]);
    let id = ed.view.update(&mut ed.cx, |v, cx| v.open_hover(word, cx));
    ed.view.update(&mut ed.cx, |v, cx| {
        assert!(!v.set_hover(id + 1, Some("stale"), None, cx));
        v.set_hover(
            id,
            Some(
                "```csharp\nclass StreamJsonRpc.JsonRpc\n```\n\nManages a JSON\\-RPC `connection`.",
            ),
            Some(word..word + 7),
            cx,
        )
    });
    ed.cx.run_until_parked();
    let hover = ed.view.read_with(&ed.cx, |v, _| v.hover().unwrap());
    assert!(hover.visible);
    assert_eq!(
        hover.text.as_deref(),
        Some("class StreamJsonRpc.JsonRpc\n\nManages a JSON-RPC connection.")
    );
    assert!(ed.cx.debug_bounds("quick-info").is_some(), "tooltip drawn");
    // Moving within the word keeps it.
    let inside = ed.position_of(word + 5);
    ed.cx
        .simulate_mouse_move(inside, None, gpui::Modifiers::none());
    assert!(ed.view.read_with(&ed.cx, |v, _| v.hover().is_some()));
    // Leaving the word closes it.
    let other = src.find("other").unwrap();
    let away = ed.position_of(other + 1);
    ed.cx
        .simulate_mouse_move(away, None, gpui::Modifiers::none());
    assert!(ed.view.read_with(&ed.cx, |v, _| v.hover().is_none()));
    assert!(ed.take_events().contains(&EditorEvent::HoverClosed));
    // So does leaving the editor.
    ed.cx.simulate_mouse_move(at, None, gpui::Modifiers::none());
    ed.cx.executor().advance_clock(HOVER_DELAY);
    ed.cx.run_until_parked();
    assert_eq!(
        ed.take_events(),
        [EditorEvent::HoverTriggered { offset: word }]
    );
    let id = ed.view.update(&mut ed.cx, |v, cx| v.open_hover(word, cx));
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_hover(id, Some("JsonRpc"), None, cx)
    });
    ed.cx.run_until_parked();
    ed.cx
        .simulate_mouse_move(point(px(-50.), px(-50.)), None, gpui::Modifiers::none());
    assert!(ed.view.read_with(&ed.cx, |v, _| v.hover().is_none()));

    // Quick Info from the keyboard (Ctrl+K, Ctrl+I at the caret) moves with scrolling.
    let id = ed.view.update(&mut ed.cx, |v, cx| v.open_hover(other, cx));
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_hover(id, Some("int other"), None, cx)
    });
    ed.cx.run_until_parked();
    let shown = ed.cx.debug_bounds("quick-info").expect("tooltip drawn");
    ed.view
        .update(&mut ed.cx, |v, cx| v.set_scroll_y(px(10.), cx));
    ed.cx.run_until_parked();
    let scrolled = ed.cx.debug_bounds("quick-info").expect("still drawn");
    assert_eq!(scrolled.origin.y, shown.origin.y - px(10.));
    ed.view
        .update(&mut ed.cx, |v, cx| v.set_scroll_y(px(0.), cx));
    ed.cx.run_until_parked();
    // Typing closes it.
    ed.cx.simulate_input("z");
    assert!(ed.view.read_with(&ed.cx, |v, _| v.hover().is_none()));
}

#[gpui::test]
fn parameter_info_tracks_the_active_argument(cx: &mut TestAppContext) {
    let mut ed = open(cx, "M(\n", None);
    ed.set_caret(2);
    let id = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_signature_help(cx));
    let label = "void M(int a, string b)";
    let data = SignatureHelpData {
        signatures: vec![
            SignatureInfo {
                label: label.into(),
                documentation: Some("Does `M`.".into()),
                parameters: vec![7..12, 14..22],
                active_parameter: None,
            },
            SignatureInfo {
                label: "void M()".into(),
                ..Default::default()
            },
        ],
        active_signature: 0,
        active_parameter: Some(0),
    };
    ed.view.update(&mut ed.cx, |v, cx| {
        v.set_signature_help(id, Some(data.clone()), cx)
    });
    ed.cx.run_until_parked();
    let s = ed
        .view
        .read_with(&ed.cx, |v, _| v.signature_help().unwrap());
    assert_eq!((s.active_signature, s.active_parameter), (0, Some(0)));
    assert!(ed.cx.debug_bounds("signature-help").is_some());
    // Typing `,` moves to the next argument at once and asks for a refresh.
    ed.cx.simulate_input("1,");
    let s = ed
        .view
        .read_with(&ed.cx, |v, _| v.signature_help().unwrap());
    assert_eq!(s.active_parameter, Some(1));
    let events = ed.take_events();
    assert!(events.contains(&EditorEvent::SignatureHelpTriggered(
        SignatureTrigger::Retrigger
    )));
    assert!(events.contains(&EditorEvent::SignatureHelpTriggered(
        SignatureTrigger::Character(',')
    )));
    // The caret moving back over the comma follows too.
    ed.cx.simulate_keystrokes("left");
    let s = ed
        .view
        .read_with(&ed.cx, |v, _| v.signature_help().unwrap());
    assert_eq!(s.active_parameter, Some(0));
    ed.cx.simulate_keystrokes("right");
    // Up and Down cycle the overloads.
    ed.cx.simulate_keystrokes("down");
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.signature_help().unwrap().active_signature),
        1
    );
    ed.cx.simulate_keystrokes("up");
    // `)` closes it.
    ed.cx.simulate_input("x)");
    assert!(
        ed.view
            .read_with(&ed.cx, |v, _| v.signature_help().is_none())
    );
    assert!(ed.take_events().contains(&EditorEvent::SignatureHelpClosed));
    assert_eq!(ed.text(), "M(1,x)\n");

    // Escape closes it as well, and a null answer closes it.
    ed.set_caret(2);
    let id = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_signature_help(cx));
    ed.view
        .update(&mut ed.cx, |v, cx| v.set_signature_help(id, Some(data), cx));
    ed.cx.simulate_keystrokes("escape");
    assert!(
        ed.view
            .read_with(&ed.cx, |v, _| v.signature_help().is_none())
    );
    let id = ed
        .view
        .update(&mut ed.cx, |v, cx| v.open_signature_help(cx));
    ed.view
        .update(&mut ed.cx, |v, cx| v.set_signature_help(id, None, cx));
    assert!(
        ed.view
            .read_with(&ed.cx, |v, _| v.signature_help().is_none())
    );
}
