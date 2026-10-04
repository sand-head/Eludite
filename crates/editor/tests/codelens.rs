//! Headless GPUI tests of the editor's CodeLens rows (brief 0052), without a language server: lens rows take layout
//! height and never move buffer lines or the caret, clicks and Ctrl+K, Ctrl+Q activate indicators, the request is
//! debounced after edits and a stale answer is dropped, unresolved lenses are resolved as they come within 50 lines of
//! the visible range, and turning lenses off removes the rows in one reflow that keeps the text in place.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::Duration;

use eludite_editor::intellisense::{CODE_LENS_DEBOUNCE, CODE_LENS_MARGIN};
use eludite_editor::{Buffer, CodeLens, EditorEvent, EditorView, key_bindings};
use eludite_ui::TestGlyph;
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, Pixels, Point, TestAppContext,
    VisualTestContext, point, px,
};

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
    fn take_events(&self) -> Vec<EditorEvent> {
        std::mem::take(&mut *self.events.borrow_mut())
    }

    /// The ids of the lens requests reported since the last call.
    fn requests(&self) -> Vec<u64> {
        self.take_events()
            .into_iter()
            .filter_map(|e| match e {
                EditorEvent::CodeLensRequested { id } => Some(id),
                _ => None,
            })
            .collect()
    }

    fn resolves(&self) -> Vec<u64> {
        self.take_events()
            .into_iter()
            .flat_map(|e| match e {
                EditorEvent::CodeLensResolve { ids } => ids,
                _ => Vec::new(),
            })
            .collect()
    }

    fn enable(&mut self) -> u64 {
        self.view
            .update(&mut self.cx, |v, cx| v.set_code_lens_enabled(true, cx));
        self.cx.run_until_parked();
        let r = self.requests();
        assert_eq!(r.len(), 1, "one request when turned on: {r:?}");
        r[0]
    }

    fn set(&mut self, id: u64, lenses: Vec<CodeLens>) -> bool {
        let ok = self
            .view
            .update(&mut self.cx, |v, cx| v.set_code_lenses(id, lenses, cx));
        self.cx.run_until_parked();
        ok
    }

    fn offset_of(&self, needle: &str) -> usize {
        self.view
            .read_with(&self.cx, |v, _| v.editor().text())
            .find(needle)
            .unwrap_or_else(|| panic!("{needle} not in the text"))
    }

    fn caret(&self) -> usize {
        self.view
            .read_with(&self.cx, |v, _| v.editor().primary_selection().head)
    }

    fn click(&mut self, at: Point<Pixels>) {
        self.cx.simulate_click(at, Modifiers::none());
        self.cx.run_until_parked();
    }

    /// The window position of the text at `offset` (a little inside the character).
    fn at(&self, offset: usize) -> Point<Pixels> {
        let p = self
            .view
            .read_with(&self.cx, |v, _| v.pixel_position_for_offset(offset))
            .expect("row painted");
        point(p.x + px(2.), p.y + px(5.))
    }

    fn type_text(&mut self, text: &str) {
        self.cx.simulate_input(text);
        self.cx.run_until_parked();
    }
}

fn lens(id: u64, offset: usize, title: &str, resolved: bool) -> CodeLens {
    CodeLens {
        id,
        offset,
        title: title.into(),
        resolved,
        glyph: None,
    }
}

const CLASS: &str = "class Calculator\n{\n    void Add() {}\n\n    void Subtract() {}\n}\n";

