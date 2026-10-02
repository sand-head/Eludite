//! A scripted `eludite-host` that runs in this process, for tests of the client and of the shell (feature `fake`).
//!
//! [`FakeHost::connector`] plugs it into [`crate::HostClient::start_in_process`]. It speaks enough of
//! `protocol/schemas/host-rpc.md` to drive a shell: the `eludite/host/*` lifecycle, `eludite/solution/open` and
//! `close` with the generation rule and status notifications, `eludite/solution/tree` (from [`FakeHost::set_tree`]),
//! generation checks on forwarded requests, and it records every message it receives. Tests inject host-to-shell
//! notifications with [`FakeHost::publish_diagnostics`] and make it unresponsive with [`FakeHost::stall_for`].
//!
//! Forwarded requests answer `null` unless a test scripts them with [`FakeHost::respond`]: a result at once, after a
//! delay, or never ([`FakeReply::Hold`]) until the shell cancels it. `$/cancelRequest` answers a request still in
//! flight with -32800 (RequestCancelled), as the real host does, unless [`FakeHost::set_ignore_cancel`] makes the fake
//! deliver late results anyway (to test that the shell drops them). [`FakeHost::set_hold_load`] keeps an opened
//! solution `loading` until [`FakeHost::finish_load`]. [`FakeHost::apply_edit`] sends the shell a `workspace/applyEdit`
//! request, as the host relays one from the language server, and waits for the shell's answer.
//!
//! Builds (brief 0017): `eludite/build/start` is accepted when a solution is open and no build runs (else -32602 or
//! -32010, as the real host answers); the fake sends the start line as output chunk 0 and then waits for the test to
//! stream more with [`FakeHost::build_output`] and [`FakeHost::build_progress`] and to end it with
//! [`FakeHost::finish_build`]. `eludite/build/cancel` answers `canceled` and sends the `canceled` finished
//! notification at once (or never, with [`FakeHost::set_build_cancel_ignored`]).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::framing;
use eludite_protocol::host::{self, Generation, methods};
use serde_json::{Value, json};

use crate::connection::Connector;

/// One message the fake received from the shell.
#[derive(Debug, Clone, PartialEq)]
pub struct Received {
    pub method: String,
    pub params: Value,
    /// The JSON-RPC id of a request (`None` for a notification).
    pub id: Option<Value>,
    /// When it was read (after any stall).
    pub at: Instant,
}

type Writer = Arc<Mutex<Box<dyn Write + Send>>>;

/// How the fake answers a forwarded request (see [`FakeHost::respond`]).
#[derive(Debug, Clone, PartialEq)]
pub enum FakeReply {
    /// This result, at once.
    Result(Value),
    /// This result after a delay (unless canceled first).
    After(Duration, Value),
    /// No answer until the request is canceled.
    Hold,
    /// An error response.
    Error(i64, String),
}

type Responder = Arc<dyn Fn(&Value) -> FakeReply + Send + Sync>;

#[derive(Default)]
struct State {
    received: Vec<Received>,
    generation: Generation,
    solution: Option<String>,
    /// `projects` of the `eludite/solution/tree` result.
    tree: Value,
    /// Delay before the tree is answered.
    tree_delay: Duration,
    stall_until: Option<Instant>,
    writer: Option<Writer>,
    connections: u32,
    responders: HashMap<String, Responder>,
    /// Forwarded requests not answered yet, by JSON id.
    inflight: HashMap<String, Writer>,
    ignore_cancel: bool,
    hold_load: bool,
    /// `eludite/languageServer/status` after initialize: state (default `running`) and capabilities.
    server_state: Option<String>,
    capabilities: Option<Value>,
    /// Answers the shell sent to the fake's own requests, by JSON id.
    responses: HashMap<String, Value>,
    next_request: u64,
    /// The running build: id, next output seq, its `eludite/build/start` params, generation.
    build: Option<FakeBuild>,
    builds_started: u64,
    build_cancel_ignored: bool,
}

