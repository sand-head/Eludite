//! A scripted generic language server in this process (feature `fake`, brief 0019): plain LSP 3.17 as
//! rust-analyzer speaks it, for tests of [`crate::ServerClient`] and of the shell.
//!
//! [`FakeServer::connector`] plugs it into [`crate::ServerClient::start_in_process`]. It answers `initialize` with
//! [`FakeServer::set_capabilities`]' capabilities and `serverInfo` `fake-analyzer`; on `initialized` it reports
//! indexing as work-done progress (`$/progress` begin, report, end) and then `experimental/serverStatus`
//! (quiescent, unless [`FakeServer::set_quiescent_after_init`] holds it busy). Every message it receives is
//! recorded ([`FakeServer::received`], [`FakeServer::wait_for`]). Requests answer `null` unless scripted with
//! [`FakeServer::respond`]; [`FakeReply::Hold`] keeps one in flight until `$/cancelRequest` (answered -32800).
//! Tests push diagnostics with [`FakeServer::publish_diagnostics`] (or script them per opened document with
//! [`FakeServer::diagnose_on_open`]), send any notification with [`FakeServer::notify`] or request with
//! [`FakeServer::request_client`], and crash it with [`FakeServer::crash`].

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::framing;
use serde_json::{Value, json};

use crate::connection::Connector;
pub use crate::fake::{FakeReply, Received};

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;
type Responder = Arc<dyn Fn(&Value) -> FakeReply + Send + Sync>;

struct State {
    received: Vec<Received>,
    writer: Option<Writer>,
    connections: u32,
    capabilities: Value,
    responders: HashMap<String, Responder>,
    inflight: HashMap<String, Writer>,
    /// Diagnostics published when a document whose URI ends with the key opens.
    on_open: Vec<(String, Value)>,
    quiescent_after_init: bool,
    /// Answers the client sent to the fake's own requests, by JSON id.
    responses: HashMap<String, Value>,
    next_request: u64,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// A scripted in-process language server. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct FakeServer {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for FakeServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeServer")
            .field("connections", &self.connections())
            .finish()
    }
}

impl Default for FakeServer {
    fn default() -> Self {
        Self::new()
    }
}

/// Capabilities like rust-analyzer's, as far as the editor features read them.
pub fn analyzer_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 2, "save": {}},
        "completionProvider": {"triggerCharacters": [":", ".", "'", "("], "resolveProvider": true},
        "hoverProvider": true,
        "signatureHelpProvider": {"triggerCharacters": ["(", ",", "<"]},
        "definitionProvider": true,
        "referencesProvider": true,
        "renameProvider": {"prepareProvider": true},
        "codeActionProvider": {"resolveProvider": true},
        "experimental": {"serverStatusNotification": true}
    })
}

impl FakeServer {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    received: Vec::new(),
                    writer: None,
                    connections: 0,
                    capabilities: analyzer_capabilities(),
                    responders: HashMap::new(),
                    inflight: HashMap::new(),
                    on_open: Vec::new(),
                    quiescent_after_init: true,
                    responses: HashMap::new(),
                    next_request: 0,
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A connector for [`crate::ServerClient::start_in_process`]: each call starts a fresh fake session on a thread,
    /// connected by OS pipes.
    pub fn connector(&self) -> Connector {
        let this = self.clone();
        Arc::new(move || {
            let (client_reads, server_writes) = io::pipe()?;
            let (server_reads, client_writes) = io::pipe()?;
            let writer: Writer = Arc::new(Mutex::new(Box::new(server_writes)));
            {
                let mut s = this.lock();
                s.writer = Some(writer.clone());
                s.connections += 1;
            }
            let session = this.clone();
            thread::Builder::new()
                .name("eludite-fake-server".into())
                .spawn(move || session.serve(BufReader::new(server_reads), writer))?;
            Ok((
                Box::new(client_reads) as Box<dyn Read + Send>,
                Box::new(client_writes) as Box<dyn Write + Send>,
            ))
        })
    }

    /// The `capabilities` of the `initialize` result.
    pub fn set_capabilities(&self, capabilities: Value) {
        self.lock().capabilities = capabilities;
    }