#[gpui::test]
fn lens_rows_take_layout_height_and_never_shift_buffer_lines_or_the_caret(cx: &mut TestAppContext) {
    let mut ed = open(cx, CLASS);
    let caret = ed.offset_of("Add") + 1;
    ed.view.update(&mut ed.cx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(caret))
    });
    let (lh, lens_h) = ed
        .view
        .read_with(&ed.cx, |v, _| (v.line_height(), v.lens_height()));
    assert!(
        lens_h > px(0.) && lens_h < lh,
        "a lens row is shorter than a text line"
    );
    let before: Vec<Pixels> = (0..5)
        .map(|r| ed.view.read_with(&ed.cx, |v, _| v.row_top(r)))
        .collect();
    let id = ed.enable();
    let (class, add, sub) = (
        ed.offset_of("Calculator"),
        ed.offset_of("Add"),
        ed.offset_of("Subtract"),
    );
    assert!(ed.set(
        id,
        vec![
            lens(1, class, "2 references", true),
            lens(2, add, "1 reference", true),
            lens(3, add, "Run Test", true),
            lens(4, sub, "0 references", true),
        ],
    ));
    // Rows 0, 2 and 4 have a lens row above them: every later line moves down by its height, nothing else moves.
    let after: Vec<Pixels> = (0..5)
        .map(|r| ed.view.read_with(&ed.cx, |v, _| v.row_top(r)))
        .collect();
    assert_eq!(
        after,
        vec![
            before[0] + lens_h,
            before[1] + lens_h,
            before[2] + lens_h * 2.,
            before[3] + lens_h * 2.,
            before[4] + lens_h * 3.,
        ]
    );
    let snapshot = ed.view.read_with(&ed.cx, |v, _| v.code_lenses());
    assert_eq!(
        snapshot.iter().map(|l| (l.id, l.row)).collect::<Vec<_>>(),
        [(1, 0), (2, 2), (3, 2), (4, 4)]
    );
    // The caret did not move, and the text is drawn on its line below the lens row.
    assert_eq!(ed.caret(), caret);
    let y = ed
        .view
        .read_with(&ed.cx, |v, _| v.pixel_position_for_offset(add).unwrap().y);
    let top = ed.view.read_with(&ed.cx, |v, _| v.row_top(2));
    let origin = ed.at(0).y - px(5.) - ed.view.read_with(&ed.cx, |v, _| v.row_top(0));
    assert_eq!(y, origin + top);
    // A click on the text lands on the buffer position under the pointer; a click on a lens row's empty part does
    // nothing.
    let target = ed.offset_of("Subtract") + 2;
    let p = ed.at(target);
    ed.click(p);
    assert_eq!(ed.caret(), target);
    let blank = point(p.x + px(400.), p.y - lens_h);
    ed.click(blank);
    assert_eq!(ed.caret(), target, "the lens row holds no text");
    // An edit above moves the lens rows with their members.
    ed.view.update(&mut ed.cx, |v, cx| {
        v.update_editor(cx, |e| {
            e.set_caret(0);
            e.insert("// header\n");
        })
    });
    ed.cx.run_until_parked();
    let rows: Vec<u32> = ed.view.read_with(&ed.cx, |v, _| {
        v.code_lenses().iter().map(|l| l.row).collect()
    });
    assert_eq!(rows, [1, 3, 3, 5]);
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.vertical_layout().lens_rows().to_vec()),
        [1, 3, 5]
    );
}

#[gpui::test]
fn indicators_activate_on_click_and_from_the_keyboard_menu(cx: &mut TestAppContext) {
    let mut ed = open(cx, CLASS);
    let id = ed.enable();
    let (class, add) = (ed.offset_of("Calculator"), ed.offset_of("Add"));
    ed.set(
        id,
        vec![
            lens(1, class, "2 references", true),
            lens(2, add, "1 reference", true),
            CodeLens {
                glyph: Some(TestGlyph::Passed),
                ..lens(3, add, "Run Test (12 ms)", true)
            },
            lens(4, add, "Debug Test", true),
        ],
    );
    ed.take_events();
    // Indicators are laid out left to right on their row, in order.
    let bounds: Vec<_> = (1..=4)
        .map(|i| {
            ed.view
                .read_with(&ed.cx, |v, _| v.code_lens_bounds(i))
                .unwrap_or_else(|| panic!("lens {i} painted"))
        })
        .collect();
    assert!(bounds[1].right() <= bounds[2].left() && bounds[2].right() <= bounds[3].left());
    assert_eq!(bounds[1].top(), bounds[3].top());
    assert!(bounds[0].top() < bounds[1].top());
    let caret = ed.caret();
    // Hovering underlines an indicator; clicking it activates it without moving the caret.
    ed.cx
        .simulate_mouse_move(bounds[2].center(), None, Modifiers::none());
    ed.cx.run_until_parked();
    assert_eq!(
        ed.view.read_with(&ed.cx, |v, _| v.hovered_code_lens()),
        Some(3)
    );
    ed.click(bounds[2].center());
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CodeLensActivated {
            id: 3,
            keyboard: false
        }]
    );
    assert_eq!(ed.caret(), caret);
    // Ctrl+K, Ctrl+Q opens the first indicator of the member at the caret: the nearest lens row at or above it.
    let inside_add = ed.offset_of("{}") + 1;
    ed.view.update(&mut ed.cx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(inside_add))
    });
    ed.take_events();
    ed.cx.simulate_keystrokes("ctrl-k ctrl-q");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CodeLensActivated {
            id: 2,
            keyboard: true
        }]
    );
    ed.view.update(&mut ed.cx, |v, cx| {
        v.update_editor(cx, |e| e.set_caret(class + 2))
    });
    ed.take_events();
    ed.cx.simulate_keystrokes("ctrl-k ctrl-q");
    assert_eq!(
        ed.take_events(),
        [EditorEvent::CodeLensActivated {
            id: 1,
            keyboard: true
        }]
    );
    // A test's glyph and title change in place (the last outcome), without a relayout.
    let top = ed.view.read_with(&ed.cx, |v, _| v.row_top(2));
    ed.view.update(&mut ed.cx, |v, cx| {
        assert!(v.update_code_lens(
            3,
            "Run Test (40 ms)".into(),
            true,
            Some(TestGlyph::Failed),
            cx
        ))
    });
    ed.cx.run_until_parked();
    let shown = ed.view.read_with(&ed.cx, |v, _| v.code_lenses());
    assert_eq!(shown[2].title, "Run Test (40 ms)");
    assert_eq!(shown[2].glyph, Some(TestGlyph::Failed));
    assert_eq!(ed.view.read_with(&ed.cx, |v, _| v.row_top(2)), top);
}

