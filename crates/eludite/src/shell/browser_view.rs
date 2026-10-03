//! The embedded browser's view (brief 0031, the spike for proposal 0002's Web Browser window).
//!
//! - [`BrowserSurface`] draws a tab of `eludite-chromium` with GPUI's `img` elements from the frame ring
//!   ([`FrameSource`]): it uploads only when the frame sequence changed, and forwards mouse, wheel, keyboard and IME
//!   input to the tab (`tab/input`, never waiting). The engine's reader thread wakes it through a channel; nothing
//!   here waits on the engine.
//! - **Partial uploads** (brief 0032, the fallback of brief 0031's section 5). The frame is drawn as tiles of
//!   [`TILE`] by [`TILE`] device pixels, each its own [`RenderImage`]; a frame that follows the last uploaded one
//!   re-creates only the tiles its dirty rectangles touch (and frees their old atlas entries), any other frame (a
//!   skipped sequence, a new size) all of them. `ELUDITE_BROWSER_TILES=0` draws one image per frame instead, for the
//!   bench's comparison.
//! - The Web Browser window's needs (brief 0032): the page's cursor ([`BrowserSurface::set_cursor`], from
//!   `tab/cursor`), the key table of `eludite_browser::keys` (Windows and X11 key codes), and IME composition through
//!   GPUI's input handler (`imeSetComposition`, `imeCommitText`, `imeFinishComposing`).
//! - [`open_spike`] is the hidden document tab of `--spike-browser URL` (and `--bench-browser`): a thread named
//!   `browser-spike` owns the [`EmbeddedChromium`] (launch, tab, shutdown), and the UI thread only receives the tab's
//!   frames and an input handle. The Web Browser window proper (tabs, address bar, commands) is brief B.
//! - [`SurfaceStats`] records, per uploaded frame, the `RenderImage` creation (the copy out of shared memory), the
//!   `img` element's paint (GPUI's atlas insert, the CPU side of its texture upload), the engine's copy into the slot
//!   and the paint-to-present latency (`OnPaint` in the engine to the end of the present that showed the frame, on
//!   `CLOCK_MONOTONIC`).

use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::{Arc, mpsc};
use std::time::{Duration, Instant};

use eludite_browser::embedded::{DirtyRect, Frame, TabControl, monotonic_ns};
use eludite_browser::{
    ChromiumSearch, EmbeddedChromium, Engine, EngineConfig, FrameSource, TabFrames,
};
use futures::StreamExt as _;
use gpui::{
    AnyElement, AnyView, App, AppContext as _, Bounds, Context, CursorStyle, Element, ElementId,
    ElementInputHandler, Entity, EntityInputHandler, FocusHandle, GlobalElementId, ImageSource,
    InspectorElementId, InteractiveElement as _, IntoElement, KeyDownEvent, KeyUpEvent, LayoutId,
    Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent, MouseUpEvent, ParentElement as _,
    Pixels, Render, RenderImage, ScrollDelta, ScrollWheelEvent, Styled as _, Task, UTF16Selection,
    Window, div, img, px,
};
use serde_json::{Value, json};

use super::Shell;

/// The spike's document tab.
pub const SPIKE_TAB: &str = "eludite.browser.spike";

/// What the document area draws for browser tabs, by tab id.
pub type Views = Rc<RefCell<HashMap<String, AnyView>>>;

/// Per-frame measurements of a [`BrowserSurface`].
#[derive(Debug, Default, Clone)]
pub struct SurfaceStats {
    /// Frames uploaded (a new `RenderImage`).
    pub uploads: u64,
    /// Renders that found the sequence changed but no ready slot (the frame was read already).
    pub empty_reads: u64,
    /// `RenderImage` creation per upload: the copy out of the slot and the image, ms.
    pub upload_ms: Vec<f64>,
    /// The `img` element's paint in frames with a new image: GPUI's atlas insert (its staging copy), ms.
    pub paint_ms: Vec<f64>,
    /// The engine's copy of each uploaded frame into its slot, ms.
    pub copy_ms: Vec<f64>,
    /// The engine's `OnPaint` to the end of the present that showed the frame, ms.
    pub latency_ms: Vec<f64>,
    /// When each paint of the `img` element ended, and whether it carried a new image: the bench splits a frame
    /// into the CPU work up to the image and the rest (the present, where a software rasterizer does its work).
    pub painted: Vec<(Instant, bool)>,
    /// The dirty rectangles of the last uploaded frame (device pixels).
    pub last_dirty: Vec<DirtyRect>,
    pub last_sequence: u64,
    pub frame_size: (u32, u32),
    /// Tiles re-created per uploaded frame (partial uploads), and how many the frame had.
    pub tiles_uploaded: Vec<usize>,
    pub tiles_total: usize,
}

impl SurfaceStats {
    /// Drop the samples (a benchmark's warm-up).
    pub fn clear_samples(&mut self) {
        self.upload_ms.clear();
        self.paint_ms.clear();
        self.copy_ms.clear();
        self.latency_ms.clear();
        self.painted.clear();
        self.tiles_uploaded.clear();
    }
}

pub type Stats = Rc<RefCell<SurfaceStats>>;

/// Where input goes: `tab/input` events (browser-rpc.md).
pub type InputSink = Rc<dyn Fn(Value)>;

