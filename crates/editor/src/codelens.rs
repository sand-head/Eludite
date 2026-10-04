//! CodeLens rows in the editor (brief 0052): Visual Studio's indicators on a display-only line above a member
//! (`3 references | Run Test | Debug Test`), in the theme's lens colour and a smaller size, separated by bars.
//!
//! - **Layout.** A lens row takes vertical space ([`crate::display::VerticalLayout`]) but holds no buffer text: the
//!   caret, selections, line numbers and the gutter refer to buffer rows only. It is anchored to its member, so it
//!   moves with the text as the person types until fresh lenses arrive. Adding or removing rows keeps the first
//!   visible line where it was on screen (one reflow, no jump).
//! - **Requests.** The editor knows nothing about LSP: it says *when* lenses are wanted
//!   ([`EditorEvent::CodeLensRequested`]: when turned on, [`crate::intellisense::CODE_LENS_DEBOUNCE`] after the last
//!   edit, on a refresh) and *which* to resolve ([`EditorEvent::CodeLensResolve`]: the unresolved ones within
//!   [`crate::intellisense::CODE_LENS_MARGIN`] lines of the visible range, as they come into it). The owner answers
//!   with [`EditorView::set_code_lenses`] (dropped when not the newest request's or when the text changed since) and
//!   [`EditorView::update_code_lens`].
//! - **Activation.** A click on an indicator, or Ctrl+K, Ctrl+Q (the first indicator of the member at the caret),
//!   emits [`EditorEvent::CodeLensActivated`]; the owner opens the References popup or runs the test, anchored at
//!   [`EditorView::code_lens_bounds`].
//!
//! The editor has no folding yet; a lens row belongs to the buffer row below it, so hiding that row will hide it.

use std::cell::{Ref, RefCell};
use std::ops::Range;

use gpui::{Bounds, Context, Pixels, Task, px};
use text::Anchor;

use crate::display::VerticalLayout;
use crate::intellisense::{
    CODE_LENS_DEBOUNCE, CodeLens, CodeLensPipeline, CodeLensSnapshot, EditorEvent, code_lens_window,
};
use crate::view::EditorView;

/// One indicator of a lens row.
#[derive(Debug, Clone)]
pub(crate) struct LensItem {
    pub id: u64,
    pub title: String,
    pub resolved: bool,
    pub glyph: Option<eludite_ui::TestGlyph>,
}

/// The indicators above one member, anchored to it.
#[derive(Debug, Clone)]
pub(crate) struct LensRow {
    pub anchor: Anchor,
    pub items: Vec<LensItem>,
}

/// The view's lens state.
#[derive(Default)]
pub(crate) struct CodeLenses {
    pub pipeline: CodeLensPipeline,
    pub rows: Vec<LensRow>,
    /// One more whenever `rows` changes shape (not when a title changes).
    epoch: u64,
    /// The layout for (buffer version, epoch, line height).
    cache: RefCell<Option<LayoutCache>>,
    debounce: Option<Task<()>>,
    /// The indicator under the mouse (underlined, as Visual Studio does).
    pub hovered: Option<u64>,
    /// The resolve window last checked, and for which epoch.
    checked: Option<(Range<u32>, u64)>,
}

struct LayoutCache {
    version: clock::Global,
    epoch: u64,
    line_height: Pixels,
    layout: VerticalLayout,
}

impl CodeLenses {
    fn items(&self) -> impl Iterator<Item = &LensItem> {
        self.rows.iter().flat_map(|r| r.items.iter())
    }
}

impl EditorView {
    /// The height of a lens row: four fifths of a text line, as Visual Studio's CodeLens line is shorter.
    pub fn lens_height(&self) -> Pixels {
        (self.style().line_height * 0.8).round()
    }

