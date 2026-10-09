//! The client against the real lldb-dap (brief 0029), debugging a small Cargo program the test writes and builds in a
//! temporary folder with `cargo build --message-format=json`: the launch configuration `eludite_dap::cargo` computes
//! from the build's artifact message and `[package.metadata.eludite.run]`, the Rust formatters from the toolchain's
//! sysroot, a breakpoint, the stack with the caller, Rust-formatted locals, `evaluate`, `variables` paging, steps, a
//! log point, `setVariable`, the panic breakpoint, `pause`, `disconnect`, and the package's test executable built with
//! `cargo test --no-run` and launched with a filter. Prints `timing:` lines for the brief's budgets. Skipped with a
//! message when lldb-dap (the setting's search order: `ELUDITE_LLDB_DAP`, `PATH`, `/usr/lib/llvm-NN`) or cargo is
//! missing.

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eludite_dap::cargo::{self, CargoPackageInfo, CargoStart};
use eludite_dap::discovery::{LldbAdapter, LldbSearch};
use eludite_dap::launch::Platform;
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{Event, FunctionBreakpoint, SourceBreakpoint};
use eludite_dap::{ClientEvent, DapClient, transport};
use serde_json::{Value, json};

use common::Recorder;

/// The adapter and cargo can be slow on a loaded machine.
const T: Duration = Duration::from_secs(30);

const MAIN_RS: &str = r#"#[derive(Debug)]
enum Shape {
    Circle(f64),
    Rect { w: i32, h: i32 },
    Empty,
}

fn add(a: i32, b: i32) -> i32 {
    let sum = a + b; // MARK: add-sum
    sum // MARK: add-return
}

