//! A fake of Eludite's MCP endpoint for the adapter's tests: the same wire as `crates/mcp`'s local endpoint (a TCP
//! listener on loopback, the token as the first line, then newline-delimited JSON-RPC 2.0, each `tools/call` on its
//! own thread, `notifications/tools/list_changed`), reached through `eludite-openai-fake-relay --mcp-relay ADDR` as
//! the adapter would reach `eludite --mcp-relay ADDR`. The same listener answers streamable HTTP (`POST /mcp`). The
//! client side is also proven against `crates/mcp`'s real server in `tests/mcp_real.rs`.

#![allow(dead_code)]

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

pub const TOKEN: &str = "fake-mcp-token-0123456789abcdef";

/// One `tools/call`.
#[derive(Debug, Clone)]
pub struct Call {
    pub name: String,
    pub arguments: Value,
    pub tool_call_id: Option<String>,
    pub at: Instant,
}

type Handler = Arc<dyn Fn(&str, &Value) -> Value + Send + Sync>;

#[derive(Default)]
struct State {
    tools: Vec<Value>,
    guides: HashMap<String, String>,
    calls: Vec<Call>,
    handler: Option<Handler>,
    delays: HashMap<String, u64>,
    writers: Vec<Arc<Mutex<TcpStream>>>,
    lists: usize,
}

pub struct FakeMcp {
    pub addr: String,
    state: Arc<Mutex<State>>,
}

/// A tool as Eludite's server lists it: `eludite.file.read` becomes `eludite-file-read`.
pub fn tool(command: &str, permission: &str) -> Value {
    json!({
        "name": command.replace('.', "-"),
        "title": command,
        "description": format!("Runs {command}. It does what the IDE's command does."),
        "inputSchema": {"$schema": "https://json-schema.org/draft/2020-12/schema", "title": format!("{command} input"),
            "type": "object", "properties": {"path": {"type": "string"}}, "additionalProperties": true},
        "outputSchema": {"type": "object"},
        "annotations": {"readOnlyHint": permission == "read", "destructiveHint": permission == "dangerous"},
        "_meta": {"eludite/command": command, "eludite/permission": permission},
    })
}

/// The core set's commands, as the IDE has them now (`build.solution`, no `build.workspace` yet).
pub const CORE_PRESENT: &[(&str, &str)] = &[
    ("eludite.file.read", "read"),
    ("eludite.file.edit", "edit_buffer"),
    ("eludite.file.open", "read"),
    ("eludite.workspace.tree", "read"),
    ("eludite.workspace.apply_edit", "edit_buffer"),
    ("eludite.search.find", "read"),
    ("eludite.search.replace", "edit_buffer"),
    ("diagnostics.list", "read"),
    ("eludite.build.solution", "execute"),
    ("eludite.build.project", "execute"),
    ("eludite.output.show", "read"),
    ("eludite.test.discover", "execute"),
    ("eludite.test.run", "execute"),
    ("eludite.test.results", "read"),
    ("eludite.terminal.open", "execute"),
    ("eludite.terminal.send", "execute"),
    ("eludite.terminal.read", "read"),
    ("eludite.terminal.wait", "read"),
    ("eludite.git.status", "read"),
    ("eludite.git.diff", "read"),
    ("eludite.git.stage", "edit_buffer"),
    ("eludite.git.commit", "execute"),
    ("eludite.git.log", "read"),
    ("eludite.editor.go_to_definition", "read"),
    ("eludite.editor.find_references", "read"),
    ("eludite.editor.hover", "read"),
    ("eludite.debug.start", "execute"),
    ("eludite.debug.snapshot", "read"),
    ("eludite.debug.toggle_breakpoint", "execute"),
    ("eludite.debug.wait", "read"),
];

/// Commands outside the core set.
pub const EXTRA: &[(&str, &str)] = &[
    ("eludite.debug.evaluate", "execute"),
    ("eludite.debug.step_over", "execute"),
    ("eludite.git.push", "dangerous"),
    ("eludite.nuget.search", "read"),
    ("eludite.browser.navigate", "execute"),
    ("eludite.help.about", "read"),
];

/// The core commands plus a few others.
pub fn eludite_tools() -> Vec<Value> {
    CORE_PRESENT
        .iter()
        .chain(EXTRA)
        .map(|(c, p)| tool(c, p))
        .collect()
}

impl FakeMcp {
    pub fn start(tools: Vec<Value>) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let state: Arc<Mutex<State>> = Arc::new(Mutex::new(State {
            tools,
            ..State::default()
        }));
        let s = state.clone();
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let s = s.clone();
                std::thread::spawn(move || {
                    let _ = serve(conn, &s);
                });
            }
        });
        Self { addr, state }
    }

    /// `session/new`'s `mcpServers` entry for the stdio relay `relay` (`eludite-openai-fake-relay`).
    pub fn stdio_server(&self, relay: &str) -> Value {
        json!({"name": "eludite", "command": relay, "args": ["--mcp-relay", self.addr],
            "env": [{"name": "ELUDITE_MCP_TOKEN", "value": TOKEN}]})
    }

    /// The same endpoint as a streamable HTTP server.
    pub fn http_server(&self) -> Value {
        json!({"type": "http", "name": "eludite", "url": format!("http://{}/mcp", self.addr), "headers": []})
    }

    pub fn calls(&self) -> Vec<Call> {
        self.state.lock().unwrap().calls.clone()
    }

    pub fn lists(&self) -> usize {
        self.state.lock().unwrap().lists
    }

    /// What `tools/call` answers (default: a text naming the tool and its arguments).
    pub fn set_handler(&self, f: impl Fn(&str, &Value) -> Value + Send + Sync + 'static) {
        self.state.lock().unwrap().handler = Some(Arc::new(f));
    }

    /// Make `tool`'s calls take `ms`.
    pub fn set_delay(&self, tool: &str, ms: u64) {
        self.state.lock().unwrap().delays.insert(tool.into(), ms);
    }

    pub fn set_guide(&self, uri: &str, text: &str) {
        self.state
            .lock()
            .unwrap()
            .guides
            .insert(uri.into(), text.into());
    }

    /// Add a tool and tell every connection the list changed.
    pub fn add_tool(&self, t: Value) {
        let writers = {
            let mut s = self.state.lock().unwrap();
            s.tools.push(t);
            s.writers.clone()
        };
        for w in writers {
            let mut w = w.lock().unwrap();
            let _ = w.write_all(
                b"{\"jsonrpc\":\"2.0\",\"method\":\"notifications/tools/list_changed\"}\n",
            );
            let _ = w.flush();
        }
    }
}

