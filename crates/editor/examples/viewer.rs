//! A standalone window showing one file in `EditorView`, plus the
//! benchmarks for brief 0009.
//!
//! ```text
//! cargo run -p eludite-editor --release --example viewer -- FILE [options]
//!   --bench-scroll        scroll top to bottom once highlighting is complete,
//!                         print frame-cost JSON and quit
//!   --lines-per-frame N   scroll step for --bench-scroll (default 40)
//!   --bench-type [N]      N synthetic keystrokes (default 500) at a middle
//!                         row, print keystroke frame-cost JSON and quit
//!   --bench-open          print time to first painted frame and to complete
//!                         highlighting, with memory, and quit
//!   --tree-limit BYTES    syntax tree retain limit (default TREE_RETAIN_LIMIT)
//!   --measure-tree        without a window: tree memory, and the cost of an
//!                         edit with the tree kept and dropped
//! cargo run -p eludite-editor --release --example viewer -- --generate LANG LINES OUT
//!   write a synthetic C# (LANG=csharp) or Rust (LANG=rust) file
//! ```
//!
//! Frame cost is measured inside the process: from the start of the frame
//! (GPUI's `on_next_frame` callback, or the key handler for typing) to the
//! end of `present`. It excludes the wait for the display's next refresh,
//! which is what PLAN.md section 9's "keystroke to frame submitted" row
//! budgets.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use eludite_editor::syntax::LanguageRegistry;
use eludite_editor::text;
use eludite_editor::{Buffer, EditorView, key_bindings};
use gpui::{
    App, AppContext as _, Bounds, Context, Entity, Focusable as _, IntoElement, Keystroke,
    ParentElement as _, Render, Styled as _, TitlebarOptions, Window, WindowBounds, WindowOptions,
    div, px, size,
};
use serde_json::{Value, json};

#[derive(Default)]
struct Args {
    file: Option<PathBuf>,
    bench_scroll: bool,
    lines_per_frame: f32,
    bench_type: Option<usize>,
    bench_open: bool,
    tree_limit: Option<usize>,
    measure_tree: bool,
    generate: Option<(String, usize, PathBuf)>,
}

fn parse_args() -> Args {
    let mut a = Args {
        lines_per_frame: 40.,
        ..Args::default()
    };
    let mut it = std::env::args().skip(1).peekable();
    while let Some(arg) = it.next() {
        match arg.as_str() {
            "--bench-scroll" => a.bench_scroll = true,
            "--lines-per-frame" => {
                a.lines_per_frame = it.next().expect("N").parse().expect("number")
            }
            "--bench-type" => {
                let n = match it.peek() {
                    Some(v) if v.parse::<usize>().is_ok() => it.next().unwrap().parse().unwrap(),
                    _ => 500,
                };
                a.bench_type = Some(n);
            }
            "--bench-open" => a.bench_open = true,
            "--tree-limit" => {
                a.tree_limit = Some(it.next().expect("BYTES").parse().expect("number"))
            }
            "--measure-tree" => a.measure_tree = true,
            "--generate" => {
                let lang = it.next().expect("LANG");
                let lines = it.next().expect("LINES").parse().expect("number");
                let out = PathBuf::from(it.next().expect("OUT"));
                a.generate = Some((lang, lines, out));
            }
            other if !other.starts_with("--") => a.file = Some(PathBuf::from(other)),
            other => panic!("unknown argument {other}"),
        }
    }
    a
}

// ----- allocator -----

/// The viewer uses the system allocator, as the `eludite` binary does.
/// `--features mimalloc-global` builds it with mimalloc as the global
/// allocator, for the comparison in brief 0011 (tree-sitter allocates from
/// its own mimalloc heap either way; see `syntax::alloc`).
#[cfg(feature = "mimalloc-global")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[cfg(feature = "mimalloc-global")]
const ALLOCATOR: &str = "mimalloc";
#[cfg(not(feature = "mimalloc-global"))]
const ALLOCATOR: &str = "system";

// ----- measurement -----

/// How long the benchmarks wait before an idle or settled RSS reading.
const IDLE_SETTLE: Duration = Duration::from_secs(2);

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.
}

fn percentile(sorted: &[f64], p: f64) -> f64 {
    if sorted.is_empty() {
        return f64::NAN;
    }
    let rank = ((p / 100.) * sorted.len() as f64).ceil() as usize;
    sorted[rank.clamp(1, sorted.len()) - 1]
}

