//! Headless GPUI tests for `EditorView`: keyboard, text input, mouse,
//! scrolling, find, and highlighting off the UI thread.

use eludite_editor::syntax::{HighlightKind, LanguageRegistry};
use eludite_editor::{Buffer, EditorView, SelectionRange, key_bindings};
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent, MouseUpEvent,
    Point, ScrollDelta, ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, point, px,
};

fn open(
    cx: &mut TestAppContext,
    text: &str,
    language: Option<&str>,
) -> (Entity<EditorView>, VisualTestContext) {
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let language = language.and_then(|id| LanguageRegistry::with_builtins().by_id(id));
    let text = text.to_owned();
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |window, cx| {
            let view = cx.new(|cx| {
                let mut buffer = Buffer::new(&text);
                buffer.set_group_interval(std::time::Duration::ZERO);
                EditorView::new(buffer, language, cx)
            });
            window.focus(&view.focus_handle(cx), cx);
            view
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let view = window.root(&mut vcx).unwrap();
    vcx.run_until_parked();
    if view.read_with(&vcx, |v, _| v.language().is_some()) {
        wait_for_highlights(&view, &mut vcx);
    }
    (view, vcx)
}

/// Highlighting runs on a real OS thread (the syntax thread), outside the
/// test scheduler, so wait for it with a timeout.
fn wait_for_highlights(view: &Entity<EditorView>, cx: &mut VisualTestContext) {
    cx.executor().allow_parking();
    for _ in 0..2000 {
        cx.run_until_parked();
        if view.read_with(cx, |v, _| v.highlights_complete()) {
            return;
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    panic!("highlighting did not complete");
}

fn text(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, _| v.editor().text())
}

fn selections(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> Vec<SelectionRange> {
    view.read_with(cx, |v, _| v.editor().selections())
}

fn position_of(
    view: &Entity<EditorView>,
    cx: &mut VisualTestContext,
    offset: usize,
) -> Point<gpui::Pixels> {
    let p = view
        .read_with(cx, |v, _| v.pixel_position_for_offset(offset))
        .expect("row painted");
    // Middle of the line, at the character's left edge.
    point(p.x + px(0.5), p.y + px(5.))
}

#[gpui::test]
fn typing_editing_and_undo_through_the_keyboard(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "", None);
    cx.simulate_input("hello world");
    assert_eq!(text(&view, &mut cx), "hello world");
    cx.simulate_keystrokes("backspace backspace");
    assert_eq!(text(&view, &mut cx), "hello wor");
    cx.simulate_keystrokes("shift-left shift-left shift-left");
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 9, head: 6 }]
    );
    cx.simulate_input("there");
    assert_eq!(text(&view, &mut cx), "hello there");
    cx.simulate_keystrokes("enter");
    cx.simulate_input("x");
    assert_eq!(text(&view, &mut cx), "hello there\nx");
    cx.simulate_keystrokes("secondary-z");
    assert_eq!(text(&view, &mut cx), "hello there\n");
    cx.simulate_keystrokes("secondary-z");
    assert_eq!(text(&view, &mut cx), "hello there");
    cx.simulate_keystrokes("secondary-y");
    assert_eq!(text(&view, &mut cx), "hello there\n");
    cx.simulate_keystrokes("secondary-a delete");
    assert_eq!(text(&view, &mut cx), "");
}

#[gpui::test]
fn multiple_carets_from_the_keyboard(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "one\ntwo\nthree", None);
    cx.simulate_keystrokes("alt-shift-down alt-shift-down");
    assert_eq!(selections(&view, &mut cx).len(), 3);
    cx.simulate_input("> ");
    assert_eq!(text(&view, &mut cx), "> one\n> two\n> three");
    cx.simulate_keystrokes("end");
    cx.simulate_input(";");
    assert_eq!(text(&view, &mut cx), "> one;\n> two;\n> three;");
    cx.simulate_keystrokes("escape");
    assert_eq!(selections(&view, &mut cx).len(), 1);
}

