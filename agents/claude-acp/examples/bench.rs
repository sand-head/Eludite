//! Budget measurements for brief 0006 (Linux: RSS and CPU from `/proc`).
//!
//! ```text
//! cargo run --release --example bench -- ready ADAPTER [RUNS]
//!     session ready: spawn the adapter, `initialize`, then `session/new`
//!     (which starts `claude` and its handshake); no prompt is sent. Uses
//!     whatever `claude` the adapter discovers (the real one by default).
//! cargo run --release --example bench -- stream ADAPTER FAKE_CLAUDE [CHUNKS] [RATE]
//!     hot path: the fake `claude` streams CHUNKS text deltas at RATE/s; the
//!     adapter's RSS and CPU time are sampled while it forwards them.
//! ```
//! Output is one JSON object per run on stdout.

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

struct Adapter {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Adapter {
    fn spawn(path: &str, env: &[(&str, String)]) -> Self {
        let mut cmd = Command::new(path);
        cmd.stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit());
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn adapter");
        let stdin = child.stdin.take().unwrap();
        let stdout = BufReader::new(child.stdout.take().unwrap());
        Self {
            child,
            stdin,
            stdout,
            next_id: 1,
        }
    }

    fn send(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        id
    }

    /// Read until the response to `id`; `on_other` sees everything else.
    fn wait(&mut self, id: i64, mut on_other: impl FnMut(&Value)) -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "adapter closed"
            );
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id") == Some(&json!(id)) && v.get("method").is_none() {
                return v;
            }
            on_other(&v);
        }
    }

    fn pid(&self) -> u32 {
        self.child.id()
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn status_kib(pid: u32, key: &str) -> Option<u64> {
    std::fs::read_to_string(format!("/proc/{pid}/status"))
        .ok()?
        .lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

/// utime + stime in clock ticks.
fn cpu_ticks(pid: u32) -> Option<u64> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let rest = &stat[stat.rfind(')')? + 2..];
    let f: Vec<&str> = rest.split_whitespace().collect();
    Some(f.get(11)?.parse::<u64>().ok()? + f.get(12)?.parse::<u64>().ok()?)
}

fn ms(d: Duration) -> f64 {
    (d.as_secs_f64() * 1e6).round() / 1e3
}

fn init_params() -> Value {
    json!({"protocolVersion": 1, "clientCapabilities": {"fs": {"readTextFile": false, "writeTextFile": false}, "terminal": false, "auth": {"terminal": true}}, "clientInfo": {"name": "bench", "version": "0"}})
}

fn ready(adapter: &str, runs: usize) {
    let cwd = std::env::temp_dir().join("eludite-claude-acp-bench");
    std::fs::create_dir_all(&cwd).unwrap();
    for run in 0..runs {
        let t0 = Instant::now();
        let mut a = Adapter::spawn(adapter, &[]);
        let t_init_sent = Instant::now();
        let id = a.send("initialize", init_params());
        a.wait(id, |_| {});
        let t_init = t0.elapsed();
        let id = a.send("session/new", json!({"cwd": cwd, "mcpServers": []}));
        let r = a.wait(id, |_| {});
        let t_ready = t0.elapsed();
        let init_to_ready = t_init_sent.elapsed();
        assert!(r.get("result").is_some(), "{r}");
        let out = json!({
            "bench": "ready",
            "run": run,
            "spawn_to_initialized_ms": ms(t_init),
            "initialize_to_session_ready_ms": ms(init_to_ready),
            "spawn_to_session_ready_ms": ms(t_ready),
            "adapter_rss_kib": status_kib(a.pid(), "VmRSS:"),
            "adapter_hwm_kib": status_kib(a.pid(), "VmHWM:"),
        });
        println!("{out}");
    }
}

fn stream(adapter: &str, fake: &str, chunks: usize, rate: f64) {
    let cwd = std::env::temp_dir().join("eludite-claude-acp-bench");
    std::fs::create_dir_all(&cwd).unwrap();
    let env = [
        ("ELUDITE_CLAUDE_PATH", fake.to_owned()),
        ("FAKE_CLAUDE_SCENARIO", "stream".to_owned()),
        ("FAKE_CLAUDE_CHUNKS", chunks.to_string()),
        ("FAKE_CLAUDE_RATE", rate.to_string()),
    ];
    let mut a = Adapter::spawn(adapter, &env);
    let id = a.send("initialize", init_params());
    a.wait(id, |_| {});
    let id = a.send("session/new", json!({"cwd": cwd, "mcpServers": []}));
    let created = a.wait(id, |_| {});
    let sid = created["result"]["sessionId"].clone();
    assert!(sid.is_string(), "{created}");
    let pid = a.pid();
    let rss_before = status_kib(pid, "VmRSS:");
    let cpu_before = cpu_ticks(pid).unwrap_or(0);
    let t0 = Instant::now();
    let id = a.send(
        "session/prompt",
        json!({"sessionId": sid, "prompt": [{"type": "text", "text": "stream"}]}),
    );
    let mut n = 0usize;
    let mut rss_samples = Vec::new();
    let r = a.wait(id, |v| {
        if v["params"]["update"]["sessionUpdate"] == "agent_message_chunk" {
            n += 1;
            if n.is_multiple_of(100) {
                rss_samples.push(status_kib(pid, "VmRSS:").unwrap_or(0));
            }
        }
    });
    let elapsed = t0.elapsed();
    let ticks = cpu_ticks(pid).unwrap_or(0) - cpu_before;
    let out = json!({
        "bench": "stream",
        "chunks_sent": chunks,
        "chunks_received": n,
        "rate_hz": rate,
        "stop": r["result"]["stopReason"],
        "response": if r.get("error").is_some() { r.clone() } else { Value::Null },
        "elapsed_s": elapsed.as_secs_f64(),
        "adapter_cpu_ms": ticks * 10, // USER_HZ = 100 on Linux
        "adapter_cpu_percent": (ticks as f64 * 10.) / (elapsed.as_secs_f64() * 1000.) * 100.,
        "adapter_rss_kib_before": rss_before,
        "adapter_rss_kib_samples_min": rss_samples.iter().min(),
        "adapter_rss_kib_samples_max": rss_samples.iter().max(),
        "adapter_rss_kib_after": status_kib(pid, "VmRSS:"),
        "adapter_hwm_kib": status_kib(pid, "VmHWM:"),
    });
    println!("{out}");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("ready") => ready(&args[1], args.get(2).map_or(5, |n| n.parse().unwrap())),
        Some("stream") => stream(
            &args[1],
            &args[2],
            args.get(3).map_or(2000, |n| n.parse().unwrap()),
            args.get(4).map_or(200., |n| n.parse().unwrap()),
        ),
        _ => eprintln!("see the doc comment in examples/bench.rs"),
    }
}