fn round3(x: f64) -> f64 {
    (x * 1000.).round() / 1000.
}

fn summarize(samples: &[f64]) -> Value {
    let mut s = samples.to_vec();
    s.sort_by(|a, b| a.partial_cmp(b).unwrap());
    json!({
        "n": s.len(),
        "p50_ms": round3(percentile(&s, 50.)),
        "p95_ms": round3(percentile(&s, 95.)),
        "p99_ms": round3(percentile(&s, 99.)),
        "max_ms": round3(s.last().copied().unwrap_or(f64::NAN)),
    })
}

fn rss_json() -> Value {
    let Ok(s) = std::fs::read_to_string("/proc/self/status") else {
        return Value::Null;
    };
    let field = |name: &str| {
        s.lines()
            .find(|l| l.starts_with(name))
            .and_then(|l| l.split_whitespace().nth(1))
            .and_then(|v| v.parse::<f64>().ok())
            .map(|kib| round3(kib / 1024.))
    };
    json!({
        "rss_mib": field("VmRSS:"),
        "peak_rss_mib": field("VmHWM:"),
        "tree_sitter_mib": round3(eludite_editor::syntax::alloc::live_bytes() as f64 / 1048576.),
    })
}

fn platform_json(window: &Window) -> Value {
    json!({
        "os": std::env::consts::OS,
        "wayland_display": std::env::var("WAYLAND_DISPLAY").unwrap_or_default(),
        "scale_factor": window.scale_factor(),
        "viewport": [f32::from(window.viewport_size().width), f32::from(window.viewport_size().height)],
    })
}

type AfterPresent = Box<dyn FnOnce(&mut App)>;

#[derive(Default)]
struct Probes {
    /// Every presented frame: (root render start, end of present).
    frames: Vec<(Instant, Instant)>,
    record_frames: bool,
    /// A keystroke waiting for its frame: (handler start, handler end).
    pending_key: Option<(Instant, Instant)>,
    /// (handler start, handler end, render start, present end)
    keys: Vec<(Instant, Instant, Instant, Instant)>,
    after_present: Vec<AfterPresent>,
}

/// Root view: the editor plus frame probes. GPUI renders the root on every
/// frame, so its render marks the start of layout and paint, and a `defer`
/// from it runs after the frame is presented.
struct Root {
    editor: Entity<EditorView>,
    probes: Rc<RefCell<Probes>>,
}

impl Render for Root {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let start = Instant::now();
        let probes = self.probes.clone();
        cx.defer(move |cx| {
            let end = Instant::now();
            let (once, record) = {
                let mut p = probes.borrow_mut();
                if let Some((k0, k1)) = p.pending_key.take() {
                    p.keys.push((k0, k1, start, end));
                }
                (std::mem::take(&mut p.after_present), p.record_frames)
            };
            if record {
                probes.borrow_mut().frames.push((start, end));
            }
            for f in once {
                f(cx);
            }
        });
        div().size_full().child(self.editor.clone())
    }
}

// ----- generators -----