#[gpui::test]
fn copy_and_paste(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "alpha beta", None);
    cx.simulate_keystrokes("ctrl-shift-right secondary-c end");
    cx.simulate_input(" ");
    cx.simulate_keystrokes("secondary-v");
    assert_eq!(text(&view, &mut cx), "alpha beta alpha ");
}

#[gpui::test]
fn mouse_click_drag_shift_and_add_caret(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "first line\nsecond line\nthird line", None);
    let none = Modifiers::none();
    // Click inside "second".
    let p = position_of(&view, &mut cx, 13);
    cx.simulate_click(p, none);
    assert_eq!(selections(&view, &mut cx), vec![SelectionRange::caret(13)]);
    // Drag from offset 11 to 17 ("second").
    let a = position_of(&view, &mut cx, 11);
    let b = position_of(&view, &mut cx, 17);
    cx.simulate_mouse_down(a, MouseButton::Left, none);
    cx.simulate_mouse_move(b, MouseButton::Left, none);
    cx.simulate_mouse_up(b, MouseButton::Left, none);
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 11, head: 17 }]
    );
    // Shift+click extends the selection.
    let c = position_of(&view, &mut cx, 3);
    cx.simulate_click(c, Modifiers::shift());
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 11, head: 3 }]
    );
    // Ctrl+Alt+click adds a caret.
    let p1 = position_of(&view, &mut cx, 1);
    cx.simulate_click(p1, none);
    let add = Modifiers {
        control: true,
        alt: true,
        ..Modifiers::none()
    };
    let p23 = position_of(&view, &mut cx, 23);
    cx.simulate_click(p23, add);
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange::caret(1), SelectionRange::caret(23)]
    );
    cx.simulate_input("#");
    assert_eq!(
        text(&view, &mut cx),
        "f#irst line\nsecond line\n#third line"
    );
    // Double-click selects a word.
    let w = position_of(&view, &mut cx, 15);
    cx.simulate_event(MouseDownEvent {
        position: w,
        modifiers: none,
        button: MouseButton::Left,
        click_count: 2,
        first_mouse: false,
    });
    cx.simulate_event(MouseUpEvent {
        position: w,
        modifiers: none,
        button: MouseButton::Left,
        click_count: 2,
    });
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 12, head: 18 }]
    );
}

#[gpui::test]
fn scroll_wheel_and_autoscroll(cx: &mut TestAppContext) {
    let lines: Vec<String> = (0..1000).map(|i| format!("line {i}")).collect();
    let (view, mut cx) = open(cx, &lines.join("\n"), None);
    let lh = view.read_with(&cx, |v, _| v.line_height());
    cx.simulate_event(ScrollWheelEvent {
        position: point(px(100.), px(100.)),
        delta: ScrollDelta::Lines(point(0., -10.)),
        modifiers: Modifiers::none(),
        touch_phase: TouchPhase::Moved,
    });
    let y = view.read_with(&cx, |v, _| v.scroll_position().y);
    assert_eq!(y, lh * 10.);
    let visible = view.read_with(&cx, |v, _| v.visible_rows());
    assert_eq!(visible.start, 10);
    // Ctrl+End moves the caret to the end and scrolls it into view.
    cx.simulate_keystrokes("ctrl-end");
    let (y, max) = view.read_with(&cx, |v, _| (v.scroll_position().y, v.max_scroll_y()));
    assert!(y > lh * 900. && y <= max, "scrolled to the end: {y:?}");
    let rows = view.read_with(&cx, |v, _| v.visible_rows());
    assert!(rows.contains(&999));
}