#[gpui::test]
fn requests_are_debounced_after_edits_and_stale_answers_are_dropped(cx: &mut TestAppContext) {
    let mut ed = open(cx, CLASS);
    // Nothing is asked for while lenses are off.
    ed.type_text("x");
    ed.cx.executor().advance_clock(CODE_LENS_DEBOUNCE * 2);
    ed.cx.run_until_parked();
    assert!(ed.requests().is_empty());
    let first = ed.enable();
    assert!(ed.set(first, vec![lens(1, 0, "1 reference", true)]));
    // Typing restarts the 150 ms debounce: no request until the person stops for that long.
    ed.type_text("a");
    ed.cx
        .executor()
        .advance_clock(CODE_LENS_DEBOUNCE - Duration::from_millis(1));
    ed.cx.run_until_parked();
    assert!(ed.requests().is_empty());
    ed.type_text("b");
    ed.cx
        .executor()
        .advance_clock(CODE_LENS_DEBOUNCE - Duration::from_millis(1));
    ed.cx.run_until_parked();
    assert!(ed.requests().is_empty());
    ed.cx.executor().advance_clock(Duration::from_millis(1));
    ed.cx.run_until_parked();
    let second = ed.requests();
    assert_eq!(second.len(), 1);
    assert!(second[0] > first);
    // An edit while the request is in flight makes its answer stale: dropped, and the lens rows stay as they were.
    ed.type_text("c");
    assert!(!ed.set(second[0], vec![lens(2, 0, "9 references", true)]));
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.code_lenses()[0].title.clone()),
        "1 reference"
    );
    ed.cx.executor().advance_clock(CODE_LENS_DEBOUNCE);
    ed.cx.run_until_parked();
    let third = ed.requests();
    assert_eq!(third.len(), 1);
    // A refresh supersedes the request in flight: the older answer is dropped, the newest applied.
    ed.view
        .update(&mut ed.cx, |v, cx| v.refresh_code_lenses(cx));
    ed.cx.run_until_parked();
    let fourth = ed.requests();
    assert_eq!(fourth.len(), 1);
    assert!(!ed.set(third[0], vec![lens(3, 0, "3 references", true)]));
    assert!(ed.set(fourth[0], vec![lens(4, 0, "4 references", true)]));
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.code_lenses()[0].title.clone()),
        "4 references"
    );
    // The same answer twice is applied once.
    assert!(!ed.set(fourth[0], vec![]));
}

