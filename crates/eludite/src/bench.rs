//! Measurement harness (`--bench-start`, `--bench-drag N`). Runs inside the
//! real app with a real window and GPU, prints one JSON line, and exits.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eludite_docking::RenderProbe;
use gpui::{
    AnyWindowHandle, App, Entity, Modifiers, MouseButton, MouseDownEvent, MouseMoveEvent,
    MouseUpEvent, PlatformInput, Point, Window, point, px,
};
use serde_json::{Value, json};

use crate::shell::Shell;

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

fn wall_ns(t: SystemTime) -> u128 {
    t.duration_since(UNIX_EPOCH).map_or(0, |d| d.as_nanos())
}

/// Nearest-rank percentile of a sorted slice.
fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn summarize(samples: &[f64]) -> Value {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.total_cmp(b));
    let r = |x: f64| (x * 1000.).round() / 1000.;
    json!({
        "n": s.len(),
        "p50_ms": r(percentile(&s, 50.)),
        "p95_ms": r(percentile(&s, 95.)),
        "p99_ms": r(percentile(&s, 99.)),
        "max_ms": r(s.last().copied().unwrap_or(f64::NAN)),
    })
}

/// (current, peak) resident set size in MiB. Linux only.
fn rss_mib() -> Value {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else {
        return Value::Null;
    };
    let field = |name: &str| {
        s.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|kib| (kib / 1024. * 10.).round() / 10.)
    };
    json!({"rss_mib": field("VmRSS:"), "peak_rss_mib": field("VmHWM:")})
}

fn platform(window: &Window) -> Value {
    json!({
        "os": std::env::consts::OS,
        "wayland_display": std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        "display": std::env::var("DISPLAY").unwrap_or_default(),
        "scale_factor": window.scale_factor(),
        "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
    })
}

/// `--bench-start`: process start (or `ELUDITE_LAUNCH_WALL_NS`) to the end of
/// the first frame's present. The window accepts input from then on.
pub fn start(shell: &Entity<Shell>, t_main: Instant, join_wait: Duration, cx: &mut App) {
    let t_main_wall = SystemTime::now() - t_main.elapsed();
    shell.update(cx, |shell, _| {
        shell.after_first_present(move |window, _| {
            let now = Instant::now();
            let launch: Option<u128> = std::env::var("ELUDITE_LAUNCH_WALL_NS")
                .ok()
                .and_then(|v| v.parse().ok());
            let out = json!({
                "bench": "start",
                "main_to_first_present_ms": ms(now - t_main),
                "launch_to_first_present_ms": launch
                    .map(|l| (wall_ns(SystemTime::now()) as f64 - l as f64) / 1e6),
                "launch_to_main_ms": launch.map(|l| (wall_ns(t_main_wall) as f64 - l as f64) / 1e6),
                "layout_loader_join_wait_ms": ms(join_wait),
                "rss": rss_mib(),
                "platform": platform(window),
            });
            println!("{out}");
            std::process::exit(0);
        });
    });
}

/// `--bounds-out PATH`: keep PATH updated with probed element bounds.
pub fn bounds_out(shell: &Entity<Shell>, path: std::path::PathBuf, cx: &mut App) {
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx));
    cx.spawn(async move |cx| {
        let mut last = String::new();
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(200))
                .await;
            let text = {
                let mut p = probe.borrow_mut();
                p.renders.clear();
                p.presents.clear();
                p.guides_visible.clear();
                let mut map = serde_json::Map::new();
                for (k, b) in &p.bounds {
                    map.insert(
                        k.clone(),
                        json!([
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height)
                        ]),
                    );
                }
                // Elements not drawn any more drop out on the next frame.
                p.bounds.clear();
                Value::Object(map).to_string()
            };
            if text != "{}" && text != last {
                last = text.clone();
                let path = path.clone();
                cx.background_executor()
                    .spawn(async move {
                        let tmp = path.with_extension("tmp");
                        if std::fs::write(&tmp, text).is_ok() {
                            let _ = std::fs::rename(&tmp, &path);
                        }
                    })
                    .detach();
            }
            cx.update(|cx| cx.refresh_windows());
        }
    })
    .detach();
}