    /// When false, the server reports `quiescent: false` after `initialized` (still indexing) until
    /// [`FakeServer::finish_indexing`].
    pub fn set_quiescent_after_init(&self, quiescent: bool) {
        self.lock().quiescent_after_init = quiescent;
    }

    /// End the indexing progress and report the server quiescent.
    pub fn finish_indexing(&self) {
        self.notify(
            "$/progress",
            json!({"token": "rustAnalyzer/Indexing", "value": {"kind": "end"}}),
        );
        self.notify(
            "experimental/serverStatus",
            json!({"health": "ok", "quiescent": true}),
        );
    }

    /// Publish `diagnostics` whenever a document whose URI ends with `uri_suffix` opens.
    pub fn diagnose_on_open(&self, uri_suffix: &str, diagnostics: Value) {
        self.lock()
            .on_open
            .push((uri_suffix.to_owned(), diagnostics));
    }

    /// Script the answers to request `method`; `reply` gets the request's params.
    pub fn respond(
        &self,
        method: &str,
        reply: impl Fn(&Value) -> FakeReply + Send + Sync + 'static,
    ) {
        self.lock()
            .responders
            .insert(method.to_owned(), Arc::new(reply));
    }

    /// Sessions started (1 plus restarts).
    pub fn connections(&self) -> u32 {
        self.lock().connections
    }

    pub fn received(&self) -> Vec<Received> {
        self.lock().received.clone()
    }

    /// Params of every message received with `method`.
    pub fn received_params(&self, method: &str) -> Vec<Value> {
        self.lock()
            .received
            .iter()
            .filter(|r| r.method == method)
            .map(|r| r.params.clone())
            .collect()
    }