#[gpui::test]
fn unresolved_lenses_resolve_within_50_lines_of_the_visible_range(cx: &mut TestAppContext) {
    let text: String = (0..600).map(|i| format!("void M{i}() {{}}\n")).collect();
    let mut ed = open(cx, &text);
    let id = ed.enable();
    // An unresolved lens above every tenth line.
    let lenses: Vec<CodeLens> = (0..60)
        .map(|i| {
            let offset = ed.offset_of(&format!("M{}()", i * 10));
            lens(i as u64 + 1, offset, "- references", false)
        })
        .collect();
    ed.set(id, lenses);
    let rows_of = |ed: &Ed, ids: &[u64]| -> Vec<u32> {
        let shown = ed.view.read_with(&ed.cx, |v, _| v.code_lenses());
        ids.iter()
            .map(|id| shown.iter().find(|l| l.id == *id).unwrap().row)
            .collect()
    };
    let visible = ed.view.read_with(&ed.cx, |v, _| v.visible_rows());
    let asked = ed.resolves();
    assert!(!asked.is_empty());
    let expected: Vec<u64> = (0..60u64)
        .filter(|i| (i * 10) < visible.end as u64 + CODE_LENS_MARGIN as u64)
        .map(|i| i + 1)
        .collect();
    assert_eq!(asked, expected, "visible {visible:?}");
    // Resolving updates the title in place.
    ed.view.update(&mut ed.cx, |v, cx| {
        assert!(v.update_code_lens(1, "3 references".into(), true, None, cx))
    });
    // Scrolling far down asks for the lenses that came into the window, once each.
    ed.view.update(&mut ed.cx, |v, cx| v.scroll_to_row(300, cx));
    ed.cx.run_until_parked();
    let visible = ed.view.read_with(&ed.cx, |v, _| v.visible_rows());
    let more = ed.resolves();
    assert!(!more.is_empty());
    for row in rows_of(&ed, &more) {
        assert!(
            row + CODE_LENS_MARGIN >= visible.start && row < visible.end + CODE_LENS_MARGIN,
            "row {row} outside the window around {visible:?}"
        );
    }
    assert!(more.iter().all(|i| !asked.contains(i)));
    ed.view.update(&mut ed.cx, |v, cx| v.scroll_to_row(301, cx));
    ed.cx.run_until_parked();
    assert!(ed.resolves().is_empty(), "nothing is asked twice");
    // A failed resolve is asked again when it is next in the window.
    let retry = more[0];
    ed.view.update(&mut ed.cx, |v, cx| {
        v.code_lens_resolve_failed(retry);
        v.scroll_to_row(300, cx);
    });
    ed.cx.run_until_parked();
    assert_eq!(ed.resolves(), [retry]);
}

#[gpui::test]
fn turning_lenses_off_removes_the_rows_in_one_reflow_that_keeps_the_text_in_place(
    cx: &mut TestAppContext,
) {
    let text: String = (0..400).map(|i| format!("void M{i}() {{}}\n")).collect();
    let mut ed = open(cx, &text);
    let id = ed.enable();
    // 200 lens rows: one above every other line.
    let lenses: Vec<CodeLens> = (0..200)
        .map(|i| {
            lens(
                i + 1,
                ed.offset_of(&format!("M{}()", i * 2)),
                "1 reference",
                true,
            )
        })
        .collect();
    ed.view.update(&mut ed.cx, |v, cx| v.scroll_to_row(100, cx));
    ed.cx.run_until_parked();
    let on_screen = |ed: &Ed, row: u32| {
        ed.view
            .read_with(&ed.cx, |v, _| v.row_top(row) - v.scroll_position().y)
    };
    let before = on_screen(&ed, 100);
    ed.set(id, lenses);
    assert_eq!(
        on_screen(&ed, 100),
        before,
        "lenses arriving do not move the text"
    );
    assert_eq!(ed.view.read_with(&ed.cx, |v, _| v.code_lens_count()), 200);
    assert_eq!(
        ed.view
            .read_with(&ed.cx, |v, _| v.vertical_layout().lens_rows().len()),
        200
    );
    let caret = ed.caret();
    ed.view
        .update(&mut ed.cx, |v, cx| v.set_code_lens_enabled(false, cx));
    ed.cx.run_until_parked();
    assert_eq!(
        on_screen(&ed, 100),
        before,
        "turning lenses off does not move the text"
    );
    assert!(ed.view.read_with(&ed.cx, |v, _| v.code_lenses().is_empty()));
    assert_eq!(ed.caret(), caret);
    assert!(ed.requests().is_empty());
    // Off: edits ask for nothing.
    ed.type_text("z");
    ed.cx.executor().advance_clock(CODE_LENS_DEBOUNCE * 2);
    ed.cx.run_until_parked();
    assert!(ed.requests().is_empty());
}
