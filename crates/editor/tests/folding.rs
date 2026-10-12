//! Headless GPUI tests of brief 0067's display map in the editor view: fold ranges from tree-sitter on corpus C# with
//! `#region`s and from a fake server (a stale answer dropped), collapsing hides rows and the caret moved into a fold
//! reveals it, clicks and Up and Down across a collapsed fold and across wrapped rows, the scroll bar's thumb and a
//! marker click, sticky lines at a scroll position, brace matching and Go To Brace, zoom by Ctrl+wheel, and the
//! keystroke budget on a 50k-line file with 1,000 collapsed folds and word wrap on.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eludite_editor::syntax::LanguageRegistry;
use eludite_editor::text;
use eludite_editor::{
    Buffer, DisplayEvent, EditorView, FoldKind, FoldRange, FoldSource, MarkerKind, key_bindings,
};
use gpui::{
    AppContext as _, Entity, Focusable as _, Modifiers, Pixels, Point, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TouchPhase, VisualTestContext, point, px, size,
};

const CORPUS: &str = include_str!("../../../corpus/languages/csharp/Outlining.cs");

struct Ed {
    view: Entity<EditorView>,
    cx: VisualTestContext,
    events: Rc<RefCell<Vec<DisplayEvent>>>,
}

fn open(cx: &mut TestAppContext, text: &str, language: Option<&str>) -> Ed {
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let language = language.and_then(|id| LanguageRegistry::with_builtins().by_id(id));
    let text = text.to_owned();
    let window = cx.update(|cx| {
        cx.open_window(
            gpui::WindowOptions {
                window_bounds: Some(gpui::WindowBounds::Windowed(gpui::Bounds::new(
                    point(px(0.), px(0.)),
                    size(px(800.), px(600.)),
                ))),
                ..Default::default()
            },
            |window, cx| {
                let view = cx.new(|cx| {
                    let mut buffer = Buffer::new(&text);
                    buffer.set_group_interval(Duration::ZERO);
                    EditorView::new(buffer, language, cx)
                });
                window.focus(&view.focus_handle(cx), cx);
                view
            },
        )
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let view = window.root(&mut vcx).unwrap();
    let events: Rc<RefCell<Vec<DisplayEvent>>> = Rc::default();
    let sink = events.clone();
    vcx.update(|_, cx| {
        cx.subscribe(&view, move |_, e: &DisplayEvent, _| {
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
    /// Fold queries run on the syntax thread, a real OS thread outside the test scheduler.
    fn wait_for_folds(&mut self) {
        self.cx.executor().allow_parking();
        let view = self.view.clone();
        let cx = &mut self.cx;
        let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(10));
        loop {
            cx.run_until_parked();
            if view.read_with(cx, |v, _| !v.folds().is_empty()) {
                return;
            }
            assert!(Instant::now() < deadline, "the syntax folds did not arrive");
            std::thread::yield_now();
        }
    }

    fn read<R>(&mut self, f: impl FnOnce(&EditorView) -> R) -> R {
        self.view.read_with(&self.cx, |v, _| f(v))
    }

    fn update<R>(
        &mut self,
        f: impl FnOnce(&mut EditorView, &mut gpui::Context<EditorView>) -> R,
    ) -> R {
        let r = self.view.update(&mut self.cx, f);
        self.cx.run_until_parked();
        r
    }

    fn folds(&mut self) -> Vec<(u32, u32, FoldKind, bool)> {
        self.read(|v| {
            v.folds()
                .iter()
                .map(|f| (f.start_row, f.end_row, f.kind, f.collapsed))
                .collect()
        })
    }

    fn caret(&mut self) -> text::Point {
        self.read(|v| v.editor().primary_head())
    }

    fn set_caret(&mut self, row: u32, column: u32) {
        self.update(|v, cx| {
            v.update_editor(cx, |e| {
                let o = e.buffer().point_to_offset(text::Point::new(row, column));
                e.set_caret(o)
            })
        });
    }

    fn position(&mut self, row: u32, column: u32) -> Point<Pixels> {
        let p = self.read(|v| {
            let o = v
                .editor()
                .buffer()
                .point_to_offset(text::Point::new(row, column));
            v.pixel_position_for_offset(o)
        });
        let p = p.expect("row painted");
        point(p.x + px(1.), p.y + px(5.))
    }
}

fn collapsed_rows(ed: &mut Ed) -> Vec<u32> {
    ed.folds()
        .into_iter()
        .filter(|f| f.3)
        .map(|f| f.0)
        .collect()
}

#[gpui::test]
fn syntax_folds_on_corpus_csharp_with_regions(cx: &mut TestAppContext) {
    let mut ed = open(cx, CORPUS, Some("csharp"));
    ed.wait_for_folds();
    let folds = ed.folds();
    let has = |s: u32, e: u32, k: FoldKind| folds.iter().any(|f| (f.0, f.1, f.2) == (s, e, k));
    assert!(has(0, 2, FoldKind::Imports), "{folds:?}");
    assert!(has(4, 56, FoldKind::Namespace), "{folds:?}");
    assert!(has(6, 8, FoldKind::Comment), "{folds:?}");
    assert!(has(9, 55, FoldKind::Type), "{folds:?}");
    assert!(has(11, 14, FoldKind::Region), "{folds:?}");
    assert!(has(16, 24, FoldKind::Region), "{folds:?}");
    assert!(has(17, 21, FoldKind::Member), "{folds:?}");
    assert!(has(26, 29, FoldKind::Member), "{folds:?}");
    assert!(has(31, 32, FoldKind::Comment), "{folds:?}");
    assert!(has(33, 41, FoldKind::Member), "{folds:?}");
    assert!(has(35, 38, FoldKind::Block), "{folds:?}");
    assert!(has(43, 52, FoldKind::Member), "{folds:?}");
    // The expression-bodied member is one line: no fold.
    assert!(!folds.iter().any(|f| f.0 == 54));
    assert_eq!(ed.read(|v| v.fold_source()), FoldSource::Syntax);
    // Regions read their names.
    let label = ed.read(|v| {
        v.folds()
            .iter()
            .find(|f| f.start_row == 11)
            .and_then(|f| f.label.clone())
    });
    assert_eq!(label.as_deref(), Some("Fields"));
    // Nothing starts collapsed unless asked.
    assert!(collapsed_rows(&mut ed).is_empty());
}

#[gpui::test]
fn regions_collapse_on_open_when_asked(cx: &mut TestAppContext) {
    cx.update(|cx| cx.bind_keys(key_bindings()));
    let language = LanguageRegistry::with_builtins().by_id("csharp");
    let window = cx.update(|cx| {
        cx.open_window(Default::default(), |_, cx| {
            cx.new(|cx| {
                let mut v = EditorView::new(Buffer::new(CORPUS), language, cx);
                v.set_collapse_regions_on_open(true);
                v
            })
        })
        .unwrap()
    });
    let mut vcx = VisualTestContext::from_window(window.into(), cx);
    let view = window.root(&mut vcx).unwrap();
    let mut ed = Ed {
        view,
        cx: vcx,
        events: Rc::default(),
    };
    ed.wait_for_folds();
    assert_eq!(collapsed_rows(&mut ed), vec![11, 16]);
}

#[gpui::test]
fn collapsing_hides_rows_and_the_caret_moved_into_a_fold_reveals_it(cx: &mut TestAppContext) {
    let mut ed = open(cx, CORPUS, Some("csharp"));
    ed.wait_for_folds();
    // Toggle Outlining Expansion on the Add method's header.
    ed.set_caret(33, 8);
    assert!(ed.update(|v, cx| v.toggle_outlining_expansion(None, cx)));
    assert_eq!(collapsed_rows(&mut ed), vec![33]);
    let layout = ed.read(|v| v.vertical_layout().clone());
    assert!(layout.is_hidden(34) && layout.is_hidden(41) && !layout.is_hidden(42));
    // Row 42 now sits right under the header.
    assert_eq!(
        layout.line_top(42) - layout.line_top(33),
        layout.line_height()
    );
    // Down from the header lands on row 42, past the hidden rows; Up comes back.
    ed.cx.simulate_keystrokes("down");
    assert_eq!(ed.caret().row, 42);
    ed.cx.simulate_keystrokes("up");
    assert_eq!(ed.caret().row, 33);
    assert_eq!(
        collapsed_rows(&mut ed),
        vec![33],
        "moving around a fold leaves it"
    );
    // A click under the header lands on row 42.
    let at = ed.position(42, 0);
    ed.cx.simulate_click(at, Modifiers::none());
    assert_eq!(ed.caret().row, 42);
    // Moving the caret into hidden text (find, go to definition, a breakpoint stop) reveals it.
    ed.set_caret(39, 12);
    assert!(collapsed_rows(&mut ed).is_empty());
    // An edit inside hidden text reveals it too.
    ed.set_caret(33, 8);
    ed.update(|v, cx| v.toggle_outlining_expansion(None, cx));
    assert_eq!(collapsed_rows(&mut ed), vec![33]);
    ed.update(|v, cx| {
        v.update_editor(cx, |e| {
            let o = e.buffer().point_to_offset(text::Point::new(40, 12));
            e.apply_edits(vec![(o..o, "x".into())]);
        })
    });
    assert!(collapsed_rows(&mut ed).is_empty());
}

#[gpui::test]
fn the_outlining_commands(cx: &mut TestAppContext) {
    let mut ed = open(cx, CORPUS, Some("csharp"));
    ed.wait_for_folds();
    // Collapse to Definitions: members and regions; types and namespaces stay open.
    ed.update(|v, cx| v.collapse_to_definitions(cx));
    let c = collapsed_rows(&mut ed);
    assert!(c.contains(&11) && c.contains(&16) && c.contains(&33) && c.contains(&43));
    assert!(!c.contains(&4) && !c.contains(&9) && !c.contains(&35));
    // Toggle All: something is collapsed, so all expand; again, all collapse.
    ed.update(|v, cx| v.toggle_all_outlining(cx));
    assert!(collapsed_rows(&mut ed).is_empty());
    ed.update(|v, cx| v.toggle_all_outlining(cx));
    assert_eq!(collapsed_rows(&mut ed).len(), ed.folds().len());
    // The caret was inside: it moved to the outermost collapsed header it was under.
    assert!(!ed.read(|v| v.vertical_layout().is_hidden(v.editor().primary_head().row)));
    ed.update(|v, cx| v.expand_all_outlining(cx));
    assert!(collapsed_rows(&mut ed).is_empty());
    // Collapse Current Region then Expand Current Region at the caret.
    ed.set_caret(37, 16);
    ed.update(|v, cx| v.collapse_current_region(cx));
    assert_eq!(collapsed_rows(&mut ed), vec![35]);
    assert_eq!(ed.caret().row, 35, "the caret moved out of the hidden rows");
    ed.update(|v, cx| v.expand_current_region(cx));
    assert!(collapsed_rows(&mut ed).is_empty());
    // Hide Selection makes a region; Stop Hiding Current removes it.
    ed.update(|v, cx| {
        v.update_editor(cx, |e| {
            let a = e.buffer().point_to_offset(text::Point::new(39, 0));
            let b = e.buffer().point_to_offset(text::Point::new(40, 5));
            e.set_selections(vec![eludite_editor::SelectionRange { tail: a, head: b }], 0)
        })
    });
    ed.update(|v, cx| v.hide_selection(cx));
    assert!(ed.read(|v| v.vertical_layout().is_hidden(40)));
    assert!(ed.folds().iter().any(|f| f.2 == FoldKind::Manual));
    ed.update(|v, cx| v.stop_hiding_current(cx));
    assert!(!ed.folds().iter().any(|f| f.2 == FoldKind::Manual));
    // Stop Outlining: no folds, no margin; Start Automatic Outlining brings them back.
    ed.update(|v, cx| v.stop_outlining(cx));
    assert!(ed.folds().is_empty());
    assert!(!ed.read(|v| v.outlining_enabled()));
    ed.update(|v, cx| v.start_automatic_outlining(cx));
    ed.wait_for_folds();
    assert!(ed.read(|v| v.outlining_enabled()));
    // A click on an expander toggles its fold.
    ed.update(|v, cx| v.scroll_to_row(30, cx));
    let hit = ed.read(|v| {
        v.expander_hits()
            .iter()
            .find(|(_, r)| *r == 43)
            .map(|(b, _)| b.center())
    });
    ed.cx.simulate_click(
        hit.expect("the expander of row 43 was painted"),
        Modifiers::none(),
    );
    assert_eq!(collapsed_rows(&mut ed), vec![43]);
}

#[gpui::test]
fn server_folds_replace_the_syntax_folds_and_a_stale_answer_is_dropped(cx: &mut TestAppContext) {
    let mut ed = open(cx, CORPUS, Some("csharp"));
    ed.wait_for_folds();
    ed.update(|v, cx| v.set_server_folds(true, cx));
    let ids: Vec<u64> = ed
        .events
        .borrow_mut()
        .drain(..)
        .filter_map(|e| match e {
            DisplayEvent::FoldRangesRequested { id } => Some(id),
            _ => None,
        })
        .collect();
    assert_eq!(ids.len(), 1);
    // An answer to an older request is dropped.
    assert!(!ed.update(|v, cx| v.set_fold_ranges(ids[0] + 7, vec![], cx)));
    // The server's ranges: a member without a kind borrows "member" from the syntax fold with its header.
    let ranges = vec![
        FoldRange::new(33, 41, FoldKind::Block),
        FoldRange {
            label: Some("Fields".into()),
            ..FoldRange::new(11, 14, FoldKind::Region)
        },
    ];
    assert!(ed.update(|v, cx| v.set_fold_ranges(ids[0], ranges, cx)));
    assert_eq!(ed.read(|v| v.fold_source()), FoldSource::Server);
    assert_eq!(
        ed.folds(),
        vec![
            (11, 14, FoldKind::Region, false),
            (33, 41, FoldKind::Member, false)
        ]
    );
    // An edit asks again after the debounce.
    ed.update(|v, cx| v.update_editor(cx, |e| e.insert("x")));
    ed.cx
        .executor()
        .advance_clock(eludite_editor::folding::FOLD_DEBOUNCE * 2);
    ed.cx.run_until_parked();
    assert!(
        ed.events
            .borrow()
            .iter()
            .any(|e| matches!(e, DisplayEvent::FoldRangesRequested { id } if *id > ids[0]))
    );
}

#[gpui::test]
fn wrapped_rows_map_clicks_and_up_and_down(cx: &mut TestAppContext) {
    // Row 1 is 50 words long (200 columns): at the window's width it wraps into several display rows.
    let long = (0..50)
        .map(|i| format!("w{i:02}"))
        .collect::<Vec<_>>()
        .join(" ");
    let text = format!("first\n{long}\nlast\n");
    let mut ed = open(cx, &text, None);
    ed.update(|v, cx| {
        v.set_word_wrap(true, true, cx);
    });
    // The layout runs with the window's real width: read the segments it used.
    let segs = ed.read(|v| v.row_segments(1));
    assert!(segs.len() > 2, "{segs:?}");
    let layout = ed.read(|v| v.vertical_layout().clone());
    assert_eq!(layout.segments(1), segs.len() as u32);
    assert_eq!(
        layout.line_top(2) - layout.line_top(1),
        layout.line_height() * segs.len() as f32
    );
    // Down from row 0 goes to the first segment of row 1, then the second, keeping the column.
    ed.set_caret(0, 2);
    ed.cx.simulate_keystrokes("down");
    assert_eq!(ed.caret(), text::Point::new(1, 2));
    ed.cx.simulate_keystrokes("down");
    assert_eq!(ed.caret(), text::Point::new(1, segs[1].start as u32 + 2));
    // End goes to the display row's end first, Home back to its start.
    ed.cx.simulate_keystrokes("end");
    assert!(ed.caret().column < segs[1].end as u32);
    assert!(ed.caret().column >= segs[1].start as u32);
    ed.cx.simulate_keystrokes("home");
    assert_eq!(ed.caret(), text::Point::new(1, segs[1].start as u32));
    // Up twice comes back to row 0.
    ed.cx.simulate_keystrokes("up up");
    assert_eq!(ed.caret().row, 0);
    // A click on the third segment lands in it.
    let col = segs[2].start as u32 + 1;
    let at = ed.position(1, col);
    ed.cx.simulate_click(at, Modifiers::none());
    assert_eq!(ed.caret(), text::Point::new(1, col));
    // The caret's position is on its segment's display row.
    let seg0 = ed.position(1, 0);
    assert!(at.y > seg0.y + px(layout.line_height() * 1.5));
    // Off again: one row each.
    ed.update(|v, cx| v.set_word_wrap(false, false, cx));
    assert_eq!(ed.read(|v| v.vertical_layout().segments(1)), 1);
}

#[gpui::test]
fn the_scroll_bar_thumb_and_a_marker_click(cx: &mut TestAppContext) {
    let text = (0..1000).map(|i| format!("line {i}\n")).collect::<String>();
    let mut ed = open(cx, &text, None);
    ed.cx.run_until_parked();
    let bars = ed.read(|v| v.scroll_bars().clone());
    let track = bars.vertical.expect("a vertical bar");
    let thumb = bars.vertical_thumb.expect("a thumb");
    // A thumb for the viewport's share of the document, at the top.
    let viewport = f32::from(bars.viewport.height);
    let expected = f32::from(track.size.height) * viewport / (bars.max_scroll_y + viewport);
    assert!((f32::from(thumb.size.height) - expected.max(18.)).abs() < 1.);
    assert!((thumb.top() - track.top()).abs() < px(1.));
    // A click on the track below the thumb pages down.
    let below = point(track.center().x, thumb.bottom() + px(20.));
    ed.cx.simulate_click(below, Modifiers::none());
    let y = ed.read(|v| v.scroll_position().y);
    assert!(y > px(viewport - 40.), "paged to {y:?}");
    // A marker for row 800 jumps there.
    ed.update(|v, cx| {
        let o = v
            .editor()
            .buffer()
            .point_to_offset(text::Point::new(800, 0));
        v.set_scroll_markers("test", vec![(o, MarkerKind::Bookmark)], cx)
    });
    ed.cx.run_until_parked();
    let marker = ed.read(|v| {
        v.scroll_bars()
            .markers
            .iter()
            .find(|(_, r)| *r == 800)
            .map(|(b, _)| b.center())
    });
    ed.cx
        .simulate_click(marker.expect("the marker was painted"), Modifiers::none());
    assert_eq!(ed.caret().row, 800);
    let visible = ed.read(|v| v.visible_rows());
    assert!(visible.contains(&800), "{visible:?}");
    // Dragging the thumb to the bottom scrolls to the end.
    let bars = ed.read(|v| v.scroll_bars().clone());
    let thumb = bars.vertical_thumb.unwrap();
    let track = bars.vertical.unwrap();
    ed.cx
        .simulate_mouse_down(thumb.center(), gpui::MouseButton::Left, Modifiers::none());
    // Up to the track's top: the start of the document.
    ed.cx.simulate_mouse_move(
        point(thumb.center().x, track.top() + px(1.)),
        Some(gpui::MouseButton::Left),
        Modifiers::none(),
    );
    assert_eq!(ed.read(|v| v.scroll_position().y), px(0.));
    // Down to the track's end: the end of the document.
    ed.cx.simulate_mouse_move(
        point(thumb.center().x, track.bottom() - px(1.)),
        Some(gpui::MouseButton::Left),
        Modifiers::none(),
    );
    ed.cx.simulate_mouse_up(
        point(thumb.center().x, track.bottom() - px(1.)),
        gpui::MouseButton::Left,
        Modifiers::none(),
    );
    let (y, max) = ed.read(|v| (v.scroll_position().y, v.max_scroll_y()));
    assert_eq!(y, max);
    // The view's own markers: the caret's row and a wavy underline.
    let markers = ed.read(|v| v.scroll_markers());
    assert!(markers.contains(&(800, MarkerKind::Caret)));
    assert!(markers.contains(&(800, MarkerKind::Bookmark)));
}

#[gpui::test]
fn sticky_lines_at_a_scroll_position(cx: &mut TestAppContext) {
    let mut ed = open(cx, CORPUS, Some("csharp"));
    ed.wait_for_folds();
    let lh = ed.read(|v| v.line_height());
    // Row 34 at the top: namespace, class and Add pinned cover rows 34 to 36, and row 37 is inside the if too.
    ed.update(|v, cx| v.scroll_to_row(34, cx));
    assert_eq!(ed.read(|v| v.sticky_lines()), vec![4, 9, 33, 35]);
    // Row 37 at the top: four pinned lines would cover it to row 40, past the if, so three.
    ed.update(|v, cx| v.scroll_to_row(37, cx));
    assert_eq!(ed.read(|v| v.sticky_lines()), vec![4, 9, 33]);
    // A click on the class's sticky line goes to its header.
    let hit = ed.read(|v| {
        v.sticky_hits()
            .iter()
            .find(|(_, r)| *r == 9)
            .map(|(b, _)| b.center())
    });
    ed.cx
        .simulate_click(hit.expect("painted"), Modifiers::none());
    assert_eq!(ed.caret().row, 9);
    // Off: none.
    ed.update(|v, cx| v.set_sticky_scroll(false, 5, cx));
    ed.update(|v, cx| v.set_scroll_y(lh * 30., cx));
    assert!(ed.read(|v| v.sticky_lines()).is_empty());
}

#[gpui::test]
fn brace_pairs_and_go_to_brace(cx: &mut TestAppContext) {
    let text = "void M() { var s = \"}\"; if (a[0]) { } }\n";
    let mut ed = open(cx, text, Some("csharp"));
    ed.cx.executor().allow_parking();
    // Wait for the highlights, which tell strings from code.
    let deadline = Instant::now() + eludite_test_support::hang_bound(Duration::from_secs(10));
    while !ed.read(|v| v.highlights_complete()) {
        assert!(Instant::now() < deadline);
        ed.cx.run_until_parked();
        std::thread::yield_now();
    }
    let open_brace = text.find('{').unwrap();
    let close_brace = text.rfind('}').unwrap();
    ed.set_caret(0, open_brace as u32);
    assert_eq!(
        ed.read(|v| v.brace_pair()),
        Some((open_brace..open_brace + 1, close_brace..close_brace + 1)),
        "the brace inside the string is skipped"
    );
    // Go To Brace goes after the close brace; again, back to the open one.
    ed.update(|v, cx| v.go_to_brace(false, cx));
    assert_eq!(ed.caret().column as usize, close_brace + 1);
    ed.update(|v, cx| v.go_to_brace(false, cx));
    assert_eq!(ed.caret().column as usize, open_brace);
    // Extend to Brace selects through the match.
    ed.update(|v, cx| v.go_to_brace(true, cx));
    let sel = ed.read(|v| v.editor().primary_selection());
    assert_eq!(sel.range(), open_brace..close_brace + 1);
    // Before a bracket: `(a[0])`'s `(`.
    let paren = text.find("(a").unwrap();
    ed.set_caret(0, paren as u32);
    let (o, c) = ed.read(|v| v.brace_pair()).unwrap();
    assert_eq!(o.start, paren);
    assert_eq!(&text[c.start..c.end], ")");
}

#[gpui::test]
fn zoom_by_ctrl_wheel_and_the_zoom_box(cx: &mut TestAppContext) {
    let mut ed = open(cx, "a\nb\nc\n", None);
    let (size0, lh0) = ed.read(|v| (v.style().font_size, v.line_height()));
    let center = ed.position(1, 0);
    ed.cx.simulate_event(ScrollWheelEvent {
        position: center,
        delta: ScrollDelta::Lines(point(0., 1.)),
        modifiers: Modifiers::control(),
        touch_phase: TouchPhase::Moved,
    });
    ed.cx.run_until_parked();
    assert_eq!(ed.read(|v| v.zoom()), 110);
    assert!(ed.read(|v| v.style().font_size) > size0);
    assert!(ed.read(|v| v.line_height()) > lh0);
    assert!(
        ed.events
            .borrow()
            .contains(&DisplayEvent::ZoomChanged { level: 110 })
    );
    // The limits.
    assert_eq!(ed.update(|v, cx| v.set_zoom(1000, cx)), 400);
    assert_eq!(ed.update(|v, cx| v.zoom_by(-100, cx)), 20);
    // A click on the zoom box resets it.
    let zoom_box = ed.read(|v| v.zoom_box()).expect("painted");
    ed.cx.simulate_click(zoom_box.center(), Modifiers::none());
    assert_eq!(ed.read(|v| v.zoom()), 100);
    assert_eq!(
        ed.read(|v| (v.style().font_size, v.line_height())),
        (size0, lh0)
    );
    // The font settings apply under the zoom.
    ed.update(|v, cx| v.set_editor_font(None, Some(28.), cx));
    assert_eq!(
        ed.read(|v| (v.style().font_size, v.line_height())),
        (px(28.), px(38.))
    );
}

#[gpui::test]
fn typing_with_1000_collapsed_folds_and_wrap_on_stays_within_the_frame_budget(
    cx: &mut TestAppContext,
) {
    // 50,000 lines: 1,000 methods of 50 lines each, every one collapsed, word wrap on.
    let mut text = String::with_capacity(3_000_000);
    for m in 0..1000 {
        text.push_str(&format!("void M{m}() {{\n"));
        for i in 0..48 {
            text.push_str(&format!("    var x{i} = Compute({i}, \"a long argument list to make some lines wrap\", {m});\n"));
        }
        text.push_str("}\n");
    }
    let mut ed = open(cx, &text, None);
    let ranges: Vec<FoldRange> = (0..1000)
        .map(|m| FoldRange::new(m * 50, m * 50 + 49, FoldKind::Member))
        .collect();
    ed.update(|v, cx| v.set_server_folds(true, cx));
    let id = ed
        .events
        .borrow()
        .iter()
        .rev()
        .find_map(|e| match e {
            DisplayEvent::FoldRangesRequested { id } => Some(*id),
            _ => None,
        })
        .unwrap();
    ed.update(|v, cx| v.set_fold_ranges(id, ranges, cx));
    ed.update(|v, cx| v.collapse_all_outlining(cx));
    ed.update(|v, cx| v.set_word_wrap(true, false, cx));
    assert_eq!(ed.read(|v| v.vertical_layout().hidden().len()), 1000);
    // Expand one method in the middle and type in it.
    ed.set_caret(25_010, 4);
    assert_eq!(collapsed_rows(&mut ed).len(), 999);
    let mut samples = Vec::new();
    for _ in 0..60 {
        let start = Instant::now();
        ed.cx.simulate_input("x");
        ed.cx.run_until_parked();
        samples.push(start.elapsed());
    }
    samples.sort();
    let p99 = samples[samples.len() * 99 / 100 - 1];
    let budget = if cfg!(debug_assertions) { 40 } else { 8 };
    eludite_test_support::assert_budget(
        "keystroke to frame, 50k lines, 1,000 folds, wrap on (p99)",
        p99,
        Duration::from_millis(budget),
    );
}
