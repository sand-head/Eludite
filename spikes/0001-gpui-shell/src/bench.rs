//! Measurement drivers and statistics.
//!
//! All drivers run inside the real app with a real window and GPU; they
//! print one JSON object to stdout and quit.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use gpui::{App, Entity, Keystroke, Window, px};
use serde_json::{Value, json};

use crate::text_view::TextView;

/// Percentile by nearest rank on a sorted copy.
pub fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

pub fn summarize(samples_ms: &[f64]) -> Value {
    let mut s = samples_ms.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mean = s.iter().sum::<f64>() / s.len().max(1) as f64;
    json!({
        "n": s.len(),
        "p50_ms": round3(percentile(&s, 50.)),
        "p95_ms": round3(percentile(&s, 95.)),
        "p99_ms": round3(percentile(&s, 99.)),
        "max_ms": round3(s.last().copied().unwrap_or(f64::NAN)),
        "min_ms": round3(s.first().copied().unwrap_or(f64::NAN)),
        "mean_ms": round3(mean),
    })
}

fn round3(x: f64) -> f64 {
    (x * 1000.).round() / 1000.
}

pub fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

pub fn wall_ns(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// Resident set size in KiB (current, peak): the working set and its peak on Windows.
#[cfg(windows)]
pub fn rss_kib() -> Option<(u64, u64)> {
    // PROCESS_MEMORY_COUNTERS, declared here to keep the spike free of a Windows API crate.
    #[repr(C)]
    #[derive(Default)]
    struct Counters {
        cb: u32,
        page_fault_count: u32,
        peak_working_set_size: usize,
        working_set_size: usize,
        quota_peak_paged_pool_usage: usize,
        quota_paged_pool_usage: usize,
        quota_peak_non_paged_pool_usage: usize,
        quota_non_paged_pool_usage: usize,
        pagefile_usage: usize,
        peak_pagefile_usage: usize,
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> isize;
        fn K32GetProcessMemoryInfo(process: isize, counters: *mut Counters, cb: u32) -> i32;
    }
    let mut c = Counters {
        cb: size_of::<Counters>() as u32,
        ..Default::default()
    };
    // SAFETY: a pseudo-handle to this process and a counters struct of the size passed.
    let ok = unsafe { K32GetProcessMemoryInfo(GetCurrentProcess(), &mut c, c.cb) };
    (ok != 0).then(|| {
        (
            c.working_set_size as u64 / 1024,
            c.peak_working_set_size as u64 / 1024,
        )
    })
}

/// Resident set size in KiB (current, peak) from /proc; None off Linux.
#[cfg(not(windows))]
pub fn rss_kib() -> Option<(u64, u64)> {
    let s = std::fs::read_to_string("/proc/self/status").ok()?;
    let field = |name: &str| {
        s.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<u64>().ok())
    };
    Some((field("VmRSS:")?, field("VmHWM:")?))
}

pub fn rss_json() -> Value {
    match rss_kib() {
        Some((rss, hwm)) => {
            json!({"rss_mib": round3(rss as f64 / 1024.), "peak_rss_mib": round3(hwm as f64 / 1024.)})
        }
        None => json!(null),
    }
}

pub fn platform_json(window: &Window) -> Value {
    json!({
        "os": std::env::consts::OS,
        "wayland_display": std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        "display": std::env::var("DISPLAY").unwrap_or_default(),
        "xdg_session_type": std::env::var("XDG_SESSION_TYPE").unwrap_or_default(),
        "scale_factor": window.scale_factor(),
        "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
        "window_active": window.is_window_active(),
    })
}

pub struct ScrollBench {
    pub tv: Entity<TextView>,
    pub step_px: f32,
    pub refresh_hz: Option<f64>,
    pub warmup: usize,
    frame_starts: Vec<Instant>,
    started: Option<Instant>,
    rss_before: Value,
}

impl ScrollBench {
    pub fn new(tv: Entity<TextView>, lines_per_frame: f32, refresh_hz: Option<f64>) -> Self {
        Self {
            tv,
            step_px: lines_per_frame * crate::text_view::LINE_HEIGHT,
            refresh_hz,
            warmup: 60,
            frame_starts: Vec::new(),
            started: None,
            rss_before: Value::Null,
        }
    }

    /// Drive one frame. Re-arms itself via `on_next_frame` until the bottom is reached.
    pub fn tick(this: Rc<RefCell<Self>>, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let mut st = this.borrow_mut();
        if std::env::var_os("SPIKE_DEBUG").is_some() {
            eprintln!("tick warmup={} frames={}", st.warmup, st.frame_starts.len());
        }
        if st.warmup > 0 {
            st.warmup -= 1;
            if st.warmup == 0 {
                st.tv.update(cx, |tv, _| {
                    tv.set_scroll_offset(px(0.));
                    let mut p = tv.probes.borrow_mut();
                    p.record_frames = true;
                    p.frame_presents.clear();
                });
                st.rss_before = rss_json();
                st.started = Some(now);
            }
            st.tv.update(cx, |_, cx| cx.notify());
        } else {
            st.frame_starts.push(now);
            let step = st.step_px;
            let done = st.tv.update(cx, |tv, cx| {
                let max = tv.max_scroll();
                let y = tv.scroll_offset() + px(step);
                cx.notify();
                if y >= max {
                    tv.set_scroll_offset(max);
                    true
                } else {
                    tv.set_scroll_offset(y);
                    false
                }
            });
            if done {
                // One more frame so the last present is recorded, then report.
                drop(st);
                let this2 = this.clone();
                window.on_next_frame(move |window, cx| {
                    this2.borrow().report(window, cx);
                    cx.quit();
                });
                return;
            }
        }
        drop(st);
        window.on_next_frame(move |window, cx| Self::tick(this, window, cx));
    }

