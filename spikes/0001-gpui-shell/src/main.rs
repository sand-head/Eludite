//! `spike-gpui-shell`: brief 0001 prototype.
//!
//! Usage:
//!   spike-gpui-shell                      interactive prototype
//!   spike-gpui-shell --bench-scroll       scroll the 100k-line buffer top to bottom, print frame-time JSON
//!   spike-gpui-shell --bench-keys N       N synthetic keystrokes at line 50,000, print latency JSON
//!   spike-gpui-shell --bench-start        print startup timings JSON after the first frame is presented
//!   spike-gpui-shell --key-probe          print one JSON line per edit (for tools/inject_keys.py)
//! Options:
//!   --lines N                  buffer size (default 100000)
//!   --scroll-lines-per-frame N scroll step for --bench-scroll (default 40)
//!   --refresh-hz HZ            display refresh rate for dropped-frame accounting (default: estimate)
//!   --layout PATH              layout JSON (default: <config dir>/eludite-spike/layout.json)
//!   --reset-layout             ignore any saved layout
//!   --exit-after-ms N          quit after N ms (smoke runs)

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant, SystemTime};

use gpui::{
    App, AppContext as _, Bounds, Focusable, TitlebarOptions, WindowBounds, WindowOptions, px, size,
};
use serde_json::json;
use spike_gpui_shell::bench::{self, ScrollBench, ms, wall_ns};
use spike_gpui_shell::buffer::Buffer;
use spike_gpui_shell::layout::Layout;
use spike_gpui_shell::shell::Shell;
use spike_gpui_shell::text_view::TextView;

#[derive(Default)]
struct Args {
    bench_scroll: bool,
    bench_keys: Option<usize>,
    bench_start: bool,
    key_probe: bool,
    lines: usize,
    lines_per_frame: f32,
    refresh_hz: Option<f64>,
    layout: Option<PathBuf>,
    reset_layout: bool,
    exit_after_ms: Option<u64>,
}

fn parse_args() -> Args {
    let mut a = Args {
        lines: 100_000,
        lines_per_frame: 40.,
        ..Default::default()
    };
    let mut it = std::env::args().skip(1);
    while let Some(arg) = it.next() {
        let mut val = || it.next().unwrap_or_else(|| panic!("{arg} needs a value"));
        match arg.as_str() {
            "--bench-scroll" => a.bench_scroll = true,
            "--bench-keys" => a.bench_keys = Some(val().parse().expect("count")),
            "--bench-start" => a.bench_start = true,
            "--key-probe" => a.key_probe = true,
            "--lines" => a.lines = val().parse().expect("lines"),
            "--scroll-lines-per-frame" => a.lines_per_frame = val().parse().expect("lines"),
            "--refresh-hz" => a.refresh_hz = Some(val().parse().expect("hz")),
            "--layout" => a.layout = Some(val().into()),
            "--reset-layout" => a.reset_layout = true,
            "--exit-after-ms" => a.exit_after_ms = Some(val().parse().expect("ms")),
            "-h" | "--help" => {
                println!("see the doc comment at the top of src/main.rs");
                std::process::exit(0);
            }
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

fn default_layout_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("APPDATA").map(PathBuf::from))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("eludite-spike").join("layout.json"))
}

struct StderrLogger;
impl log::Log for StderrLogger {
    fn enabled(&self, m: &log::Metadata) -> bool {
        m.level() <= log::Level::Warn
            || (m.target().starts_with("gpui_wgpu") && m.level() <= log::Level::Info)
    }
    fn log(&self, r: &log::Record) {
        if self.enabled(r.metadata()) {
            eprintln!("[{} {}] {}", r.level(), r.target(), r.args());
        }
    }
    fn flush(&self) {}
}
static LOGGER: StderrLogger = StderrLogger;