fn generate(lang: &str, lines: usize) -> String {
    let mut out = String::with_capacity(lines * 40);
    let mut n = 0usize;
    let mut block = 0usize;
    match lang {
        "csharp" => {
            out.push_str("using System;\nusing System.Collections.Generic;\nusing System.Linq;\n\nnamespace Eludite.Generated\n{\n");
            n += 6;
            while n + 2 < lines {
                let b = block;
                let body = format!(
                    r#"    /// <summary>Generated type number {b}.</summary>
    [Serializable]
    public sealed class Widget{b} : IComparable<Widget{b}>
    {{
        private readonly List<string> _names = new List<string>();
        private const int Limit = {limit};
        public int Id {{ get; set; }}
        public string Label {{ get; private set; }} = "widget-{b}";

        public Widget{b}(int id)
        {{
            Id = id;
            for (var i = 0; i < Limit; i++)
            {{
                _names.Add($"name {{i}} of {{id}}");
            }}
        }}

        // Compares by id, then by label.
        public int CompareTo(Widget{b} other)
        {{
            if (other == null) return 1;
            var byId = Id.CompareTo(other.Id);
            return byId != 0 ? byId : string.Compare(Label, other.Label, StringComparison.Ordinal);
        }}

        public IEnumerable<string> Matching(string prefix)
        {{
            return _names.Where(n => n.StartsWith(prefix)).OrderBy(n => n.Length);
        }}

        public override string ToString() => $"Widget{b}({{Id}}, {{Label}}) \t{{_names.Count}}";
    }}

"#,
                    limit = b % 97 + 3
                );
                n += body.lines().count();
                out.push_str(&body);
                block += 1;
            }
            out.push_str("}\n");
        }
        "rust" => {
            out.push_str(
                "//! Generated file.\n\nuse std::collections::HashMap;\nuse std::fmt;\n\n",
            );
            n += 5;
            while n < lines {
                let b = block;
                let body = format!(
                    r#"/// Generated type number {b}.
#[derive(Debug, Clone, PartialEq)]
pub struct Widget{b} {{
    id: u64,
    label: String,
    names: Vec<String>,
}}

const LIMIT_{b}: usize = {limit};

impl Widget{b} {{
    pub fn new(id: u64) -> Self {{
        let names = (0..LIMIT_{b}).map(|i| format!("name {{i}} of {{id}}")).collect();
        Self {{ id, label: "widget-{b}".to_string(), names }}
    }}

    // Counts names by first letter.
    pub fn histogram(&self) -> HashMap<char, usize> {{
        let mut out = HashMap::new();
        for name in &self.names {{
            if let Some(c) = name.chars().next() {{
                *out.entry(c).or_insert(0) += 1;
            }}
        }}
        out
    }}
}}

impl fmt::Display for Widget{b} {{
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {{
        write!(f, "Widget{b}({{}}, {{}})\n", self.id, self.label)
    }}
}}

"#,
                    limit = b % 97 + 3
                );
                n += body.lines().count();
                out.push_str(&body);
                block += 1;
            }
        }
        other => panic!("unknown language {other}; use csharp or rust"),
    }
    out
}

// ----- benches -----

/// Scroll from top to bottom, `step` pixels per frame, then report.
fn bench_scroll(window: &mut Window, root: Entity<Root>, step: f32) {
    struct St {
        warmup: usize,
        starts: Vec<Instant>,
        rss_before: Value,
        t0: Instant,
    }
    let st = Rc::new(RefCell::new(St {
        warmup: 60,
        starts: Vec::new(),
        rss_before: Value::Null,
        t0: Instant::now(),
    }));
    fn tick(st: Rc<RefCell<St>>, root: Entity<Root>, step: f32, window: &mut Window, cx: &mut App) {
        let now = Instant::now();
        let (editor, probes) = {
            let r = root.read(cx);
            (r.editor.clone(), r.probes.clone())
        };
        let mut s = st.borrow_mut();
        if s.warmup > 0 {
            s.warmup -= 1;
            if s.warmup == 0 {
                editor.update(cx, |v, cx| v.set_scroll_y(px(0.), cx));
                probes.borrow_mut().record_frames = true;
                probes.borrow_mut().frames.clear();
                s.rss_before = rss_json();
                s.t0 = now;
            }
            editor.update(cx, |_, cx| cx.notify());
        } else {
            s.starts.push(now);
            let done = editor.update(cx, |v, cx| {
                let y = v.scroll_position().y + px(step);
                let max = v.max_scroll_y();
                v.set_scroll_y(y.min(max), cx);
                y >= max
            });
            if done {
                drop(s);
                let st = st.clone();
                window.on_next_frame(move |window, cx| {
                    let s = st.borrow();
                    let frames = probes.borrow().frames.clone();
                    let mut work = Vec::new();
                    let mut render_to_present = Vec::new();
                    let mut j = 0;
                    for (i, start) in s.starts.iter().enumerate() {
                        while j < frames.len() && frames[j].0 < *start {
                            j += 1;
                        }
                        let next = s.starts.get(i + 1);
                        if let Some((r, p)) = frames.get(j)
                            && next.is_none_or(|n| p < n)
                        {
                            work.push(ms(*p - *start));
                            render_to_present.push(ms(*p - *r));
                        }
                    }
                    let intervals: Vec<f64> =
                        s.starts.windows(2).map(|w| ms(w[1] - w[0])).collect();
                    let (lines, lh, hl) = editor.read_with(cx, |v, _| {
                        (
                            v.editor().buffer().line_count(),
                            v.line_height(),
                            v.highlights_complete(),
                        )
                    });
                    let out = json!({
                        "bench": "scroll",
                        "lines": lines,
                        "lines_per_frame": step / f32::from(lh),
                        "frames": s.starts.len(),
                        "duration_ms": round3(ms(s.t0.elapsed())),
                        "highlighting_complete": hl,
                        "frame_cost": summarize(&work),
                        "render_to_present": summarize(&render_to_present),
                        "frame_interval": summarize(&intervals),
                        "rss_before": s.rss_before,
                        "rss_after": rss_json(),
                        "platform": platform_json(window),
                    });
                    println!("{out}");
                    cx.quit();
                });
                return;
            }
        }
        drop(s);
        window.on_next_frame(move |window, cx| tick(st, root, step, window, cx));
    }
    window.on_next_frame(move |window, cx| tick(st, root, step, window, cx));
}