    fn report(&self, window: &Window, cx: &mut App) {
        let presents = self.tv.read(cx).probes.borrow().frame_presents.clone();
        let intervals: Vec<f64> = self
            .frame_starts
            .windows(2)
            .map(|w| ms(w[1] - w[0]))
            .collect();
        // Work per frame: frame start (before layout) to end of present.
        // Pair each frame start with the first present after it and before the next start.
        let mut work = Vec::new();
        let mut j = 0;
        for (i, s) in self.frame_starts.iter().enumerate() {
            while j < presents.len() && presents[j] < *s {
                j += 1;
            }
            let next = self.frame_starts.get(i + 1);
            if let Some(p) = presents.get(j)
                && next.is_none_or(|n| p < n)
            {
                work.push(ms(*p - *s));
            }
        }
        let mut sorted = intervals.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let hz = self
            .refresh_hz
            .unwrap_or_else(|| 1000. / percentile(&sorted, 50.));
        let period = 1000. / hz;
        let over = |k: f64| intervals.iter().filter(|&&x| x > k * period).count();
        // Frames the display could have shown but we did not: sum over intervals of round(x/period)-1.
        let missed: f64 = intervals
            .iter()
            .map(|x| ((x / period).round() - 1.).max(0.))
            .sum();
        let total = self.started.map(|s| ms(s.elapsed())).unwrap_or(0.);
        let out = json!({
            "bench": "scroll",
            "lines": self.tv.read(cx).buffer.len(),
            "lines_per_frame": self.step_px / crate::text_view::LINE_HEIGHT,
            "refresh_hz": hz,
            "refresh_hz_source": if self.refresh_hz.is_some() { "cli" } else { "estimated from p50 interval" },
            "frames": self.frame_starts.len(),
            "duration_ms": round3(total),
            "frame_interval": summarize(&intervals),
            "frame_work": summarize(&work),
            "intervals_over_1_5x_refresh": over(1.5),
            "intervals_over_2x_refresh": over(2.0),
            "missed_vblanks_estimate": missed,
            "rss_before": self.rss_before,
            "rss_after": rss_json(),
            "platform": platform_json(window),
        });
        println!("{}", serde_json::to_string(&out).unwrap());
    }
}

/// Tiny deterministic PRNG for jittering synthetic keystroke timing.
pub struct Lcg(u64);
impl Lcg {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }
    pub fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

/// Synthetic keystrokes dispatched through `Window::dispatch_keystroke` at
/// random phase relative to vsync (15-45 ms apart). Measures handler entry to
/// end of present of the frame that shows the edit.
pub fn bench_keys(
    window_handle: gpui::AnyWindowHandle,
    tv: Entity<TextView>,
    count: usize,
    line: usize,
    cx: &mut App,
) {
    tv.update(cx, |tv, cx| {
        tv.place_cursor(line);
        tv.probes.borrow_mut().key_samples.clear();
        cx.notify();
    });
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let mut rng = Lcg::new(0x5eed);
        // Let the scroll to `line` settle.
        executor.timer(Duration::from_millis(300)).await;
        let rss_before = cx.update(|_| rss_json());
        for i in 0..count {
            let delay = 15. + 30. * rng.next_f64();
            executor
                .timer(Duration::from_micros((delay * 1000.) as u64))
                .await;
            let key = if i % 40 == 39 {
                "enter".to_owned()
            } else if i % 17 == 16 {
                "backspace".to_owned()
            } else {
                ((b'a' + (i % 26) as u8) as char).to_string()
            };
            let ks = Keystroke::parse(&key).expect("valid keystroke");
            let _ = window_handle.update(cx, |_, window, cx| {
                window.dispatch_keystroke(ks, cx);
            });
        }
        executor.timer(Duration::from_millis(200)).await;
        let _ = window_handle.update(cx, |_, window, cx| {
            let probes = tv.read(cx).probes.clone();
            let probes = probes.borrow();
            let samples: Vec<f64> = probes
                .key_samples
                .iter()
                .map(|s| s.to_present_ns as f64 / 1e6)
                .collect();
            let render: Vec<f64> = probes
                .key_samples
                .iter()
                .map(|s| s.render_to_present_ns as f64 / 1e6)
                .collect();
            let out = json!({
                "bench": "keys",
                "method": "Window::dispatch_keystroke at random vsync phase; handler entry to end of present",
                "keystrokes_sent": count,
                "samples": samples.len(),
                "at_line": line,
                "key_to_present": summarize(&samples),
                "render_to_present": summarize(&render),
                "rss_before": rss_before,
                "rss_after": rss_json(),
                "platform": platform_json(window),
            });
            println!("{}", serde_json::to_string(&out).unwrap());
            cx.quit();
        });
    })
    .detach();
}