fn twice(i: i32) -> i32 {
    i * 2 // MARK: log
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.iter().any(|a| a == "sleep") {
        println!("sleeping");
        std::thread::sleep(std::time::Duration::from_secs(60));
        return;
    }
    let text = String::from("hello");
    let v: Vec<i32> = vec![1, 2, 3];
    let o: Option<i32> = Some(5);
    let n: Option<i32> = None;
    let shape = Shape::Rect { w: 2, h: 3 };
    let circle = Shape::Circle(1.5);
    let empty = Shape::Empty;
    let big: Vec<u32> = (0..10_000).collect();
    let mut count = 7;
    let r = add(count, 3); // MARK: call-add
    count += r; // MARK: after-add
    let width = add(text.len() as i32, v.len() as i32); // MARK: std-then-add
    let mut total = 0;
    for i in 0..100 {
        total += twice(i); // MARK: loop
    }
    if args.iter().any(|a| a == "panic") {
        panic!("boom"); // MARK: panic
    }
    println!(
        "{text} {v:?} {o:?} {n:?} {shape:?} {circle:?} {empty:?} {} {count} {width} {total} args={:?} env={}",
        big.len(),
        &args[1..],
        std::env::var("LLDB_TEST_ENV").unwrap_or_default()
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn my_test() {
        let answer = 40 + 2; // MARK: my-test
        assert_eq!(answer, 42);
    }

    #[test]
    fn other_test() {
        assert_eq!(1 + 1, 2);
    }
}
"#;

const CARGO_TOML: &str = r#"[package]
name = "lldbtest"
version = "0.1.0"
edition = "2024"

[package.metadata.eludite.run]
args = ["--from-metadata"]
env = { LLDB_TEST_ENV = "from-metadata" }

[profile.dev]
debug = 2
"#;

fn line_of(mark: &str) -> i64 {
    MAIN_RS
        .lines()
        .position(|l| l.ends_with(&format!("// MARK: {mark}")))
        .unwrap_or_else(|| panic!("MARK: {mark}")) as i64
        + 1
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1e3
}

fn p95(mut v: Vec<Duration>) -> Duration {
    v.sort();
    v[(v.len() * 95).div_ceil(100).saturating_sub(1)]
}

/// The temporary package, built, and what the launches need.
struct Program {
    /// Kept for its removal at the end; the paths are `start.root`, canonical.
    _dir: tempfile::TempDir,
    adapter: LldbAdapter,
    cargo: PathBuf,
    start: CargoStart,
    artifacts: Vec<cargo::Artifact>,
    init: Vec<String>,
    source: String,
}

/// Whether the adapter at `path` starts at all: the LLVM installer's lldb-dap on Windows dies before `main` when the
/// Python it was built against is not installed (an NTSTATUS exit code), as a signal kills one elsewhere.
fn runs(path: &Path) -> Result<(), String> {
    let out = std::process::Command::new(path)
        .arg("--help")
        .stdin(std::process::Stdio::null())
        .output()
        .map_err(|e| e.to_string())?;
    #[cfg(windows)]
    if let Some(code) = out.status.code()
        && (code as u32) >= 0xC000_0000
    {
        return Err(format!(
            "exit status {:#x}: {}",
            code as u32,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt as _;
        if let Some(signal) = out.status.signal() {
            return Err(format!("killed by signal {signal}"));
        }
    }
    Ok(())
}

fn run_cargo(cargo: &Path, dir: &Path, args: &[&str]) -> std::process::Output {
    std::process::Command::new(cargo)
        .args(args)
        .current_dir(dir)
        // Full debug information whatever the user's configuration says (this repository's local one asks for
        // line tables only).
        .env("CARGO_PROFILE_DEV_DEBUG", "2")
        .env_remove("CARGO_TARGET_DIR")
        .stdin(std::process::Stdio::null())
        .output()
        .expect("cargo runs")
}

fn setup() -> Option<Program> {
    let adapter = match (LldbSearch {
        configured: std::env::var_os("ELUDITE_LLDB_DAP")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        ..LldbSearch::from_env()
    })
    .find(Platform::current())
    {
        Ok(a) => a,
        Err(e) => {
            eludite_test_support::skip("lldb-dap", e);
            return None;
        }
    };
    let cargo = PathBuf::from("cargo");
    if std::process::Command::new(&cargo)
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: cargo is not on PATH");
        return None;
    }
    let version = adapter.version();
    eprintln!(
        "adapter: {} ({})",
        adapter.describe(version.as_deref()),
        adapter.path.display()
    );
    if let Err(e) = runs(&adapter.path) {
        eprintln!("skipped: {} does not run here: {e}", adapter.path.display());
        return None;
    }
    let dir = tempfile::Builder::new()
        .prefix("eludite-lldb-test")
        .tempdir()
        .unwrap();
    // Canonical: cargo reports the executable under the real path, and lldb matches a breakpoint's file to the
    // debug information's path, which is `/private/var/...` on macOS where the temporary folder is `/var/...`.
    // Without Windows' verbatim prefix, which cargo does not spell either.
    let root = dir.path().canonicalize().unwrap();
    let root = root
        .to_str()
        .and_then(|s| s.strip_prefix(r"\\?\"))
        .map_or(root.clone(), PathBuf::from);
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("Cargo.toml"), CARGO_TOML).unwrap();
    std::fs::write(root.join("src/main.rs"), MAIN_RS).unwrap();
    // The repository's toolchain, also when the test runs outside cargo.
    let pin = std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../rust-toolchain.toml"),
    )
    .unwrap();
    let channel = pin
        .lines()
        .find_map(|l| l.trim().strip_prefix("channel = "))
        .unwrap()
        .trim_matches('"')
        .to_owned();
    std::fs::write(
        root.join("rust-toolchain.toml"),
        format!("[toolchain]\nchannel = \"{channel}\"\n"),
    )
    .unwrap();
    let clock = Instant::now();
    let out = run_cargo(&cargo, &root, &["build", "--message-format=json"]);
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    eprintln!(
        "timing: cargo build of the test program: {:.0} ms",
        ms(clock.elapsed())
    );
    let artifacts = cargo::parse_artifacts(&String::from_utf8_lossy(&out.stdout));
    let manifest = root.join("Cargo.toml");
    let start = CargoStart {
        package: CargoPackageInfo {
            name: "lldbtest".into(),
            manifest: manifest.clone(),
            bins: vec!["lldbtest".into()],
            has_lib: false,
        },
        root: root.clone(),
        workspace_manifest: manifest,
        target_dir: root.join("target"),
        release: false,
        target: None,
        test: false,
        args: None,
    };
    let clock = Instant::now();
    let sysroot = cargo::sysroot(&cargo::rustc_for(&cargo), &root).expect("rustc --print sysroot");
    eprintln!(
        "timing: rustc --print sysroot: {:.0} ms ({})",
        ms(clock.elapsed()),
        sysroot.display()
    );
    let mut init = vec![cargo::STEP_AVOID.to_owned()];
    init.extend(cargo::rust_init_commands(&cargo::formatters_dir(&sysroot)));
    assert!(
        init.len() >= 4,
        "the toolchain has the formatters: {init:?}"
    );
    let source = root.join("src/main.rs").to_string_lossy().into_owned();
    Some(Program {
        _dir: dir,
        adapter,
        cargo,
        start,
        artifacts,
        init,
        source,
    })
}

impl Program {
    /// Start lldb-dap and launch `launch` with breakpoints at `marks` and function breakpoints `functions`.
    fn launch(
        &self,
        launch: &cargo::CargoLaunch,
        init: &[String],
        marks: &[&str],
        functions: &[&str],
    ) -> (DapClient, Recorder, session::Started) {
        let rec = Recorder::default();
        let client = DapClient::start(
            common::recorded(
                transport::connect(&self.adapter.transport()).unwrap(),
                "lldb",
                &self.adapter.describe(self.adapter.version().as_deref()),
                Some(&self.start.root),
            ),
            rec.sink(),
        );
        let started = session::start(
            &client,
            &StartPlan {
                adapter_id: "lldb".into(),
                kind: StartKind::Launch,
                arguments: launch.lldb_arguments(init),
                breakpoints: vec![(
                    self.source.clone(),
                    marks
                        .iter()
                        .map(|m| SourceBreakpoint {
                            line: line_of(m),
                            ..Default::default()
                        })
                        .collect(),
                )],
                function_breakpoints: functions
                    .iter()
                    .map(|f| FunctionBreakpoint {
                        name: (*f).to_owned(),
                        ..Default::default()
                    })
                    .collect(),
                exception_filters: Vec::new(),
                exception_options: Vec::new(),
            },
            T,
        )
        .unwrap();
        (client, rec, started)
    }
}

fn req(client: &DapClient, command: &str, args: Value) -> Value {
    client
        .request_wait(command, args, T)
        .unwrap_or_else(|e| panic!("{command}: {e}"))
}

fn stopped(rec: &Recorder, n: usize) -> eludite_dap::types::StoppedEvent {
    match rec.wait_nth(n, "stopped", |e| {
        matches!(e, ClientEvent::Event(Event::Stopped(_)))
    }) {
        ClientEvent::Event(Event::Stopped(s)) => s,
        _ => unreachable!(),
    }
}

fn frames(client: &DapClient, tid: i64, levels: i64) -> Vec<Value> {
    req(
        client,
        "stackTrace",
        json!({"threadId": tid, "levels": levels}),
    )["stackFrames"]
        .as_array()
        .unwrap()
        .clone()
}

/// The first scope's variables of frame `frame`, as (name, value, reference).
fn locals(client: &DapClient, frame: &Value) -> (i64, Vec<(String, String, i64)>) {
    let scopes = req(client, "scopes", json!({"frameId": frame}));
    let reference = scopes["scopes"][0]["variablesReference"].as_i64().unwrap();
    let vars = req(
        client,
        "variables",
        json!({"variablesReference": reference}),
    );
    (
        reference,
        vars["variables"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| {
                (
                    v["name"].as_str().unwrap().to_owned(),
                    v["value"].as_str().unwrap().to_owned(),
                    v["variablesReference"].as_i64().unwrap_or(0),
                )
            })
            .collect(),
    )
}

fn value_of<'a>(vars: &'a [(String, String, i64)], name: &str) -> &'a (String, String, i64) {
    vars.iter()
        .find(|v| v.0 == name)
        .unwrap_or_else(|| panic!("no local {name} in {vars:?}"))
}