    /// Waits until a message with `method` whose params satisfy `pred` has been received.
    pub fn wait_for(
        &self,
        method: &str,
        timeout: Duration,
        pred: impl Fn(&Value) -> bool,
    ) -> Option<Received> {
        let deadline = Instant::now() + timeout;
        let mut s = self.lock();
        loop {
            if let Some(r) = s
                .received
                .iter()
                .find(|r| r.method == method && pred(&r.params))
            {
                return Some(r.clone());
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            s = self
                .shared
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// `textDocument/publishDiagnostics` (plain LSP: no generation).
    pub fn publish_diagnostics(&self, uri: &str, version: Option<i32>, diagnostics: Value) {
        let mut params = json!({"uri": uri, "diagnostics": diagnostics});
        if let Some(v) = version {
            params["version"] = json!(v);
        }
        self.notify("textDocument/publishDiagnostics", params);
    }

    /// Sends any notification to the client.
    pub fn notify(&self, method: &str, params: Value) {
        let writer = self.lock().writer.clone();
        if let Some(w) = writer {
            send(
                &w,
                json!({"jsonrpc": "2.0", "method": method, "params": params}),
            );
        }
    }

    /// Sends the client a request and waits up to `timeout` for its whole JSON-RPC response.
    pub fn request_client(&self, method: &str, params: Value, timeout: Duration) -> Option<Value> {
        let (writer, id) = {
            let mut s = self.lock();
            s.next_request += 1;
            (s.writer.clone(), format!("fake-server-{}", s.next_request))
        };
        send(
            &writer?,
            json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}),
        );
        let key = json!(id).to_string();
        let deadline = Instant::now() + timeout;
        let mut s = self.lock();
        loop {
            if let Some(r) = s.responses.remove(&key) {
                return Some(r);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            s = self
                .shared
                .changed
                .wait_timeout(s, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
    }

    /// The server dies: its output closes (the client sees the process exit) and it reads nothing more.
    pub fn crash(&self) {
        let writer = self.lock().writer.take();
        if let Some(w) = writer {
            *w.lock().unwrap_or_else(|e| e.into_inner()) = Box::new(io::sink());
        }
    }

    fn serve(self, mut input: impl BufRead, out: Writer) {
        while let Ok(Some(body)) = framing::read_message(&mut input) {
            if !self
                .lock()
                .writer
                .as_ref()
                .is_some_and(|w| Arc::ptr_eq(w, &out))
            {
                // Crashed, or replaced by a newer session.
                break;
            }
            let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                continue;
            };
            if msg.get("method").is_none()
                && let Some(id) = msg.get("id")
            {
                self.lock().responses.insert(id.to_string(), msg.clone());
                self.shared.changed.notify_all();
                continue;
            }
            let method = msg["method"].as_str().unwrap_or_default().to_owned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            self.lock().received.push(Received {
                method: method.clone(),
                params: params.clone(),
                id: msg.get("id").cloned(),
                at: Instant::now(),
            });
            self.shared.changed.notify_all();
            match msg.get("id").cloned() {
                Some(id) => self.answer(&out, &id, &method, &params),
                None => {
                    if method == "exit" {
                        break;
                    }
                    self.on_notification(&out, &method, &params);
                }
            }
        }
        let mut s = self.lock();
        if s.writer.as_ref().is_some_and(|w| Arc::ptr_eq(w, &out)) {
            s.writer = None;
        }
    }

    fn on_notification(&self, out: &Writer, method: &str, params: &Value) {
        let notify = |method: &str, params: Value| {
            send(
                out,
                json!({"jsonrpc": "2.0", "method": method, "params": params}),
            )
        };
        match method {
            "initialized" => {
                let token = "rustAnalyzer/Indexing";
                notify(
                    "$/progress",
                    json!({"token": token, "value": {"kind": "begin", "title": "Indexing", "percentage": 0}}),
                );
                notify(
                    "$/progress",
                    json!({"token": token, "value": {"kind": "report", "message": "1/2 (core)", "percentage": 50}}),
                );
                if self.lock().quiescent_after_init {
                    notify(
                        "$/progress",
                        json!({"token": token, "value": {"kind": "end"}}),
                    );
                    notify(
                        "experimental/serverStatus",
                        json!({"health": "ok", "quiescent": true}),
                    );
                } else {
                    notify(
                        "experimental/serverStatus",
                        json!({"health": "ok", "quiescent": false}),
                    );
                }
            }
            "textDocument/didOpen" => {
                let uri = params["textDocument"]["uri"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let version = params["textDocument"]["version"].clone();
                let scripted: Vec<Value> = self
                    .lock()
                    .on_open
                    .iter()
                    .filter(|(suffix, _)| uri.ends_with(suffix.as_str()))
                    .map(|(_, d)| d.clone())
                    .collect();
                for diagnostics in scripted {
                    notify(
                        "textDocument/publishDiagnostics",
                        json!({"uri": uri, "version": version, "diagnostics": diagnostics}),
                    );
                }
            }
            "$/cancelRequest" => self.cancel(&params["id"]),
            _ => {}
        }
    }

    fn answer(&self, out: &Writer, id: &Value, method: &str, params: &Value) {
        match method {
            "initialize" => {
                let caps = self.lock().capabilities.clone();
                send(
                    out,
                    json!({"jsonrpc": "2.0", "id": id, "result": {
                        "capabilities": caps,
                        "serverInfo": {"name": "fake-analyzer", "version": "0.0.0-fake"}
                    }}),
                );
            }
            "shutdown" => send(out, json!({"jsonrpc": "2.0", "id": id, "result": null})),
            _ => {
                let responder = self.lock().responders.get(method).cloned();
                let reply = responder.map_or(FakeReply::Result(Value::Null), |r| r(params));
                match reply {
                    FakeReply::Result(v) => {
                        send(out, json!({"jsonrpc": "2.0", "id": id, "result": v}))
                    }
                    FakeReply::Error(code, message) => send(
                        out,
                        json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
                    ),
                    FakeReply::Hold => {
                        self.lock().inflight.insert(id.to_string(), out.clone());
                    }
                    FakeReply::After(delay, v) => {
                        let key = id.to_string();
                        self.lock().inflight.insert(key.clone(), out.clone());
                        let this = self.clone();
                        let id = id.clone();
                        thread::spawn(move || {
                            thread::sleep(delay);
                            let w = this.lock().inflight.remove(&key);
                            if let Some(w) = w {
                                send(&w, json!({"jsonrpc": "2.0", "id": id, "result": v}));
                            }
                        });
                    }
                }
            }
        }
    }

    fn cancel(&self, id: &Value) {
        let w = self.lock().inflight.remove(&id.to_string());
        if let Some(w) = w {
            send(
                &w,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32800, "message": "canceled"}}),
            );
        }
    }
}

fn send(out: &Writer, message: Value) {
    let body = serde_json::to_vec(&message).expect("json");
    let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = framing::write_message(&mut *w, &body);
}