    /// Where every buffer row and lens row is, for the current text and lenses.
    pub fn vertical_layout(&self) -> Ref<'_, VerticalLayout> {
        let snapshot = self.editor.buffer().snapshot();
        let line_height = self.style().line_height;
        let fresh = self.lenses.cache.borrow().as_ref().is_some_and(|c| {
            c.epoch == self.lenses.epoch
                && c.line_height == line_height
                && &c.version == snapshot.version()
        });
        if !fresh {
            let rows = self
                .lenses
                .rows
                .iter()
                .map(|r| {
                    snapshot
                        .offset_to_point(snapshot.offset_for_anchor(&r.anchor))
                        .row
                })
                .collect();
            let layout = VerticalLayout::new(
                f32::from(line_height),
                f32::from(self.lens_height()),
                self.editor.buffer().line_count(),
                rows,
            );
            *self.lenses.cache.borrow_mut() = Some(LayoutCache {
                version: snapshot.version().clone(),
                epoch: self.lenses.epoch,
                line_height,
                layout,
            });
        }
        Ref::map(self.lenses.cache.borrow(), |c| {
            &c.as_ref().expect("filled above").layout
        })
    }

    /// The top of buffer row `row`'s text, in content pixels (subtract the scroll position for the viewport). Lens
    /// rows above it are counted, so overlays drawn beside the text line up with it.
    pub fn row_top(&self, row: u32) -> Pixels {
        px(self.vertical_layout().line_top(row))
    }

    /// The bottom of buffer row `row`'s text, in content pixels.
    pub fn row_bottom(&self, row: u32) -> Pixels {
        self.row_top(row) + self.style().line_height
    }

    /// Whether the editor shows lenses and asks for them.
    pub fn code_lens_enabled(&self) -> bool {
        self.lenses.pipeline.enabled()
    }

    /// Turn lenses on (they are asked for at once with [`EditorEvent::CodeLensRequested`]) or off (the rows go, in one
    /// reflow that keeps the text in view where it was).
    pub fn set_code_lens_enabled(&mut self, enabled: bool, cx: &mut Context<Self>) {
        if !self.lenses.pipeline.set_enabled(enabled) {
            return;
        }
        if enabled {
            self.refresh_code_lenses(cx);
        } else {
            self.lenses.debounce = None;
            self.lenses.hovered = None;
            self.replace_lens_rows(Vec::new());
            cx.notify();
        }
    }

    /// Ask for the lenses again now (the server's `workspace/codeLens/refresh`, a setting changed). Nothing while
    /// lenses are off.
    pub fn refresh_code_lenses(&mut self, cx: &mut Context<Self>) {
        let version = self.editor.buffer().snapshot().version().clone();
        if let Some(id) = self.lenses.pipeline.request(&version) {
            self.lenses.debounce = None;
            cx.emit(EditorEvent::CodeLensRequested { id });
        }
    }

    /// The answer to [`EditorEvent::CodeLensRequested`] `id`: the document's lenses, in the order each row shows
    /// them. Dropped (returns false) when another request superseded it or the text changed since it was made; the
    /// debounce asks again.
    pub fn set_code_lenses(
        &mut self,
        id: u64,
        lenses: Vec<CodeLens>,
        cx: &mut Context<Self>,
    ) -> bool {
        let version = self.editor.buffer().snapshot().version().clone();
        if !self.lenses.pipeline.accept(id, &version) {
            return false;
        }
        let buffer = self.editor.buffer();
        let mut rows: Vec<(u32, LensRow)> = Vec::new();
        for lens in lenses {
            let offset = lens.offset.min(buffer.len());
            let row = buffer.offset_to_point(offset).row;
            let item = LensItem {
                id: lens.id,
                title: lens.title,
                resolved: lens.resolved,
                glyph: lens.glyph,
            };
            match rows.iter_mut().find(|(r, _)| *r == row) {
                Some((_, existing)) => existing.items.push(item),
                None => rows.push((
                    row,
                    LensRow {
                        anchor: buffer.anchor_before(offset),
                        items: vec![item],
                    },
                )),
            }
        }
        rows.sort_by_key(|(r, _)| *r);
        if self
            .lenses
            .hovered
            .is_some_and(|h| !rows.iter().any(|(_, r)| r.items.iter().any(|i| i.id == h)))
        {
            self.lenses.hovered = None;
        }
        self.replace_lens_rows(rows.into_iter().map(|(_, r)| r).collect());
        self.check_lens_resolve(cx);
        cx.notify();
        true
    }

    /// A resolved (or re-titled) lens: its text, whether it is resolved, its outcome glyph. False when no lens has
    /// that id (it was replaced since).
    pub fn update_code_lens(
        &mut self,
        id: u64,
        title: String,
        resolved: bool,
        glyph: Option<eludite_ui::TestGlyph>,
        cx: &mut Context<Self>,
    ) -> bool {
        let Some(item) = self
            .lenses
            .rows
            .iter_mut()
            .flat_map(|r| r.items.iter_mut())
            .find(|i| i.id == id)
        else {
            return false;
        };
        if item.title != title || item.resolved != resolved || item.glyph != glyph {
            item.title = title;
            item.resolved = resolved;
            item.glyph = glyph;
            cx.notify();
        }
        true
    }

    /// Lens `id`'s resolve failed (the text changed under it): ask again when it is next in the window.
    pub fn code_lens_resolve_failed(&mut self, id: u64) {
        self.lenses.pipeline.retry(id);
        self.lenses.checked = None;
    }

    /// The lenses shown, in document order.
    pub fn code_lenses(&self) -> Vec<CodeLensSnapshot> {
        let buffer = self.editor.buffer();
        self.lenses
            .rows
            .iter()
            .flat_map(|r| {
                let row = buffer
                    .offset_to_point(buffer.offset_for_anchor(&r.anchor))
                    .row;
                r.items.iter().map(move |i| CodeLensSnapshot {
                    id: i.id,
                    row,
                    title: i.title.clone(),
                    resolved: i.resolved,
                    glyph: i.glyph,
                })
            })
            .collect()
    }

    /// Where indicator `id` was painted in the last frame (window coordinates): the References popup's anchor.
    pub fn code_lens_bounds(&self, id: u64) -> Option<Bounds<Pixels>> {
        self.layout
            .as_ref()?
            .lens_hits
            .iter()
            .find(|(_, i)| *i == id)
            .map(|(b, _)| *b)
    }

    /// The indicator under the mouse.
    pub fn hovered_code_lens(&self) -> Option<u64> {
        self.lenses.hovered
    }

    /// Replace the rows, keeping the first visible text line where it is on screen.
    fn replace_lens_rows(&mut self, rows: Vec<LensRow>) {
        let (top_row, line_top) = {
            let l = self.vertical_layout();
            let y = f32::from(self.scroll_position().y);
            let row = l.hit(y).row;
            (row, l.line_top(row) - y)
        };
        let at_top = self.scroll_position().y <= px(0.);
        self.lenses.rows = rows;
        self.lenses.epoch += 1;
        self.lenses.checked = None;
        let y = if at_top {
            0.
        } else {
            (self.vertical_layout().line_top(top_row) - line_top).max(0.)
        };
        self.set_scroll_y_quietly(px(y));
    }

    /// After an edit: the lenses are asked for again once the person stops typing for
    /// [`crate::intellisense::CODE_LENS_DEBOUNCE`].
    pub(crate) fn lens_edited(&mut self, cx: &mut Context<Self>) {
        if !self.lenses.pipeline.enabled() {
            return;
        }
        self.lenses.pipeline.edited();
        self.lenses.debounce = Some(cx.spawn(async move |this, cx| {
            cx.background_executor().timer(CODE_LENS_DEBOUNCE).await;
            this.update(cx, |v, cx| {
                v.lenses.debounce = None;
                let version = v.editor.buffer().snapshot().version().clone();
                if let Some(id) = v.lenses.pipeline.debounced(&version) {
                    cx.emit(EditorEvent::CodeLensRequested { id });
                }
            })
            .ok();
        }));
    }

    /// Ask for the unresolved lenses that came into the window (the visible rows and the margin).
    pub(crate) fn check_lens_resolve(&mut self, cx: &mut Context<Self>) {
        if !self.lenses.pipeline.enabled() || self.lenses.rows.is_empty() {
            return;
        }
        let (window, rows): (Range<u32>, Vec<(u64, u32, bool)>) = {
            let l = self.vertical_layout();
            let height = self
                .layout
                .as_ref()
                .map_or(l.line_height() * 60., |p| f32::from(p.bounds.size.height));
            let visible = l.visible_rows(f32::from(self.scroll_position().y), height);
            let window = code_lens_window(visible, l.line_count());
            if self.lenses.checked.as_ref() == Some(&(window.clone(), self.lenses.epoch)) {
                return;
            }
            let buffer = self.editor.buffer();
            let rows = self
                .lenses
                .rows
                .iter()
                .flat_map(|r| {
                    let row = buffer
                        .offset_to_point(buffer.offset_for_anchor(&r.anchor))
                        .row;
                    r.items.iter().map(move |i| (i.id, row, i.resolved))
                })
                .collect();
            (window, rows)
        };
        self.lenses.checked = Some((window.clone(), self.lenses.epoch));
        let ids = self.lenses.pipeline.to_resolve(rows, window);
        if !ids.is_empty() {
            cx.emit(EditorEvent::CodeLensResolve { ids });
        }
    }

    /// Ctrl+K, Ctrl+Q (Visual Studio's Show CodeLens Menu): the first indicator of the member at the caret (the
    /// nearest lens row at or above the caret's line).
    pub(crate) fn show_code_lens_menu(&mut self, cx: &mut Context<Self>) {
        let caret = self.editor.primary_head().row;
        let buffer = self.editor.buffer();
        let first = self
            .lenses
            .rows
            .iter()
            .map(|r| {
                (
                    buffer
                        .offset_to_point(buffer.offset_for_anchor(&r.anchor))
                        .row,
                    r,
                )
            })
            .filter(|(row, _)| *row <= caret)
            .max_by_key(|(row, _)| *row)
            .and_then(|(_, r)| r.items.first())
            .map(|i| i.id);
        if let Some(id) = first {
            cx.emit(EditorEvent::CodeLensActivated { id, keyboard: true });
        }
    }

    /// The lens rows among `rows` with their indicators, for drawing: (buffer row, the row's first non-blank visual
    /// column, its items).
    pub(crate) fn lens_rows_in(&self, rows: Range<u32>) -> Vec<(u32, Vec<LensItem>)> {
        let buffer = self.editor.buffer();
        let mut out: Vec<(u32, Vec<LensItem>)> = Vec::new();
        for r in &self.lenses.rows {
            let row = buffer
                .offset_to_point(buffer.offset_for_anchor(&r.anchor))
                .row;
            if !rows.contains(&row) {
                continue;
            }
            match out.iter_mut().find(|(x, _)| *x == row) {
                Some((_, items)) => items.extend(r.items.iter().cloned()),
                None => out.push((row, r.items.clone())),
            }
        }
        out
    }

    /// Every lens item, for the number of indicators shown (benchmarks).
    pub fn code_lens_count(&self) -> usize {
        self.lenses.items().count()
    }
}