fn top_line(client: &DapClient, tid: i64) -> (String, i64) {
    let f = &frames(client, tid, 1)[0];
    (
        f["name"].as_str().unwrap().to_owned(),
        f["line"].as_i64().unwrap(),
    )
}

#[test]
fn lldb_dap_debugs_a_cargo_program_with_rust_values() {
    let Some(p) = setup() else { return };
    let launch = p.start.binary_launch(&p.artifacts).unwrap();
    // The executable from the build's artifact message; the run table's arguments and environment.
    assert_eq!(
        launch.program,
        p.start.root.join(format!(
            "target/debug/lldbtest{}",
            std::env::consts::EXE_SUFFIX
        )),
        "{:?}",
        p.artifacts
    );
    assert_eq!(launch.args, ["--from-metadata"]);
    assert_eq!(launch.cwd, p.start.root);

    // Launch to the first stop (cold: the first lldb-dap of the test), at the call of `add` in main.
    let clock = Instant::now();
    let (client, rec, started) = p.launch(&launch, &p.init, &["call-add", "add-sum"], &[]);
    let caps = &started.capabilities;
    assert!(caps.supports_hit_conditional_breakpoints && caps.supports_log_points);
    assert!(caps.supports_function_breakpoints && caps.supports_set_variable);
    // Apple's lldb-dap (Xcode's) does not support `restart`; the shell then restarts by stopping and launching.
    assert!(caps.supports_delayed_stack_trace_loading);
    eprintln!("capabilities: restart {}", caps.supports_restart_request);
    eprintln!(
        "capabilities: gotoTargets {}, terminate {}, exceptionInfo {}, filters {:?}",
        caps.supports_goto_targets_request,
        caps.supports_terminate_request,
        caps.supports_exception_info_request,
        caps.exception_breakpoint_filters
            .iter()
            .map(|f| f.filter.as_str())
            .collect::<Vec<_>>()
    );
    assert!(started.breakpoints[0].1.iter().all(|b| b.verified));
    let mut n = 0;
    let mut next_stop = || {
        n += 1;
        stopped(&rec, n)
    };
    let s = next_stop();
    eprintln!(
        "timing: launch to the first stopped (first lldb-dap of the run): {:.0} ms",
        ms(clock.elapsed())
    );
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    assert_eq!(
        top_line(&client, tid),
        ("lldbtest::main".into(), line_of("call-add"))
    );
    // setVariable on an i32 of the top frame: `count` becomes 40 before `add(count, 3)` reads it. lldb-dap 18's scope
    // references (1 locals, 2 globals, 3 registers) always mean the frame of the last `scopes` request.
    let (scope, _) = locals(&client, &frames(&client, tid, 1)[0]["id"]);
    let set = req(
        &client,
        "setVariable",
        json!({"variablesReference": scope, "name": "count", "value": "40"}),
    );
    // lldb-dap 18 answers with `result` where DAP says `value`.
    eprintln!("setVariable answer: {set}");
    assert_eq!(set.get("value").unwrap_or(&set["result"]), "40");
    req(&client, "continue", json!({"threadId": tid}));
    assert_eq!(next_stop().reason, "breakpoint");
    // The stack with the caller.
    let stack = frames(&client, tid, 20);
    assert_eq!(stack[0]["name"], "lldbtest::add");
    assert_eq!(stack[0]["line"], line_of("add-sum"));
    assert_eq!(stack[1]["name"], "lldbtest::main");
    assert_eq!(stack[1]["line"], line_of("call-add"));
    let std_frame = stack.iter().find_map(|f| {
        f["source"]["path"]
            .as_str()
            .filter(|p| p.starts_with("/rustc/"))
    });
    eprintln!("a standard library frame's path: {std_frame:?}");
    assert!(std_frame.is_some_and(cargo::is_rustc_path));
    // Locals with the Rust formatters; `a` is the value setVariable gave `count`.
    let (_, add_locals) = locals(&client, &stack[0]["id"]);
    assert_eq!(
        value_of(&add_locals, "a").1,
        "40",
        "setVariable reached the program"
    );
    assert_eq!(value_of(&add_locals, "b").1, "3");
    let main_frame = stack[1]["id"].clone();
    let (_, main_locals) = locals(&client, &main_frame);
    eprintln!("main's locals: {main_locals:?}");
    assert_eq!(value_of(&main_locals, "text").1, "\"hello\"");
    assert_eq!(value_of(&main_locals, "v").1, "size=3");
    assert_eq!(value_of(&main_locals, "o").1, "Some(5)");
    assert_eq!(value_of(&main_locals, "n").1, "None");
    assert_eq!(value_of(&main_locals, "shape").1, "Rect{w:2, h:3}");
    assert_eq!(value_of(&main_locals, "circle").1, "Circle(1.5)");
    assert_eq!(value_of(&main_locals, "empty").1, "Empty");
    assert_eq!(value_of(&main_locals, "big").1, "size=10000");
    let v_children = req(
        &client,
        "variables",
        json!({"variablesReference": value_of(&main_locals, "v").2}),
    );
    let items: Vec<&str> = v_children["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x["value"].as_str().unwrap())
        .collect();
    assert_eq!(items, ["1", "2", "3"]);
    // Paging the 10,000 elements of `big` (brief 0025's `variables` pages by `start` and `count`).
    let big = value_of(&main_locals, "big").2;
    let mut pages = Vec::new();
    for k in 0..20 {
        let clock = Instant::now();
        let page = req(
            &client,
            "variables",
            json!({"variablesReference": big, "start": k * 500, "count": 50}),
        );
        pages.push(clock.elapsed());
        let vars = page["variables"].as_array().unwrap();
        assert_eq!(vars.len(), 50);
        assert_eq!(vars[0]["value"], (k * 500).to_string());
    }
    eprintln!(
        "timing: a page of 50 of a Vec of 10,000: p95 {:.1} ms, max {:.1} ms (20 pages)",
        ms(p95(pages.clone())),
        ms(*pages.iter().max().unwrap())
    );
    // evaluate: locals and indexing work; Rust method calls do not (LLDB's evaluator is C++-flavored).
    let eval = |expr: &str, frame: &Value| {
        client.request_wait(
            "evaluate",
            json!({"expression": expr, "frameId": frame, "context": "watch"}),
            T,
        )
    };
    assert_eq!(eval("count", &main_frame).unwrap()["result"], "40");
    assert_eq!(eval("v[1]", &main_frame).unwrap()["result"], "2");
    assert_eq!(eval("count + 1", &main_frame).unwrap()["result"], "41");
    assert_eq!(eval("o", &main_frame).unwrap()["result"], "Some(5)");
    assert_eq!(eval("text", &main_frame).unwrap()["result"], "\"hello\"");
    for expr in [
        "v.len()",
        "text.len()",
        "&v[..2]",
        "count as i64",
        "o.is_some()",
    ] {
        eprintln!("evaluate {expr}: {:?}", eval(expr, &main_frame));
    }
    assert!(eval("v.len()", &main_frame).is_err());
    // next, then stepOut back to main.
    req(&client, "next", json!({"threadId": tid}));
    assert_eq!(next_stop().reason, "step");
    // The next line of `add`: the tail expression, or the closing brace where rustc put no line entry for it.
    let (name, line) = top_line(&client, tid);
    assert_eq!(name, "lldbtest::add");
    assert!(
        [line_of("add-return"), line_of("add-return") + 1].contains(&line),
        "{line}"
    );
    req(&client, "stepOut", json!({"threadId": tid}));
    assert_eq!(next_stop().reason, "step");
    assert_eq!(top_line(&client, tid).0, "lldbtest::main");
    while top_line(&client, tid).1 != line_of("after-add") {
        // stepOut stops on the call's line, part way through it.
        req(&client, "next", json!({"threadId": tid}));
        next_stop();
    }
    let (_, main_locals) = locals(&client, &frames(&client, tid, 1)[0]["id"]);
    assert_eq!(value_of(&main_locals, "r").1, "43");
    // stepIn on a line that calls the standard library (`String::len`, `Vec::len`) and then a user function lands in
    // the user function.
    req(&client, "next", json!({"threadId": tid}));
    next_stop();
    assert_eq!(top_line(&client, tid).1, line_of("std-then-add"));
    req(&client, "stepIn", json!({"threadId": tid}));
    next_stop();
    assert_eq!(
        top_line(&client, tid),
        ("lldbtest::add".into(), line_of("add-sum"))
    );
    req(&client, "stepOut", json!({"threadId": tid}));
    next_stop();
    // 50 `next`s through the loop, timed.
    let mut steps = Vec::new();
    for _ in 0..50 {
        let clock = Instant::now();
        req(&client, "next", json!({"threadId": tid}));
        next_stop();
        steps.push(clock.elapsed());
    }
    eprintln!(
        "timing: next round trip over 50 steps: p50 {:.1} ms, p95 {:.1} ms, max {:.1} ms",
        ms({
            let mut s = steps.clone();
            s.sort();
            s[s.len() / 2]
        }),
        ms(p95(steps.clone())),
        ms(*steps.iter().max().unwrap())
    );
    assert_eq!(top_line(&client, tid).0, "lldbtest::main");
    // A log point in `twice`: a line per call, no stop; then the program ends with its output.
    let bps = req(
        &client,
        "setBreakpoints",
        json!({"source": {"path": p.source}, "breakpoints": [{"line": line_of("log"), "logMessage": "twice of {i}"}]}),
    );
    assert_eq!(bps["breakpoints"][0]["verified"], true);
    req(&client, "continue", json!({"threadId": tid}));
    let exited = rec.wait_nth(1, "exited", |e| {
        matches!(e, ClientEvent::Event(Event::Exited(_)))
    });
    assert!(matches!(exited, ClientEvent::Event(Event::Exited(x)) if x.exit_code == 0));
    let logged = rec
        .events()
        .iter()
        .filter(
            |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("twice of ")),
        )
        .count();
    eprintln!("log point lines: {logged}");
    assert!(logged >= 10, "{logged}");
    let output: String = rec
        .events()
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Event(Event::Output(o)) if o.category.as_deref() == Some("stdout") => {
                Some(o.output.clone())
            }
            _ => None,
        })
        .collect();
    eprintln!("program output: {output:?}");
    assert!(
        output.contains("hello [1, 2, 3] Some(5) None Rect { w: 2, h: 3 }")
            && output.contains("args=[\"--from-metadata\"] env=from-metadata"),
        "{output}"
    );
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);

    // The panic, through the Rust panics row's function breakpoint on `rust_panic` (warm: lldb-dap ran once).
    let mut panicking = launch.clone();
    panicking.args = vec!["panic".into()];
    let clock = Instant::now();
    let (client, rec, started) = p.launch(&panicking, &p.init, &["call-add"], &["rust_panic"]);
    assert!(
        started.function_breakpoints[0].verified,
        "{:?}",
        started.function_breakpoints
    );
    let s = stopped(&rec, 1);
    eprintln!(
        "timing: launch to the first stopped (warm): {:.0} ms",
        ms(clock.elapsed())
    );
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    // A conditional breakpoint in the loop, then a hit count (lldb-dap: break on the Nth hit, counted from when the
    // breakpoint was set), then none.
    let i_at_stop = |n: usize| {
        let s = stopped(&rec, n);
        assert_eq!(s.reason, "breakpoint");
        let (_, vars) = locals(&client, &frames(&client, tid, 1)[0]["id"]);
        value_of(&vars, "i").1.clone()
    };
    let set_loop = |bp: Value| {
        req(
            &client,
            "setBreakpoints",
            json!({"source": {"path": p.source}, "breakpoints": [bp]}),
        )
    };
    let answer = set_loop(json!({"line": line_of("loop"), "condition": "i == 7"}));
    assert_eq!(answer["breakpoints"][0]["verified"], true);
    req(&client, "continue", json!({"threadId": tid}));
    assert_eq!(i_at_stop(2), "7");
    set_loop(json!({"line": line_of("loop"), "hitCondition": "3"}));
    req(&client, "continue", json!({"threadId": tid}));
    let at = i_at_stop(3);
    eprintln!("hit condition 3 set at i = 7 stopped at i = {at}");
    assert_eq!(at, "10");
    req(
        &client,
        "setBreakpoints",
        json!({"source": {"path": p.source}, "breakpoints": []}),
    );
    req(&client, "continue", json!({"threadId": tid}));
    let s = stopped(&rec, 4);
    let stack = frames(&client, tid, 40);
    let names: Vec<&str> = stack.iter().map(|f| f["name"].as_str().unwrap()).collect();
    eprintln!("panic stop ({}): {names:?}", s.reason);
    // Apple's lldb-dap marks optimized frames ` [opt]`.
    assert!(
        names[0].trim_end_matches(" [opt]").ends_with("rust_panic"),
        "{names:?}"
    );
    let main_ix = names
        .iter()
        .position(|n| *n == "lldbtest::main")
        .expect("the stack runs through main");
    assert_eq!(stack[main_ix]["line"], line_of("panic"));
    let info = client.request_wait("exceptionInfo", json!({"threadId": tid}), T);
    eprintln!("exceptionInfo at the panic: {info:?}");
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });

    // Pause the sleeping program; disconnect ends it.
    let mut sleeping = launch.clone();
    sleeping.args = vec!["sleep".into()];
    let (client, rec, _) = p.launch(&sleeping, &p.init, &[], &[]);
    rec.wait_nth(
        1,
        "sleeping",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("sleeping")),
    );
    let clock = Instant::now();
    req(&client, "pause", json!({"threadId": 0}));
    let s = stopped(&rec, 1);
    eprintln!(
        "timing: pause to stopped: {:.1} ms (reason {}, description {:?}, text {:?})",
        ms(clock.elapsed()),
        s.reason,
        s.description,
        s.text
    );
    // lldb-dap 18 reports the stop a pause causes (SIGSTOP) as an exception stop.
    assert!(s.reason == "pause" || s.reason == "exception", "{s:?}");
    let clock = Instant::now();
    req(&client, "disconnect", json!({"terminateDebuggee": true}));
    rec.wait_nth(1, "closed", |e| matches!(e, ClientEvent::Closed { .. }));
    eprintln!(
        "timing: disconnect to the adapter's exit: {:.1} ms",
        ms(clock.elapsed())
    );

    // Without the formatters: what LLDB alone shows.
    let init: Vec<String> = vec![cargo::STEP_AVOID.to_owned()];
    let (client, rec, _) = p.launch(&launch, &init, &["call-add"], &[]);
    let s = stopped(&rec, 1);
    let stack = frames(&client, s.thread_id.unwrap(), 1);
    let (_, raw) = locals(&client, &stack[0]["id"]);
    for name in ["text", "v", "o", "shape"] {
        eprintln!(
            "without the formatters: {name} = {}",
            value_of(&raw, name).1
        );
    }
    assert_ne!(value_of(&raw, "text").1, "\"hello\"");
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
}

