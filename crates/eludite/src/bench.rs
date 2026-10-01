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
                // The Agents window's prompt box and buttons, and the review views' (brief 0016's manual run).
                let agents = cx.update(|cx| {
                    let s = shell.read(cx);
                    let w = s.agents().window.read(cx);
                    w.painted
                        .borrow()
                        .iter()
                        .map(|(k, b)| (k.clone(), *b))
                        .collect::<Vec<_>>()
                });
                for (k, b) in agents {
                    map.insert(
                        k,
                        json!([
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height)
                        ]),
                    );
                }
                // The Error List's toolbar (brief 0014's filter run).
                let toolbar = cx.update(|cx| shell.read(cx).error_list().read(cx).painted_bounds());
                for (k, b) in toolbar {
                    map.insert(
                        k.into(),
                        json!([
                            f32::from(b.origin.x),
                            f32::from(b.origin.y),
                            f32::from(b.size.width),
                            f32::from(b.size.height)
                        ]),
                    );
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

/// Wait (polling every half millisecond, up to 10 s) until `done` holds.
async fn until(
    cx: &mut gpui::AsyncWindowContext,
    executor: &gpui::BackgroundExecutor,
    mut done: impl FnMut(&mut App) -> bool,
) -> bool {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if cx.update(|_, cx| done(cx)).unwrap_or(false) {
            return true;
        }
        executor.timer(Duration::from_micros(500)).await;
    }
    false
}

/// The first frame rendered at or after `t`, and its present (waits up to 1 s).
async fn frame_after(
    probe: &Rc<RefCell<RenderProbe>>,
    executor: &gpui::BackgroundExecutor,
    t: Instant,
) -> Option<(Instant, Instant)> {
    let deadline = Instant::now() + Duration::from_secs(1);
    loop {
        {
            let p = probe.borrow();
            if let Some(f) = p
                .renders
                .iter()
                .copied()
                .zip(p.presents.iter().copied())
                .find(|(r, _)| *r >= t)
            {
                return Some(f);
            }
        }
        if Instant::now() > deadline {
            return None;
        }
        executor.timer(Duration::from_micros(500)).await;
    }
}