/// Deterministic jitter for keystroke timing.
struct Lcg(u64);
impl Lcg {
    fn next_f64(&mut self) -> f64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        (self.0 >> 11) as f64 / (1u64 << 53) as f64
    }
}

fn bench_type(handle: gpui::AnyWindowHandle, root: Entity<Root>, count: usize, cx: &mut App) {
    let (editor, probes) = {
        let r = root.read(cx);
        (r.editor.clone(), r.probes.clone())
    };
    let row = editor.read(cx).editor().buffer().line_count() / 2;
    editor.update(cx, |v, cx| {
        v.update_editor(cx, |e| {
            let b = e.buffer();
            let at = b.point_to_offset(eludite_editor::text::Point::new(row, b.line_len(row)));
            e.set_caret(at);
        });
    });
    let executor = cx.background_executor().clone();
    cx.spawn(async move |cx| {
        let mut rng = Lcg(0x5eed);
        executor.timer(Duration::from_millis(500)).await;
        cx.update(|_| {
            let mut p = probes.borrow_mut();
            p.keys.clear();
            p.frames.clear();
            p.record_frames = true;
        });
        let rss_before = cx.update(|_| rss_json());
        let mut rss_curve = Vec::new();
        for i in 0..count {
            if i % 50 == 0 {
                rss_curve.push(json!([i, cx.update(|_| rss_json())["rss_mib"].clone()]));
            }
            let delay = 15. + 30. * rng.next_f64();
            executor.timer(Duration::from_micros((delay * 1000.) as u64)).await;
            let key = if i % 40 == 39 {
                "enter".to_owned()
            } else if i % 17 == 16 {
                "backspace".to_owned()
            } else {
                ((b'a' + (i % 26) as u8) as char).to_string()
            };
            let ks = Keystroke::parse(&key).expect("keystroke");
            let probes = probes.clone();
            let _ = handle.update(cx, |_, window, cx| {
                let t0 = Instant::now();
                window.dispatch_keystroke(ks, cx);
                let t1 = Instant::now();
                let mut p = probes.borrow_mut();
                if p.pending_key.is_none() {
                    p.pending_key = Some((t0, t1));
                }
            });
        }
        executor.timer(Duration::from_millis(300)).await;
        let rss_after = cx.update(|_| rss_json());
        // Settled: highlighting caught up with the last keystroke, then idle.
        loop {
            executor.timer(Duration::from_millis(10)).await;
            if editor.read_with(cx, |v, _| v.highlights_complete()) {
                break;
            }
        }
        executor.timer(IDLE_SETTLE).await;
        rss_curve.push(json!([count, cx.update(|_| rss_json())["rss_mib"].clone()]));
        let _ = handle.update(cx, |_, window, cx| {
            let p = probes.borrow();
            let handler: Vec<f64> = p.keys.iter().map(|k| ms(k.1 - k.0)).collect();
            let render: Vec<f64> = p.keys.iter().map(|k| ms(k.3 - k.2)).collect();
            let cost: Vec<f64> = p.keys.iter().map(|k| ms(k.1 - k.0) + ms(k.3 - k.2)).collect();
            let to_present: Vec<f64> = p.keys.iter().map(|k| ms(k.3 - k.0)).collect();
            let all_frames: Vec<f64> = p.frames.iter().map(|f| ms(f.1 - f.0)).collect();
            let lines = editor.read(cx).editor().buffer().line_count();
            let out = json!({
                "bench": "type",
                "method": "Window::dispatch_keystroke at random phase (15-45 ms apart); frame cost = key handler + render to end of present",
                "lines": lines,
                "at_row": row,
                "keystrokes": count,
                "samples": p.keys.len(),
                "keystroke_frame_cost": summarize(&cost),
                "key_handler": summarize(&handler),
                "render_to_present": summarize(&render),
                "key_to_present": summarize(&to_present),
                "all_frames_render_to_present": summarize(&all_frames),
                "highlight_updates": editor.read(cx).highlight_progress().0,
                "rss_before": rss_before,
                "rss_after": rss_after,
                "rss_settled": rss_json(),
                "rss_curve_mib": rss_curve,
                "allocator": ALLOCATOR,
                "platform": platform_json(window),
            });
            println!("{out}");
            cx.quit();
        });
    })
    .detach();
}

