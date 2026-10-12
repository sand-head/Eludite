//! Headless GPUI tests of brief 0068's editing operations through the keyboard and the mouse: Tab and Shift+Tab,
//! brace completion and Enter, comments, moving lines, Expand Selection with a tree from the syntax thread, Alt+drag
//! box selection, Quick Replace, Go To Line and the context menu hook; and the budgets on a 50,000-line file.

use std::rc::Rc;

use eludite_editor::syntax::LanguageRegistry;
use eludite_editor::{Buffer, ContextMenuEntry, Editor, EditorView, SelectionRange, key_bindings};
use eludite_test_support::{assert_budget, hang_bound};
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Point, TestAppContext, VisualTestContext, point, px,
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
    (view, vcx)
}

fn text(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, _| v.editor().text())
}

fn selections(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> Vec<SelectionRange> {
    view.read_with(cx, |v, _| v.editor().selections())
}

fn select(view: &Entity<EditorView>, cx: &mut VisualTestContext, ranges: Vec<SelectionRange>) {
    view.update(cx, |v, cx| {
        let n = ranges.len();
        v.update_editor(cx, |e| e.set_selections(ranges, n - 1))
    });
}

fn selected(view: &Entity<EditorView>, cx: &mut VisualTestContext) -> String {
    view.read_with(cx, |v, _| {
        let e = v.editor();
        e.buffer().text_for_range(e.primary_selection().range())
    })
}

#[gpui::test]
fn tab_and_shift_tab_indent_and_outdent_selected_lines(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "one\ntwo\nthree\n", None);
    select(&view, &mut cx, vec![SelectionRange { tail: 0, head: 9 }]);
    cx.simulate_keystrokes("tab");
    // Tab no longer replaces the selection: every touched line is indented and the selection stays.
    assert_eq!(text(&view, &mut cx), "    one\n    two\n    three\n");
    assert_eq!(selected(&view, &mut cx), "    one\n    two\n    t");
    cx.simulate_keystrokes("shift-tab");
    assert_eq!(text(&view, &mut cx), "one\ntwo\nthree\n");
    cx.simulate_keystrokes("ctrl-z");
    assert_eq!(text(&view, &mut cx), "    one\n    two\n    three\n");
}

#[gpui::test]
fn brace_completion_and_enter_between_braces(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "class C ", Some("csharp"));
    view.update(&mut cx, |v, cx| v.update_editor(cx, |e| e.set_caret(8)));
    cx.simulate_input("{");
    assert_eq!(text(&view, &mut cx), "class C {}");
    cx.simulate_keystrokes("enter");
    assert_eq!(text(&view, &mut cx), "class C {\n    \n}");
    cx.simulate_input("M(");
    assert_eq!(text(&view, &mut cx), "class C {\n    M()\n}");
    cx.simulate_input(")");
    assert_eq!(text(&view, &mut cx), "class C {\n    M()\n}");
}

#[gpui::test]
fn comment_and_uncomment_through_the_chords(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "a();\nb();\n", Some("csharp"));
    select(&view, &mut cx, vec![SelectionRange { tail: 0, head: 9 }]);
    cx.simulate_keystrokes("ctrl-k ctrl-c");
    assert_eq!(text(&view, &mut cx), "// a();\n// b();\n");
    cx.simulate_keystrokes("ctrl-k ctrl-u");
    assert_eq!(text(&view, &mut cx), "a();\nb();\n");
    cx.simulate_keystrokes("ctrl-/");
    assert_eq!(text(&view, &mut cx), "// a();\n// b();\n");
}

#[gpui::test]
fn line_commands_through_their_keys(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "one\ntwo\nthree", None);
    select(&view, &mut cx, vec![SelectionRange::caret(5)]);
    cx.simulate_keystrokes("alt-down");
    assert_eq!(text(&view, &mut cx), "one\nthree\ntwo");
    cx.simulate_keystrokes("alt-up alt-up");
    assert_eq!(text(&view, &mut cx), "two\none\nthree");
    cx.simulate_keystrokes("ctrl-d");
    assert_eq!(text(&view, &mut cx), "two\ntwo\none\nthree");
    cx.simulate_keystrokes("ctrl-shift-l");
    assert_eq!(text(&view, &mut cx), "two\none\nthree");
    cx.simulate_keystrokes("ctrl-shift-u");
    // The caret kept its column; the character after it is upper-cased.
    assert_eq!(text(&view, &mut cx), "two\noNe\nthree");
}