/// Asks the engine for a view size: CSS width, height and the scale factor.
pub type FitSink = Rc<dyn Fn(u32, u32, f32)>;

/// The side of a tile, device pixels.
pub const TILE: u32 = 256;

/// A frame as tiles of [`TILE`] pixels (row-major), or one image (`ELUDITE_BROWSER_TILES=0`).
struct Tiles {
    size: (u32, u32),
    /// Tile columns and rows.
    grid: (u32, u32),
    tile: u32,
    images: Vec<Arc<RenderImage>>,
}

impl Tiles {
    fn grid(size: (u32, u32), tile: u32) -> (u32, u32) {
        (size.0.div_ceil(tile).max(1), size.1.div_ceil(tile).max(1))
    }

    /// The tile at column `c`, row `r` of `f`, as an image (BGRA, as the engine paints and GPUI's `RenderImage` holds).
    fn cut(f: &Frame<'_>, tile: u32, c: u32, r: u32) -> Option<Arc<RenderImage>> {
        let (x0, y0) = (c * tile, r * tile);
        let w = tile.min(f.width.saturating_sub(x0));
        let h = tile.min(f.height.saturating_sub(y0));
        if w == 0 || h == 0 {
            return None;
        }
        let row = w as usize * 4;
        let mut data = Vec::with_capacity(row * h as usize);
        for y in y0..y0 + h {
            let o = y as usize * f.stride as usize + x0 as usize * 4;
            data.extend_from_slice(f.pixels.get(o..o + row)?);
        }
        let buffer = image::RgbaImage::from_raw(w, h, data)?;
        Some(Arc::new(RenderImage::new(vec![image::Frame::new(buffer)])))
    }

    /// The tiles a dirty rectangle touches.
    fn touched(&self, d: &DirtyRect) -> impl Iterator<Item = usize> + '_ {
        let t = self.tile;
        let (cols, rows) = self.grid;
        let (x, y) = (d.x.max(0) as u32, d.y.max(0) as u32);
        let (w, h) = (d.width.max(0) as u32, d.height.max(0) as u32);
        let c0 = (x / t).min(cols);
        let r0 = (y / t).min(rows);
        let c1 = (x + w).div_ceil(t).min(cols);
        let r1 = (y + h).div_ceil(t).min(rows);
        (r0..r1).flat_map(move |r| (c0..c1).map(move |c| (r * cols + c) as usize))
    }
}

/// A tab of the embedded engine, drawn with `img`.
pub struct BrowserSurface {
    source: Arc<dyn FrameSource>,
    input: Option<InputSink>,
    /// Ask the engine for this view size (CSS pixels) when the surface's bounds change; `None`: the tab keeps its
    /// size and the surface clips it.
    fit: Option<FitSink>,
    tiles: Option<Tiles>,
    /// Tiles of [`TILE`] pixels (partial uploads), or one image per frame.
    tiled: bool,
    uploaded: u64,
    /// The page's cursor (`tab/cursor`).
    cursor: CursorStyle,
    /// The IME composition in progress (UTF-16 length of its text), for GPUI's input handler.
    composing: Option<usize>,
    /// A frame was uploaded in this render: the paint is timed and the present recorded.
    fresh: Option<u64>,
    stats: Stats,
    /// The surface's bounds in the last frame (input coordinates are relative to its origin).
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    /// The CSS size last asked of the engine.
    fitted: Rc<Cell<Option<(u32, u32)>>>,
    /// Keep drawing the current image and upload nothing (the bench's baseline).
    frozen: bool,
    focus: FocusHandle,
    _wake: Task<()>,
}