#[gpui::test]
fn find_in_buffer(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "foo bar\nFoo baz foo", None);
    cx.simulate_keystrokes("secondary-f");
    assert!(view.read_with(&cx, |v, _| v.is_find_bar_open()));
    cx.simulate_input("foo");
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 0, head: 3 }]
    );
    cx.simulate_keystrokes("enter");
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 8, head: 11 }]
    );
    cx.simulate_keystrokes("f3");
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 16, head: 19 }]
    );
    cx.simulate_keystrokes("shift-f3");
    assert_eq!(
        selections(&view, &mut cx),
        vec![SelectionRange { tail: 8, head: 11 }]
    );
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(&cx, |v, _| v.is_find_bar_open()));
    // Typing goes to the buffer again.
    cx.simulate_input("X");
    assert_eq!(text(&view, &mut cx), "foo bar\nX baz foo");
}

#[gpui::test]
fn highlighting_runs_off_thread_and_stale_highlights_follow_edits(cx: &mut TestAppContext) {
    let src = "class A\n{\n    int x = 1;\n}\n";
    let (view, mut cx) = open(cx, src, Some("csharp"));
    assert!(view.read_with(&cx, |v, _| v.highlights_complete()));
    let kind = |cx: &mut VisualTestContext, row: u32, col: u32| {
        view.read_with(cx, |v, _| {
            v.highlights()
                .kind_at(eludite_editor::text::Point::new(row, col))
        })
    };
    assert_eq!(kind(&mut cx, 0, 0), Some(HighlightKind::Keyword));
    assert_eq!(kind(&mut cx, 2, 4), Some(HighlightKind::TypeBuiltin));

    // Insert text before `int` and check the highlights before the background
    // step has run: they are stale but already moved with the text.
    view.update(&mut cx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(src.find("int").unwrap());
            e.insert("static ");
        })
    });
    assert!(!view.read_with(&cx, |v, _| v.highlights_complete()));
    assert_eq!(kind(&mut cx, 2, 11), Some(HighlightKind::TypeBuiltin));
    assert_eq!(kind(&mut cx, 2, 4), None, "new text is not highlighted yet");

    wait_for_highlights(&view, &mut cx);
    assert_eq!(kind(&mut cx, 2, 4), Some(HighlightKind::Keyword));
    assert_eq!(kind(&mut cx, 2, 11), Some(HighlightKind::TypeBuiltin));
}

#[gpui::test]
fn highlighting_over_the_tree_limit_follows_edits(cx: &mut TestAppContext) {
    let src = "class A\n{\n    int x = 1;\n}\n";
    let (view, mut cx) = open(cx, src, Some("csharp"));
    view.update(&mut cx, |v, _| v.set_syntax_tree_limit(0));
    let kind = |cx: &mut VisualTestContext, row: u32, col: u32| {
        view.read_with(cx, |v, _| {
            v.highlights()
                .kind_at(eludite_editor::text::Point::new(row, col))
        })
    };
    // The first edit re-parses from the tree kept by the initial pass and
    // then drops it; the second has no tree and parses from scratch.
    for (needle, insert, number) in [("int", "static ", 19), ("x =", "y, ", 22)] {
        view.update(&mut cx, |v, cx| {
            v.update_editor(cx, |e| {
                let at = e.text().find(needle).unwrap();
                e.set_caret(at);
                e.insert(insert);
            })
        });
        wait_for_highlights(&view, &mut cx);
        assert!(view.read_with(&cx, |v, _| v.highlight_progress().1.dropped_tree));
        assert_eq!(
            kind(&mut cx, 2, 4),
            Some(HighlightKind::Keyword),
            "{insert}"
        );
        assert_eq!(kind(&mut cx, 2, 11), Some(HighlightKind::TypeBuiltin));
        assert_eq!(
            kind(&mut cx, 2, number),
            Some(HighlightKind::Number),
            "{insert}"
        );
    }
    assert_eq!(
        text(&view, &mut cx),
        "class A\n{\n    static int y, x = 1;\n}\n"
    );
}