/// `--bench-navigate N` (brief 0014), in the real app against the real host:
///
/// 1. **Go To Definition in a file**, N times: F12 on the first occurrence of `ELUDITE_BENCH_DEFINITION` (default
///    `ISdkDiscoverer`) in the opened file, wait for the caret at the definition in the other file and the frame that
///    shows it, then Ctrl+- back. Host latency = request written to reply read; UI latency = key to request written
///    + reply read to caret placed + that frame's render to end of present; F12-to-caret = key to caret placed.
/// 2. **Find All References**, N/4 times (at least 10): Shift+F12 on the first occurrence of
///    `ELUDITE_BENCH_REFERENCES` (default `HostRpcTarget`); host as above, UI = key to request written + reply read
///    to rows in the window (line text read off the UI thread) + the frame's render to present; key-to-populated.
/// 3. **Keystroke frame cost with the window holding 1000 rows**: the window is filled with 1000 rows built from the
///    file's lines (a symbol with that many references does not exist in the solution), shown, and 300 keys are typed
///    into the file as `--bench-type` types them.
pub fn navigate(
    shell: Entity<Shell>,
    file: std::path::PathBuf,
    count: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    use eludite_commands::workspace;
    const WARMUP: usize = 3;
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    let definition =
        std::env::var("ELUDITE_BENCH_DEFINITION").unwrap_or_else(|_| "ISdkDiscoverer".into());
    let references =
        std::env::var("ELUDITE_BENCH_REFERENCES").unwrap_or_else(|_| "HostRpcTarget".into());
    let id = file.to_string_lossy().into_owned();
    cx.spawn_in(window, async move |_, cx| {
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
        let Ok(Some(editor)) = cx.update(|_, cx| shell.read(cx).editor(&file)) else {
            eprintln!("eludite bench: {} is not open", file.display());
            std::process::exit(1);
        };
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        // Put the caret inside the first whole-word occurrence of `word` in the opened file, active and focused.
        let place = |cx: &mut gpui::AsyncWindowContext, word: &str| {
            let word = word.to_owned();
            let editor = editor.clone();
            let id = id.clone();
            cx.update(|window, cx| {
                let _ = shell.update(cx, |s, cx| {
                    s.invoke(workspace::FILE_OPEN, json!({ "path": id }), window, cx)
                });
                editor.update(cx, |v, cx| {
                    v.update_editor(cx, |e| {
                        let text = e.text();
                        let at = text
                            .match_indices(word.as_str())
                            .map(|(i, _)| i)
                            .find(|&i| {
                                let before = text[..i].chars().next_back();
                                let after = text[i + word.len()..].chars().next();
                                let ident = |c: Option<char>| c.is_some_and(|c| c.is_alphanumeric() || c == '_');
                                !ident(before) && !ident(after)
                            });
                        match at {
                            Some(at) => e.set_caret(at + 1),
                            None => {
                                eprintln!("eludite bench: `{word}` is not in the file");
                                std::process::exit(1);
                            }
                        }
                    })
                });
                window.focus(&editor.focus_handle(cx), cx);
            })
            .ok();
        };
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

        // 1. Go To Definition.
        let (mut d_host, mut d_ui, mut d_caret, mut d_visible) = (vec![], vec![], vec![], vec![]);
        let mut d_timeouts = 0;
        let mut target_title = String::new();
        place(cx, &definition);
        for i in 0..(WARMUP + count) {
            executor.timer(Duration::from_millis(30)).await;
            let before = cx
                .update(|_, cx| shell.read(cx).navigation_timings().len())
                .unwrap_or_default();
            let f12 = key(cx, "f12");
            let shell2 = shell.clone();
            let done = until(cx, &executor, move |cx| {
                shell2.read(cx).navigation_timings().len() > before
            })
            .await;
            let timing = cx
                .update(|_, cx| shell.read(cx).navigation_timings().get(before).cloned())
                .ok()
                .flatten();
            let ok = done
                && timing.as_ref().is_some_and(|t| t.applied.is_some())
                && cx
                    .update(|_, cx| shell.read(cx).active_document().as_deref() != Some(id.as_str()))
                    .unwrap_or(false);
            if !ok {
                d_timeouts += 1;
                place(cx, &definition);
                continue;
            }
            let t = timing.expect("checked");
            let applied = t.applied.expect("checked");
            let frame = frame_after(&probe, &executor, applied).await;
            if i == 0 {
                target_title = cx
                    .update(|_, cx| shell.read(cx).active_document().unwrap_or_default())
                    .unwrap_or_default();
            }
            if i >= WARMUP
                && let (Some(sent), Some(received), Some((render, present))) = (t.sent, t.received, frame)
            {
                d_host.push(ms(received - sent));
                d_ui.push(
                    ms(sent.saturating_duration_since(f12.0))
                        + ms(applied.saturating_duration_since(received))
                        + ms(present.saturating_duration_since(render)),
                );
                d_caret.push(ms(applied.saturating_duration_since(f12.0)));
                d_visible.push(ms(present.saturating_duration_since(f12.0)));
            }
            // Back to the symbol (Ctrl+-), which also exercises the history.
            let _ = key(cx, "ctrl--");
            let shell2 = shell.clone();
            let id2 = id.clone();
            until(cx, &executor, move |cx| {
                shell2.read(cx).active_document().as_deref() == Some(id2.as_str())
            })
            .await;
            place(cx, &definition);
        }

        // 2. Find All References.
        let runs = (count / 4).max(10);
        let (mut r_host, mut r_ui, mut r_total, mut r_count) = (vec![], vec![], vec![], 0);
        let mut r_timeouts = 0;
        for i in 0..(WARMUP + runs) {
            place(cx, &references);
            executor.timer(Duration::from_millis(30)).await;
            let before = cx
                .update(|_, cx| shell.read(cx).references_timings().len())
                .unwrap_or_default();
            let key_at = key(cx, "shift-f12");
            let shell2 = shell.clone();
            if !until(cx, &executor, move |cx| {
                shell2.read(cx).references_timings().len() > before
            })
            .await
            {
                r_timeouts += 1;
                continue;
            }
            let t = cx
                .update(|_, cx| shell.read(cx).references_timings()[before].clone())
                .expect("window");
            let applied = t.applied.expect("set with the rows");
            let frame = frame_after(&probe, &executor, applied).await;
            r_count = t.count;
            if i >= WARMUP
                && let (Some(sent), Some(received), Some((render, present))) = (t.sent, t.received, frame)
            {
                r_host.push(ms(received - sent));
                r_ui.push(
                    ms(sent.saturating_duration_since(key_at.0))
                        + ms(applied.saturating_duration_since(received))
                        + ms(present.saturating_duration_since(render)),
                );
                r_total.push(ms(present.saturating_duration_since(key_at.0)));
            }
        }

        // 3. Typing with the window open and holding 1000 rows.
        place(cx, &references);
        let fill = cx
            .update(|_, cx| {
                let lines: Vec<String> = editor
                    .read(cx)
                    .editor()
                    .text()
                    .lines()
                    .map(str::to_owned)
                    .collect();
                let start = Instant::now();
                let refs = synthetic_references(&lines, &file, 1000);
                shell.update(cx, |s, cx| {
                    s.references_window()
                        .update(cx, |w, cx| {
                            w.start("Synthetic".into(), cx);
                            w.finish(refs, cx)
                        })
                });
                start
            })
            .expect("window");
        let fill_frame = frame_after(&probe, &executor, fill).await;
        executor.timer(Duration::from_millis(500)).await;
        // The end of a line in the middle of the file, as `--bench-type` does.
        let _ = cx.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.update_editor(cx, |e| {
                    let b = e.buffer();
                    let row = b.line_count() / 2;
                    let at = b.point_to_offset(eludite_editor::text::Point::new(row, b.line_len(row)));
                    e.set_caret(at);
                })
            });
            window.focus(&editor.focus_handle(cx), cx);
        });
        let mut typed: Vec<(Instant, Instant)> = Vec::new();
        for i in 0..300usize {
            let pause = if i % 25 == 24 { 250 } else { 0 };
            executor
                .timer(Duration::from_millis(pause + 15 + (i as u64 * 7) % 30))
                .await;
            let k = if i % 17 == 16 {
                "backspace".to_owned()
            } else {
                // Comment text, so completion stays closed and the measure is the window's effect alone.
                ((b'a' + (i % 26) as u8) as char).to_string()
            };
            if i == 0 {
                let _ = key(cx, "space");
                let _ = key(cx, "/");
                let _ = key(cx, "/");
            }
            typed.push(key(cx, &k));
        }
        executor.timer(Duration::from_millis(300)).await;
        let rows_open = cx
            .update(|_, cx| {
                let s = shell.read(cx);
                (s.references_window().read(cx).references().len(), s.active_document())
            })
            .ok();
        let _ = cx.update(|window, cx| {
            let p = probe.borrow();
            let frames: Vec<(Instant, Instant)> =
                p.renders.iter().copied().zip(p.presents.iter().copied()).collect();
            let mut cost = Vec::new();
            for (t0, t1) in &typed {
                if let Some((r, pr)) = frames.iter().find(|(r, _)| r >= t1) {
                    cost.push(ms(*t1 - *t0) + ms(pr.saturating_duration_since(*r)));
                }
            }
            let out = json!({
                "bench": "navigate_in_shell",
                "method": "definition: F12 on the symbol, wait for the caret at the definition in the other file and the frame showing it, Ctrl+- back; host = request written to reply read; ui = key to request written + reply read to caret placed + that frame's render to end of present; f12_to_caret = key to caret placed; f12_to_visible = key to end of that present. references: Shift+F12, same split, rows in the window (lines read off the UI thread). typing: 300 keys in a comment with the Find All References window showing 1000 rows; frame cost = key handler + the next frame's render to end of present",
                "file": file.to_string_lossy(),
                "definition": {
                    "symbol": definition,
                    "target": target_title,
                    "runs": count,
                    "timeouts": d_timeouts,
                    "host_latency": summarize(&d_host),
                    "ui_latency": summarize(&d_ui),
                    "f12_to_caret": summarize(&d_caret),
                    "f12_to_visible": summarize(&d_visible),
                },
                "references": {
                    "symbol": references,
                    "count": r_count,
                    "runs": runs,
                    "timeouts": r_timeouts,
                    "host_latency": summarize(&r_host),
                    "ui_latency": summarize(&r_ui),
                    "key_to_populated_visible": summarize(&r_total),
                },
                "typing_with_1000_rows": {
                    "rows": rows_open.as_ref().map(|r| r.0),
                    "fill_render_to_present_ms": fill_frame.map(|(r, p)| ms(p.saturating_duration_since(r))),
                    "keystrokes": typed.len(),
                    "keystroke_frame_cost": summarize(&cost),
                },
                "rss": rss_mib(),
                "platform": platform(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}

/// `n` Find All References rows built from `lines` of `file`, spread over 25 files in 5 projects.
fn synthetic_references(
    lines: &[String],
    file: &std::path::Path,
    n: usize,
) -> Vec<crate::shell::references::Reference> {
    use crate::shell::references::{Reference, sort_references};
    let dir = file.parent().unwrap_or(file);
    let mut refs: Vec<Reference> = (0..n)
        .map(|i| {
            let line = lines
                .get(i % lines.len().max(1))
                .map(|l| l.trim().to_owned())
                .unwrap_or_default();
            let end = line.char_indices().nth(6).map_or(line.len(), |(b, _)| b);
            Reference {
                project: Some(format!("Project{}", i % 5)),
                path: dir.join(format!("File{}.cs", i % 25)),
                line: (i % lines.len().max(1)) as u32 + 1,
                column: 1,
                text: line,
                highlight: 0..end,
            }
        })
        .collect();
    sort_references(&mut refs);
    refs
}

/// `--bench-refactor N` (brief 0015), in the real app against the real host:
///
/// 1. **The light bulb**, N times: the caret moves alternately to the first `ELUDITE_BENCH_BULB_A` (default
///    `DotnetCliSdkDiscoverer(`, a constructor Roslyn offers "Use primary constructor" for) and the first
///    `ELUDITE_BENCH_BULB_B` (default `DiscoverAsync`) in the opened file; wait for the bulb's answer and the frame that
///    shows it. Caret-stop-to-visible = caret moved to the end of that frame's present (it includes the
///    [`crate::shell::code_actions::LIGHTBULB_DEBOUNCE`]); host = request written to reply read; UI = (caret moved to
///    request written, less the debounce) + reply read to bulb set + that frame's render to end of present.
/// 2. **Typing with the light bulb active**: 300 keys in a comment at the end of a line in the middle of the file,
///    as `--bench-type` types them (each key moves the caret, so the bulb's debounce restarts; its requests run in the
///    pauses); keystroke frame cost = key handler + the next frame's render to end of present.
/// 3. **Rename preview**, N times: the Rename dialog on the first `ELUDITE_BENCH_RENAME` (default `_dotnetPath`),
///    one letter typed per run; name change to preview shown = key to the end of the present of the frame showing the
///    preview (includes [`crate::shell::rename::RENAME_PREVIEW_DEBOUNCE`]); host = `textDocument/rename` written to
///    reply read; UI = (key to request written, less the debounce) + reply read to preview handed to the dialog (the
///    changed lines are computed off the UI thread) + that frame's render to present.
/// 4. **Apply to 10 closed files**, 20 times: `eludite.workspace.apply_edit` with three empty inserts in each of the
///    first 10 `.cs` files of the solution's folder that are not open (the files are rewritten with the same bytes):
///    command invoked to the applier's summary (read, edit and atomic write off the UI thread).
pub fn refactor(
    shell: Entity<Shell>,
    file: std::path::PathBuf,
    count: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    use crate::shell::code_actions::LIGHTBULB_DEBOUNCE;
    use crate::shell::rename::RENAME_PREVIEW_DEBOUNCE;
    use eludite_commands::workspace;
    const WARMUP: usize = 3;
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    let env = |k: &str, d: &str| std::env::var(k).unwrap_or_else(|_| d.into());
    let bulb_a = env("ELUDITE_BENCH_BULB_A", "DotnetCliSdkDiscoverer(");
    let bulb_b = env("ELUDITE_BENCH_BULB_B", "DiscoverAsync");
    let rename_symbol = env("ELUDITE_BENCH_RENAME", "_dotnetPath");
    let id = file.to_string_lossy().into_owned();
    cx.spawn_in(window, async move |_, cx| {
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
        executor.timer(Duration::from_millis(3000)).await;
        let Ok(Some(editor)) = cx.update(|_, cx| shell.read(cx).editor(&file)) else {
            eprintln!("eludite bench: {} is not open", file.display());
            std::process::exit(1);
        };
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        let offset_of = |cx: &mut gpui::AsyncWindowContext, needle: &str| -> usize {
            let needle = needle.to_owned();
            cx.update(|_, cx| editor.read(cx).editor().text().find(&needle))
                .ok()
                .flatten()
                .unwrap_or_else(|| {
                    eprintln!("eludite bench: `{needle}` is not in the file");
                    std::process::exit(1);
                })
        };
        let set_caret = |cx: &mut gpui::AsyncWindowContext, at: usize| -> Instant {
            let mut t = Instant::now();
            let _ = cx.update(|window, cx| {
                window.focus(&editor.focus_handle(cx), cx);
                t = Instant::now();
                editor.update(cx, |v, cx| v.update_editor(cx, |e| e.set_caret(at)));
            });
            t
        };
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
        let debounce = ms(LIGHTBULB_DEBOUNCE);

        // 1. The light bulb.
        let a = offset_of(cx, &bulb_a);
        let b = offset_of(cx, &bulb_b);
        let (mut l_host, mut l_ui, mut l_total, mut l_actions) = (vec![], vec![], vec![], vec![]);
        let mut l_probe = vec![];
        let mut r_files = 0usize;
        let mut l_timeouts = 0;
        for i in 0..(WARMUP + count) {
            executor.timer(Duration::from_millis(200)).await;
            let before = cx
                .update(|_, cx| shell.read(cx).lightbulb_timings().len())
                .unwrap_or_default();
            let moved = set_caret(cx, if i % 2 == 0 { a } else { b });
            let shell2 = shell.clone();
            let shown = until(cx, &executor, move |cx| {
                shell2.read(cx).lightbulb_timings()[before..]
                    .iter()
                    .any(|t| t.shown.is_some())
            })
            .await;
            let timing = cx
                .update(|_, cx| {
                    shell.read(cx).lightbulb_timings()[before..]
                        .iter()
                        .find(|t| t.shown.is_some())
                        .cloned()
                })
                .ok()
                .flatten();
            let Some(t) = timing.filter(|_| shown) else {
                l_timeouts += 1;
                continue;
            };
            let applied = t.shown.expect("found");
            let frame = frame_after(&probe, &executor, applied).await;
            if i >= WARMUP
                && let (Some(sent), Some(received), Some((render, present))) = (t.sent, t.received, frame)
            {
                l_host.push(ms(received - sent));
                l_ui.push(
                    (ms(sent.saturating_duration_since(moved)) - debounce).max(0.)
                        + ms(applied.saturating_duration_since(received))
                        + ms(present.saturating_duration_since(render)),
                );
                l_total.push(ms(present.saturating_duration_since(moved)));
                l_actions.push(t.actions as f64);
                if let Some(m) = t.moved {
                    l_probe.push(ms(m.saturating_duration_since(moved)));
                }
            }
        }
        let bulb_kind = cx
            .update(|_, cx| editor.read(cx).lightbulb().map(|(_, k)| format!("{k:?}")))
            .ok()
            .flatten();

        // 2. Typing with the light bulb active.
        let _ = cx.update(|window, cx| {
            editor.update(cx, |v, cx| {
                v.update_editor(cx, |e| {
                    let b = e.buffer();
                    let row = b.line_count() / 2;
                    let at = b.point_to_offset(eludite_editor::text::Point::new(row, b.line_len(row)));
                    e.set_caret(at);
                })
            });
            window.focus(&editor.focus_handle(cx), cx);
        });
        let bulbs_before = cx
            .update(|_, cx| shell.read(cx).lightbulb_timings().len())
            .unwrap_or_default();
        let mut typed: Vec<(Instant, Instant)> = Vec::new();
        for i in 0..300usize {
            let pause = if i % 25 == 24 { 250 } else { 0 };
            executor
                .timer(Duration::from_millis(pause + 15 + (i as u64 * 7) % 30))
                .await;
            let k = if i % 17 == 16 {
                "backspace".to_owned()
            } else {
                ((b'a' + (i % 26) as u8) as char).to_string()
            };
            if i == 0 {
                let _ = key(cx, "space");
                let _ = key(cx, "/");
                let _ = key(cx, "/");
            }
            typed.push(key(cx, &k));
        }
        executor.timer(Duration::from_millis(500)).await;
        let bulb_requests_while_typing = cx
            .update(|_, cx| shell.read(cx).lightbulb_timings().len() - bulbs_before)
            .unwrap_or_default();
        let cost: Vec<f64> = {
            let p = probe.borrow();
            let frames: Vec<(Instant, Instant)> =
                p.renders.iter().copied().zip(p.presents.iter().copied()).collect();
            typed
                .iter()
                .filter_map(|(t0, t1)| {
                    frames
                        .iter()
                        .find(|(r, _)| r >= t1)
                        .map(|(r, pr)| ms(*t1 - *t0) + ms(pr.saturating_duration_since(*r)))
                })
                .collect()
        };
        // Undo the typing (one step per burst) so the rename sees the file as it was.
        for _ in 0..40 {
            let _ = cx.update(|_, cx| editor.update(cx, |v, cx| v.update_editor(cx, |e| e.undo())));
        }
        executor.timer(Duration::from_millis(1000)).await;

        // 3. Rename preview through the dialog.
        let at = offset_of(cx, &rename_symbol);
        set_caret(cx, at + 1);
        let _ = cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.run(workspace::EDITOR_RENAME, json!({ "path": id }), window, cx)
            })
        });
        let shell2 = shell.clone();
        let opened = until(cx, &executor, move |cx| shell2.read(cx).rename_dialog().is_some()).await;
        let (mut r_host, mut r_ui, mut r_total, mut r_edits) = (vec![], vec![], vec![], 0usize);
        let mut r_timeouts = 0;
        let rename_debounce = ms(RENAME_PREVIEW_DEBOUNCE);
        if opened {
            for i in 0..(WARMUP + count) {
                executor.timer(Duration::from_millis(100)).await;
                let before = cx
                    .update(|_, cx| shell.read(cx).rename_timings().len())
                    .unwrap_or_default();
                let letter = ((b'a' + (i % 26) as u8) as char).to_string();
                let (k0, _) = key(cx, &letter);
                let shell2 = shell.clone();
                if !until(cx, &executor, move |cx| shell2.read(cx).rename_timings().len() > before).await {
                    r_timeouts += 1;
                    continue;
                }
                let t = cx
                    .update(|_, cx| shell.read(cx).rename_timings()[before].clone())
                    .expect("window");
                let shown = t.shown.expect("set with the preview");
                let frame = frame_after(&probe, &executor, shown).await;
                r_edits = t.edits;
                r_files = t.files;
                if i >= WARMUP
                    && let (Some(sent), Some(received), Some((render, present))) = (t.sent, t.received, frame)
                {
                    r_host.push(ms(received - sent));
                    r_ui.push(
                        (ms(sent.saturating_duration_since(k0)) - rename_debounce).max(0.)
                            + ms(shown.saturating_duration_since(received))
                            + ms(present.saturating_duration_since(render)),
                    );
                    r_total.push(ms(present.saturating_duration_since(k0)));
                }
            }
            let _ = key(cx, "escape");
        }

        // 4. Apply a no-op edit to 10 closed files.
        let root = cx
            .update(|_, cx| shell.read(cx).solution_dir())
            .ok()
            .flatten()
            .unwrap_or_else(|| file.parent().unwrap_or(&file).to_path_buf());
        let mut files: Vec<std::path::PathBuf> = Vec::new();
        let mut stack = vec![root.join("src")];
        while let Some(dir) = stack.pop() {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            let mut entries: Vec<_> = entries.flatten().map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
                if p.is_dir() && name != "bin" && name != "obj" {
                    stack.push(p);
                } else if name.ends_with(".cs") && p != file {
                    files.push(p);
                }
            }
        }
        files.sort();
        files.truncate(10);
        let mut changes = serde_json::Map::new();
        for f in &files {
            let insert = json!({"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": ""});
            changes.insert(
                crate::shell::documents::path_to_uri(f),
                json!([insert.clone(), insert.clone(), insert]),
            );
        }
        let edit = json!({ "changes": changes });
        let mut apply = vec![];
        let mut a_failed = 0;
        for _ in 0..20 {
            executor.timer(Duration::from_millis(200)).await;
            let t0 = Instant::now();
            let out = cx
                .update(|window, cx| {
                    shell.update(cx, |s, cx| {
                        s.invoke(
                            workspace::WORKSPACE_APPLY_EDIT,
                            json!({"edit": edit.clone(), "label": "bench"}),
                            window,
                            cx,
                        )
                    })
                })
                .ok()
                .and_then(Result::ok);
            if out.is_none() {
                a_failed += 1;
                continue;
            }
            let shell2 = shell.clone();
            let done = until(cx, &executor, move |cx| {
                shell2.read(cx).apply_edit_output().state != workspace::ApplyEditState::Applying
            })
            .await;
            let applied = cx
                .update(|_, cx| shell.read(cx).apply_edit_output())
                .ok();
            match applied {
                Some(o) if done && o.applied => apply.push(ms(t0.elapsed())),
                _ => a_failed += 1,
            }
        }
        let _ = cx.update(|window, cx| {
            let out = json!({
                "bench": "refactor_in_shell",
                "method": "light bulb: the caret moves between two positions with code actions; caret_stop_to_visible = caret moved to the end of the present of the frame showing the bulb (includes the 50 ms debounce); host = request written to reply read; ui = caret moved to request written less the debounce + reply read to bulb set + render to present. typing: 300 keys in a comment with the bulb active; frame cost = key handler + next frame's render to present. rename preview: one letter typed per run in the Rename dialog; name_change_to_visible includes the 150 ms debounce; host = textDocument/rename written to reply read; ui = key to request written less the debounce + reply read to preview handed to the dialog + render to present. apply: eludite.workspace.apply_edit of three empty inserts into each of 10 closed files (rewritten atomically with the same bytes); invoke to summary",
                "file": file.to_string_lossy(),
                "lightbulb": {
                    "positions": [bulb_a, bulb_b],
                    "runs": count,
                    "timeouts": l_timeouts,
                    "debounce_ms": debounce,
                    "kind_at_end": bulb_kind,
                    "actions": summarize(&l_actions),
                    "caret_moved_to_probe": summarize(&l_probe),
                    "host_latency": summarize(&l_host),
                    "ui_latency": summarize(&l_ui),
                    "caret_stop_to_visible": summarize(&l_total),
                },
                "typing_with_lightbulb": {
                    "keystrokes": typed.len(),
                    "lightbulb_requests": bulb_requests_while_typing,
                    "keystroke_frame_cost": summarize(&cost),
                },
                "rename_preview": {
                    "symbol": rename_symbol,
                    "dialog_opened": opened,
                    "edits": r_edits,
                    "files": r_files,
                    "runs": count,
                    "timeouts": r_timeouts,
                    "debounce_ms": rename_debounce,
                    "host_latency": summarize(&r_host),
                    "ui_latency": summarize(&r_ui),
                    "name_change_to_visible": summarize(&r_total),
                },
                "apply_10_closed_files": {
                    "files": files.iter().map(|f| f.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                    "failed": a_failed,
                    "invoke_to_summary": summarize(&apply),
                },
                "rss": rss_mib(),
                "platform": platform(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}

/// `--bench-agent-ready N`: from the first presented frame, start the selected agent N times (each a new session) and
/// report spawn, `initialize` and `session/new` times and window-open-to-ready. No prompt is sent (no model call).
pub fn agent_ready(shell: &Entity<Shell>, runs: usize, t_main: Instant, cx: &mut App) {
    let shell2 = shell.clone();
    shell.update(cx, |s, _| {
        s.after_first_present(move |_, cx| {
            let first_present = t_main.elapsed();
            let shell = shell2.clone();
            cx.spawn(async move |cx| {
                // The registry is searched off the UI thread at startup.
                loop {
                    let ready = cx.update(|cx| !shell.read(cx).agents().registry.is_empty());
                    if ready {
                        break;
                    }
                    cx.background_executor()
                        .timer(Duration::from_millis(5))
                        .await;
                }
                let agent = cx.update(|cx| {
                    shell
                        .read(cx)
                        .agents()
                        .selected_agent()
                        .map(|a| a.command_line())
                        .unwrap_or_default()
                });
                let mut results = Vec::new();
                for _ in 0..runs {
                    let started = Instant::now();
                    let g = cx.update(|cx| {
                        shell.update(cx, |s, cx| {
                            let _ = s.agents_start(None, true, cx);
                            s.agents().generation
                        })
                    });
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(1))
                            .await;
                        let done = cx.update(|cx| {
                            let a = shell.read(cx).agents();
                            a.generation == g
                                && (a.ready_ms.is_some()
                                    || matches!(
                                        a.state,
                                        crate::shell::agents::window::StateKind::Error
                                            | crate::shell::agents::window::StateKind::NeedsLogin
                                    ))
                        });
                        if done || started.elapsed() > Duration::from_secs(120) {
                            break;
                        }
                    }
                    let run = cx.update(|cx| {
                        let a = shell.read(cx).agents();
                        let mut m = serde_json::Map::new();
                        for (name, ms) in &a.timings {
                            m.insert(format!("{name}_ms"), json!(ms));
                        }
                        m.insert("ready_ms".into(), json!(a.ready_ms));
                        m.insert("state".into(), json!(a.state.as_str()));
                        Value::Object(m)
                    });
                    results.push(run);
                }
                let ready: Vec<f64> = results
                    .iter()
                    .filter_map(|r| r["ready_ms"].as_f64())
                    .collect();
                let out = json!({
                    "bench": "agent_ready",
                    "agent": agent,
                    "main_to_first_present_ms": ms(first_present),
                    "runs": results,
                    "ready": summarize(&ready),
                    "rss": rss_mib(),
                    "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
                });
                println!("{out}");
                cx.update(|cx| shell.update(cx, |s, cx| s.agents_stop(cx)));
                cx.background_executor()
                    .timer(Duration::from_millis(300))
                    .await;
                std::process::exit(0);
            })
            .detach();
        });
    });
}

/// `--bench-agent-stream PATH`: the fake agent at PATH (a real child process) streams 2000 message chunks at 200 per
/// second into the Agents window; report the UI thread's frame work while it streams (the window's render to the end
/// of the frame), the cost of applying each batch of events, and the batch sizes.
pub fn agent_stream(shell: &Entity<Shell>, cx: &mut App) {
    let shell2 = shell.clone();
    shell.update(cx, |s, _| {
        s.after_first_present(move |window, cx| {
            let shell = shell2.clone();
            let platform = platform(window);
            shell.update(cx, |s, cx| {
                let _ = s.commands_invoke_view_show(eludite_docking::ids::AGENTS);
                let _ = s.agents_start(None, true, cx);
            });
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor().timer(Duration::from_millis(5)).await;
                    if cx.update(|cx| shell.read(cx).agents().ready_ms.is_some()) {
                        break;
                    }
                }
                cx.update(|cx| {
                    shell.update(cx, |s, cx| {
                        s.agents_probe(true, cx);
                        let _ = s.agents_prompt("stream", cx);
                    })
                });
                let started = Instant::now();
                loop {
                    cx.background_executor().timer(Duration::from_millis(20)).await;
                    let done = cx.update(|cx| {
                        let a = shell.read(cx).agents();
                        a.last_stop.is_some()
                    });
                    if done || started.elapsed() > Duration::from_secs(60) {
                        break;
                    }
                }
                cx.background_executor().timer(Duration::from_millis(300)).await;
                let out = cx.update(|cx| {
                    shell.update(cx, |s, cx| {
                        s.agents_probe(false, cx);
                        let (frames, apply, batches, chunks) = s.agents_probe_results(cx);
                        json!({
                            "bench": "agent_stream",
                            "stream_s": ms(started.elapsed()) / 1e3,
                            "frames": frames.len(),
                            "frame_work": summarize(&frames),
                            "apply_per_batch": summarize(&apply),
                            "batches": batches.len(),
                            "events_per_batch": batches.iter().sum::<usize>() as f64 / batches.len().max(1) as f64,
                            "chunk_to_apply": summarize(&chunks),
                            "rss": rss_mib(),
                            "platform": platform,
                            "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
                        })
                    })
                });
                println!("{out}");
                cx.update(|cx| shell.update(cx, |s, cx| s.agents_stop(cx)));
                cx.background_executor().timer(Duration::from_millis(300)).await;
                std::process::exit(0);
            })
            .detach();
        });
    });
}

/// `--bench-diff N`: N times, hold a 20-edit change to a 2000-line file as an agent's pending change and open its
/// review view; report the time from the edit to the first presented frame showing the diff (the diff is computed
/// off the UI thread, then drawn virtualized).
pub fn diff(shell: &Entity<Shell>, runs: usize, cx: &mut App) {
    use gpui::AppContext as _;
    let shell2 = shell.clone();
    shell.update(cx, |s, _| {
        s.after_first_present(move |window, cx| {
            let shell = shell2.clone();
            let handle = window.window_handle();
            let dir =
                std::env::temp_dir().join(format!("eludite-bench-diff-{}", std::process::id()));
            let _ = std::fs::create_dir_all(&dir);
            let file = dir.join("Big.cs");
            let text: String = (0..2000)
                .map(|i| format!("    int field{i} = {i}; // line {i}\n"))
                .collect();
            let _ = std::fs::write(&file, text);
            cx.spawn(async move |cx| {
                let mut total = Vec::new();
                for _ in 0..runs {
                    let t0 = Instant::now();
                    let id = cx.update_window(handle, |_, window, cx| {
                        shell.update(cx, |s, cx| s.bench_capture_big_edit(&file, window, cx))
                    });
                    let Ok(Some(id)) = id else { break };
                    let tab = crate::shell::agents::review::review_tab(id);
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(1))
                            .await;
                        let shown = cx.update(|cx| {
                            let s = shell.read(cx);
                            let reviews = s.agents().reviews.borrow();
                            let v = reviews.get(&tab)?.read(cx);
                            Some(ms(v.opened.duration_since(t0)) + v.first_diff_frame_ms?)
                        });
                        if let Some(ms) = shown {
                            total.push(ms);
                            break;
                        }
                        if t0.elapsed() > Duration::from_secs(10) {
                            break;
                        }
                    }
                    let _ = cx.update_window(handle, |_, window, cx| {
                        shell.update(cx, |s, cx| s.bench_reject_all(window, cx))
                    });
                    cx.background_executor()
                        .timer(Duration::from_millis(50))
                        .await;
                }
                let out = json!({
                    "bench": "diff_review",
                    "lines": 2000,
                    "edits": 20,
                    "edit_to_diff_frame": summarize(&total),
                    "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
                });
                println!("{out}");
                let _ = std::fs::remove_dir_all(&dir);
                std::process::exit(0);
            })
            .detach();
        });
    });
}