impl BrowserSurface {
    pub fn new(
        source: Arc<dyn FrameSource>,
        input: Option<InputSink>,
        fit: Option<FitSink>,
        cx: &mut Context<Self>,
    ) -> Self {
        // The engine's reader thread announces frames; a task on the UI thread turns that into a notify, coalesced.
        let (tx, mut rx) = futures::channel::mpsc::unbounded::<()>();
        source.set_listener(Some(Box::new(move || {
            let _ = tx.unbounded_send(());
        })));
        let wake = cx.spawn(async move |this, cx| {
            while rx.next().await.is_some() {
                while rx.try_recv().is_ok() {}
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    break;
                }
            }
        });
        Self {
            source,
            input,
            fit,
            tiles: None,
            tiled: std::env::var("ELUDITE_BROWSER_TILES").map_or(true, |v| v.trim() != "0"),
            uploaded: 0,
            cursor: CursorStyle::Arrow,
            composing: None,
            fresh: None,
            stats: Stats::default(),
            bounds: Rc::default(),
            fitted: Rc::default(),
            frozen: false,
            focus: cx.focus_handle(),
            _wake: wake,
        }
    }

    pub fn stats(&self) -> Stats {
        self.stats.clone()
    }

    /// Stop (or resume) uploading: the surface keeps drawing its current image.
    pub fn set_frozen(&mut self, frozen: bool) {
        self.frozen = frozen;
    }

    /// The page's cursor, as `tab/cursor` names it (CSS keywords).
    pub fn set_cursor(&mut self, css: &str, cx: &mut Context<Self>) {
        let c = cursor_style(css);
        if c != self.cursor {
            self.cursor = c;
            cx.notify();
        }
    }

    /// The focus handle the window focuses when the page should take the keys.
    pub fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    /// Upload the newest frame if the sequence changed: the tiles its dirty rectangles touch (all of them when frames
    /// were skipped or the size changed), their old atlas entries freed.
    fn refresh(&mut self, window: &mut Window) {
        let seq = self.source.sequence();
        if seq == self.uploaded || self.frozen {
            return;
        }
        let t0 = Instant::now();
        let last = self.stats.borrow().last_sequence;
        let tile = if self.tiled { TILE } else { u32::MAX };
        let mut old = self.tiles.take();
        let mut freed: Vec<Arc<RenderImage>> = Vec::new();
        let mut made: Option<(u64, u64, u64, Vec<DirtyRect>, (u32, u32), usize)> = None;
        self.source.read(true, &mut |f| {
            let size = (f.width, f.height);
            let tile = tile.min(size.0.max(size.1).max(1));
            let partial = matches!(&old, Some(t) if t.size == size && t.tile == tile)
                && last != 0
                && f.sequence == last + 1
                && !f.dirty.is_empty();
            let mut tiles = match old.take() {
                Some(t) if partial => t,
                prev => {
                    if let Some(prev) = prev {
                        freed.extend(prev.images);
                    }
                    Tiles {
                        size,
                        grid: Tiles::grid(size, tile),
                        tile,
                        images: Vec::new(),
                    }
                }
            };
            let mut redo: Vec<usize> = if partial {
                f.dirty.iter().flat_map(|d| tiles.touched(d)).collect()
            } else {
                (0..(tiles.grid.0 * tiles.grid.1) as usize).collect()
            };
            redo.sort_unstable();
            redo.dedup();
            let cols = tiles.grid.0;
            let mut fresh = Vec::with_capacity(redo.len());
            for &i in &redo {
                let (c, r) = (i as u32 % cols, i as u32 / cols);
                match Tiles::cut(f, tiles.tile, c, r) {
                    Some(img) => fresh.push((i, img)),
                    None => return,
                }
            }
            if partial {
                for (i, img) in fresh {
                    freed.push(std::mem::replace(&mut tiles.images[i], img));
                }
            } else {
                tiles.images = fresh.into_iter().map(|(_, img)| img).collect();
            }
            made = Some((
                f.sequence,
                f.paint_ns,
                f.copy_ns,
                f.dirty.to_vec(),
                size,
                redo.len(),
            ));
            old = Some(tiles);
        });
        self.tiles = old;
        for img in freed {
            let _ = window.drop_image(img);
        }
        let Some((sequence, paint_ns, copy_ns, dirty, size, redone)) = made else {
            self.stats.borrow_mut().empty_reads += 1;
            self.uploaded = seq;
            return;
        };
        let upload = t0.elapsed();
        self.uploaded = seq.max(sequence);
        self.fresh = Some(paint_ns);
        let mut s = self.stats.borrow_mut();
        s.uploads += 1;
        s.upload_ms.push(upload.as_secs_f64() * 1e3);
        s.copy_ms.push(copy_ns as f64 / 1e6);
        s.last_dirty = dirty;
        s.last_sequence = sequence;
        s.frame_size = size;
        s.tiles_uploaded.push(redone);
        s.tiles_total = self.tiles.as_ref().map_or(0, |t| t.images.len());
    }

    fn send(&self, event: Value) {
        if let Some(input) = &self.input {
            input(event);
        }
    }

    /// A window position in the tab's CSS pixels.
    fn local(&self, p: gpui::Point<Pixels>) -> (i64, i64) {
        let o = self.bounds.get().map(|b| b.origin).unwrap_or_default();
        (
            f32::from(p.x - o.x).round() as i64,
            f32::from(p.y - o.y).round() as i64,
        )
    }
}

/// CEF's `cef_event_flags_t` for the modifiers (browser-rpc.md).
fn flags(m: &Modifiers) -> u32 {
    (if m.shift { 2 } else { 0 })
        | (if m.control { 4 } else { 0 })
        | (if m.alt { 8 } else { 0 })
        | (if m.platform { 128 } else { 0 })
}

fn button(b: MouseButton) -> Option<&'static str> {
    match b {
        MouseButton::Left => Some("left"),
        MouseButton::Middle => Some("middle"),
        MouseButton::Right => Some("right"),
        _ => None,
    }
}

/// The Windows virtual-key code CEF expects for a GPUI key name, and the X11 key code it reads the DOM `code` from
/// (`eludite_browser::keys::SHELL_KEYS`, brief 0032).
pub fn key_codes(key: &str) -> Option<(i64, i64)> {
    eludite_browser::keys::shell_key(key).map(|k| (k.windows, k.x11))
}

/// GPUI's cursor for a CSS cursor keyword (`tab/cursor`).
pub fn cursor_style(css: &str) -> CursorStyle {
    match css {
        "pointer" => CursorStyle::PointingHand,
        "text" => CursorStyle::IBeam,
        "vertical-text" => CursorStyle::IBeamCursorForVerticalLayout,
        "crosshair" | "cell" => CursorStyle::Crosshair,
        "grab" => CursorStyle::OpenHand,
        "grabbing" | "move" | "all-scroll" => CursorStyle::ClosedHand,
        "not-allowed" | "no-drop" => CursorStyle::OperationNotAllowed,
        "ew-resize" | "col-resize" | "e-resize" | "w-resize" => CursorStyle::ResizeLeftRight,
        "ns-resize" | "row-resize" | "n-resize" | "s-resize" => CursorStyle::ResizeUpDown,
        "nesw-resize" | "ne-resize" | "sw-resize" => CursorStyle::ResizeUpRightDownLeft,
        "nwse-resize" | "nw-resize" | "se-resize" => CursorStyle::ResizeUpLeftDownRight,
        "context-menu" => CursorStyle::ContextualMenu,
        "copy" | "alias" => CursorStyle::DragCopy,
        _ => CursorStyle::Arrow,
    }
}