#[gpui::test]
fn decorations_follow_edits(cx: &mut TestAppContext) {
    use eludite_editor::{Decoration, DecorationStyle};
    let (view, mut cx) = open(cx, "let a = b;", None);
    view.update(&mut cx, |v, cx| {
        let buffer = v.editor().buffer();
        let range = buffer.anchor_before(8)..buffer.anchor_after(9);
        v.set_decorations(
            "diagnostics",
            vec![Decoration {
                range,
                style: DecorationStyle::Underline {
                    color: gpui::rgb(0xFF0000),
                    wavy: true,
                },
            }],
            cx,
        );
        v.update_editor(cx, |e| e.buffer_mut().edit([(0..0, "    ")]));
    });
    cx.run_until_parked();
    // The view draws without panicking and the anchors moved with the text.
    let offset = view.read_with(&cx, |v, _| {
        let b = v.editor().buffer();
        b.offset_for_anchor(&b.anchor_before(12))
    });
    assert_eq!(offset, 12);
}

#[gpui::test]
fn read_only_documents_navigate_but_do_not_edit(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "class JsonRpc\n{\n}\n", None);
    view.update(&mut cx, |v, cx| v.set_read_only(true, cx));
    assert!(view.read_with(&cx, |v, _| v.is_read_only()));
    cx.simulate_input("x");
    cx.simulate_keystrokes("enter backspace delete tab ctrl-v ctrl-x ctrl-z");
    assert_eq!(text(&view, &mut cx), "class JsonRpc\n{\n}\n");
    // Movement, selection and find still work.
    cx.simulate_keystrokes("ctrl-right shift-end");
    assert_eq!(
        view.read_with(&cx, |v, _| v.editor().selected_text()),
        "JsonRpc"
    );
    // `secondary` is Cmd on macOS and Ctrl elsewhere, like the find_in_buffer test.
    cx.simulate_keystrokes("secondary-f");
    assert!(view.read_with(&cx, |v, _| v.is_find_bar_open()));
    cx.simulate_keystrokes("escape");
    // Programmatic edits (the owner) still apply; editable again afterwards.
    view.update(&mut cx, |v, cx| {
        v.set_read_only(false, cx);
    });
    cx.simulate_input("x");
    assert!(text(&view, &mut cx).contains("x"));
}

#[gpui::test]
fn ctrl_click_moves_the_caret_and_asks_for_the_definition(cx: &mut TestAppContext) {
    use eludite_editor::EditorEvent;
    use std::cell::RefCell;
    use std::rc::Rc;
    let (view, mut cx) = open(cx, "var rpc = new JsonRpc();\n", None);
    let events: Rc<RefCell<Vec<usize>>> = Rc::default();
    let sink = events.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &EditorEvent, _| {
            if let EditorEvent::GoToDefinition { offset } = e {
                sink.borrow_mut().push(*offset);
            }
        })
        .detach()
    });
    cx.run_until_parked();
    let offset = "var rpc = new Js".len();
    let at = position_of(&view, &mut cx, offset);
    let click = |cx: &mut VisualTestContext, modifiers: Modifiers| {
        cx.simulate_event(MouseDownEvent {
            position: at,
            modifiers,
            button: MouseButton::Left,
            click_count: 1,
            first_mouse: false,
        });
        cx.simulate_event(MouseUpEvent {
            position: at,
            modifiers,
            button: MouseButton::Left,
            click_count: 1,
        });
    };
    click(&mut cx, Modifiers::none());
    assert!(
        events.borrow().is_empty(),
        "a plain click only moves the caret"
    );
    view.update(&mut cx, |v, cx| v.update_editor(cx, |e| e.set_caret(0)));
    click(&mut cx, Modifiers::control());
    assert_eq!(*events.borrow(), [offset]);
    assert_eq!(
        view.read_with(&cx, |v, _| v.editor().primary_selection().head),
        offset
    );
    // Ctrl+Alt+click still adds a caret and is not a navigation.
    click(
        &mut cx,
        Modifiers {
            control: true,
            alt: true,
            ..Default::default()
        },
    );
    assert_eq!(events.borrow().len(), 1);
}

