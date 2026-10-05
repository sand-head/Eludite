//! Budget measurements for brief 0059 (Linux: RSS and threads from `/proc`).
//!
//! ```text
//! cargo build --release
//! cargo run --release --example bench -- target/release/eludite-openai-acp target/release/eludite-openai-fake-relay [TURNS]
//! ```
//!
//! Against the tests' loopback fake server and fake MCP endpoint (behind the relay), it measures:
//! - `first_chunk_ms`: the fake server's first SSE byte to the first `agent_message_chunk` on the adapter's stdout;
//! - `hop_ms`: a response's last byte to the next request's first byte, without the MCP round trip (the time from
//!   the last byte to the MCP server receiving `tools/call`, plus the time from its reply to the next request; both
//!   include one relay hop, so this is an upper bound);
//! - `map_ms`: 187 MCP tools mapped and a request body built and serialized, in process;
//! - `core_request_tokens`: the `--tools core` request's estimated tokens (bytes / 4) before any history;
//! - `rss_mb` and `threads` of the adapter after 100 requests (two turns of 50, the per-turn cap);
//! - `binary_mb`: the adapter's size on disk.
//!
//! Output is one JSON object per measurement on stdout.

#[path = "../tests/fake_mcp.rs"]
mod fake_mcp;
#[path = "../tests/fake_server.rs"]
mod fake_server;

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use eludite_openai_acp::mcp::{McpTool, ToolSet, ToolsMode};
use eludite_openai_acp::provider::{RequestLimits, request_body};
use fake_mcp::{FakeMcp, eludite_tools, text_result};
use fake_server::{Ev, FakeServer, Reply, finish, text, tool_frag, usage};
use serde_json::{Value, json};

struct Adapter {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
    next_id: i64,
}

impl Adapter {
    fn spawn(path: &str, args: &[&str]) -> Self {
        let mut child = Command::new(path)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn adapter");
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
        writeln!(
            self.stdin,
            "{}",
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params})
        )
        .unwrap();
        self.stdin.flush().unwrap();
        id
    }

    /// Read until the response to `id`; `on_other` sees everything else with its arrival time.
    fn wait(&mut self, id: i64, mut on_other: impl FnMut(&Value, Instant)) -> Value {
        let mut line = String::new();
        loop {
            line.clear();
            assert!(
                self.stdout.read_line(&mut line).unwrap() > 0,
                "adapter closed"
            );
            let at = Instant::now();
            let v: Value = serde_json::from_str(&line).unwrap();
            if v.get("id") == Some(&json!(id)) && v.get("method").is_none() {
                return v;
            }
            on_other(&v, at);
        }
    }
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn proc_status(pid: u32, key: &str) -> Option<u64> {
    let s = std::fs::read_to_string(format!("/proc/{pid}/status")).ok()?;
    s.lines()
        .find(|l| l.starts_with(key))?
        .split_whitespace()
        .nth(1)?
        .parse()
        .ok()
}