/// `--bench-output SECS` (brief 0017): stream 10,000 lines a second into the Output window for SECS seconds, as a
/// build's output arrives (160-line chunks every 16 ms: the host's 16 ms flush with ~100-byte lines), through the
/// shell's build-output handler, with the window following. Reports each frame's cost while streaming (render to end
/// of present, the brief 0009 method without a key), the append cost per chunk, the final line count and RSS.
pub fn output_stream(shell: &Entity<Shell>, secs: u64, cx: &mut App) {
    let shell2 = shell.clone();
    shell.update(cx, |s, _| {
        s.after_first_present(move |window, cx| {
            let shell = shell2.clone();
            let platform = platform(window);
            let probe = Rc::new(RefCell::new(RenderProbe::default()));
            shell.update(cx, |s, cx| {
                s.bench_stream_begin(cx);
                s.set_probe(Some(probe.clone()), cx);
            });
            cx.spawn(async move |cx| {
                let executor = cx.background_executor().clone();
                executor.timer(Duration::from_millis(500)).await;
                probe.borrow_mut().renders.clear();
                probe.borrow_mut().presents.clear();
                let chunk_lines = 160usize;
                let interval = Duration::from_millis(16);
                let chunks = (secs * 1000 / 16) as usize;
                let mut apply = Vec::with_capacity(chunks);
                let started = Instant::now();
                let mut n = 0usize;
                for c in 0..chunks {
                    let due = started + interval * c as u32;
                    if let Some(wait) = due.checked_duration_since(Instant::now()) {
                        executor.timer(wait).await;
                    }
                    let text: String = (0..chunk_lines)
                        .map(|i| {
                            let k = n + i;
                            format!(
                                "  Restored /src/Project{:03}/Project{:03}.csproj (in {} ms). CSC : warning CS{:04}: line {k} of the bench\n",
                                k % 1000,
                                k % 1000,
                                k % 97,
                                k % 9999
                            )
                        })
                        .collect();
                    n += chunk_lines;
                    cx.update(|cx| {
                        shell.update(cx, |s, cx| {
                            let t0 = Instant::now();
                            s.bench_stream_chunk(&text, cx);
                            apply.push(ms(t0.elapsed()));
                        })
                    });
                }
                let streamed = started.elapsed();
                executor.timer(Duration::from_millis(300)).await;
                let out = cx.update(|cx| {
                    let p = probe.borrow();
                    let frames: Vec<f64> = p
                        .renders
                        .iter()
                        .copied()
                        .zip(p.presents.iter().copied())
                        .map(|(r, pr)| ms(pr.saturating_duration_since(r)))
                        .collect();
                    let lines = shell.read(cx).output_lines(cx);
                    json!({
                        "bench": "output_stream",
                        "method": "160-line chunks every 16 ms through the shell's build-output handler, the Output window following; frame cost = render to end of present for every frame while streaming",
                        "seconds": ms(streamed) / 1e3,
                        "lines": lines,
                        "chunks": apply.len(),
                        "frames": frames.len(),
                        "frame_cost": summarize(&frames),
                        "append_per_chunk": summarize(&apply),
                        "rss": rss_mib(),
                        "platform": platform,
                        "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
                    })
                });
                println!("{out}");
                cx.update(|cx| cx.quit());
            })
            .detach();
        });
    });
}