#[gpui::test]
fn expand_selection_waits_for_a_tree_from_the_syntax_thread(cx: &mut TestAppContext) {
    let src = "class C { void M() { var x = F(a + 1); } }";
    let (view, mut cx) = open(cx, src, Some("csharp"));
    let at = src.find("a + 1").unwrap();
    select(&view, &mut cx, vec![SelectionRange::caret(at)]);
    let expand = |cx: &mut VisualTestContext| {
        cx.simulate_keystrokes("alt-shift-=");
        // The tree is parsed on the syntax thread, a real OS thread: wait for it within a hang bound.
        let deadline = std::time::Instant::now() + hang_bound(std::time::Duration::from_secs(20));
        cx.executor().allow_parking();
        while view.read_with(cx, |v, _| v.is_expanding_selection()) {
            assert!(std::time::Instant::now() < deadline, "the tree arrives");
            cx.run_until_parked();
            std::thread::yield_now();
        }
    };
    expand(&mut cx);
    assert_eq!(selected(&view, &mut cx), "a");
    expand(&mut cx);
    assert_eq!(selected(&view, &mut cx), "a + 1");
    expand(&mut cx);
    assert_eq!(selected(&view, &mut cx), "(a + 1)");
    expand(&mut cx);
    assert_eq!(selected(&view, &mut cx), "F(a + 1)");
    cx.simulate_keystrokes("alt-shift--");
    assert_eq!(selected(&view, &mut cx), "(a + 1)");
}

#[gpui::test]
fn alt_drag_makes_a_box_and_typing_edits_every_row(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "abcdef\nab\nabcdef\n", None);
    let pos = |cx: &mut VisualTestContext, offset: usize| -> Point<gpui::Pixels> {
        let p = view
            .read_with(cx, |v, _| v.pixel_position_for_offset(offset))
            .expect("row painted");
        point(p.x + px(0.5), p.y + px(5.))
    };
    let start = pos(&mut cx, 1);
    let end_row = pos(&mut cx, 14);
    let alt = Modifiers {
        alt: true,
        ..Default::default()
    };
    cx.simulate_event(MouseDownEvent {
        position: start,
        modifiers: alt,
        button: MouseButton::Left,
        click_count: 1,
        first_mouse: false,
    });
    cx.simulate_event(MouseMoveEvent {
        position: end_row,
        modifiers: alt,
        pressed_button: Some(MouseButton::Left),
    });
    cx.simulate_event(MouseUpEvent {
        position: end_row,
        modifiers: alt,
        button: MouseButton::Left,
        click_count: 1,
    });
    let sels = selections(&view, &mut cx);
    assert_eq!(sels.len(), 3, "{sels:?}");
    cx.simulate_input("-");
    assert_eq!(text(&view, &mut cx), "a-ef\na-\na-ef\n");
    // Shift+Alt+Down from a caret is carets on consecutive rows.
    view.update(&mut cx, |v, cx| v.update_editor(cx, |e| e.set_caret(0)));
    cx.simulate_keystrokes("alt-shift-down alt-shift-down");
    assert_eq!(selections(&view, &mut cx).len(), 3);
    cx.simulate_input(">");
    assert_eq!(text(&view, &mut cx), ">a-ef\n>a-\n>a-ef\n");
}

#[gpui::test]
fn quick_replace_replaces_all_as_one_undo_step(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "Foo foo FOO food\n", None);
    cx.simulate_keystrokes("secondary-h");
    assert!(view.read_with(&cx, |v, _| v.is_quick_replace()));
    cx.simulate_input("foo");
    assert_eq!(view.read_with(&cx, |v, _| v.find_status()), "1 of 4");
    // Match Whole Word, in the bar.
    cx.simulate_keystrokes("alt-w");
    assert_eq!(view.read_with(&cx, |v, _| v.find_status()), "1 of 3");
    let replace = view.read_with(&cx, |v, _| v.replace_input().unwrap().clone());
    replace.update(&mut cx, |i, cx| i.set_text("bar", cx));
    cx.simulate_keystrokes("alt-a");
    assert_eq!(text(&view, &mut cx), "bar bar bar food\n");
    assert_eq!(view.read_with(&cx, |v, _| v.find_status()), "3 replaced");
    view.update(&mut cx, |v, cx| {
        v.update_editor(cx, |e| {
            assert!(e.undo());
        })
    });
    assert_eq!(text(&view, &mut cx), "Foo foo FOO food\n");
    cx.simulate_keystrokes("escape");
    assert!(!view.read_with(&cx, |v, _| v.is_find_bar_open()));
}