#[derive(Debug, Clone)]
struct FakeBuild {
    id: u64,
    seq: u64,
    params: Value,
    generation: Generation,
    path: String,
    started: Instant,
}

struct Shared {
    state: Mutex<State>,
    changed: Condvar,
}

/// A scripted in-process host. Cheap to clone; clones share state.
#[derive(Clone)]
pub struct FakeHost {
    shared: Arc<Shared>,
}

impl std::fmt::Debug for FakeHost {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FakeHost")
            .field("generation", &self.generation())
            .finish()
    }
}

impl Default for FakeHost {
    fn default() -> Self {
        Self::new()
    }
}

impl FakeHost {
    pub fn new() -> Self {
        Self {
            shared: Arc::new(Shared {
                state: Mutex::new(State {
                    tree: json!([]),
                    ..State::default()
                }),
                changed: Condvar::new(),
            }),
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.shared.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// A connector for [`crate::HostClient::start_in_process`]: each call starts a fresh fake session on a thread,
    /// connected by OS pipes.
    pub fn connector(&self) -> Connector {
        let this = self.clone();
        Arc::new(move || {
            let (shell_reads, host_writes) = io::pipe()?;
            let (host_reads, shell_writes) = io::pipe()?;
            let session = this.clone();
            let writer: Writer = Arc::new(Mutex::new(Box::new(host_writes)));
            {
                let mut s = session.lock();
                s.writer = Some(writer.clone());
                s.connections += 1;
                s.generation = 0;
                s.solution = None;
            }
            thread::Builder::new()
                .name("eludite-fake-host".into())
                .spawn(move || session.serve(BufReader::new(host_reads), writer))?;
            Ok((
                Box::new(shell_reads) as Box<dyn Read + Send>,
                Box::new(shell_writes) as Box<dyn Write + Send>,
            ))
        })
    }

    /// `projects` for `eludite/solution/tree` (see `protocol/schemas/host/solution-tree.json`).
    pub fn set_tree(&self, projects: Value) {
        self.lock().tree = projects;
    }

    /// Answer `eludite/solution/tree` only after `delay`.
    pub fn set_tree_delay(&self, delay: Duration) {
        self.lock().tree_delay = delay;
    }

    /// Stop reading and answering for `duration` from now (an unresponsive host).
    pub fn stall_for(&self, duration: Duration) {
        self.lock().stall_until = Some(Instant::now() + duration);
    }

    pub fn generation(&self) -> Generation {
        self.lock().generation
    }

    /// Script the answers to forwarded request `method` (for example `textDocument/completion`). `reply` gets the
    /// request's params (with `eluditeGeneration`).
    pub fn respond(
        &self,
        method: &str,
        reply: impl Fn(&Value) -> FakeReply + Send + Sync + 'static,
    ) {
        self.lock()
            .responders
            .insert(method.to_owned(), Arc::new(reply));
    }

    /// When true, `$/cancelRequest` is recorded but ignored: delayed results are still delivered (a host whose
    /// result was already on the wire).
    pub fn set_ignore_cancel(&self, ignore: bool) {
        self.lock().ignore_cancel = ignore;
    }

    /// The `eludite/languageServer/status` the fake sends after `eludite/host/initialize`: `state` (`running` by
    /// default, or `starting` to keep IntelliSense on its fallback) and the server's LSP `capabilities`.
    pub fn set_language_server(&self, state: &str, capabilities: Option<Value>) {
        let mut s = self.lock();
        s.server_state = Some(state.to_owned());
        s.capabilities = capabilities;
    }

    /// When true, `eludite/solution/open` reports `loading` and stops there until [`FakeHost::finish_load`].
    pub fn set_hold_load(&self, hold: bool) {
        self.lock().hold_load = hold;
    }

    /// Send `loaded` for the open solution (after [`FakeHost::set_hold_load`]).
    pub fn finish_load(&self) {
        let (generation, path) = {
            let s = self.lock();
            (s.generation, s.solution.clone())
        };
        if let Some(path) = path {
            let projects = self.lock().tree.as_array().map_or(0, Vec::len);
            self.notify(
                methods::SOLUTION_STATUS,
                json!({"generation": generation, "path": path, "state": "loaded", "elapsedMs": 1.0,
                       "counts": {"projects": projects, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
            );
        }
    }

    /// Requests received but not answered yet (held or delayed).
    pub fn inflight(&self) -> usize {
        self.lock().inflight.len()
    }

    /// Sessions started (1 plus restarts).
    pub fn connections(&self) -> u32 {
        self.lock().connections
    }

    /// Everything received so far.
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

    /// Sends `textDocument/publishDiagnostics` for `uri` at `version` under the current generation.
    pub fn publish_diagnostics(&self, uri: &str, version: i32, diagnostics: Value) {
        let generation = self.generation();
        self.notify(
            methods::PUBLISH_DIAGNOSTICS,
            json!({"uri": uri, "version": version, "diagnostics": diagnostics, "eluditeGeneration": generation}),
        );
    }

    /// Sends the shell `workspace/applyEdit` with `params` (`{ label?, edit }`; `eluditeGeneration` is added unless
    /// present) and waits up to `timeout` for the response: the whole JSON-RPC response (`result` or `error`), or
    /// `None` on timeout.
    pub fn apply_edit(&self, mut params: Value, timeout: Duration) -> Option<Value> {
        if params.get(host::GENERATION_FIELD).is_none() {
            params[host::GENERATION_FIELD] = json!(self.generation());
        }
        self.request_shell(methods::APPLY_EDIT, params, timeout)
    }

    /// Sends the shell a request and waits up to `timeout` for its response (see [`FakeHost::apply_edit`]).
    pub fn request_shell(&self, method: &str, params: Value, timeout: Duration) -> Option<Value> {
        let (writer, id) = {
            let mut s = self.lock();
            s.next_request += 1;
            (s.writer.clone(), format!("fake-{}", s.next_request))
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

    /// The id of the build that is running, if any.
    pub fn running_build(&self) -> Option<u64> {
        self.lock().build.as_ref().map(|b| b.id)
    }

    /// When true, `eludite/build/cancel` answers but the build goes on (a host whose kill is slow).
    pub fn set_build_cancel_ignored(&self, ignored: bool) {
        self.lock().build_cancel_ignored = ignored;
    }

    /// Sends the running build's next output chunk (`text` should end with a newline).
    pub fn build_output(&self, text: &str) {
        let next = {
            let mut s = self.lock();
            s.build.as_mut().map(|b| {
                b.seq += 1;
                (b.id, b.seq - 1)
            })
        };
        if let Some((id, seq)) = next {
            self.notify(
                methods::BUILD_OUTPUT,
                json!({"buildId": id, "seq": seq, "text": text}),
            );
        }
    }

    /// Sends `eludite/build/progress` for the running build.
    pub fn build_progress(&self, completed: u32, total: u32, errors: u32, warnings: u32) {
        let build = self.lock().build.clone();
        if let Some(b) = build {
            self.notify(
                methods::BUILD_PROGRESS,
                json!({"buildId": b.id, "elapsedMs": b.started.elapsed().as_secs_f64() * 1e3,
                       "projectsTotal": total, "projectsCompleted": completed,
                       "errors": errors, "warnings": warnings}),
            );
        }
    }

    /// Ends the running build with `result` (`succeeded`, `failed`, `canceled`) and `diagnostics` (the
    /// `build-finished.json` shape). Every project of the tree is listed, failed when it has an error.
    pub fn finish_build(&self, result: &str, diagnostics: Value) {
        let (build, tree) = {
            let mut s = self.lock();
            (s.build.take(), s.tree.clone())
        };
        let Some(b) = build else { return };
        let diags = diagnostics.as_array().cloned().unwrap_or_default();
        let count = |sev: &str, project: Option<&str>| {
            diags
                .iter()
                .filter(|d| {
                    d["severity"] == sev && project.is_none_or(|p| d["project"].as_str() == Some(p))
                })
                .count()
        };
        let projects: Vec<Value> = tree
            .as_array()
            .map(Vec::as_slice)
            .unwrap_or_default()
            .iter()
            .map(|p| {
                let path = p["path"].as_str().unwrap_or_default();
                let errors = count("error", Some(path));
                let r = if result == "canceled" {
                    "canceled"
                } else if errors > 0 {
                    "failed"
                } else {
                    "succeeded"
                };
                json!({"name": p["name"], "path": path, "result": r, "elapsedMs": 1.0,
                       "errors": errors, "warnings": count("warning", Some(path))})
            })
            .collect();
        let failed = projects.iter().filter(|p| p["result"] == "failed").count();
        let summary = json!({"projectsSucceeded": projects.len() - failed, "projectsFailed": failed,
                             "errors": count("error", None), "warnings": count("warning", None)});
        let target = b.params["target"].clone();
        self.notify(
            methods::BUILD_FINISHED,
            json!({"buildId": b.id, "generation": b.generation, "target": target, "path": b.path,
                   "result": result, "exitCode": if result == "succeeded" { 0 } else { 1 },
                   "elapsedMs": b.started.elapsed().as_secs_f64() * 1e3,
                   "summary": summary, "projects": projects, "diagnostics": diagnostics}),
        );
    }

    /// Sends any notification to the shell.
    pub fn notify(&self, method: &str, params: Value) {
        let writer = self.lock().writer.clone();
        if let Some(w) = writer {
            send(
                &w,
                json!({"jsonrpc": "2.0", "method": method, "params": params}),
            );
        }
    }

    fn serve(self, input: impl BufRead, out: Writer) {
        self.serve_messages(input, &out);
        // The session is over: drop every handle on its output so the client sees end of stream.
        let mut s = self.lock();
        if s.writer.as_ref().is_some_and(|w| Arc::ptr_eq(w, &out)) {
            s.writer = None;
        }
    }

    fn serve_messages(&self, mut input: impl BufRead, out: &Writer) {
        while let Ok(Some(body)) = framing::read_message(&mut input) {
            loop {
                let until = self.lock().stall_until;
                match until {
                    Some(t) if t > Instant::now() => thread::sleep(t - Instant::now()),
                    _ => break,
                }
            }
            let Ok(msg) = serde_json::from_slice::<Value>(&body) else {
                continue;
            };
            if msg.get("method").is_none()
                && let Some(id) = msg.get("id")
            {
                // A response to one of the fake's own requests (workspace/applyEdit).
                self.lock().responses.insert(id.to_string(), msg.clone());
                self.shared.changed.notify_all();
                continue;
            }
            let method = msg["method"].as_str().unwrap_or_default().to_owned();
            let params = msg.get("params").cloned().unwrap_or(Value::Null);
            {
                let mut s = self.lock();
                s.received.push(Received {
                    method: method.clone(),
                    params: params.clone(),
                    id: msg.get("id").cloned(),
                    at: Instant::now(),
                });
            }
            self.shared.changed.notify_all();
            let Some(id) = msg.get("id").cloned() else {
                if method == methods::HOST_EXIT {
                    return;
                }
                if method == methods::CANCEL_REQUEST {
                    self.cancel(&params["id"]);
                }
                continue;
            };
            self.answer(out, &id, &method, &params);
        }
    }

    fn answer(&self, out: &Writer, id: &Value, method: &str, params: &Value) {
        let reply =
            |result: Value| send(out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
        let error = |code: i64, message: &str, data: Option<Value>| {
            let mut e = json!({"code": code, "message": message});
            if let Some(d) = data {
                e["data"] = d;
            }
            send(out, json!({"jsonrpc": "2.0", "id": id, "error": e}));
        };
        let notify = |method: &str, params: Value| {
            send(
                out,
                json!({"jsonrpc": "2.0", "method": method, "params": params}),
            )
        };
        match method {
            methods::HOST_INITIALIZE => {
                reply(json!({"hostName": host::HOST_NAME, "hostVersion": "fake",
                             "capabilities": {"languageServer": true}}));
                let (state, capabilities) = {
                    let s = self.lock();
                    (
                        s.server_state.clone().unwrap_or_else(|| "running".into()),
                        s.capabilities.clone(),
                    )
                };
                let mut status = json!({"state": state, "serverInfo": {"name": "fake-ls"}});
                if let Some(c) = capabilities {
                    status["capabilities"] = c;
                }
                notify(methods::LANGUAGE_SERVER_STATUS, status);
            }
            methods::PING => reply(json!({"pong": true, "timestamp": "2026-10-01T00:00:00Z"})),
            methods::HOST_SHUTDOWN => reply(Value::Null),
            methods::SOLUTION_OPEN => {
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                let generation = {
                    let mut s = self.lock();
                    s.generation += 1;
                    s.solution = Some(path.clone());
                    s.generation
                };
                reply(json!({"generation": generation}));
                notify(
                    methods::SOLUTION_STATUS,
                    json!({"generation": generation, "path": path, "state": "loading", "phase": "projectLoad"}),
                );
                if self.lock().hold_load {
                    return;
                }
                let projects = self.lock().tree.as_array().map_or(0, Vec::len);
                notify(
                    methods::SOLUTION_STATUS,
                    json!({"generation": generation, "path": path, "state": "loaded", "elapsedMs": 1.0,
                           "counts": {"projects": projects, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
                );
            }
            methods::SOLUTION_CLOSE => {
                let (generation, path) = {
                    let mut s = self.lock();
                    let path = s.solution.take();
                    if path.is_some() {
                        s.generation += 1;
                    }
                    (s.generation, path)
                };
                reply(json!({"generation": generation}));
                if let Some(path) = path {
                    notify(
                        methods::SOLUTION_STATUS,
                        json!({"generation": generation, "path": path, "state": "closed"}),
                    );
                }
            }
            methods::SOLUTION_TREE => {
                let (generation, path, projects, delay) = {
                    let s = self.lock();
                    (
                        s.generation,
                        s.solution.clone(),
                        s.tree.clone(),
                        s.tree_delay,
                    )
                };
                let result = match path {
                    Some(p) => json!({"generation": generation, "path": p, "projects": projects}),
                    None => json!({"generation": generation, "path": null, "projects": []}),
                };
                let out = out.clone();
                let id = id.clone();
                thread::spawn(move || {
                    thread::sleep(delay);
                    send(&out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
                });
            }
            methods::BUILD_START => self.build_start(&reply, &error, &notify, params),
            methods::BUILD_CANCEL => {
                let (running, ignored) = {
                    let s = self.lock();
                    (s.build.as_ref().map(|b| b.id), s.build_cancel_ignored)
                };
                let wanted = params["buildId"].as_u64();
                match running {
                    Some(id) if wanted.is_none_or(|w| w == id) => {
                        reply(json!({"canceled": true, "buildId": id}));
                        if !ignored {
                            self.build_output("Build canceled.\n");
                            self.finish_build("canceled", json!([]));
                        }
                    }
                    _ => reply(json!({"canceled": false})),
                }
            }
            m if methods::FORWARDED_TYPED_REQUESTS.contains(&m)
                || methods::FORWARDED_UNTYPED_REQUESTS.contains(&m) =>
            {
                let current = self.generation();
                match params[host::GENERATION_FIELD].as_u64() {
                    None => error(-32602, "eluditeGeneration is required", None),
                    Some(g) if g != current => error(
                        host::error_codes::CONTENT_MODIFIED,
                        "stale",
                        Some(json!({"requestedGeneration": g, "currentGeneration": current})),
                    ),
                    Some(_) => self.forwarded(out, id, m, params),
                }
            }
            other => error(-32601, &format!("{other} not found"), None),
        }
    }
}

impl FakeHost {
    fn build_start(
        &self,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
        notify: &dyn Fn(&str, Value),
        params: &Value,
    ) {
        let mut s = self.lock();
        let Some(solution) = s.solution.clone() else {
            drop(s);
            return error(-32602, "no solution is open", None);
        };
        if let Some(b) = &s.build {
            let id = b.id;
            drop(s);
            return error(
                host::error_codes::BUILD_IN_PROGRESS,
                "a build is already running",
                Some(json!({"buildId": id})),
            );
        }
        if !matches!(
            params["target"].as_str(),
            Some("build" | "rebuild" | "clean")
        ) {
            drop(s);
            return error(-32602, "target must be build, rebuild or clean", None);
        }
        s.builds_started += 1;
        let id = s.builds_started;
        let path = params["project"]
            .as_str()
            .map(str::to_owned)
            .unwrap_or(solution);
        let generation = s.generation;
        s.build = Some(FakeBuild {
            id,
            seq: 1,
            params: params.clone(),
            generation,
            path: path.clone(),
            started: Instant::now(),
        });
        drop(s);
        let configuration = params["configuration"].as_str().unwrap_or("Debug");
        reply(
            json!({"buildId": id, "generation": generation, "path": path, "target": params["target"],
                     "configuration": configuration, "platform": params.get("platform").cloned().unwrap_or(Value::Null),
                     "toolchain": {"kind": "dotnet", "path": "dotnet"}, "binlog": null,
                     "commandLine": format!("dotnet build {path}")}),
        );
        notify(
            methods::BUILD_OUTPUT,
            json!({"buildId": id, "seq": 0,
                   "text": format!("Build started...\n> dotnet build {path} -c {configuration}\n")}),
        );
    }

    fn forwarded(&self, out: &Writer, id: &Value, method: &str, params: &Value) {
        let responder = self.lock().responders.get(method).cloned();
        let reply = responder.map_or(FakeReply::Result(Value::Null), |r| r(params));
        let key = id.to_string();
        match reply {
            FakeReply::Result(v) => {
                send(out, json!({"jsonrpc": "2.0", "id": id, "result": v}));
            }
            FakeReply::Error(code, message) => send(
                out,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}),
            ),
            FakeReply::Hold => {
                self.lock().inflight.insert(key, out.clone());
            }
            FakeReply::After(delay, v) => {
                self.lock().inflight.insert(key.clone(), out.clone());
                let this = self.clone();
                let id = id.clone();
                thread::spawn(move || {
                    thread::sleep(delay);
                    // Still in flight (not canceled), or canceled while the fake ignores cancels.
                    let w = {
                        let mut s = this.lock();
                        let w = s.inflight.remove(&key);
                        if w.is_none() && !s.ignore_cancel {
                            return;
                        }
                        w.or_else(|| s.writer.clone())
                    };
                    if let Some(w) = w {
                        send(&w, json!({"jsonrpc": "2.0", "id": id, "result": v}));
                    }
                });
            }
        }
    }

    fn cancel(&self, id: &Value) {
        let key = id.to_string();
        let w = {
            let mut s = self.lock();
            if s.ignore_cancel {
                // The delayed result stays scheduled; a held request stays held.
                return;
            }
            s.inflight.remove(&key)
        };
        if let Some(w) = w {
            send(
                &w,
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": host::error_codes::REQUEST_CANCELLED, "message": "canceled"}}),
            );
        }
    }
}

fn send(out: &Writer, message: Value) {
    let body = serde_json::to_vec(&message).expect("json");
    let mut w = out.lock().unwrap_or_else(|e| e.into_inner());
    let _ = framing::write_message(&mut *w, &body);
}