#[test]
fn lldb_dap_debugs_the_package_test_executable_with_a_filter() {
    let Some(p) = setup() else { return };
    let mut start = p.start.clone();
    start.test = true;
    start.args = Some(vec!["my_test".into()]);
    let clock = Instant::now();
    let mut lines = Vec::new();
    // Full debug information: the package's `[profile.dev]` says `debug = 2`.
    let artifacts = start
        .build_tests(&p.cargo, |l| lines.push(l.to_owned()))
        .unwrap();
    eprintln!(
        "timing: cargo test --no-run: {:.0} ms; first line {:?}",
        ms(clock.elapsed()),
        lines.first()
    );
    let launch = start.test_launch(&artifacts).unwrap();
    eprintln!(
        "test executable: {} {:?}",
        launch.program.display(),
        launch.args
    );
    assert!(
        launch
            .program
            .starts_with(p.start.root.join("target/debug/deps"))
    );
    assert_eq!(launch.args, ["my_test", "--nocapture"]);
    let (client, rec, _) = p.launch(&launch, &p.init, &["my-test"], &[]);
    let s = stopped(&rec, 1);
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    let stack = frames(&client, tid, 5);
    eprintln!("test stop: {}", stack[0]["name"]);
    assert_eq!(stack[0]["name"], "lldbtest::tests::my_test");
    assert_eq!(stack[0]["line"], line_of("my-test"));
    req(&client, "continue", json!({"threadId": tid}));
    let exited = rec.wait_nth(1, "exited", |e| {
        matches!(e, ClientEvent::Event(Event::Exited(_)))
    });
    let output: String = rec
        .events()
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Event(Event::Output(o)) if o.category.as_deref() == Some("stdout") => {
                Some(o.output.clone())
            }
            _ => None,
        })
        .collect();
    eprintln!("test output: {output:?}");
    assert!(matches!(exited, ClientEvent::Event(Event::Exited(x)) if x.exit_code == 0));
    assert!(
        output.contains("1 passed") && output.contains("1 filtered out"),
        "{output}"
    );
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
}