#[gpui::test]
fn go_to_line_dialog(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "a\nb\n    c\nd", None);
    cx.simulate_keystrokes("ctrl-g");
    assert!(view.read_with(&cx, |v, _| v.is_go_to_line_open()));
    // The caret's line is selected; typing replaces it.
    cx.simulate_input("3");
    cx.simulate_keystrokes("enter");
    assert!(!view.read_with(&cx, |v, _| v.is_go_to_line_open()));
    assert_eq!(selections(&view, &mut cx), vec![SelectionRange::caret(8)]);
    cx.simulate_keystrokes("ctrl-g escape");
    assert!(!view.read_with(&cx, |v, _| v.is_go_to_line_open()));
}

#[gpui::test]
fn right_click_moves_the_caret_and_opens_the_owners_menu(cx: &mut TestAppContext) {
    let (view, mut cx) = open(cx, "hello world\n", None);
    view.update(&mut cx, |v, _| {
        v.set_context_menu_handler(Some(Rc::new(|caret, _| {
            vec![
                ContextMenuEntry::Item {
                    label: format!("At {caret}").into(),
                    shortcut: None,
                    command: "eludite.edit.cut".into(),
                    args: serde_json::json!({}),
                    enabled: true,
                },
                ContextMenuEntry::Separator,
            ]
        })))
    });
    let p = view
        .read_with(&cx, |v, _| v.pixel_position_for_offset(6))
        .unwrap();
    cx.simulate_event(MouseDownEvent {
        position: point(p.x + px(0.5), p.y + px(5.)),
        modifiers: Modifiers::default(),
        button: MouseButton::Right,
        click_count: 1,
        first_mouse: false,
    });
    let menu = view.read_with(&cx, |v, _| v.context_menu().map(<[_]>::to_vec));
    let menu = menu.expect("the menu opens");
    assert!(matches!(&menu[0], ContextMenuEntry::Item { label, .. } if label.as_ref() == "At 6"));
    assert_eq!(selections(&view, &mut cx), vec![SelectionRange::caret(6)]);
}

/// Time `f` once and check it against `budget` (asserted on a quiet machine, reported on CI: ADR-0014).
fn measure(what: &str, budget: std::time::Duration, f: impl FnOnce()) {
    let t = std::time::Instant::now();
    f();
    assert_budget(what, t.elapsed(), budget);
}

/// 50,000 lines of C#.
fn big() -> String {
    (0..50_000)
        .map(|i| format!("    var value{i} = Compute(foo, {i}); // foo\n"))
        .collect()
}

#[test]
fn operations_fit_the_frame_budget_on_a_50k_line_file() {
    let text = big();
    let editor = |text: &str| {
        let mut e = Editor::new(Buffer::new(text));
        e.set_language_editing(eludite_editor::syntax::editing::for_language("csharp"));
        e
    };
    let budget = std::time::Duration::from_millis(8);
    // Move a 1,000-line block down.
    let mut e = editor(&text);
    let start = e
        .buffer()
        .point_to_offset(eludite_editor::text::Point::new(20_000, 0));
    let end = e
        .buffer()
        .point_to_offset(eludite_editor::text::Point::new(21_000, 0));
    e.set_selections(
        vec![SelectionRange {
            tail: start,
            head: end,
        }],
        0,
    );
    measure("move a 1,000-line block", budget, || {
        assert!(e.move_lines(false));
    });
    // Indent and comment a 1,000-line block.
    measure("indent a 1,000-line block", budget, || {
        assert!(e.indent_lines());
    });
    measure("comment a 1,000-line block", budget, || {
        assert!(e.comment_selection());
    });
    // Duplicate and Expand Selection at a caret.
    e.set_caret(start + 10);
    measure("duplicate a line", budget, || {
        assert!(e.duplicate());
    });
    measure("expand selection lexically", budget, || {
        assert!(e.expand_selection(None));
    });
    // Replace All of 10,000 matches.
    let mut e = editor(
        &(0..50_000)
            .map(|i| {
                if i % 5 == 0 {
                    "    target();\n".to_owned()
                } else {
                    format!("    other{i}();\n")
                }
            })
            .collect::<String>(),
    );
    e.set_find_query(eludite_editor::FindQuery {
        text: "target".into(),
        case_sensitive: true,
    });
    assert_eq!(e.find_match_count(), 10_000);
    measure("replace all of 10,000 matches", budget, || {
        assert_eq!(e.replace_all("renamed"), 10_000);
    });
    assert!(e.undo());
    assert_eq!(e.find_match_count(), 10_000);
}
