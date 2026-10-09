//! The program the client tests debug, a sink that records what the client reports, and the `RECORD_DAP` mode of the
//! real-adapter tests (brief 0033).

#![allow(dead_code)]

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_dap::fake::{FakeProgram, FakeStep, FakeThrow, FakeVar};
use eludite_dap::record::{self, RecordOptions};
use eludite_dap::types::Event;
use eludite_dap::{ClientEvent, Connection, EventSink};

pub const PROGRAM: &str = "/src/App/Program.cs";
pub const CALC: &str = "/src/App/Calc.cs";
/// A hang bound for the real adapters (netcoredbg, Mono, js-debug) as much as the fake: generous, since it only catches a
/// hang, and a cold adapter on a shared CI runner is slow.
pub const T: Duration = Duration::from_secs(30);

#[allow(unused_imports)] // Not every test binary that includes this module times something.
pub use eludite_test_support::assert_budget;

/// `Main` calls `Calc.Add(1, 2)` and keeps the result.
pub fn program() -> FakeProgram {
    let args = || FakeVar::new("args", "{string[0]}", "string[]");
    let order = FakeVar::new("order", "{App.Order}", "App.Order").with_children(vec![
        FakeVar::new("Id", "7", "int"),
        FakeVar::new("Name", "\"A\"", "string"),
    ]);
    let mut throwing = FakeStep::new(PROGRAM, 9, "App.Program.Main()", 0, vec![args()]);
    throwing.throws = Some(FakeThrow {
        exception: "System.InvalidOperationException".into(),
        message: "boom".into(),
        handled: true,
        ..Default::default()
    });
    FakeProgram {
        steps: vec![
            FakeStep::new(PROGRAM, 5, "App.Program.Main()", 0, vec![args()]),
            FakeStep::new(
                PROGRAM,
                6,
                "App.Program.Main()",
                0,
                vec![args(), FakeVar::new("x", "1", "int")],
            ),
            FakeStep::new(
                CALC,
                10,
                "App.Calc.Add(int, int)",
                1,
                vec![FakeVar::new("a", "1", "int"), FakeVar::new("b", "2", "int")],
            ),
            FakeStep::new(
                CALC,
                11,
                "App.Calc.Add(int, int)",
                1,
                vec![
                    FakeVar::new("a", "1", "int"),
                    FakeVar::new("b", "2", "int"),
                    FakeVar::new("sum", "3", "int"),
                ],
            ),
            FakeStep::new(
                PROGRAM,
                7,
                "App.Program.Main()",
                0,
                vec![
                    args(),
                    FakeVar::new("x", "1", "int"),
                    FakeVar::new("y", "3", "int"),
                    order,
                ],
            ),
            FakeStep::new(PROGRAM, 8, "App.Program.Main()", 0, vec![args()]),
            throwing,
        ],
        other_threads: vec![(2, ".NET TP Worker".into())],
        output_at_start: vec!["listening\n".into()],
        ..FakeProgram::default()
    }
}

/// Records every client event.
#[derive(Clone, Default)]
pub struct Recorder(pub Arc<Mutex<Vec<ClientEvent>>>);

impl Recorder {
    pub fn sink(&self) -> EventSink {
        let r = self.0.clone();
        Arc::new(move |e| r.lock().unwrap().push(e))
    }

    pub fn events(&self) -> Vec<ClientEvent> {
        self.0.lock().unwrap().clone()
    }