impl Render for BrowserSurface {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.refresh(window);
        let fresh = self.fresh.take();
        if let Some(paint_ns) = fresh {
            // Effects deferred from a render run after the frame is presented.
            let stats = self.stats.clone();
            cx.defer(move |_| {
                let now = monotonic_ns();
                stats
                    .borrow_mut()
                    .latency_ms
                    .push(now.saturating_sub(paint_ns) as f64 / 1e6);
            });
        }
        let surface = div()
            .id("browser-surface")
            .debug_selector(|| "browser-surface".into())
            .size_full()
            .overflow_hidden()
            .bg(gpui::white())
            .cursor(self.cursor)
            .track_focus(&self.focus)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, e: &MouseDownEvent, window, cx| {
                    window.focus(&this.focus, cx);
                    this.mouse(
                        "mouseDown",
                        e.button,
                        e.position,
                        e.click_count,
                        &e.modifiers,
                    );
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, e: &MouseDownEvent, _, _| {
                    this.mouse(
                        "mouseDown",
                        e.button,
                        e.position,
                        e.click_count,
                        &e.modifiers,
                    )
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, e: &MouseUpEvent, _, _| {
                    this.mouse("mouseUp", e.button, e.position, e.click_count, &e.modifiers)
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, e: &MouseUpEvent, _, _| {
                    this.mouse("mouseUp", e.button, e.position, e.click_count, &e.modifiers)
                }),
            )
            .on_mouse_move(cx.listener(|this, e: &MouseMoveEvent, _, _| {
                let (x, y) = this.local(e.position);
                this.send(
                    json!({"type": "mouseMove", "x": x, "y": y, "modifiers": flags(&e.modifiers)}),
                );
            }))
            .on_scroll_wheel(cx.listener(|this, e: &ScrollWheelEvent, _, _| {
                let (dx, dy) = match e.delta {
                    ScrollDelta::Pixels(p) => (f32::from(p.x), f32::from(p.y)),
                    ScrollDelta::Lines(l) => (l.x * 40., l.y * 40.),
                };
                let (x, y) = this.local(e.position);
                this.send(
                    json!({"type": "wheel", "x": x, "y": y, "deltaX": dx.round() as i64,
                    "deltaY": dy.round() as i64, "modifiers": flags(&e.modifiers)}),
                );
            }))
            .on_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                let k = &e.keystroke;
                let m = flags(&k.modifiers) | if e.is_held { 8192 } else { 0 };
                if let Some((code, native)) = key_codes(&k.key) {
                    this.send(json!({"type": "rawKeyDown", "windowsKeyCode": code,
                        "nativeKeyCode": native, "modifiers": m}));
                }
                if let Some(ch) = k.key_char.as_deref().filter(|c| !c.is_empty()) {
                    for unit in ch.encode_utf16() {
                        let s = String::from_utf16_lossy(&[unit]);
                        this.send(json!({"type": "char", "character": s, "modifiers": m}));
                    }
                }
                cx.stop_propagation();
            }))
            .on_key_up(cx.listener(|this, e: &KeyUpEvent, _, _| {
                if let Some((code, native)) = key_codes(&e.keystroke.key) {
                    this.send(
                        json!({"type": "keyUp", "windowsKeyCode": code, "nativeKeyCode": native,
                        "modifiers": flags(&e.keystroke.modifiers)}),
                    );
                }
            }));
        // The surface's own bounds: input coordinates, and the tab's size when it fits the document area.
        let (bounds, fitted, fit) = (self.bounds.clone(), self.fitted.clone(), self.fit.clone());
        let (entity, focus) = (cx.entity(), self.focus.clone());
        let surface = surface.child(
            gpui::canvas(
                move |b, window, _| {
                    bounds.set(Some(b));
                    if let Some(fit) = &fit {
                        let size = (
                            f32::from(b.size.width).floor().max(1.) as u32,
                            f32::from(b.size.height).floor().max(1.) as u32,
                        );
                        if fitted.get() != Some(size) {
                            fitted.set(Some(size));
                            fit(size.0, size.1, window.scale_factor());
                        }
                    }
                },
                // IME: GPUI's input handler while the page has the focus (composition and commits reach the page).
                move |b, _, window, cx| {
                    if focus.is_focused(window) {
                        window.handle_input(
                            &focus,
                            ElementInputHandler::new(b, entity.clone()),
                            cx,
                        );
                    }
                },
            )
            .absolute()
            .size_full(),
        );
        let Some(tiles) = self.tiles.as_ref() else {
            return surface
                .child(
                    div()
                        .p_4()
                        .text_color(gpui::black())
                        .child("Loading\u{2026}"),
                )
                .into_any_element();
        };
        let scale = window.scale_factor();
        let (w, h) = (tiles.size.0 as f32 / scale, tiles.size.1 as f32 / scale);
        let cols = tiles.grid.0;
        let step = tiles.tile as f32 / scale;
        let images = tiles.images.iter().enumerate().map(|(i, image)| {
            let (c, r) = (i as u32 % cols, i as u32 / cols);
            let size = image.size(0);
            img(ImageSource::Render(image.clone()))
                .absolute()
                .left(px(c as f32 * step))
                .top(px(r as f32 * step))
                .w(px(size.width.0 as f32 / scale))
                .h(px(size.height.0 as f32 / scale))
                .into_any_element()
        });
        surface
            .child(Timed {
                child: div()
                    .relative()
                    .w(px(w))
                    .h(px(h))
                    .children(images)
                    .into_any_element(),
                stats: self.stats.clone(),
                fresh: fresh.is_some(),
            })
            .into_any_element()
    }
}