fn percentiles(mut v: Vec<f64>) -> Value {
    v.sort_by(|a, b| a.total_cmp(b));
    let at = |p: f64| v[((v.len() as f64 - 1.0) * p).round() as usize];
    json!({"n": v.len(), "p50": at(0.5), "p95": at(0.95), "max": at(1.0)})
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let abs = |p: &String| {
        std::fs::canonicalize(p)
            .unwrap_or_else(|e| panic!("{p}: {e}"))
            .to_string_lossy()
            .into_owned()
    };
    let adapter = abs(args.first().expect("ADAPTER path"));
    let relay = abs(args.get(1).expect("RELAY path"));
    let turns: usize = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(50);

    // In process: 187 tools mapped and the body built.
    let tools: Vec<McpTool> = (0..187)
        .map(|i| McpTool {
            name: format!("eludite-tool-{i}"),
            description: "A command with a long enough description to be realistic. ".repeat(3),
            input_schema: json!({"type": "object", "properties": {"path": {"type": "string"}, "line": {"type": "integer", "minimum": 1},
                "options": {"type": "object", "properties": {"a": {"type": "boolean"}, "b": {"enum": ["x", "y"]}}}}, "required": ["path"]}),
            command: Some(format!("eludite.tool.t{i}")),
            permission: Some("read".into()),
        })
        .collect();
    let mut map = Vec::new();
    for _ in 0..50 {
        let t0 = Instant::now();
        let mut set = ToolSet::new(ToolsMode::All);
        set.set(&["eludite".into()], vec![tools.clone()]);
        let body = request_body(
            "m",
            &[json!({"role": "system", "content": "x"})],
            &set.function_tools(),
            &RequestLimits::default(),
        );
        let text = body.to_string();
        map.push(t0.elapsed().as_secs_f64() * 1000.0);
        assert!(text.len() > 1000);
    }
    println!(
        "{}",
        json!({"measure": "map_ms", "tools": 187, "stats": percentiles(map)})
    );

    let server = FakeServer::start();
    server.llama_models(&["m"], 1_000_000);
    let mcp = FakeMcp::start(eludite_tools());
    let replied: Arc<Mutex<Vec<Instant>>> = Arc::default();
    let r2 = replied.clone();
    mcp.set_handler(move |_, _| {
        r2.lock().unwrap().push(Instant::now());
        text_result("ok", false)
    });
    let mut a = Adapter::spawn(&adapter, &["--base-url", &server.url]);
    let pid = a.child.id();
    let id = a.send(
        "initialize",
        json!({"protocolVersion": 1, "clientCapabilities": {}}),
    );
    a.wait(id, |_, _| {});
    let cwd = std::env::temp_dir();
    let id = a.send(
        "session/new",
        json!({"cwd": cwd, "mcpServers": [mcp.stdio_server(&relay)]}),
    );
    let session = a.wait(id, |_, _| {})["result"]["sessionId"]
        .as_str()
        .unwrap()
        .to_owned();

    // First chunk latency: text turns.
    let mut first = Vec::new();
    for _ in 0..turns {
        server.push(Reply::sse(vec![
            Ev::Delay(5),
            text("Hello"),
            text(" there"),
            finish("stop"),
            usage(10, 2),
        ]));
        let before = server.requests().len();
        let id = a.send(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": "hi"}]}),
        );
        let mut chunk_at = None;
        a.wait(id, |v, at| {
            if chunk_at.is_none() && v["params"]["update"]["sessionUpdate"] == "agent_message_chunk"
            {
                chunk_at = Some(at);
            }
        });
        let req = server.requests()[before].at;
        // The server writes its first byte 5 ms after it read the request.
        let first_byte = req + Duration::from_millis(5);
        first.push(
            chunk_at
                .unwrap()
                .saturating_duration_since(first_byte)
                .as_secs_f64()
                * 1000.0,
        );
    }
    println!(
        "{}",
        json!({"measure": "first_chunk_ms", "stats": percentiles(first)})
    );
    let core = server.chats()[0].to_string().len().div_ceil(4);
    println!(
        "{}",
        json!({"measure": "core_request_tokens", "estimate": core})
    );

    // The hop and memory: two turns of 50 requests, each answer a tool call.
    server.set_fallback(Reply::sse(vec![
        tool_frag(0, Some(""), Some("diagnostics-list"), "{}"),
        finish("tool_calls"),
        usage(10, 2),
    ]));
    let start = server.chat_requests().len();
    let calls_before = mcp.calls().len();
    for _ in 0..2 {
        let id = a.send(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": "loop"}]}),
        );
        let r = a.wait(id, |_, _| {});
        assert_eq!(r["result"]["stopReason"], "max_turn_requests", "{r}");
    }
    let reqs = server.chat_requests();
    let calls = mcp.calls();
    let replies = replied.lock().unwrap().clone();
    let mut hops = Vec::new();
    for k in 0..49 {
        // Request k's answer ended about when its last byte went out; we only know when the request came in, so
        // the hop is measured from the MCP call's arrival back to the last byte by the time the stream takes.
        let req = &reqs[start + k];
        let next = &reqs[start + k + 1];
        let call = &calls[calls_before + k];
        let reply = replies[calls_before + k];
        let before = call.at.saturating_duration_since(req.at);
        let after = next.at.saturating_duration_since(reply);
        hops.push((before + after).as_secs_f64() * 1000.0);
    }
    println!(
        "{}",
        json!({"measure": "hop_ms", "note": "request arrival to tools/call arrival, plus MCP reply to the next request; includes the stream itself and two relay hops (an upper bound)", "stats": percentiles(hops)})
    );
    std::thread::sleep(Duration::from_millis(200));
    println!(
        "{}",
        json!({"measure": "after_100_requests", "rss_mb": proc_status(pid, "VmRSS:").map(|k| k as f64 / 1024.0),
            "threads": proc_status(pid, "Threads:")})
    );
    let size = std::fs::metadata(&adapter).map(|m| m.len()).unwrap_or(0);
    println!(
        "{}",
        json!({"measure": "binary_mb", "bytes": size, "mb": size as f64 / 1_048_576.0})
    );
}