    /// Wait for the `n`th (1-based) event matching `pred` and return it.
    pub fn wait_nth(
        &self,
        n: usize,
        what: &str,
        pred: impl Fn(&ClientEvent) -> bool,
    ) -> ClientEvent {
        let deadline = Instant::now() + eludite_test_support::hang_bound(T);
        loop {
            let found: Vec<ClientEvent> = self.events().into_iter().filter(|e| pred(e)).collect();
            if found.len() >= n {
                return found[n - 1].clone();
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what} #{n}"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    pub fn stopped(&self, n: usize) -> eludite_dap::types::StoppedEvent {
        match self.wait_nth(n, "stopped", |e| {
            matches!(e, ClientEvent::Event(Event::Stopped(_)))
        }) {
            ClientEvent::Event(Event::Stopped(s)) => s,
            _ => unreachable!(),
        }
    }

    pub fn closed(&self) -> ClientEvent {
        self.wait_nth(1, "closed", |e| matches!(e, ClientEvent::Closed { .. }))
    }
}

/// A tracepoint emulated as the shell does where the adapter has no log points (brief 0026): at each of `hits` stops,
/// `threads` and `stackTrace` (as the shell reads a stop), `evaluate` of `expression` in the top frame, then `continue`.
/// Returns each hit's evaluated value and the time from the `stopped` event's arrival to the `continue` answer.
pub fn emulate_tracepoint(
    client: &eludite_dap::DapClient,
    rec: &Recorder,
    hits: usize,
    expression: &str,
) -> Vec<(String, Duration)> {
    let mut out = Vec::new();
    for n in 1..=hits {
        let s = rec.stopped(n);
        let seen = Instant::now();
        let tid = s.thread_id.unwrap_or(0);
        let threads = client
            .request_channel("threads", serde_json::Value::Null)
            .unwrap();
        let st = client
            .request_wait(
                "stackTrace",
                serde_json::json!({"threadId": tid, "startFrame": 0, "levels": 200}),
                T,
            )
            .unwrap();
        let _ = threads.recv_timeout(T);
        let frame = st["stackFrames"][0]["id"].clone();
        let value = client
            .request_wait(
                "evaluate",
                serde_json::json!({"expression": expression, "frameId": frame, "context": "watch"}),
                T,
            )
            .map(|e| e["result"].as_str().unwrap_or_default().to_owned())
            .unwrap_or_else(|e| format!("{{{expression}: {e}}}"));
        client
            .request_wait("continue", serde_json::json!({"threadId": tid}), T)
            .unwrap();
        out.push((value, seen.elapsed()));
    }
    out
}

/// The mean and the p95 of `times`, in milliseconds.
pub fn mean_p95(times: &[Duration]) -> (f64, f64) {
    let mut v: Vec<f64> = times.iter().map(|d| d.as_secs_f64() * 1e3).collect();
    v.sort_by(f64::total_cmp);
    let mean = v.iter().sum::<f64>() / v.len().max(1) as f64;
    let p95 = v
        .get((v.len() * 95).div_ceil(100).saturating_sub(1))
        .copied()
        .unwrap_or_default();
    (mean, p95)
}

/// The repository's root.
pub fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    std::fs::canonicalize(&root).unwrap_or(root)
}

/// `RECORD_DAP`: the folder the real-adapter tests write their sessions to, when set.
pub fn record_dir() -> Option<PathBuf> {
    std::env::var_os("RECORD_DAP")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

static SESSIONS: Mutex<Option<HashMap<String, usize>>> = Mutex::new(None);

/// With `RECORD_DAP=<dir>`, record the session on `connection` to `<dir>/<adapter>/client/<test>.dap.json` (the
/// test's name from its thread; `-2`, `-3`, ... for its later sessions), scrubbed with `${ROOT}` (the repository),
/// `tmp` as `${TMP}` (the test's temporary folder) and `${HOME}` (the user's home). Without it, `connection` as it is.
pub fn recorded(
    connection: Connection,
    adapter: &str,
    version: &str,
    tmp: Option<&std::path::Path>,
) -> Connection {
    let Some(dir) = record_dir() else {
        return connection;
    };
    let test = std::thread::current()
        .name()
        .unwrap_or("session")
        .rsplit("::")
        .next()
        .unwrap_or("session")
        .to_owned();
    let n = {
        let mut m = SESSIONS.lock().unwrap();
        let c = m
            .get_or_insert_with(HashMap::new)
            .entry(test.clone())
            .or_default();
        *c += 1;
        *c
    };
    let name = if n == 1 { test } else { format!("{test}-{n}") };
    let mut roots = vec![("${ROOT}".to_owned(), repo_root())];
    if let Some(t) = tmp {
        roots.push(("${TMP}".to_owned(), t.to_path_buf()));
    }
    // The toolchain's and the user's own paths (the Rust sysroot under ~/.rustup).
    if let Some(home) = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE")) {
        roots.push(("${HOME}".to_owned(), PathBuf::from(home)));
    }
    let path = dir
        .join(adapter)
        .join("client")
        .join(format!("{name}.dap.json"));
    eprintln!("recording: {}", path.display());
    let (connection, _handle) = record::record(
        connection,
        RecordOptions {
            path,
            adapter: adapter.to_owned(),
            version: version.to_owned(),
            roots,
        },
    );
    connection
}