/// IME (brief 0032): GPUI's input handler hands the composition and the committed text to the page. Typed keys do
/// not come this way (the key handlers send them and stop the event), so this carries IME only.
impl EntityInputHandler for BrowserSurface {
    fn text_for_range(
        &mut self,
        _range: std::ops::Range<usize>,
        _adjusted: &mut Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<String> {
        None
    }

    fn selected_text_range(
        &mut self,
        _ignore_disabled_input: bool,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<UTF16Selection> {
        let at = self.composing.unwrap_or(0);
        Some(UTF16Selection {
            range: at..at,
            reversed: false,
        })
    }

    fn marked_text_range(
        &self,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<std::ops::Range<usize>> {
        self.composing.map(|n| 0..n)
    }

    fn unmark_text(&mut self, _window: &mut Window, _cx: &mut Context<Self>) {
        if self.composing.take().is_some() {
            self.send(json!({"type": "imeFinishComposing", "keepSelection": false}));
        }
    }

    fn replace_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        text: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        self.composing = None;
        self.send(json!({"type": "imeCommitText", "text": text}));
    }

    fn replace_and_mark_text_in_range(
        &mut self,
        _range: Option<std::ops::Range<usize>>,
        new_text: &str,
        new_selected_range: Option<std::ops::Range<usize>>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) {
        let n = new_text.encode_utf16().count();
        let sel = new_selected_range.unwrap_or(n..n);
        self.composing = (n > 0).then_some(n);
        if n == 0 {
            self.send(json!({"type": "imeCancelComposition"}));
            return;
        }
        self.send(json!({"type": "imeSetComposition", "text": new_text,
            "selectionStart": sel.start, "selectionEnd": sel.end,
            "underlines": [{"from": 0, "to": n, "thick": false}]}));
    }

    fn bounds_for_range(
        &mut self,
        _range_utf16: std::ops::Range<usize>,
        element_bounds: Bounds<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<Bounds<Pixels>> {
        // The candidate window goes at the surface's top left: the page's caret is not known here.
        Some(Bounds::new(
            element_bounds.origin,
            gpui::size(px(1.), px(16.)),
        ))
    }

    fn character_index_for_point(
        &mut self,
        _point: gpui::Point<Pixels>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> Option<usize> {
        None
    }
}

impl BrowserSurface {
    fn mouse(
        &self,
        kind: &str,
        b: MouseButton,
        p: gpui::Point<Pixels>,
        clicks: usize,
        m: &Modifiers,
    ) {
        let Some(button) = button(b) else { return };
        let (x, y) = self.local(p);
        self.send(json!({"type": kind, "x": x, "y": y, "button": button,
            "clickCount": clicks.clamp(1, 3), "modifiers": flags(m)}));
    }
}

/// The `img` element, its paint timed when it carries a new image.
struct Timed {
    child: AnyElement,
    stats: Stats,
    fresh: bool,
}

impl IntoElement for Timed {
    type Element = Self;

    fn into_element(self) -> Self::Element {
        self
    }
}

impl Element for Timed {
    type RequestLayoutState = ();
    type PrepaintState = ();

    fn id(&self) -> Option<ElementId> {
        None
    }

    fn source_location(&self) -> Option<&'static std::panic::Location<'static>> {
        None
    }

    fn request_layout(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        window: &mut Window,
        cx: &mut App,
    ) -> (LayoutId, ()) {
        (self.child.request_layout(window, cx), ())
    }

    fn prepaint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        self.child.prepaint(window, cx);
    }

    fn paint(
        &mut self,
        _: Option<&GlobalElementId>,
        _: Option<&InspectorElementId>,
        _: Bounds<Pixels>,
        _: &mut (),
        _: &mut (),
        window: &mut Window,
        cx: &mut App,
    ) {
        let t0 = Instant::now();
        self.child.paint(window, cx);
        let end = Instant::now();
        let mut s = self.stats.borrow_mut();
        if self.fresh {
            s.paint_ms.push((end - t0).as_secs_f64() * 1e3);
        }
        if s.painted.len() < 100_000 {
            s.painted.push((end, self.fresh));
        }
    }
}

// ---- the spike's document tab ----

/// What the spike's thread hands the UI once the tab exists.
pub struct Ready {
    pub frames: Arc<TabFrames>,
    pub control: TabControl,
    pub tab: String,
    pub pid: u32,
    /// Spawn to `initialize`'s answer.
    pub launch: Duration,
    /// Spawn to the tab's first frame.
    pub cold_to_first_frame: Duration,
}

/// Requests to the spike's thread, which owns the engine.
pub enum SpikeRequest {
    /// Open another tab and answer the time from `tab/create` to its first frame (the engine running).
    OpenTab(String, mpsc::Sender<Result<Duration, String>>),
    Shutdown(mpsc::Sender<()>),
}

/// The spike's state, shared with the bench.
pub struct Spike {
    pub ready: Option<Ready>,
    pub error: Option<String>,
    pub surface: Option<Entity<BrowserSurface>>,
    pub requests: mpsc::Sender<SpikeRequest>,
}

/// Open `url` in the embedded engine in a hidden document tab ("Web Browser"). `size`: the tab's CSS size; `None`
/// fits the tab to the document area. The engine starts on a thread; the tab shows "Loading" until it has a frame.
pub fn open_spike(
    shell: &Entity<Shell>,
    url: String,
    size: Option<(u32, u32)>,
    cx: &mut App,
) -> Rc<RefCell<Spike>> {
    let (req_tx, req_rx) = mpsc::channel::<SpikeRequest>();
    let spike = Rc::new(RefCell::new(Spike {
        ready: None,
        error: None,
        surface: None,
        requests: req_tx,
    }));
    let profile = super::browser::profile_dir(shell.read(cx).solution_dir().as_deref());
    let viewport = size.unwrap_or((1280, 800));
    let (ready_tx, ready_rx) = futures::channel::oneshot::channel::<Result<Ready, String>>();
    let _ = std::thread::Builder::new()
        .name("browser-spike".into())
        .spawn(move || spike_thread(url, profile, viewport, ready_tx, req_rx));
    let shell = shell.clone();
    let state = spike.clone();
    cx.spawn(async move |cx| {
        let ready = ready_rx
            .await
            .unwrap_or_else(|_| Err("the browser thread ended".into()));
        cx.update(|cx| match ready {
            Ok(ready) => {
                let control = ready.control.clone();
                let tab = ready.tab.clone();
                let input: InputSink = {
                    let (control, tab) = (control.clone(), tab.clone());
                    Rc::new(move |e: Value| control.input(&tab, e))
                };
                let fit: Option<FitSink> = match size {
                    Some(_) => None,
                    None => Some(Rc::new(move |w, h, s| control.resize(&tab, w, h, s))),
                };
                let frames: Arc<dyn FrameSource> = ready.frames.clone();
                let surface = cx.new(|cx| BrowserSurface::new(frames, Some(input), fit, cx));
                shell.update(cx, |s, cx| {
                    s.show_browser_view(SPIKE_TAB, "Web Browser", surface.clone().into(), cx)
                });
                let mut st = state.borrow_mut();
                st.surface = Some(surface);
                st.ready = Some(ready);
            }
            Err(e) => {
                eprintln!("eludite: the embedded browser did not start: {e}");
                state.borrow_mut().error = Some(e);
            }
        });
    })
    .detach();
    spike
}

fn spike_thread(
    url: String,
    profile: std::path::PathBuf,
    viewport: (u32, u32),
    ready: futures::channel::oneshot::Sender<Result<Ready, String>>,
    requests: mpsc::Receiver<SpikeRequest>,
) {
    let t0 = Instant::now();
    let mut engine = EmbeddedChromium::new(
        EngineConfig {
            executable: None,
            profile_dir: profile,
            headless: true,
            viewport,
        },
        ChromiumSearch::defaults(),
        Arc::new(|l: &str| eprintln!("eludite: {l}")),
    );
    let opened = engine
        .launch()
        .map_err(|e| e.to_string())
        .and_then(|_| engine.open_tab(&url).map_err(|e| e.to_string()))
        .and_then(|tab| {
            let frames = engine.frames(&tab).ok_or("the tab has no frames")?;
            let control = engine.handle().ok_or("the engine is not running")?;
            let deadline = Instant::now() + Duration::from_secs(10);
            while frames.sequence() == 0 && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(1));
            }
            Ok(Ready {
                cold_to_first_frame: t0.elapsed(),
                launch: engine.last_launch().unwrap_or_default(),
                pid: engine.pid().unwrap_or(0),
                frames,
                control,
                tab,
            })
        });
    let ok = opened.is_ok();
    let _ = ready.send(opened);
    if !ok {
        return;
    }
    while let Ok(r) = requests.recv() {
        match r {
            SpikeRequest::OpenTab(url, reply) => {
                let r = engine
                    .open_tab(&url)
                    .map_err(|e| e.to_string())
                    .and_then(|tab| {
                        let frames = engine.frames(&tab).ok_or("no frames")?;
                        let deadline = Instant::now() + Duration::from_secs(10);
                        while frames.first_frame_after_create().is_none()
                            && Instant::now() < deadline
                        {
                            std::thread::sleep(Duration::from_micros(200));
                        }
                        frames
                            .first_frame_after_create()
                            .ok_or_else(|| "no frame in 10 s".to_owned())
                    });
                let _ = reply.send(r);
            }
            SpikeRequest::Shutdown(done) => {
                engine.shutdown();
                let _ = done.send(());
                return;
            }
        }
    }
    engine.shutdown();
}

impl Shell {
    /// Show a browser view in a document tab (the spike's hidden tab; brief B's window replaces it).
    pub fn show_browser_view(
        &mut self,
        id: &str,
        title: &str,
        view: AnyView,
        cx: &mut Context<Self>,
    ) {
        self.browser_views.borrow_mut().insert(id.to_owned(), view);
        self.controller.open_document(id, title);
        self.dock.update(cx, |_, cx| cx.notify());
        cx.notify();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicU64, Ordering};