/// `--measure-tree`: what retaining the syntax tree costs and what dropping
/// it costs, without GPUI. Prints tree-sitter's live bytes with the tree and
/// without it, the time of a full parse and highlight pass, and the time to
/// bring highlights up to date after a one-character edit with the tree kept
/// (incremental) and with it dropped (full re-parse, every row redone).
fn measure_tree(buffer: Buffer, language: std::sync::Arc<eludite_editor::syntax::Language>) {
    use eludite_editor::syntax::{Highlighter, alloc::live_bytes};
    let mib = |b: usize| round3(b as f64 / 1048576.);
    let base = live_bytes();
    let mut text = text::Buffer::new(
        text::ReplicaId::LOCAL,
        text::BufferId::new(1).unwrap(),
        buffer.text(),
    );
    drop(buffer);
    let pass = |h: &mut Highlighter, snapshot: &text::BufferSnapshot| {
        let t = Instant::now();
        let mut parse = Duration::ZERO;
        let mut first = None;
        loop {
            let u = h.step(snapshot, 0..60).expect("not cancelled");
            parse += u.stats.parse;
            first.get_or_insert(t.elapsed());
            if u.complete {
                return (
                    ms(parse),
                    ms(first.unwrap()),
                    ms(t.elapsed()),
                    ms(u.stats.release),
                );
            }
        }
    };
    let edit_middle = |text: &mut text::Buffer| {
        let row = text.max_point().row / 2;
        let at = text.point_to_offset(text::Point::new(row, 0));
        text.edit([(at..at, "x")]);
    };
    // Retained: initial pass, then one edit re-parsed incrementally.
    let mut kept = Highlighter::new(language.clone());
    kept.set_retain_limit(usize::MAX);
    let (parse_ms, first_ms, pass_ms, _) = pass(&mut kept, text.snapshot());
    let with_tree = live_bytes() - base;
    edit_middle(&mut text);
    let (inc_parse_ms, inc_first_ms, inc_ms, inc_release_ms) = pass(&mut kept, text.snapshot());
    drop(kept);
    // Dropped: initial pass drops the tree; one edit re-parses from scratch.
    let mut gated = Highlighter::new(language);
    gated.set_retain_limit(0);
    pass(&mut gated, text.snapshot());
    let without_tree = live_bytes() - base;
    edit_middle(&mut text);
    let (full_parse_ms, full_first_ms, full_ms, full_release_ms) =
        pass(&mut gated, text.snapshot());
    let snapshot = text.snapshot();
    println!(
        "{}",
        json!({
            "bench": "measure-tree",
            "bytes": snapshot.len(),
            "lines": snapshot.max_point().row + 1,
            "tree_sitter_mib_with_tree": mib(with_tree),
            "tree_sitter_mib_without_tree": mib(without_tree),
            "tree_bytes_per_source_byte": round3(with_tree as f64 / snapshot.len() as f64),
            "initial": {"parse_ms": round3(parse_ms), "first_rows_ms": round3(first_ms), "complete_ms": round3(pass_ms)},
            "edit_tree_kept": {"parse_ms": round3(inc_parse_ms), "first_rows_ms": round3(inc_first_ms), "complete_ms": round3(inc_ms), "release_ms": round3(inc_release_ms)},
            "edit_tree_dropped": {"parse_ms": round3(full_parse_ms), "first_rows_ms": round3(full_first_ms), "complete_ms": round3(full_ms), "release_ms": round3(full_release_ms)},
            "allocator": ALLOCATOR,
        })
    );
}