struct DragBench {
    probe: Rc<RefCell<RenderProbe>>,
    frames: usize,
    warmup: usize,
    path: Vec<Point<gpui::Pixels>>,
    step: usize,
    frame_starts: Vec<Instant>,
    end: Point<gpui::Pixels>,
}

/// `--bench-drag N`: press on the Output tab, start a drag, then move the
/// pointer every frame for N frames back and forth across the document area
/// and over the Dock Left guide, with the guides drawn. Each frame's cost is
/// frame start (the `on_next_frame` callback, where the move is dispatched)
/// to the end of that frame's present: input, layout, paint and present,
/// without the wait for the next refresh.
pub fn drag(shell: &Entity<Shell>, window: AnyWindowHandle, frames: usize, cx: &mut App) {
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx));
    let st = Rc::new(RefCell::new(DragBench {
        probe,
        frames,
        warmup: 30,
        path: Vec::new(),
        step: 0,
        frame_starts: Vec::new(),
        end: point(px(0.), px(0.)),
    }));
    let _ = window.update(cx, |_, window, _| {
        window.on_next_frame(move |window, cx| tick(st, window, cx));
    });
}

fn mouse_move(window: &mut Window, cx: &mut App, at: Point<gpui::Pixels>) {
    window.dispatch_event(
        PlatformInput::MouseMove(MouseMoveEvent {
            position: at,
            pressed_button: Some(MouseButton::Left),
            modifiers: Modifiers::none(),
        }),
        cx,
    );
}

fn tick(st: Rc<RefCell<DragBench>>, window: &mut Window, cx: &mut App) {
    let now = Instant::now();
    let mut b = st.borrow_mut();
    if b.warmup > 0 {
        b.warmup -= 1;
        if b.warmup == 0 {
            let bounds = b.probe.borrow().bounds.clone();
            let (Some(tab), Some(docs), Some(guide)) = (
                bounds.get("tab-output"),
                bounds.get("documents"),
                bounds.get("guide-left"),
            ) else {
                // Guides are only laid out during a drag; start it first.
                if let Some(tab) = bounds.get("tab-output") {
                    let start = tab.center();
                    window.dispatch_event(
                        PlatformInput::MouseDown(MouseDownEvent {
                            button: MouseButton::Left,
                            position: start,
                            modifiers: Modifiers::none(),
                            click_count: 1,
                            first_mouse: false,
                        }),
                        cx,
                    );
                    mouse_move(window, cx, start + point(px(40.), px(-40.)));
                    b.warmup = 10;
                } else {
                    eprintln!("eludite bench: Output tab not found");
                    std::process::exit(1);
                }
                drop(b);
                window.refresh();
                window.on_next_frame(move |window, cx| tick(st, window, cx));
                return;
            };
            let _ = tab;
            // A loop: left to right across the documents at mid height, then
            // onto the left guide, and back.
            let y = docs.center().y;
            let (x0, x1) = (docs.left() + px(20.), docs.right() - px(20.));
            let n = 60;
            for i in 0..=n {
                let f = i as f32 / n as f32;
                b.path.push(point(x0 + (x1 - x0) * f, y));
            }
            for i in (0..=n).rev() {
                let f = i as f32 / n as f32;
                b.path.push(point(x0 + (x1 - x0) * f, y));
            }
            b.path.push(guide.center());
            b.end = docs.center();
            b.probe.borrow_mut().renders.clear();
            b.probe.borrow_mut().presents.clear();
            b.probe.borrow_mut().guides_visible.clear();
        }
        drop(b);
        window.refresh();
        window.on_next_frame(move |window, cx| tick(st, window, cx));
        return;
    }

    if b.frame_starts.len() == b.frames {
        // One extra frame has been presented; report.
        report(&b, window, cx);
        window.dispatch_event(
            PlatformInput::MouseUp(MouseUpEvent {
                button: MouseButton::Left,
                position: b.end,
                modifiers: Modifiers::none(),
                click_count: 1,
            }),
            cx,
        );
        cx.quit();
        return;
    }
    b.frame_starts.push(now);
    let at = b.path[b.step % b.path.len()];
    b.step += 1;
    drop(b);
    mouse_move(window, cx, at);
    window.on_next_frame(move |window, cx| tick(st, window, cx));
}