/// `--bench-build N` (brief 0017), in the real app against the real host: once the solution has loaded, N times
/// press Ctrl+Shift+B (`Window::dispatch_keystroke`) and wait for the build to finish. Reports key to the first Output
/// line applied and presented (PLAN.md 9: under 100 ms), the host's finished notification (as the pump received it)
/// to the Error List rows set and presented (brief 0017: under 200 ms), the build times and the frame cost while the
/// output streamed.
pub fn build_keys(
    shell: Entity<Shell>,
    count: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    cx.spawn_in(window, async move |_, cx| {
        loop {
            executor.timer(Duration::from_millis(50)).await;
            let Ok(ready) = cx.update(|_, cx| shell.read(cx).timings().loaded.is_some()) else {
                return;
            };
            if ready {
                break;
            }
        }
        executor.timer(Duration::from_millis(1500)).await;
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        let mut first_line = Vec::new();
        let mut first_line_shown = Vec::new();
        let mut rows = Vec::new();
        let mut rows_shown = Vec::new();
        let mut builds = Vec::new();
        let mut frames = Vec::new();
        let mut results = Vec::new();
        for _ in 0..count {
            {
                let mut p = probe.borrow_mut();
                p.renders.clear();
                p.presents.clear();
            }
            let ks = Keystroke::parse("ctrl-shift-b").expect("keystroke");
            let t0 = Instant::now();
            let _ = cx.update(|window, cx| window.dispatch_keystroke(ks, cx));
            let deadline = Instant::now() + Duration::from_secs(600);
            loop {
                executor.timer(Duration::from_millis(5)).await;
                let Ok((building, t)) = cx.update(|_, cx| {
                    let s = shell.read(cx);
                    (s.builds().is_building(), s.builds().timings.clone())
                }) else {
                    return;
                };
                if (!building && t.rows_set.is_some()) || Instant::now() > deadline {
                    break;
                }
            }
            executor.timer(Duration::from_millis(200)).await;
            let Ok((t, result)) = cx.update(|_, cx| {
                let s = shell.read(cx);
                (
                    s.builds().timings.clone(),
                    s.builds().last.as_ref().map(|f| format!("{:?}", f.result)),
                )
            }) else {
                return;
            };
            results.push(result);
            let pairs: Vec<(Instant, Instant)> = {
                let p = probe.borrow();
                p.renders.iter().copied().zip(p.presents.iter().copied()).collect()
            };
            let shown = |at: Instant| pairs.iter().find(|(r, _)| *r >= at).map(|(_, pr)| *pr);
            if let Some(first) = t.first_output {
                first_line.push(ms(first.saturating_duration_since(t0)));
                if let Some(pr) = shown(first) {
                    first_line_shown.push(ms(pr.saturating_duration_since(t0)));
                }
            }
            if let (Some(received), Some(set)) = (t.finished_received, t.rows_set) {
                rows.push(ms(set.saturating_duration_since(received)));
                if let Some(pr) = shown(set) {
                    rows_shown.push(ms(pr.saturating_duration_since(received)));
                }
                builds.push(ms(received.saturating_duration_since(t0)));
            }
            frames.extend(pairs.iter().map(|(r, pr)| ms(pr.saturating_duration_since(*r))));
            executor.timer(Duration::from_millis(1000)).await;
        }
        let _ = cx.update(|window, cx| {
            let out = json!({
                "bench": "build_keys",
                "method": "Window::dispatch_keystroke(ctrl-shift-b); first line = key to the host's first eludite/build/output chunk appended (and to the end of the present of the frame that shows it); rows = the pump receiving eludite/build/finished to the Error List rows set (and to the present showing them)",
                "builds": count,
                "results": results,
                "key_to_first_output_line": summarize(&first_line),
                "key_to_first_output_line_presented": summarize(&first_line_shown),
                "finished_to_error_list_rows": summarize(&rows),
                "finished_to_error_list_rows_presented": summarize(&rows_shown),
                "key_to_finished": summarize(&builds),
                "frame_cost_while_building": summarize(&frames),
                "rss": rss_mib(),
                "platform": platform(window),
                "loadavg": std::fs::read_to_string("/proc/loadavg").unwrap_or_default().trim(),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}

/// Write an `eludite/ping` JSON-RPC request into process `pid`'s stdin (Linux: through `/proc/<pid>/fd/0`, the pipe
/// netcoredbg holds), as a second terminal would.
fn write_ping(pid: i64, id: usize) -> std::io::Result<()> {
    #[cfg(target_os = "linux")]
    {
        use std::io::Write as _;
        let body = format!(r#"{{"jsonrpc":"2.0","id":{id},"method":"eludite/ping"}}"#);
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .open(format!("/proc/{pid}/fd/0"))?;
        write!(f, "Content-Length: {}\r\n\r\n{body}", body.len())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (pid, id);
        Err(std::io::Error::other(
            "writing to another process's stdin needs Linux",
        ))
    }
}

/// `--bench-debug N` (brief 0018), in the real app with the real netcoredbg and `eludite-host` as the debuggee:
///
/// 1. A breakpoint on the first line of the opened file containing `ELUDITE_BENCH_BREAK` (default
///    `var timestamp = _timeProvider`, Ping's first statement), through `eludite.debug.toggle_breakpoint`.
/// 2. N sessions. Each: F5; once the program runs and the breakpoint is bound, `ELUDITE_BENCH_PINGS` (default 20)
///    times write an `eludite/ping` into the debuggee's stdin, wait for the break (locals loaded), F10 twice, F5.
///    Then Shift+F5. F5 to the first break covers the launch, the adapter's handshake, binding the breakpoint, the
///    ping and the stop; the first session of the process is reported apart from the others.
/// 3. Step round trip: F10 to the break shown (locals loaded and given to the windows), and to the end of the
///    present of the first frame rendered after it. Frame cost while stepping: render to end of present of every
///    frame drawn from an F10 to its break shown.
/// 4. The Locals window drawing 200 variables: rows given to the window to the end of the present of the frame that
///    draws them, 20 times.
pub fn debug(
    shell: Entity<Shell>,
    file: std::path::PathBuf,
    sessions: usize,
    window: &mut Window,
    cx: &mut gpui::Context<Shell>,
) {
    use crate::shell::debug::state::{FlatRow, Mode};
    use eludite_commands::debug as cmds;
    let probe = Rc::new(RefCell::new(RenderProbe::default()));
    let executor = cx.background_executor().clone();
    let needle = std::env::var("ELUDITE_BENCH_BREAK")
        .unwrap_or_else(|_| "var timestamp = _timeProvider".into());
    let pings: usize = std::env::var("ELUDITE_BENCH_PINGS")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let fail = |m: String| -> ! {
        eprintln!("eludite bench: {m}");
        std::process::exit(1)
    };
    cx.spawn_in(window, async move |_, cx| {
        loop {
            executor.timer(Duration::from_millis(20)).await;
            let Ok(ready) = cx.update(|_, cx| {
                let t = shell.read(cx).timings();
                t.loaded.is_some() && t.tree.is_some()
            }) else {
                return;
            };
            if ready {
                break;
            }
        }
        executor.timer(Duration::from_millis(2000)).await;
        let text = std::fs::read_to_string(&file).unwrap_or_else(|e| fail(format!("{}: {e}", file.display())));
        let line = text
            .lines()
            .position(|l| l.contains(needle.as_str()))
            .unwrap_or_else(|| fail(format!("`{needle}` is not in {}", file.display())))
            + 1;
        let set = cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.invoke(
                    cmds::TOGGLE_BREAKPOINT,
                    json!({"path": file.to_string_lossy(), "line": line}),
                    window,
                    cx,
                )
            })
        });
        if !matches!(set, Ok(Ok(_))) {
            fail(format!("cannot set the breakpoint: {set:?}"));
        }
        cx.update(|_, cx| shell.update(cx, |s, cx| s.set_probe(Some(probe.clone()), cx)))
            .ok();
        let key = |cx: &mut gpui::AsyncWindowContext, k: &str| -> Instant {
            let ks = Keystroke::parse(k).expect("keystroke");
            let mut t0 = Instant::now();
            let _ = cx.update(|window, cx| {
                t0 = Instant::now();
                window.dispatch_keystroke(ks, cx);
            });
            t0
        };
        let model = |cx: &mut gpui::AsyncWindowContext| {
            cx.update(|_, cx| {
                let d = shell.read(cx).debugger();
                let bound = d.model.breakpoints.all().iter().all(|b| b.verified);
                (
                    d.model.mode,
                    d.model.stop,
                    d.model.locals_loading,
                    bound,
                    d.model.session.as_ref().and_then(|s| s.process_id),
                    d.timings.clone(),
                )
            })
            .expect("window")
        };
        let mut first = Vec::new();
        let mut later = Vec::new();
        let (mut step_state, mut step_visible, mut windows) = (Vec::new(), Vec::new(), Vec::new());
        let mut timeouts = 0;
        let mut ping_id = 0;
        for session in 0..sessions {
            let f5 = key(cx, "f5");
            let shell2 = shell.clone();
            let ready = until(cx, &executor, move |cx| {
                let d = shell2.read(cx).debugger();
                d.model.mode == Mode::Running
                    && d.model.session.as_ref().and_then(|s| s.process_id).is_some()
                    && d.model.breakpoints.all().iter().all(|b| b.verified)
            })
            .await;
            if !ready {
                fail(format!("session {session} did not start: {:?}", model(cx).0));
            }
            let (_, _, _, _, pid, timings) = model(cx);
            let running = Instant::now();
            let pid = pid.expect("checked");
            for p in 0..pings {
                let (_, stop0, ..) = model(cx);
                ping_id += 1;
                let ping_at = Instant::now();
                if let Err(e) = write_ping(pid, ping_id) {
                    fail(format!("cannot write the ping to process {pid}: {e}"));
                }
                let shell2 = shell.clone();
                if !until(cx, &executor, move |cx| {
                    let m = &shell2.read(cx).debugger().model;
                    m.mode == Mode::Break && m.stop > stop0 && !m.locals_loading
                })
                .await
                {
                    timeouts += 1;
                    break;
                }
                let (.., t) = model(cx);
                let shown = t.locals_shown.expect("locals shown");
                if p == 0 {
                    let frame = frame_after(&probe, &executor, shown).await;
                    let row = json!({
                        "f5_to_break_shown_ms": ms(shown - f5),
                        "f5_to_break_visible_ms": frame.map(|(_, pr)| ms(pr.saturating_duration_since(f5))),
                        "f5_to_running_ms": ms(running.saturating_duration_since(f5)),
                        "ping_to_break_shown_ms": ms(shown.saturating_duration_since(ping_at)),
                        "start_to_first_break_ms": timings.start.zip(t.first_break).map(|(a, b)| ms(b - a)),
                    });
                    if session == 0 {
                        first.push(row);
                    } else {
                        later.push(row);
                    }
                }
                for _ in 0..2 {
                    let (_, stop0, ..) = model(cx);
                    let t0 = key(cx, "f10");
                    let shell2 = shell.clone();
                    if !until(cx, &executor, move |cx| {
                        let m = &shell2.read(cx).debugger().model;
                        m.mode == Mode::Break && m.stop > stop0 && !m.locals_loading
                    })
                    .await
                    {
                        timeouts += 1;
                        break;
                    }
                    let (.., t) = model(cx);
                    let shown = t.locals_shown.expect("locals shown");
                    let frame = frame_after(&probe, &executor, shown).await;
                    step_state.push(ms(shown.saturating_duration_since(t0)));
                    if let Some((_, present)) = frame {
                        step_visible.push(ms(present.saturating_duration_since(t0)));
                        windows.push((t0, present));
                    }
                }
                let _ = key(cx, "f5");
                let shell2 = shell.clone();
                until(cx, &executor, move |cx| {
                    shell2.read(cx).debugger().model.mode == Mode::Running
                })
                .await;
                executor.timer(Duration::from_millis(30)).await;
            }
            let _ = key(cx, "shift-f5");
            let shell2 = shell.clone();
            until(cx, &executor, move |cx| {
                shell2.read(cx).debugger().model.mode == Mode::Design
            })
            .await;
            executor.timer(Duration::from_millis(300)).await;
        }
        // Frame cost while stepping.
        let frame_cost: Vec<f64> = {
            let p = probe.borrow();
            p.renders
                .iter()
                .copied()
                .zip(p.presents.iter().copied())
                .filter(|(r, _)| windows.iter().any(|(a, b)| r >= a && r <= b))
                .map(|(r, pr)| ms(pr.saturating_duration_since(r)))
                .collect()
        };
        // The Locals window drawing 200 variables.
        let _ = cx.update(|window, cx| {
            shell.update(cx, |s, cx| {
                s.invoke("eludite.view.show", json!({"id": "locals"}), window, cx)
            })
        });
        executor.timer(Duration::from_millis(300)).await;
        let mut locals_render = Vec::new();
        let mut locals_visible = Vec::new();
        for i in 0..20usize {
            let n = 200 - (i % 2);
            let rows: Vec<FlatRow> = (0..n)
                .map(|k| FlatRow {
                    path: vec![k],
                    depth: 0,
                    name: format!("local{k}"),
                    value: format!("\"value {k} of run {i}\""),
                    type_name: "string".into(),
                    expanded: (k % 10 == 0).then_some(false),
                    error: false,
                })
                .collect();
            let t = cx
                .update(|_, cx| {
                    let w = shell.read(cx).debugger().windows.locals.clone();
                    let t = Instant::now();
                    w.update(cx, |w, cx| w.set_rows(rows, None, cx));
                    t
                })
                .expect("window");
            if let Some((r, pr)) = frame_after(&probe, &executor, t).await {
                locals_render.push(ms(pr.saturating_duration_since(r)));
                locals_visible.push(ms(pr.saturating_duration_since(t)));
            }
            executor.timer(Duration::from_millis(50)).await;
        }
        let _ = cx.update(|window, cx| {
            let out = json!({
                "bench": "debug_in_shell",
                "method": "F5 with a breakpoint on Ping's first statement in the real eludite-host under netcoredbg; once running and bound, eludite/ping written to the debuggee's stdin; break shown = locals loaded and given to the windows; visible = end of the present of the first frame rendered after that. Steps: F10 twice per break; step state = key to break shown, step visible = key to end of that present. Frame cost = render to end of present of each frame drawn between an F10 and its present. Locals: 200 rows given to the Locals window to the end of the present of the frame drawing them",
                "file": file.to_string_lossy(),
                "line": line,
                "sessions": sessions,
                "pings_per_session": pings,
                "timeouts": timeouts,
                "first_session": first,
                "later_sessions": later,
                "f5_to_break_shown_later": summarize(&later.iter().filter_map(|r| r["f5_to_break_shown_ms"].as_f64()).collect::<Vec<_>>()),
                "step_to_shown": summarize(&step_state),
                "step_to_visible": summarize(&step_visible),
                "frame_cost_while_stepping": summarize(&frame_cost),
                "locals_200_render_to_present": summarize(&locals_render),
                "locals_200_set_to_present": summarize(&locals_visible),
                "rss": rss_mib(),
                "platform": platform(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}
