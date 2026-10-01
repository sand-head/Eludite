//! Measurement harness (`--bench-start`, `--bench-drag N`). Runs inside the
//! real app with a real window and GPU, prints one JSON line, and exits.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use eludite_docking::RenderProbe;
use gpui::{
    AnyWindowHandle, App, Entity, Focusable as _, Keystroke, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, PlatformInput, Point, Window, point, px,
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
    let shell = shell.clone();
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
                drop(p);
                // Where the active editor's caret is drawn, for pointing at text (brief 0013's hover run).
                let caret = cx.update(|cx| {
                    let s = shell.read(cx);
                    let id = s.active_document()?;
                    let editor = s.editor(std::path::Path::new(&id))?;
                    let v = editor.read(cx);
                    let at = v.pixel_position_for_offset(v.editor().primary_selection().head)?;
                    Some(json!([
                        f32::from(at.x),
                        f32::from(at.y),
                        2.0,
                        f32::from(v.line_height())
                    ]))
                });
                if let Some(c) = caret {
                    map.insert("editor-caret".into(), c);
                }
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

/// `--timings-out PATH`: once editable text, the tree and the first diagnostics for the opened file have all arrived
/// (or after 180 s),
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
            let done = t.editable.is_some()
                && t.tree.is_some()
                && t.first_diagnostics.is_some()
                && t.loaded.is_some();
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

/// `--bench-type N`: keystroke frame cost in the shell while the host's diagnostics arrive (brief 0012 budget,
/// method as brief 0009's viewer: `Window::dispatch_keystroke`, frame cost = key handler + render to end of present,
/// excluding the wait for the next refresh). Prints one JSON line and quits. Without a solution (`with_host`
/// false) it types into the file at once, with no host and no diagnostics: the baseline.
pub fn type_keys(
    shell: Entity<Shell>,
    file: std::path::PathBuf,
    count: usize,
    with_host: bool,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    cx.spawn_in(window, async move |_, cx| {
        // Wait for the load and the first diagnostics, as a user would.
        loop {
            executor.timer(Duration::from_millis(20)).await;
            let Ok(ready) = cx.update(|_, cx| {
                let t = shell.read(cx).timings();
                if with_host {
                    t.loaded.is_some() && t.first_diagnostics.is_some()
                } else {
                    shell.read(cx).editor(&file).is_some()
                }
            }) else {
                return;
            };
            if ready {
                break;
            }
        }
        executor.timer(Duration::from_millis(1000)).await;
        let Ok(Some(editor)) = cx.update(|window, cx| {
            let editor = shell.read(cx).editor(&file)?;
            // The end of a line in the middle of the file.
            editor.update(cx, |v, cx| {
                v.update_editor(cx, |e| {
                    let b = e.buffer();
                    let row = b.line_count() / 2;
                    let at = b.point_to_offset(eludite_editor::text::Point::new(row, b.line_len(row)));
                    e.set_caret(at);
                })
            });
            window.focus(&editor.focus_handle(cx), cx);
            Some(editor)
        }) else {
            eprintln!("eludite bench: {} is not open", file.display());
            std::process::exit(1);
        };
        let events_before = cx
            .update(|_, cx| shell.read(cx).timings().diagnostics_events)
            .unwrap_or_default();
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        let mut seed: u64 = 0x5eed;
        let mut next = move || {
            seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            (seed >> 11) as f64 / (1u64 << 53) as f64
        };
        let mut keys: Vec<(Instant, Instant)> = Vec::with_capacity(count);
        for i in 0..count {
            let pause = if i % 25 == 24 { 250. } else { 0. };
            let delay = pause + 15. + 30. * next();
            executor
                .timer(Duration::from_micros((delay * 1000.) as u64))
                .await;
            let key = if i % 17 == 16 {
                "backspace".to_owned()
            } else {
                ((b'a' + (i % 26) as u8) as char).to_string()
            };
            let ks = Keystroke::parse(&key).expect("keystroke");
            let _ = cx.update(|window, cx| {
                let t0 = Instant::now();
                window.dispatch_keystroke(ks, cx);
                keys.push((t0, Instant::now()));
            });
        }
        executor.timer(Duration::from_millis(300)).await;
        let _ = cx.update(|window, cx| {
            let p = probe.borrow();
            let frames: Vec<(Instant, Instant)> = p
                .renders
                .iter()
                .copied()
                .zip(p.presents.iter().copied())
                .collect();
            let mut cost = Vec::new();
            let mut handler = Vec::new();
            for (t0, t1) in &keys {
                handler.push(ms(*t1 - *t0));
                if let Some((r, pr)) = frames.iter().find(|(r, _)| r >= t1) {
                    cost.push(ms(*t1 - *t0) + ms(pr.saturating_duration_since(*r)));
                }
            }
            let all: Vec<f64> = frames.iter().map(|(r, pr)| ms(pr.saturating_duration_since(*r))).collect();
            let events = shell.read(cx).timings().diagnostics_events - events_before;
            let lines = editor.read(cx).editor().buffer().line_count();
            let out = json!({
                "bench": "type_in_shell",
                "method": "Window::dispatch_keystroke, bursts of 25 keys 15-45 ms apart with 250 ms pauses; frame cost = key handler + render to end of present (the next frame after the key)",
                "file": file.to_string_lossy(),
                "with_host": with_host,
                "lines": lines,
                "keystrokes": count,
                "samples": cost.len(),
                "diagnostics_events_during_run": events,
                "keystroke_frame_cost": summarize(&cost),
                "key_handler": summarize(&handler),
                "all_frames_render_to_present": summarize(&all),
                "rss": rss_mib(),
                "platform": platform(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}

/// `--bench-complete N` (brief 0013): completion latency, keystroke frame cost with the list open, and memory over
/// N completion cycles in the real app against the real host. One cycle types `.` after a member-access target,
/// waits for the language server's list to be drawn, types two filter keys, then Escape and three Backspaces.
///
/// Per trigger: host latency = request written to the host until its reply was read (the session's waiter thread);
/// UI latency = the shell's own work in trigger-to-pixels: the key handler, flush and hand-off to the worker before
/// the request is written, applying the items, and rendering and presenting the frame that shows the list. The idle
/// wait for that frame (the display refresh) is reported separately; trigger-to-visible is the whole interval.
/// Keystroke frame cost is brief 0009's method (key handler + the next frame's render to end of present).
pub fn complete(
    shell: Entity<Shell>,
    file: std::path::PathBuf,
    count: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    const WARMUP: usize = 10;
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    let needle =
        std::env::var("ELUDITE_BENCH_COMPLETE_AFTER").unwrap_or_else(|_| "_sdkDiscoverer".into());
    cx.spawn_in(window, async move |_, cx| {
        // The solution loaded and the file's semantics warmed, as a user would have it.
        loop {
            executor.timer(Duration::from_millis(20)).await;
            let Ok(ready) = cx.update(|_, cx| {
                let t = shell.read(cx).timings();
                t.loaded.is_some() && t.first_diagnostics.is_some()
            }) else {
                return;
            };
            if ready {
                break;
            }
        }
        executor.timer(Duration::from_millis(2000)).await;
        let Ok(Some(editor)) = cx.update(|window, cx| {
            let editor = shell.read(cx).editor(&file)?;
            let found = editor.update(cx, |v, cx| {
                v.update_editor(cx, |e| {
                    let b = e.buffer();
                    // A statement that already uses the target as `target.Member`, so the new line is in a body.
                    let access = format!("{needle}.");
                    let row = (0..b.line_count()).find(|r| b.line(*r).contains(access.as_str()))?;
                    let indent: String = b.line(row).chars().take_while(|c| c.is_whitespace()).collect();
                    e.set_caret(b.point_to_offset(eludite_editor::text::Point::new(row, b.line_len(row))));
                    e.insert(&format!("\n{indent}{needle}"));
                    Some(())
                })
            });
            if found.is_none() {
                eprintln!("eludite bench: no line contains {needle}");
                std::process::exit(1);
            }
            window.focus(&editor.focus_handle(cx), cx);
            Some(editor)
        }) else {
            eprintln!("eludite bench: {} is not open", file.display());
            std::process::exit(1);
        };
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        let key = |cx: &mut gpui::AsyncWindowContext, k: &str| -> (Instant, Instant) {
            let ks = Keystroke::parse(k).expect("keystroke");
            let mut out = (Instant::now(), Instant::now());
            let _ = cx.update(|window, cx| {
                let t0 = Instant::now();
                window.dispatch_keystroke(ks, cx);
                out = (t0, Instant::now());
            });
            out
        };
        let mut host = Vec::new();
        let mut ui = Vec::new();
        let mut total = Vec::new();
        let mut frame_wait = Vec::new();
        let mut filter_keys: Vec<(Instant, Instant)> = Vec::new();
        let mut dot_keys: Vec<(Instant, Instant)> = Vec::new();
        let mut timeouts = 0;
        let mut rss = Vec::new();
        for i in 0..(WARMUP + count) {
            if i == WARMUP || (i > WARMUP && (i - WARMUP).is_multiple_of(50)) {
                rss.push(json!({"cycle": i - WARMUP, "rss": rss_mib()}));
            }
            let before = cx
                .update(|_, cx| shell.read(cx).completion_timings().len())
                .unwrap_or_default();
            let dot = key(cx, ".");
            // Wait for the server's list to be applied, then for the frame that shows it.
            let deadline = Instant::now() + Duration::from_secs(5);
            let mut applied = None;
            while Instant::now() < deadline {
                executor.timer(Duration::from_micros(500)).await;
                let found = cx
                    .update(|_, cx| {
                        let s = shell.read(cx);
                        let visible = editor.read(cx).completion().is_some_and(|c| c.visible);
                        s.completion_timings()[before.min(s.completion_timings().len())..]
                            .iter()
                            .find(|t| !t.dropped && t.applied.is_some() && t.items > 0)
                            .cloned()
                            .filter(|_| visible)
                    })
                    .ok()
                    .flatten();
                if let Some(t) = found {
                    applied = Some(t);
                    break;
                }
            }
            let Some(t) = applied else {
                timeouts += 1;
                let _ = key(cx, "escape");
                let _ = key(cx, "backspace");
                executor.timer(Duration::from_millis(200)).await;
                continue;
            };
            let applied_at = t.applied.expect("checked");
            // The first frame rendered after the items were applied, and its present.
            let mut frame = None;
            while frame.is_none() && Instant::now() < deadline {
                executor.timer(Duration::from_micros(500)).await;
                let p = probe.borrow();
                frame = p
                    .renders
                    .iter()
                    .copied()
                    .zip(p.presents.iter().copied())
                    .find(|(r, _)| *r >= applied_at);
            }
            let presented = frame.map(|(_, p)| p);
            if i >= WARMUP
                && let (Some(p), Some(sent), Some(received)) = (presented, t.sent, t.received)
            {
                if std::env::var_os("ELUDITE_BENCH_SAMPLES").is_some() {
                    eprintln!(
                        "[bench] sent +{:.3} received +{:.3} applied +{:.3} render +{:.3} presented +{:.3} ms after the key",
                        ms(sent.saturating_duration_since(dot.0)),
                        ms(received.saturating_duration_since(dot.0)),
                        ms(applied_at.saturating_duration_since(dot.0)),
                        ms(frame.map_or(dot.0, |f| f.0).saturating_duration_since(dot.0)),
                        ms(p.saturating_duration_since(dot.0))
                    );
                }
                let (render, _) = frame.expect("presented implies a frame");
                host.push(ms(received - sent));
                // The shell's own work: before the request is written, applying the reply, and drawing the frame
                // that shows the list. The idle wait from applying to that frame's start (the display's refresh)
                // is reported separately.
                ui.push(
                    ms(sent.saturating_duration_since(dot.0))
                        + ms(applied_at.saturating_duration_since(received))
                        + ms(p.saturating_duration_since(render)),
                );
                frame_wait.push(ms(render.saturating_duration_since(applied_at)));
                total.push(ms(p.saturating_duration_since(dot.0)));
                dot_keys.push(dot);
            }
            // Filter with the list open.
            for k in ["d", "i"] {
                executor.timer(Duration::from_millis(25)).await;
                let t = key(cx, k);
                if i >= WARMUP {
                    filter_keys.push(t);
                }
            }
            executor.timer(Duration::from_millis(30)).await;
            let _ = key(cx, "escape");
            for _ in 0..3 {
                executor.timer(Duration::from_millis(8)).await;
                let _ = key(cx, "backspace");
            }
            executor.timer(Duration::from_millis(60)).await;
        }
        rss.push(json!({"cycle": count, "rss": rss_mib()}));
        executor.timer(Duration::from_millis(2000)).await;
        let settled = rss_mib();
        let _ = cx.update(|window, cx| {
            let p = probe.borrow();
            let frames: Vec<(Instant, Instant)> =
                p.renders.iter().copied().zip(p.presents.iter().copied()).collect();
            let frame_cost = |keys: &[(Instant, Instant)]| {
                let mut cost = Vec::new();
                for (t0, t1) in keys {
                    if let Some((r, pr)) = frames.iter().find(|(r, _)| r >= t1) {
                        cost.push(ms(*t1 - *t0) + ms(pr.saturating_duration_since(*r)));
                    }
                }
                cost
            };
            let out = json!({
                "bench": "complete_in_shell",
                "method": "per cycle: `.` after the target, wait for the language server's list to be drawn, two filter keys 25 ms apart, Escape, three Backspaces; host = request written to reply read (includes decoding the reply); ui = key to request written + reply read to items applied + render to end of present of the frame showing the list; wait_for_next_frame = items applied to that frame's render start (idle, the display refresh); trigger_to_visible = key to end of that present",
                "file": file.to_string_lossy(),
                "triggers": count,
                "timeouts": timeouts,
                "host_latency": summarize(&host),
                "ui_latency": summarize(&ui),
                "wait_for_next_frame": summarize(&frame_wait),
                "trigger_to_visible": summarize(&total),
                "keystroke_frame_cost_filtering": summarize(&frame_cost(&filter_keys)),
                "keystroke_frame_cost_trigger": summarize(&frame_cost(&dot_keys)),
                "rss_by_cycle": rss,
                "rss_settled": settled,
                "platform": platform(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}