#[gpui::test]
fn breakpoint_margin_execution_point_and_data_tip_expressions(cx: &mut TestAppContext) {
    use eludite_editor::{BreakpointGlyph, EditorEvent, ExecutionKind};
    use std::cell::RefCell;
    use std::rc::Rc;
    let src = "class A\n{\n    int M() { return this.order.Name.Length; }\n    void N() { }\n}\n";
    let (view, mut cx) = open(cx, src, None);
    let rows: Rc<RefCell<Vec<u32>>> = Rc::default();
    let sink = rows.clone();
    cx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &EditorEvent, _| {
            if let EditorEvent::BreakpointMarginClicked { row } = e {
                sink.borrow_mut().push(*row);
            }
        })
        .detach()
    });
    cx.run_until_parked();
    // A click in the margin reports its row and does not move the caret.
    let at = view
        .read_with(&cx, |v, _| v.breakpoint_margin_point(2))
        .expect("row 2 painted");
    cx.simulate_click(at, Modifiers::none());
    assert_eq!(*rows.borrow(), [2]);
    assert_eq!(
        view.read_with(&cx, |v, _| v.editor().primary_selection().head),
        0
    );
    // Clicking the text still places the caret (the margin is only the strip at the far left).
    let text_at = position_of(&view, &mut cx, src.find("int").unwrap());
    cx.simulate_click(text_at, Modifiers::none());
    assert_eq!(rows.borrow().len(), 1);

    // Glyphs and the execution point stay on their lines as lines are inserted above them.
    view.update(&mut cx, |v, cx| {
        v.set_breakpoint_glyphs(
            vec![
                (2, BreakpointGlyph::Enabled),
                (3, BreakpointGlyph::Conditional),
            ],
            cx,
        );
        let start = src.find("return").unwrap();
        v.set_execution_point(Some((start..start + 31, ExecutionKind::Current)), cx);
    });
    cx.run_until_parked();
    view.update(&mut cx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// one\n// two\n");
        })
    });
    cx.run_until_parked();
    assert_eq!(
        view.read_with(&cx, |v, _| v.breakpoint_glyphs()),
        [
            (4, BreakpointGlyph::Enabled),
            (5, BreakpointGlyph::Conditional)
        ]
    );
    assert_eq!(
        view.read_with(&cx, |v, _| v.execution_point()),
        Some((4, ExecutionKind::Current))
    );
    view.update(&mut cx, |v, cx| v.set_execution_point(None, cx));
    assert_eq!(view.read_with(&cx, |v, _| v.execution_point()), None);

    // The data-tip expression is the member chain up to the hovered member.
    let text = text(&view, &mut cx);
    let expr = |cx: &mut VisualTestContext, needle: &str| {
        let at = text.find(needle).unwrap() + 1;
        view.read_with(cx, |v, _| v.expression_at(at))
            .map(|(_, e)| e)
    };
    assert_eq!(expr(&mut cx, "Name").as_deref(), Some("this.order.Name"));
    assert_eq!(expr(&mut cx, "order").as_deref(), Some("this.order"));
    assert_eq!(expr(&mut cx, "return").as_deref(), Some("return"));
    assert_eq!(expr(&mut cx, "{ }"), None);
    let (range, e) = view
        .read_with(&cx, |v, _| v.expression_at(text.find("Length").unwrap()))
        .unwrap();
    assert_eq!(&text[range], e);
    // A data tip uses Quick Info's popup.
    let id = view.update(&mut cx, |v, cx| {
        let start = text.find("this.order").unwrap();
        v.open_data_tip(start..start + 10, cx)
    });
    view.update(&mut cx, |v, cx| {
        v.set_hover(id, Some("this.order = {App.Order}"), None, cx)
    });
    let tip = view.read_with(&cx, |v, _| v.hover()).unwrap();
    assert_eq!(tip.text.as_deref(), Some("this.order = {App.Order}"));
}