fn report(b: &DragBench, window: &Window, _cx: &App) {
    let probe = b.probe.borrow();
    let presents = &probe.presents;
    // Pair each frame start with the first present after it and before the
    // next frame start.
    let mut cost = Vec::new();
    let mut j = 0;
    for (i, s) in b.frame_starts.iter().enumerate() {
        while j < presents.len() && presents[j] < *s {
            j += 1;
        }
        let next = b.frame_starts.get(i + 1);
        if let Some(p) = presents.get(j)
            && next.is_none_or(|n| p < n)
        {
            cost.push(ms(*p - *s));
        }
    }
    let render_to_present: Vec<f64> = probe
        .renders
        .iter()
        .zip(presents.iter())
        .map(|(r, p)| ms(*p - *r))
        .collect();
    let intervals: Vec<f64> = b.frame_starts.windows(2).map(|w| ms(w[1] - w[0])).collect();
    let guides = probe.guides_visible.iter().filter(|g| **g).count();
    // The slowest frames and where they fall in the run (warm-up or steady).
    let mut worst: Vec<(usize, f64)> = cost.iter().copied().enumerate().collect();
    worst.sort_by(|a, b| b.1.total_cmp(&a.1));
    worst.truncate(10);
    let out = json!({
        "bench": "drag",
        "method": "on_next_frame -> Window::dispatch_event(MouseMove) during an active tool window drag; frame start to end of present",
        "frames": b.frame_starts.len(),
        "frames_presented": cost.len(),
        "renders_with_guides_visible": guides,
        "renders": probe.renders.len(),
        "frame_cost": summarize(&cost),
        "render_to_present": summarize(&render_to_present),
        "frame_interval": summarize(&intervals),
        "worst_frames": worst.iter().map(|(i, c)| json!([i, c])).collect::<Vec<_>>(),
        "rss": rss_mib(),
        "platform": platform(window),
    });
    println!("{out}");
}

/// `--timings-out PATH`: once editable text, the tree and the first diagnostics have all arrived (or after 180 s),
/// write how long each took from the `eludite.solution.open` command, as JSON.
pub fn timings_out(
    shell: Entity<Shell>,
    path: std::path::PathBuf,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    let started = Instant::now();
    cx.spawn_in(window, async move |_, cx| {
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(20))
                .await;
            let Ok(t) = cx.update(|_, cx| shell.read(cx).timings().clone()) else {
                return;
            };
            let done =
                t.editable.is_some() && t.tree.is_some() && t.first_nonempty_diagnostics.is_some();
            if !done && started.elapsed() < Duration::from_secs(180) {
                continue;
            }
            let since = |x: Option<Instant>| match (t.open, x) {
                (Some(o), Some(x)) => json!(ms(x.saturating_duration_since(o))),
                _ => Value::Null,
            };
            let out = json!({
                "bench": "open_solution",
                "open_to_editable_ms": since(t.editable),
                "open_to_tree_ms": since(t.tree),
                "open_to_first_diagnostics_ms": since(t.first_diagnostics),
                "open_to_first_nonempty_diagnostics_ms": since(t.first_nonempty_diagnostics),
                "open_to_loaded_ms": since(t.loaded),
                "rss": rss_mib(),
            });
            if let Err(e) = std::fs::write(&path, format!("{out}\n")) {
                eprintln!("eludite: --timings-out {}: {e}", path.display());
            }
            return;
        }
    })
    .detach();
}
