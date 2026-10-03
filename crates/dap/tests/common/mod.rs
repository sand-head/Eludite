//! The program the client tests debug, and a sink that records what the client reports.

#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_dap::fake::{FakeProgram, FakeStep, FakeThrow, FakeVar};
use eludite_dap::types::Event;
use eludite_dap::{ClientEvent, EventSink};

pub const PROGRAM: &str = "/src/App/Program.cs";
pub const CALC: &str = "/src/App/Calc.cs";
pub const T: Duration = Duration::from_secs(10);

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
        let deadline = Instant::now() + T;
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
