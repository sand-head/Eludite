//! A scrolling, editable view over the generated buffer, built on GPUI's
//! `uniform_list` so only visible lines are laid out each frame.
//!
//! Instrumentation: every key event that edits the buffer records its arrival
//! time; the next render schedules a `defer` callback, which GPUI runs after
//! that frame has been submitted to the platform (`Window::present`). The
//! difference is the keystroke-to-present latency.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Instant, SystemTime};

use gpui::{
    App, Context, FocusHandle, Focusable, IntoElement, KeyDownEvent, ParentElement, Pixels, Render,
    ScrollStrategy, SharedString, Styled, UniformListScrollHandle, Window, div, prelude::*, px,
    rgb, uniform_list,
};

use crate::buffer::{Buffer, Cursor};

pub const LINE_HEIGHT: f32 = 20.;
pub const FONT_SIZE: f32 = 14.;

pub fn mono_font() -> SharedString {
    if let Ok(f) = std::env::var("SPIKE_FONT") {
        return f.into();
    }
    if cfg!(target_os = "windows") {
        "Consolas".into()
    } else if cfg!(target_os = "macos") {
        "Menlo".into()
    } else {
        "Noto Sans Mono".into()
    }
}

/// One keystroke latency sample.
#[derive(Debug, Clone, Copy)]
pub struct KeySample {
    /// Wall-clock time the key handler ran (for correlating with an external injector).
    pub handler_wall: SystemTime,
    /// Handler entry to end of `present` for the frame showing the edit.
    pub to_present_ns: u64,
    /// Wall-clock time after present.
    pub present_wall: SystemTime,
    /// Render of the view (inside `Window::draw`) to end of present: the
    /// frame's own cost, excluding the wait for the next frame tick.
    pub render_to_present_ns: u64,
}

pub type SampleSink = Box<dyn Fn(&KeySample)>;

#[derive(Default)]
pub struct Probes {
    pub key_samples: Vec<KeySample>,
    /// Callback per sample (used by `--key-probe` to print lines as they happen).
    pub on_sample: Option<SampleSink>,
    /// When set, every render of the view records the time its frame finished presenting.
    pub record_frames: bool,
    pub frame_presents: Vec<Instant>,
    /// One-shot callbacks run after the next frame containing this view is presented.
    pub after_present: Vec<Box<dyn FnOnce()>>,
}

pub struct TextView {
    pub buffer: Buffer,
    pub cursor: Cursor,
    pub scroll: UniformListScrollHandle,
    focus: FocusHandle,
    font: SharedString,
    pending_key: Option<(Instant, SystemTime)>,
    pub probes: Rc<RefCell<Probes>>,
}

impl TextView {
    pub fn new(buffer: Buffer, cx: &mut Context<Self>) -> Self {
        Self {
            buffer,
            cursor: Cursor::default(),
            scroll: UniformListScrollHandle::new(),
            focus: cx.focus_handle(),
            font: mono_font(),
            pending_key: None,
            probes: Rc::default(),
        }
    }

    pub fn scroll_offset(&self) -> Pixels {
        -self.scroll.0.borrow().base_handle.offset().y
    }

    pub fn max_scroll(&self) -> Pixels {
        self.scroll.0.borrow().base_handle.max_offset().y
    }

    pub fn set_scroll_offset(&self, y: Pixels) {
        let h = self.scroll.0.borrow().base_handle.clone();
        let x = h.offset().x;
        h.set_offset(gpui::point(x, -y));
    }

    pub fn place_cursor(&mut self, line: usize) {
        self.cursor = Cursor {
            line: line.min(self.buffer.len() - 1),
            col: 0,
        };
        self.scroll
            .scroll_to_item(self.cursor.line, ScrollStrategy::Center);
    }

