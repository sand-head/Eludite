//! A small ACP client for the tests: the adapter binary on pipes, newline-delimited JSON-RPC, every message the
//! adapter sends kept with the time it arrived.

#![allow(dead_code)]

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const ADAPTER: &str = env!("CARGO_BIN_EXE_eludite-openai-acp");
pub const RELAY: &str = env!("CARGO_BIN_EXE_eludite-openai-fake-relay");
pub const T: Duration = Duration::from_secs(20);

pub struct Adapter {
    child: Child,
    stdin: ChildStdin,
    rx: Receiver<(Value, Instant)>,
    next_id: i64,
    /// Every message received so far that was not a response we waited for.
    pub seen: Vec<(Value, Instant)>,
}

impl Drop for Adapter {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

/// A fresh directory for a test.
pub fn temp_dir(name: &str) -> std::path::PathBuf {
    let d = std::env::temp_dir().join(format!(
        "eludite-openai-acp-test-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

impl Adapter {
    /// Start the adapter with `args`; `env` is added, and `OPENAI_API_KEY` and the log variable are cleared unless
    /// given.
    pub fn spawn(args: &[&str], env: &[(&str, &str)]) -> Self {
        let mut cmd = Command::new(ADAPTER);
        cmd.args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .env_remove("OPENAI_API_KEY")
            .env_remove("ELUDITE_OPENAI_API_KEY")
            .env_remove("ELUDITE_OPENAI_ACP_LOG");
        for (k, v) in env {
            cmd.env(k, v);
        }
        let mut child = cmd.spawn().expect("spawn the adapter");
        let stdin = child.stdin.take().unwrap();
        let stdout = child.stdout.take().unwrap();
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                let v: Value = serde_json::from_str(&line)
                    .unwrap_or_else(|e| panic!("stdout carries JSON-RPC only: {e}: {line}"));
                if tx.send((v, Instant::now())).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            stdin,
            rx,
            next_id: 1,
            seen: Vec::new(),
        }
    }

    pub fn send(&mut self, method: &str, params: Value) -> i64 {
        let id = self.next_id;
        self.next_id += 1;
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
        id
    }

    pub fn notify(&mut self, method: &str, params: Value) {
        let msg = json!({"jsonrpc": "2.0", "method": method, "params": params});
        writeln!(self.stdin, "{msg}").unwrap();
        self.stdin.flush().unwrap();
    }

    /// Wait for the response to `id`; other messages are kept in `seen`.
    pub fn wait(&mut self, id: i64) -> (Value, Instant) {
        let deadline = Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (v, at) = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("no response to {id}; seen {:#?}", self.updates()));
            if v.get("id") == Some(&json!(id)) && v.get("method").is_none() {
                return (v, at);
            }
            assert!(
                v.get("method") != Some(&json!("session/request_permission")),
                "the adapter never asks for permission"
            );
            self.seen.push((v, at));
        }
    }

    pub fn call(&mut self, method: &str, params: Value) -> Value {
        let id = self.send(method, params);
        self.wait(id).0
    }

    /// Read messages (kept in `seen`) until one satisfies `pred`; returns when it arrived.
    pub fn wait_for(&mut self, pred: impl Fn(&Value) -> bool) -> Instant {
        let deadline = Instant::now() + T;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let (v, at) = self
                .rx
                .recv_timeout(left)
                .unwrap_or_else(|_| panic!("timed out; seen {:#?}", self.updates()));
            let hit = pred(&v);
            self.seen.push((v, at));
            if hit {
                return at;
            }
        }
    }

    /// Messages arriving within `d`.
    pub fn drain(&mut self, d: Duration) {
        while let Ok(m) = self.rx.recv_timeout(d) {
            self.seen.push(m);
        }
    }

    pub fn initialize(&mut self) -> Value {
        let r = self.call(
            "initialize",
            json!({"protocolVersion": 1, "clientCapabilities": {}, "clientInfo": {"name": "test", "version": "0"}}),
        );
        r["result"].clone()
    }

    /// `session/new`; the whole response.
    pub fn new_session(&mut self, cwd: &std::path::Path, mcp: Vec<Value>) -> Value {
        self.call(
            "session/new",
            json!({"cwd": cwd.to_string_lossy(), "mcpServers": mcp}),
        )
    }

    /// `initialize` and `session/new`; the session id.
    pub fn start(&mut self, cwd: &std::path::Path, mcp: Vec<Value>) -> String {
        self.initialize();
        let r = self.new_session(cwd, mcp);
        r["result"]["sessionId"]
            .as_str()
            .unwrap_or_else(|| panic!("session/new failed: {r}"))
            .to_owned()
    }

    pub fn prompt(&mut self, session: &str, text: &str) -> Value {
        self.call(
            "session/prompt",
            json!({"sessionId": session, "prompt": [{"type": "text", "text": text}]}),
        )
    }

    /// The `session/update` payloads seen so far.
    pub fn updates(&self) -> Vec<Value> {
        self.seen
            .iter()
            .filter(|(v, _)| v["method"] == "session/update")
            .map(|(v, _)| v["params"]["update"].clone())
            .collect()
    }

    pub fn updates_of(&self, kind: &str) -> Vec<Value> {
        self.updates()
            .into_iter()
            .filter(|u| u["sessionUpdate"] == kind)
            .collect()
    }

    /// The agent's message text so far.
    pub fn message_text(&self) -> String {
        self.updates_of("agent_message_chunk")
            .iter()
            .filter_map(|u| u["content"]["text"].as_str().map(str::to_owned))
            .collect()
    }

    pub fn clear(&mut self) {
        self.seen.clear();
    }
}

/// The stop reason of a `session/prompt` response.
pub fn stop_reason(r: &Value) -> &str {
    r["result"]["stopReason"]
        .as_str()
        .unwrap_or_else(|| panic!("not a stop: {r}"))
}