fn main() {
    let t_main = Instant::now();
    let t_main_wall = SystemTime::now();
    let _ = log::set_logger(&LOGGER).map(|()| log::set_max_level(log::LevelFilter::Info));
    let args = parse_args();
    let benching =
        args.bench_scroll || args.bench_keys.is_some() || args.bench_start || args.key_probe;

    let buffer = Buffer::generate(args.lines);
    let t_buffer = t_main.elapsed();

    let layout_path = if benching {
        None
    } else {
        args.layout.clone().or_else(default_layout_path)
    };
    if let Some(dir) = layout_path.as_ref().and_then(|p| p.parent()) {
        let _ = std::fs::create_dir_all(dir);
    }
    let layout = match (&layout_path, args.reset_layout) {
        (Some(p), false) => Layout::load(p).unwrap_or_default(),
        _ => Layout::default_vs(),
    };

    gpui_platform::application().run(move |cx: &mut App| {
        let t_app = t_main.elapsed();
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();

        let bounds = Bounds::centered(None, size(px(1400.), px(900.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Eludite spike 0001".into()),
                ..Default::default()
            }),
            app_id: Some("eludite-spike".into()),
            // Benchmarks must not be throttled to 30 fps if the compositor
            // withholds focus; activity is still reported in the JSON.
            inactive_frame_interval: if benching { None } else { WindowOptions::default().inactive_frame_interval },
            ..Default::default()
        };
        let mut tv_out = None;
        let window = cx
            .open_window(options, |window, cx| {
                let tv = cx.new(|cx| TextView::new(buffer, cx));
                tv.focus_handle(cx).focus(window, cx);
                tv_out = Some(tv.clone());
                cx.new(|cx| Shell::new(layout, layout_path, tv, cx))
            })
            .expect("failed to open window");
        let tv = tv_out.expect("text view created");
        let t_open = t_main.elapsed();
        cx.activate(true);

        // Floating windows from a restored layout.
        if let Ok(shell) = window.entity(cx) {
            Shell::sync_floating_windows(&shell, cx);
        }

        if args.bench_start {
            let launch_ns: Option<u128> = std::env::var("SPIKE_LAUNCH_WALL_NS").ok().and_then(|v| v.parse().ok());
            tv.read(cx).probes.borrow_mut().after_present.push(Box::new(move || {
                let first_present = t_main.elapsed();
                let now_wall = wall_ns(SystemTime::now());
                let out = json!({
                    "bench": "start",
                    "main_to_buffer_ready_ms": ms(t_buffer),
                    "main_to_app_ready_ms": ms(t_app),
                    "main_to_window_opened_ms": ms(t_open),
                    "main_to_first_present_ms": ms(first_present),
                    "launch_to_main_ms": launch_ns.map(|l| (wall_ns(t_main_wall) as f64 - l as f64) / 1e6),
                    "launch_to_first_present_ms": launch_ns.map(|l| (now_wall as f64 - l as f64) / 1e6),
                    "rss": bench::rss_json(),
                });
                println!("{}", serde_json::to_string(&out).unwrap());
                std::process::exit(0);
            }));
        }

        if args.bench_scroll {
            let st = Rc::new(RefCell::new(ScrollBench::new(tv.clone(), args.lines_per_frame, args.refresh_hz)));
            let _ = window.update(cx, |_, window, _| {
                window.on_next_frame(move |window, cx| ScrollBench::tick(st, window, cx));
            });
        }

        if let Some(n) = args.bench_keys {
            let handle = window.into();
            let tv2 = tv.clone();
            // Start after the first frame is on screen.
            tv.read(cx).probes.borrow_mut().after_present.push(Box::new(move || {}));
            let line = (args.lines / 2).min(50_000);
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(500)).await;
                cx.update(|cx| bench::bench_keys(handle, tv2, n, line, cx));
            })
            .detach();
        }

        if args.key_probe {
            let line = (args.lines / 2).min(50_000);
            tv.update(cx, |tv, cx| {
                tv.place_cursor(line);
                tv.probes.borrow_mut().on_sample = Some(Box::new(|s| {
                    println!(
                        "{}",
                        json!({"handler_wall_ns": wall_ns(s.handler_wall), "present_wall_ns": wall_ns(s.present_wall), "to_present_ns": s.to_present_ns, "render_to_present_ns": s.render_to_present_ns})
                    );
                }));
                cx.notify();
            });
            // Report readiness and focus changes; quit on focus loss so an
            // injector never types into another application.
            let _ = window.update(cx, |_, window, cx| {
                let sub = cx.observe_window_activation(window, |_, window, _| {
                    if window.is_window_active() {
                        println!("READY");
                    } else {
                        println!("FOCUS-LOST");
                        std::process::exit(3);
                    }
                });
                std::mem::forget(sub);
                if window.is_window_active() {
                    println!("READY");
                }
            });
        }

        if let Some(after) = args.exit_after_ms {
            cx.spawn(async move |cx| {
                cx.background_executor().timer(Duration::from_millis(after)).await;
                cx.update(|cx| cx.quit());
            })
            .detach();
        }
    });
}