/// Brief 0027: lldb-dap attaches to a running native process by `pid` (the attach plan for runtime `native`), pauses it
/// and detaches; the process keeps running. A copy of `sleep` stands in for the program (no build needed).
#[cfg(unix)]
#[test]
fn lldb_dap_attaches_to_a_running_process_by_pid_and_detaches() {
    use eludite_dap::attach::{AttachAdapter, attach_plan};
    use eludite_dap::processes::{self, Runtime};
    let Ok(adapter) = (LldbSearch {
        configured: std::env::var_os("ELUDITE_LLDB_DAP")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        ..LldbSearch::from_env()
    })
    .find(Platform::current()) else {
        eludite_test_support::skip("lldb-dap", "no lldb-dap");
        return;
    };
    let mut child = std::process::Command::new("sleep")
        .arg("60")
        .spawn()
        .expect("sleep");
    let pid = child.id();
    let listed = processes::list()
        .unwrap()
        .into_iter()
        .find(|p| p.pid == pid)
        .unwrap();
    assert_eq!(listed.runtime, Runtime::Native);
    let plan = attach_plan(
        AttachAdapter::for_runtime(listed.runtime).unwrap(),
        pid,
        None,
        Platform::current(),
    )
    .unwrap();
    let rec = Recorder::default();
    let clock = Instant::now();
    let client = DapClient::start(
        common::recorded(
            transport::connect(&adapter.transport()).unwrap(),
            "lldb",
            &adapter.describe(adapter.version().as_deref()),
            None,
        ),
        rec.sink(),
    );
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: plan.adapter_id.into(),
            kind: StartKind::Attach,
            arguments: plan.arguments,
            breakpoints: Vec::new(),
            exception_filters: Vec::new(),
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        },
        T,
    );
    let started = match started {
        Ok(s) => s,
        Err(e) => {
            let _ = child.kill();
            let _ = child.wait();
            // Attaching needs ptrace; a container may forbid it.
            eprintln!("skipped: lldb-dap could not attach to process {pid}: {e}");
            return;
        }
    };
    eprintln!(
        "timing: lldb-dap attach to a running process: {:.0} ms",
        ms(clock.elapsed())
    );
    eprintln!(
        "capabilities: restart {}",
        started.capabilities.supports_restart_request
    );
    // lldb-dap stops the process to attach and resumes it after `configurationDone`; a pause that arrives before the
    // resume is lost, so pause until a stop is reported.
    let deadline = Instant::now() + eludite_test_support::hang_bound(T);
    let s = loop {
        let _ = client.request_wait("pause", json!({"threadId": 0}), T);
        let wait = Instant::now() + Duration::from_millis(1500);
        let found = loop {
            let s = rec.events().into_iter().find_map(|e| match e {
                ClientEvent::Event(Event::Stopped(s)) => Some(s),
                _ => None,
            });
            if s.is_some() || Instant::now() > wait {
                break s;
            }
            std::thread::sleep(Duration::from_millis(10));
        };
        if let Some(s) = found {
            break s;
        }
        assert!(Instant::now() < deadline, "no stop after pause");
    };
    assert!(s.thread_id.is_some());
    client
        .request_wait("disconnect", json!({"terminateDebuggee": false}), T)
        .unwrap();
    std::thread::sleep(Duration::from_millis(200));
    assert!(
        child.try_wait().unwrap().is_none(),
        "the process keeps running after the debugger detached"
    );
    assert!(processes::alive(pid));
    client.kill();
    let _ = child.kill();
    let _ = child.wait();
}