    /// Apply a key. Returns true if it was handled.
    pub fn apply_key(&mut self, ev: &KeyDownEvent) -> bool {
        let ks = &ev.keystroke;
        let m = ks.modifiers;
        let b = &mut self.buffer;
        let c = &mut self.cursor;
        match ks.key.as_str() {
            "backspace" => b.backspace(c),
            "enter" => b.newline(c),
            "left" => b.move_left(c),
            "right" => b.move_right(c),
            "up" => b.move_vertical(c, -1),
            "down" => b.move_vertical(c, 1),
            "pageup" => b.move_vertical(c, -40),
            "pagedown" => b.move_vertical(c, 40),
            "home" => c.col = 0,
            "end" => c.col = b.lines[c.line].text.len(),
            "tab" => b.insert(c, "    "),
            _ => match &ks.key_char {
                Some(s) if !m.control && !m.alt && !m.platform && !s.is_empty() => b.insert(c, s),
                _ => return false,
            },
        }
        self.scroll
            .scroll_to_item(self.cursor.line, ScrollStrategy::Nearest);
        true
    }

    fn on_key(&mut self, ev: &KeyDownEvent, _window: &mut Window, cx: &mut Context<Self>) {
        let t = (Instant::now(), SystemTime::now());
        if self.apply_key(ev) {
            // Keep the earliest unrendered keystroke: that is the one whose latency matters.
            self.pending_key.get_or_insert(t);
            cx.stop_propagation();
            cx.notify();
        }
    }

    fn render_line(&self, ix: usize) -> impl IntoElement + use<> {
        let line = &self.buffer.lines[ix];
        let gutter = div()
            .w(px(64.))
            .flex_none()
            .pr_3()
            .flex()
            .justify_end()
            .text_color(rgb(0x2B91AF))
            .child(SharedString::from((ix + 1).to_string()));
        let row = div()
            .id(ix)
            .h(px(LINE_HEIGHT))
            .flex()
            .flex_row()
            .whitespace_nowrap()
            .child(gutter);
        if ix == self.cursor.line {
            let (a, b) = line.text.split_at(self.cursor.col);
            row.bg(rgb(0x2A2A2A))
                .child(SharedString::from(a.to_owned()))
                .child(div().w(px(2.)).h_full().bg(rgb(0xAEAFAD)))
                .child(SharedString::from(b.to_owned()))
        } else {
            row.child(SharedString::from(line.text.clone()))
        }
    }
}

impl Focusable for TextView {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TextView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if let Some((t, wall)) = self.pending_key.take() {
            let probes = self.probes.clone();
            let rendered = Instant::now();
            // Runs after this frame's `present` (end of the frame's effect cycle).
            cx.defer(move |_| {
                let s = KeySample {
                    handler_wall: wall,
                    to_present_ns: t.elapsed().as_nanos() as u64,
                    present_wall: SystemTime::now(),
                    render_to_present_ns: rendered.elapsed().as_nanos() as u64,
                };
                let mut p = probes.borrow_mut();
                if let Some(f) = &p.on_sample {
                    f(&s);
                }
                p.key_samples.push(s);
            });
        }
        {
            let mut p = self.probes.borrow_mut();
            let once = std::mem::take(&mut p.after_present);
            if p.record_frames || !once.is_empty() {
                let probes = self.probes.clone();
                let record = p.record_frames;
                cx.defer(move |_| {
                    let now = Instant::now();
                    if record {
                        probes.borrow_mut().frame_presents.push(now);
                    }
                    for f in once {
                        f();
                    }
                });
            }
        }
        let count = self.buffer.len();
        div()
            .id("text-view")
            .track_focus(&self.focus)
            .on_key_down(cx.listener(Self::on_key))
            .size_full()
            .bg(rgb(0x1E1E1E))
            .text_color(rgb(0xDCDCDC))
            .font_family(self.font.clone())
            .text_size(px(FONT_SIZE))
            .line_height(px(LINE_HEIGHT))
            .child(
                uniform_list(
                    "lines",
                    count,
                    cx.processor(|this, range: std::ops::Range<usize>, _w, _cx| {
                        range.map(|ix| this.render_line(ix)).collect::<Vec<_>>()
                    }),
                )
                .track_scroll(&self.scroll)
                .size_full(),
            )
    }
}