    /// A frame source the test drives: a sequence, a 4 by 2 frame and its dirty rectangles.
    #[derive(Default)]
    struct Fake {
        sequence: AtomicU64,
        reads: AtomicU64,
        dirty: Mutex<Vec<DirtyRect>>,
    }

    impl FrameSource for Fake {
        fn sequence(&self) -> u64 {
            self.sequence.load(Ordering::SeqCst)
        }

        fn read(
            &self,
            _consume: bool,
            f: &mut dyn FnMut(&eludite_browser::embedded::Frame<'_>),
        ) -> bool {
            self.reads.fetch_add(1, Ordering::SeqCst);
            let seq = self.sequence();
            let pixels = vec![seq as u8; 4 * 2 * 4];
            let dirty = self.dirty.lock().unwrap().clone();
            f(&eludite_browser::embedded::Frame {
                sequence: seq,
                width: 4,
                height: 2,
                stride: 16,
                pixels: &pixels,
                dirty: &dirty,
                paint_ns: monotonic_ns(),
                copy_ns: 1000,
            });
            true
        }

        fn set_listener(&self, _f: Option<Box<dyn Fn() + Send + Sync>>) {}
    }

    #[gpui::test]
    fn the_surface_uploads_only_when_the_sequence_changes_and_reports_dirty_rectangles(
        cx: &mut gpui::TestAppContext,
    ) {
        let fake = Arc::new(Fake::default());
        let source: Arc<dyn FrameSource> = fake.clone();
        let sent: Rc<RefCell<Vec<Value>>> = Rc::default();
        let sink = sent.clone();
        let input: InputSink = Rc::new(move |e| sink.borrow_mut().push(e));
        let surface = cx.new(|cx| BrowserSurface::new(source, Some(input), None, cx));
        let stats = surface.read_with(cx, |s, _| s.stats());
        let window = cx.add_empty_window();
        let draw = |window: &mut gpui::VisualTestContext| {
            let s = surface.clone();
            window.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(200.), px(100.)),
                move |_, _| s.into_any_element(),
            );
        };