/// A text result.
pub fn text_result(text: &str, is_error: bool) -> Value {
    json!({"content": [{"type": "text", "text": text}], "isError": is_error})
}

/// The answer to one JSON-RPC request (`None` for notifications).
fn answer(msg: &Value, state: &Mutex<State>) -> Option<Value> {
    let id = msg.get("id")?.clone();
    let params = msg.get("params").cloned().unwrap_or(Value::Null);
    let result = match msg["method"].as_str().unwrap_or("") {
        "initialize" => json!({"protocolVersion": params["protocolVersion"],
            "capabilities": {"tools": {"listChanged": true}, "resources": {}},
            "serverInfo": {"name": "eludite", "title": "Eludite", "version": "0"}}),
        "tools/list" => {
            let mut s = state.lock().unwrap();
            s.lists += 1;
            json!({"tools": s.tools})
        }
        "resources/read" => {
            let uri = params["uri"].as_str().unwrap_or("");
            match state.lock().unwrap().guides.get(uri) {
                Some(t) => {
                    json!({"contents": [{"uri": uri, "mimeType": "text/markdown", "text": t}]})
                }
                None => {
                    return Some(json!({"jsonrpc": "2.0", "id": id,
                        "error": {"code": -32002, "message": format!("resource not found: {uri}")}}));
                }
            }
        }
        "tools/call" => {
            let name = params["name"].as_str().unwrap_or("").to_owned();
            let args = params.get("arguments").cloned().unwrap_or(json!({}));
            let (handler, delay) = {
                let mut s = state.lock().unwrap();
                s.calls.push(Call {
                    name: name.clone(),
                    arguments: args.clone(),
                    tool_call_id: params
                        .pointer("/_meta/eludite~1toolCallId")
                        .and_then(Value::as_str)
                        .map(str::to_owned),
                    at: Instant::now(),
                });
                (s.handler.clone(), s.delays.get(&name).copied())
            };
            if let Some(ms) = delay {
                std::thread::sleep(Duration::from_millis(ms));
            }
            match handler {
                Some(h) => h(&name, &args),
                None => text_result(&format!("{name} ran with {args}"), false),
            }
        }
        "ping" => json!({}),
        other => {
            return Some(json!({"jsonrpc": "2.0", "id": id,
                "error": {"code": -32601, "message": format!("method not found: {other}")}}));
        }
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn serve(conn: TcpStream, state: &Arc<Mutex<State>>) -> std::io::Result<()> {
    let _ = conn.set_nodelay(true);
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut first = String::new();
    reader.read_line(&mut first)?;
    if first.starts_with("POST ") {
        return serve_http(first, reader, conn, state);
    }
    if first.trim_end() != TOKEN {
        return Ok(());
    }
    let writer = Arc::new(Mutex::new(conn));
    state.lock().unwrap().writers.push(writer.clone());
    for line in reader.lines() {
        let line = line?;
        let Ok(msg) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (state, writer) = (state.clone(), writer.clone());
        let run = move || {
            if let Some(reply) = answer(&msg, &state) {
                let mut w = writer.lock().unwrap();
                let _ = w.write_all(format!("{reply}\n").as_bytes());
                let _ = w.flush();
            }
        };
        if line.contains("\"tools/call\"") {
            std::thread::spawn(run);
        } else {
            run();
        }
    }
    Ok(())
}

fn serve_http(
    mut first: String,
    mut reader: BufReader<TcpStream>,
    mut w: TcpStream,
    state: &Mutex<State>,
) -> std::io::Result<()> {
    loop {
        let mut len = 0usize;
        loop {
            let mut h = String::new();
            reader.read_line(&mut h)?;
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':')
                && k.eq_ignore_ascii_case("content-length")
            {
                len = v.trim().parse().unwrap_or(0);
            }
        }
        let mut body = vec![0u8; len];
        reader.read_exact(&mut body)?;
        let msg: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        match answer(&msg, state) {
            Some(reply) => {
                let text = reply.to_string();
                write!(
                    w,
                    "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nMcp-Session-Id: s1\r\nContent-Length: {}\r\n\r\n{text}",
                    text.len()
                )?;
            }
            None => write!(w, "HTTP/1.1 202 Accepted\r\nContent-Length: 0\r\n\r\n")?,
        }
        w.flush()?;
        first.clear();
        if reader.read_line(&mut first)? == 0 {
            return Ok(());
        }
    }
}