fn main() {
    eludite_editor::syntax::alloc::disable_transparent_huge_pages();
    let t_main = Instant::now();
    let args = parse_args();
    if let Some((lang, lines, out)) = &args.generate {
        let text = generate(lang, *lines);
        std::fs::write(out, &text).expect("write");
        println!(
            "{}",
            json!({"generated": out, "lines": text.lines().count(), "bytes": text.len()})
        );
        return;
    }
    let path = args
        .file
        .clone()
        .expect("usage: viewer FILE [--bench-scroll|--bench-type [N]|--bench-open]");
    let t_read = Instant::now();
    let bytes = std::fs::read(&path).expect("read file");
    let read_ms = ms(t_read.elapsed());
    let t_buffer = Instant::now();
    let buffer = Buffer::from_bytes(&bytes).expect("UTF-8 file");
    drop(bytes);
    let buffer_ms = ms(t_buffer.elapsed());
    let registry = LanguageRegistry::with_builtins();
    let language = registry.for_path(&path);
    if args.measure_tree {
        measure_tree(buffer, language.expect("a C# or Rust file"));
        return;
    }
    let benching = args.bench_scroll || args.bench_type.is_some() || args.bench_open;

    gpui_platform::application().run(move |cx: &mut App| {
        cx.bind_keys(key_bindings());
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        let bounds = Bounds::centered(None, size(px(1200.), px(900.)), cx);
        let title = path.display().to_string();
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some(title.into()),
                ..Default::default()
            }),
            app_id: Some("eludite-editor-viewer".into()),
            inactive_frame_interval: if benching {
                None
            } else {
                WindowOptions::default().inactive_frame_interval
            },
            ..Default::default()
        };
        let probes = Rc::new(RefCell::new(Probes::default()));
        let mut root_out = None;
        let window = cx
            .open_window(options, |window, cx| {
                let editor = cx.new(|cx| {
                    let mut view = EditorView::new(buffer, language, cx);
                    if let Some(limit) = args.tree_limit {
                        view.set_syntax_tree_limit(limit);
                    }
                    view
                });
                window.focus(&editor.focus_handle(cx), cx);
                let root = cx.new(|_| Root {
                    editor,
                    probes: probes.clone(),
                });
                root_out = Some(root.clone());
                root
            })
            .expect("open window");
        let root = root_out.expect("root");
        cx.activate(true);

        if args.bench_open {
            let editor = root.read(cx).editor.clone();
            probes.borrow_mut().after_present.push(Box::new(move |cx| {
                let first_paint = ms(t_main.elapsed());
                let rss_first = rss_json();
                let lines = editor.read(cx).editor().buffer().line_count();
                let bytes = editor.read(cx).editor().buffer().len();
                // Poll until highlighting covers the whole file.
                let editor2 = editor.clone();
                cx.spawn(async move |cx| {
                    loop {
                        cx.background_executor()
                            .timer(Duration::from_millis(5))
                            .await;
                        let done = editor2.read_with(cx, |v, _| v.highlights_complete());
                        if done {
                            break;
                        }
                    }
                    let complete = ms(t_main.elapsed());
                    let rss_highlighted = rss_json();
                    let (updates, stats) = editor2.read_with(cx, |v, _| v.highlight_progress());
                    // Idle: give the allocator time to return freed pages.
                    cx.background_executor().timer(IDLE_SETTLE).await;
                    let out = json!({
                        "bench": "open",
                        "bytes": bytes,
                        "lines": lines,
                        "read_ms": round3(read_ms),
                        "buffer_ms": round3(buffer_ms),
                        "main_to_first_paint_ms": round3(first_paint),
                        "main_to_highlighting_complete_ms": round3(complete),
                        "highlight_updates": updates,
                        "last_step_parse_ms": round3(ms(stats.parse)),
                        "rss_at_first_paint": rss_first,
                        "rss_highlighted": rss_highlighted,
                        "rss_idle": rss_json(),
                        "allocator": ALLOCATOR,
                    });
                    println!("{out}");
                    cx.update(|cx| cx.quit());
                })
                .detach();
            }));
        }

        if args.bench_scroll {
            let root2 = root.clone();
            let lines_per_frame = args.lines_per_frame;
            let editor = root.read(cx).editor.clone();
            // Start once highlighting is complete, so the run measures
            // scrolling through highlighted text.
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(10))
                        .await;
                    if editor.read_with(cx, |v, _| v.highlights_complete()) {
                        break;
                    }
                }
                let _ = window.update(cx, |_, window, cx| {
                    let lh = f32::from(editor.read(cx).line_height());
                    bench_scroll(window, root2, lines_per_frame * lh)
                });
            })
            .detach();
        }

        if let Some(n) = args.bench_type {
            let handle = window.into();
            let editor = root.read(cx).editor.clone();
            let root2 = root.clone();
            cx.spawn(async move |cx| {
                loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(10))
                        .await;
                    if editor.read_with(cx, |v, _| v.highlights_complete()) {
                        break;
                    }
                }
                cx.update(|cx| bench_type(handle, root2, n, cx));
            })
            .detach();
        }
    });
}