        // No frame yet: nothing read, nothing uploaded.
        draw(window);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 0);
        assert_eq!(stats.borrow().uploads, 0);

        *fake.dirty.lock().unwrap() = vec![DirtyRect {
            x: 0,
            y: 0,
            width: 4,
            height: 2,
        }];
        fake.sequence.store(1, Ordering::SeqCst);
        draw(window);
        assert_eq!(stats.borrow().uploads, 1);
        assert_eq!(stats.borrow().frame_size, (4, 2));
        assert_eq!(stats.borrow().last_sequence, 1);
        assert_eq!(
            stats.borrow().paint_ms.len(),
            1,
            "the paint of the new image is timed"
        );

        // The same sequence: drawn again from the same image, no read, no upload.
        draw(window);
        draw(window);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 1);
        assert_eq!(stats.borrow().uploads, 1);
        assert_eq!(stats.borrow().paint_ms.len(), 1);

        // A new frame with a small dirty rectangle: one upload, the rectangle reported.
        *fake.dirty.lock().unwrap() = vec![DirtyRect {
            x: 1,
            y: 1,
            width: 2,
            height: 1,
        }];
        fake.sequence.store(2, Ordering::SeqCst);
        draw(window);
        assert_eq!(stats.borrow().uploads, 2);
        assert_eq!(fake.reads.load(Ordering::SeqCst), 2);
        assert_eq!(
            stats.borrow().last_dirty,
            vec![DirtyRect {
                x: 1,
                y: 1,
                width: 2,
                height: 1
            }]
        );
        assert_eq!(stats.borrow().copy_ms, vec![0.001, 0.001]);

        // Input goes to the tab in its CSS pixels. (The test window's root is empty: draw the surface again between
        // events, since the press focuses it and the window refreshes.)
        let at = gpui::point(px(10.), px(5.));
        window.simulate_mouse_down(at, MouseButton::Left, Modifiers::none());
        draw(window);
        window.simulate_mouse_up(at, MouseButton::Left, Modifiers::none());
        let sent = sent.borrow();
        assert!(
            sent.iter().any(|e| e["type"] == "mouseDown"
                && e["x"] == 10
                && e["y"] == 5
                && e["button"] == "left"),
            "{sent:?}"
        );
        assert!(
            sent.iter().any(|e| e["type"] == "mouseUp" && e["x"] == 10),
            "{sent:?}"
        );
    }

    /// A frame source of a given size whose next frame's sequence and dirty rectangles the test sets.
    struct Sized {
        size: (u32, u32),
        sequence: AtomicU64,
        dirty: Mutex<Vec<DirtyRect>>,
    }

    impl FrameSource for Sized {
        fn sequence(&self) -> u64 {
            self.sequence.load(Ordering::SeqCst)
        }

        fn read(
            &self,
            _consume: bool,
            f: &mut dyn FnMut(&eludite_browser::embedded::Frame<'_>),
        ) -> bool {
            let (w, h) = self.size;
            let pixels = vec![self.sequence() as u8; (w * h * 4) as usize];
            let dirty = self.dirty.lock().unwrap().clone();
            f(&eludite_browser::embedded::Frame {
                sequence: self.sequence(),
                width: w,
                height: h,
                stride: w * 4,
                pixels: &pixels,
                dirty: &dirty,
                paint_ns: 0,
                copy_ns: 0,
            });
            true
        }

        fn set_listener(&self, _f: Option<Box<dyn Fn() + Send + Sync>>) {}
    }

    #[gpui::test]
    fn frames_upload_only_the_tiles_their_dirty_rectangles_touch(cx: &mut gpui::TestAppContext) {
        let fake = Arc::new(Sized {
            size: (600, 300),
            sequence: AtomicU64::new(0),
            dirty: Mutex::default(),
        });
        let source: Arc<dyn FrameSource> = fake.clone();
        let surface = cx.new(|cx| BrowserSurface::new(source, None, None, cx));
        surface.update(cx, |s, _| s.tiled = true);
        let stats = surface.read_with(cx, |s, _| s.stats());
        let window = cx.add_empty_window();
        let mut frame = |seq: u64, dirty: Vec<DirtyRect>| {
            fake.sequence.store(seq, Ordering::SeqCst);
            *fake.dirty.lock().unwrap() = dirty;
            let s = surface.clone();
            window.draw(
                gpui::point(px(0.), px(0.)),
                gpui::size(px(700.), px(400.)),
                move |_, _| s.into_any_element(),
            );
        };
        let small = DirtyRect {
            x: 300,
            y: 10,
            width: 20,
            height: 20,
        };
        // The first frame: every tile (3 by 2 of 256 pixels).
        frame(1, vec![small]);
        // The next one: the one tile its dirty rectangle touches.
        frame(2, vec![small]);
        // Across a tile boundary: the four tiles around (256, 256)... clipped to the frame's two rows.
        frame(
            3,
            vec![DirtyRect {
                x: 250,
                y: 250,
                width: 10,
                height: 10,
            }],
        );
        // A skipped sequence: the dirty rectangles of the frames in between are unknown, so every tile.
        frame(5, vec![small]);
        let s = stats.borrow();
        assert_eq!(s.tiles_uploaded, vec![6, 1, 4, 6]);
        assert_eq!(s.tiles_total, 6);
        assert_eq!(s.frame_size, (600, 300));
    }

    #[gpui::test]
    fn ime_composition_reaches_the_page(cx: &mut gpui::TestAppContext) {
        let sent: Rc<RefCell<Vec<Value>>> = Rc::default();
        let sink = sent.clone();
        let input: InputSink = Rc::new(move |e| sink.borrow_mut().push(e));
        let source: Arc<dyn FrameSource> = Arc::new(Fake::default());
        let surface = cx.new(|cx| BrowserSurface::new(source, Some(input), None, cx));
        let window = cx.add_empty_window();
        window.update(|window, cx| {
            surface.update(cx, |s, cx| {
                s.replace_and_mark_text_in_range(None, "\u{306B}", Some(1..1), window, cx);
                assert_eq!(s.marked_text_range(window, cx), Some(0..1));
                s.replace_and_mark_text_in_range(None, "\u{306B}\u{307B}", Some(2..2), window, cx);
                s.replace_text_in_range(None, "\u{65E5}\u{672C}", window, cx);
                assert_eq!(s.marked_text_range(window, cx), None);
                s.replace_and_mark_text_in_range(None, "x", None, window, cx);
                s.unmark_text(window, cx);
            })
        });
        let sent = sent.borrow();
        let kinds: Vec<&str> = sent.iter().map(|e| e["type"].as_str().unwrap()).collect();
        assert_eq!(
            kinds,
            [
                "imeSetComposition",
                "imeSetComposition",
                "imeCommitText",
                "imeSetComposition",
                "imeFinishComposing"
            ]
        );
        assert_eq!(sent[1]["text"], "\u{306B}\u{307B}");
        assert_eq!(sent[1]["selectionStart"], 2);
        assert_eq!(sent[2]["text"], "\u{65E5}\u{672C}");
    }

    #[test]
    fn key_codes_cursors_and_flags() {
        assert_eq!(key_codes("a"), Some((65, 38)));
        assert_eq!(key_codes("Z"), Some((90, 52)));
        assert_eq!(key_codes("7"), Some((55, 16)));
        assert_eq!(key_codes("enter"), Some((13, 36)));
        assert_eq!(key_codes("f5"), Some((116, 71)));
        assert_eq!(key_codes("left"), Some((37, 113)));
        assert_eq!(key_codes("é"), None);
        assert_eq!(key_codes("fly"), None);
        assert_eq!(cursor_style("pointer"), CursorStyle::PointingHand);
        assert_eq!(cursor_style("text"), CursorStyle::IBeam);
        assert_eq!(cursor_style("whatever"), CursorStyle::Arrow);
        assert_eq!(
            flags(&Modifiers {
                shift: true,
                control: true,
                ..Default::default()
            }),
            6
        );
    }
}
