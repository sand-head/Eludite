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
//!
//! NuGet (brief 0048): `eludite/nuget/*` answers from [`FakeHost::set_nuget`]'s script (see `fake_nuget.rs`), each on a
//! thread of its own, so a private source can ask the shell for credentials (`eludite/nuget/credentials`) and wait.
//!
//! `eludite/build/status` (brief 0020) answers the running build with every output chunk sent so far and the last
//! finished build. A new connection (the client restarting the fake after [`crate::HostClient::kill`]) ends the
//! running build, as a real host restart does, unless [`FakeHost::set_build_survives_restart`] keeps it (a host the
//! shell reattaches to); [`FakeHost::build_output_unsent`] records a chunk without sending it (one sent while the
//! shell was away).
//!
//! Project properties (brief 0049): `eludite/project/*` and the solution configurations are served from a scripted
//! model by the child module `projects` (see its docs and [`FakeHost::set_project_properties`]).
//!
//! Resources (proposal 0005): `eludite/resx/sets` answers the sets a test gave ([`FakeHost::set_resx_sets`]) and
//! `eludite/resx/designer` writes or deletes a stand-in designer file (the child module `resx`).

use std::collections::HashMap;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::sync::{Arc, Condvar, Mutex, MutexGuard};
use std::thread;
use std::time::{Duration, Instant};

use eludite_protocol::framing;
use eludite_protocol::host::{self, Generation, methods};
use serde_json::{Value, json};

use crate::connection::Connector;
use crate::fake_nuget::{FakeNuGet, host_of, output_updates};

/// `eludite/project/*` and the solution configurations (brief 0049).
mod projects;
/// `eludite/resx/*` (proposal 0005).
mod resx;

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
    build_survives_restart: bool,
    /// The last finished build's `eludite/build/status` `last` member.
    last_build: Option<Value>,
    /// `eludite/test/*` (brief 0035).
    tests: FakeTests,
    /// `eludite/nuget/*` (brief 0048).
    nuget: FakeNuGet,
    /// `eludite/project/*` and the solution configurations (brief 0049).
    projects: projects::FakeProjects,
    /// `eludite/resx/*` (proposal 0005).
    resx: resx::FakeResx,
}

#[derive(Default)]
struct FakeTests {
    /// `[{ "container": <test-discover.json container>, "tests": [<test item>...] }]`.
    catalogue: Vec<Value>,
    /// Result members by test id (`outcome`, `message`, `stackTrace`, `output`, `durationMs`); unlisted tests pass.
    outcomes: serde_json::Map<String, Value>,
    hold: bool,
    survives_restart: bool,
    next_id: u64,
    running: Option<FakeTestRun>,
    last: Option<Value>,
    discoveries: u64,
}

#[derive(Debug, Clone)]
struct FakeTestRun {
    id: u64,
    generation: Generation,
    kind: &'static str,
    debug: bool,
    containers: Vec<Value>,
    /// Requested test ids per container id.
    requested: Vec<(String, Vec<String>)>,
    seq: u64,
    /// For `eludite/test/status`: tests sent (with their container) and the latest result per test.
    tests: Vec<Value>,
    results: Vec<Value>,
    started: Instant,
}

#[derive(Debug, Clone)]
struct FakeBuild {
    id: u64,
    seq: u64,
    params: Value,
    generation: Generation,
    path: String,
    started: Instant,
    /// Every chunk recorded so far (sent or not): `eludite/build/status` replays them.
    output: String,
    /// The last progress sent.
    progress: Option<Value>,
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
                if !s.build_survives_restart {
                    s.build = None;
                }
                if !s.tests.survives_restart {
                    s.tests.running = None;
                }
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

    /// When true, the running build outlives a restart of the fake (a host the shell reattaches to); by default a new
    /// connection ends it, as a real host restart does.
    pub fn set_build_survives_restart(&self, survives: bool) {
        self.lock().build_survives_restart = survives;
    }

    /// Records the running build's next output chunk without sending it (output the shell missed while it was away);
    /// `eludite/build/status` still includes it.
    pub fn build_output_unsent(&self, text: &str) {
        let mut s = self.lock();
        if let Some(b) = s.build.as_mut() {
            b.seq += 1;
            b.output.push_str(text);
        }
    }

    /// Sends the running build's next output chunk (`text` should end with a newline).
    pub fn build_output(&self, text: &str) {
        let next = {
            let mut s = self.lock();
            s.build.as_mut().map(|b| {
                b.seq += 1;
                b.output.push_str(text);
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
        let progress = {
            let mut s = self.lock();
            s.build.as_mut().map(|b| {
                let p = json!({"buildId": b.id, "elapsedMs": b.started.elapsed().as_secs_f64() * 1e3,
                               "projectsTotal": total, "projectsCompleted": completed,
                               "errors": errors, "warnings": warnings});
                b.progress = Some(p.clone());
                p
            })
        };
        if let Some(p) = progress {
            self.notify(methods::BUILD_PROGRESS, p);
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
        let elapsed = b.started.elapsed().as_secs_f64() * 1e3;
        self.lock().last_build = Some(
            json!({"buildId": b.id, "generation": b.generation, "target": target, "path": b.path,
                   "result": result, "elapsedMs": elapsed, "summary": summary}),
        );
        self.notify(
            methods::BUILD_FINISHED,
            json!({"buildId": b.id, "generation": b.generation, "target": target, "path": b.path,
                   "result": result, "exitCode": if result == "succeeded" { 0 } else { 1 },
                   "elapsedMs": elapsed,
                   "summary": summary, "projects": projects, "diagnostics": diagnostics}),
        );
    }

    /// The NuGet sources, packages and projects `eludite/nuget/*` answers from (brief 0048; see `fake_nuget.rs`).
    pub fn set_nuget(&self, script: Value) {
        self.lock().nuget.set(&script);
    }

    /// The next restores (and changes' restores) fail with this diagnostic (`{ severity, code, message, file }`), or
    /// succeed again with `None`.
    pub fn set_nuget_restore_failure(&self, diagnostic: Option<Value>) {
        self.lock().nuget.restore_failure = diagnostic;
    }

    /// Searches answer only after `delay`.
    pub fn set_nuget_search_delay(&self, delay: Duration) {
        self.lock().nuget.search_delay = delay;
    }

    /// The scripted projects with their packages as changes left them.
    pub fn nuget_projects(&self) -> Value {
        Value::Array(self.lock().nuget.projects.clone())
    }

    /// The scripted package sources as changes left them.
    pub fn nuget_sources(&self) -> Value {
        Value::Array(self.lock().nuget.sources.clone())
    }

    /// What `eludite/nuget/icon` answers for `url`.
    pub fn set_nuget_icon(&self, url: &str, path: &str) {
        self.lock()
            .nuget
            .icons
            .insert(url.to_owned(), path.to_owned());
    }

    /// `eludite/nuget/*` requests answered so far.
    pub fn nuget_calls(&self) -> u64 {
        self.lock().nuget.calls
    }

    /// The test containers and their tests (brief 0035): `[{ "container": {...}, "tests": [...] }]`.
    pub fn set_tests(&self, catalogue: Value) {
        self.lock().tests.catalogue = catalogue.as_array().cloned().unwrap_or_default();
    }

    /// Result members by test id for the runs that are not held (`{"t1": {"outcome": "failed", "message": ...}}`).
    pub fn set_test_outcomes(&self, outcomes: Value) {
        self.lock().tests.outcomes = outcomes.as_object().cloned().unwrap_or_default();
    }

    /// When true, runs wait for [`FakeHost::test_results`] and [`FakeHost::finish_test_run`], and discoveries (their
    /// tests sent) for [`FakeHost::finish_test_run`].
    pub fn set_hold_test_runs(&self, hold: bool) {
        self.lock().tests.hold = hold;
    }

    /// When true, the running discovery or run outlives a restart of the fake.
    pub fn set_test_survives_restart(&self, survives: bool) {
        self.lock().tests.survives_restart = survives;
    }

    /// The discovery or run that is going: its id.
    pub fn running_test_run(&self) -> Option<u64> {
        self.lock().tests.running.as_ref().map(|r| r.id)
    }

    /// How many discoveries the fake answered.
    pub fn test_discoveries(&self) -> u64 {
        self.lock().tests.discoveries
    }

    /// Sends a `results` update of the running run for `container` (`results`: test-update.json result items).
    pub fn test_results(&self, container: &str, results: Value) {
        self.test_update(
            json!({"kind": "results", "container": container, "results": results}),
            true,
        );
    }

    /// Records a `results` update of the running run without sending it (one the shell missed while it was away).
    pub fn test_results_unsent(&self, container: &str, results: Value) {
        self.test_update(
            json!({"kind": "results", "container": container, "results": results}),
            false,
        );
    }

    /// Sends an `output` update of the running run.
    pub fn test_output(&self, text: &str) {
        self.test_update(json!({"kind": "output", "text": text}), true);
    }

    /// Ends the running discovery or run with `state` (`completed`, `failed`, `canceled`): each container finishes,
    /// then the run, with the summary over what it reported.
    pub fn finish_test_run(&self, state: &str) {
        let run = self.lock().tests.running.clone();
        let Some(run) = run else { return };
        for (container, _) in &run.requested {
            let count = run
                .results
                .iter()
                .chain(run.tests.iter())
                .filter(|r| r["container"] == json!(container))
                .count();
            self.test_update(
                json!({"kind": "containerFinished", "container": container, "state": state, "count": count}),
                true,
            );
        }
        let run = self.lock().tests.running.clone().expect("still running");
        let total: usize = if run.kind == "discover" {
            run.tests.len()
        } else {
            run.requested.iter().map(|(_, t)| t.len()).sum()
        };
        let count = |o: &str| run.results.iter().filter(|r| r["outcome"] == o).count();
        let (passed, failed, skipped) = if run.kind == "discover" {
            (0, 0, 0)
        } else {
            (count("passed"), count("failed"), count("skipped"))
        };
        let not_run = if run.kind == "discover" {
            0
        } else {
            total.saturating_sub(passed + failed + skipped)
        };
        let summary = json!({"total": total, "passed": passed, "failed": failed, "skipped": skipped,
                             "notRun": not_run});
        let elapsed = run.started.elapsed().as_secs_f64() * 1e3;
        self.test_update(
            json!({"kind": "finished", "state": state, "summary": summary, "elapsedMs": elapsed}),
            true,
        );
        let mut s = self.lock();
        s.tests.last = Some(
            json!({"runId": run.id, "kind": run.kind, "generation": run.generation,
                                   "state": state, "summary": summary, "elapsedMs": elapsed}),
        );
        s.tests.running = None;
    }

    /// Numbers `update` as the running run's next one, records it for `eludite/test/status`, and sends it (unless
    /// `send` is false).
    fn test_update(&self, mut update: Value, send: bool) {
        let update = {
            let mut s = self.lock();
            let Some(run) = s.tests.running.as_mut() else {
                return;
            };
            update["runId"] = json!(run.id);
            update["generation"] = json!(run.generation);
            update["seq"] = json!(run.seq);
            run.seq += 1;
            let container = update["container"].clone();
            for t in update["tests"].as_array().into_iter().flatten() {
                run.tests.push(json!({"container": container, "test": t}));
            }
            for r in update["results"].as_array().into_iter().flatten() {
                let mut r = r.clone();
                r["container"] = container.clone();
                run.results
                    .retain(|x| !(x["id"] == r["id"] && x["container"] == container));
                run.results.push(r);
            }
            update
        };
        if send {
            self.notify(methods::TEST_UPDATE, update);
        }
    }

    fn answer_test(
        &self,
        method: &str,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
    ) {
        match method {
            methods::TEST_STATUS => {
                let s = self.lock();
                let running: Vec<Value> = s
                    .tests
                    .running
                    .iter()
                    .map(|r| {
                        let mut v = json!({"runId": r.id, "kind": r.kind, "generation": r.generation,
                                           "containers": r.containers,
                                           "elapsedMs": r.started.elapsed().as_secs_f64() * 1e3,
                                           "nextSeq": r.seq, "tests": r.tests, "results": r.results});
                        if r.debug {
                            v["debug"] = json!(true);
                        }
                        v
                    })
                    .collect();
                let mut result = json!({"running": running});
                if let Some(last) = &s.tests.last {
                    result["last"] = last.clone();
                }
                drop(s);
                reply(result);
            }
            methods::TEST_CANCEL => {
                let running = self.running_test_run();
                match running {
                    Some(id) if params["runId"].as_u64().is_none_or(|w| w == id) => {
                        reply(json!({"canceled": true, "runId": id}));
                        self.finish_test_run("canceled");
                    }
                    _ => reply(json!({"canceled": false})),
                }
            }
            methods::TEST_ATTACHED => {
                let accepted = self
                    .running_test_run()
                    .is_some_and(|id| params["runId"].as_u64() == Some(id));
                reply(json!({"accepted": accepted}));
            }
            methods::TEST_DISCOVER => {
                let (generation, solution, catalogue) = {
                    let s = self.lock();
                    (s.generation, s.solution.clone(), s.tests.catalogue.clone())
                };
                if solution.is_none() {
                    return error(-32602, "no solution is open", None);
                }
                let containers: Vec<Value> =
                    catalogue.iter().map(|c| c["container"].clone()).collect();
                let id = {
                    let mut s = self.lock();
                    s.tests.next_id += 1;
                    s.tests.discoveries += 1;
                    let id = s.tests.next_id;
                    s.tests.running = Some(FakeTestRun {
                        id,
                        generation,
                        kind: "discover",
                        debug: false,
                        containers: containers.clone(),
                        requested: catalogue
                            .iter()
                            .map(|c| {
                                (
                                    c["container"]["id"].as_str().unwrap_or_default().to_owned(),
                                    vec![],
                                )
                            })
                            .collect(),
                        seq: 0,
                        tests: vec![],
                        results: vec![],
                        started: Instant::now(),
                    });
                    id
                };
                reply(json!({"runId": id, "generation": generation, "containers": containers}));
                for c in &catalogue {
                    self.test_update(
                        json!({"kind": "output", "container": c["container"]["id"],
                               "text": format!("Discovering {}\n", c["container"]["name"].as_str().unwrap_or_default())}),
                        true,
                    );
                    self.test_update(
                        json!({"kind": "discovered", "container": c["container"]["id"], "tests": c["tests"]}),
                        true,
                    );
                }
                if !self.lock().tests.hold {
                    self.finish_test_run("completed");
                }
            }
            _ => self.run_tests(params, reply, error),
        }
    }

    fn run_tests(
        &self,
        params: &Value,
        reply: &dyn Fn(Value),
        error: &dyn Fn(i64, &str, Option<Value>),
    ) {
        let (generation, catalogue, running, hold) = {
            let s = self.lock();
            if s.solution.is_none() {
                drop(s);
                return error(-32602, "no solution is open", None);
            }
            (
                s.generation,
                s.tests.catalogue.clone(),
                s.tests.running.clone(),
                s.tests.hold,
            )
        };
        let named: Vec<(String, Option<Vec<String>>)> = match params["containers"].as_array() {
            Some(list) => list
                .iter()
                .map(|c| {
                    (
                        c["id"].as_str().unwrap_or_default().to_owned(),
                        c["tests"].as_array().map(|t| {
                            t.iter()
                                .filter_map(|x| x.as_str().map(str::to_owned))
                                .collect()
                        }),
                    )
                })
                .collect(),
            None => catalogue
                .iter()
                .map(|c| {
                    (
                        c["container"]["id"].as_str().unwrap_or_default().to_owned(),
                        None,
                    )
                })
                .collect(),
        };
        let mut requested = Vec::new();
        let mut containers = Vec::new();
        for (id, tests) in named {
            let Some(c) = catalogue.iter().find(|c| c["container"]["id"] == json!(id)) else {
                return error(
                    -32602,
                    &format!("{id} is not a test container of the open solution"),
                    None,
                );
            };
            if let Some(r) = &running
                && r.kind == "run"
                && r.requested.iter().any(|(rc, _)| *rc == id)
            {
                return error(
                    host::error_codes::TEST_RUN_IN_PROGRESS,
                    "a run is running it",
                    Some(json!({"runId": r.id, "container": id})),
                );
            }
            let all: Vec<String> = c["tests"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|t| t["id"].as_str().map(str::to_owned))
                .collect();
            requested.push((id.clone(), tests.unwrap_or(all)));
            containers.push(c["container"].clone());
        }
        let debug = params["debug"].as_bool().unwrap_or(false);
        if debug && requested.len() != 1 {
            return error(-32602, "a debug run names exactly one container", None);
        }
        let id = {
            let mut s = self.lock();
            s.tests.next_id += 1;
            let id = s.tests.next_id;
            s.tests.running = Some(FakeTestRun {
                id,
                generation,
                kind: "run",
                debug,
                containers: containers.clone(),
                requested: requested.clone(),
                seq: 0,
                tests: vec![],
                results: vec![],
                started: Instant::now(),
            });
            id
        };
        let ids: Vec<&String> = requested.iter().map(|(c, _)| c).collect();
        let mut answer = json!({"runId": id, "generation": generation, "containers": ids});
        if debug {
            answer["debug"] = json!(true);
        }
        reply(answer);
        if debug {
            let c = &containers[0];
            if c["protocol"] == "vstest" {
                self.test_update(
                    json!({"kind": "attach", "container": c["id"], "processId": 4242}),
                    true,
                );
            } else {
                let runtime = c["runtime"].as_str().unwrap_or("dotnet");
                self.test_update(
                    json!({"kind": "launch", "container": c["id"],
                           "launch": {"program": c["program"], "args": ["--server", "--client-port", "40000"],
                                      "cwd": "/", "env": {"TESTINGPLATFORM_TELEMETRY_OPTOUT": "1"},
                                      "runtime": runtime}}),
                    true,
                );
            }
            return;
        }
        if hold {
            return;
        }
        let outcomes = self.lock().tests.outcomes.clone();
        for (container, tests) in &requested {
            let results: Vec<Value> = tests
                .iter()
                .map(|t| {
                    let mut r = json!({"id": t, "outcome": "passed", "durationMs": 1.0});
                    if let Some(o) = outcomes.get(t).and_then(Value::as_object) {
                        for (k, v) in o {
                            r[k] = v.clone();
                        }
                    }
                    r
                })
                .collect();
            self.test_results(container, json!(results));
        }
        self.finish_test_run("completed");
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
                    let mut tree = s.tree.clone();
                    s.nuget.decorate_tree(&mut tree);
                    (s.generation, s.solution.clone(), tree, s.tree_delay)
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
            methods::BUILD_STATUS => {
                let s = self.lock();
                let running = s.build.as_ref().map(|b| {
                    let mut r = json!({"buildId": b.id, "generation": b.generation, "path": b.path,
                                       "target": b.params["target"],
                                       "configuration": b.params["configuration"].as_str().unwrap_or("Debug"),
                                       "toolchain": {"kind": "dotnet", "path": "dotnet"},
                                       "commandLine": format!("dotnet build {}", b.path),
                                       "elapsedMs": b.started.elapsed().as_secs_f64() * 1e3,
                                       "output": {"firstSeq": 0, "nextSeq": b.seq, "text": b.output,
                                                  "truncated": false}});
                    if let Some(p) = &b.progress {
                        r["progress"] = p.clone();
                    }
                    r
                });
                let mut result = json!({ "running": running });
                if let Some(last) = &s.last_build {
                    result["last"] = last.clone();
                }
                drop(s);
                reply(result);
            }
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
            methods::TEST_DISCOVER
            | methods::TEST_RUN
            | methods::TEST_CANCEL
            | methods::TEST_ATTACHED
            | methods::TEST_STATUS => self.answer_test(method, params, &reply, &error),
            methods::NUGET_SEARCH
            | methods::NUGET_INSTALLED
            | methods::NUGET_UPDATES
            | methods::NUGET_CHANGE
            | methods::NUGET_SOURCES
            | methods::NUGET_RESTORE
            | methods::NUGET_ICON => {
                let (this, out, id) = (self.clone(), out.clone(), id.clone());
                let (method, params) = (method.to_owned(), params.clone());
                thread::spawn(move || this.serve_nuget(&out, &id, &method, &params));
            }
            methods::PROJECT_PROPERTIES
            | methods::PROJECT_SET_PROPERTY
            | methods::PROJECT_LAUNCH_PROFILES
            | methods::PROJECT_SET_LAUNCH_PROFILE
            | methods::SOLUTION_CONFIGURATIONS
            | methods::SOLUTION_SET_CONFIGURATION => {
                self.answer_projects(method, params, &reply, &error, &notify)
            }
            methods::RESX_SETS | methods::RESX_DESIGNER => {
                self.answer_resx(method, params, &reply, &error, &notify)
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
    /// One `eludite/nuget/*` request, on a thread of its own (brief 0048).
    fn serve_nuget(&self, out: &Writer, id: &Value, method: &str, params: &Value) {
        let reply =
            |result: Value| send(out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
        let error = |code: i64, message: &str, data: Option<Value>| {
            let mut e = json!({"code": code, "message": message});
            if let Some(d) = data {
                e["data"] = d;
            }
            send(out, json!({"jsonrpc": "2.0", "id": id, "error": e}));
        };
        let (current, open) = {
            let mut s = self.lock();
            s.nuget.calls += 1;
            (s.generation, s.solution.clone())
        };
        if method == methods::NUGET_ICON {
            let url = params["url"].as_str().unwrap_or_default();
            let path = match url.strip_prefix("file://") {
                Some(p) => Some(p.to_owned()),
                None => self.lock().nuget.icons.get(url).cloned(),
            };
            return reply(json!({ "path": path }));
        }
        let Some(generation) = params["generation"].as_u64() else {
            return error(
                -32602,
                "params.generation (a non-negative integer) is required",
                None,
            );
        };
        if generation != current {
            return error(
                host::error_codes::CONTENT_MODIFIED,
                "stale",
                Some(json!({"requestedGeneration": generation, "currentGeneration": current})),
            );
        }
        let Some(solution) = open else {
            return error(-32602, "no solution is open", None);
        };
        let operation = params["operation"].as_u64();
        let mut seq = 0;
        let mut lines = |lines: &[String]| {
            if let Some(op) = operation {
                for u in output_updates(op, generation, &mut seq, lines) {
                    self.notify(methods::NUGET_UPDATE, u);
                }
            }
        };
        match method {
            methods::NUGET_SEARCH => {
                let mut retry = false;
                let (results, rows, needs) = loop {
                    let (results, rows, needs) = self.lock().nuget.search(params);
                    let Some(host) = needs.clone().filter(|_| params["interactive"] == true) else {
                        break (results, rows, needs);
                    };
                    let source = self
                        .lock()
                        .nuget
                        .sources
                        .iter()
                        .find(|s| {
                            s["private"] == true
                                && host_of(s["url"].as_str().unwrap_or_default()) == host
                        })
                        .cloned()
                        .unwrap_or(Value::Null);
                    let answer = self.request_shell(
                        methods::NUGET_CREDENTIALS,
                        json!({"operation": operation, "source": source["name"], "url": source["url"],
                               "host": host, "proxy": false, "isRetry": retry}),
                        Duration::from_secs(60),
                    );
                    let given = answer
                        .as_ref()
                        .map(|a| a["result"].clone())
                        .filter(|r| r.is_object() && r["canceled"] != true);
                    let Some(given) = given else {
                        break (results, rows, needs);
                    };
                    let mut s = self.lock();
                    let ok = s.nuget.credentials.as_ref().is_some_and(|(u, p)| {
                        given["username"] == u.as_str() && given["password"] == p.as_str()
                    });
                    if ok {
                        s.nuget.authed = true;
                    } else if retry {
                        drop(s);
                        break (results, rows, needs);
                    }
                    retry = true;
                };
                let delay = self.lock().nuget.search_delay;
                thread::sleep(delay);
                if !rows.is_empty() && rows.iter().all(|r| r.get("error").is_some()) {
                    let reason = if needs.is_some() {
                        "credentialsRequired"
                    } else {
                        "sourceFailed"
                    };
                    let message = match &needs {
                        Some(h) => format!(
                            "credentials_required: {h} asked for credentials and none were given"
                        ),
                        None => "every package source failed".into(),
                    };
                    return error(
                        -32014,
                        &message,
                        Some(json!({"reason": reason, "host": needs, "source": rows[0]["name"]})),
                    );
                }
                reply(
                    json!({"generation": generation, "results": results, "sources": rows, "elapsedMs": 3.0}),
                );
            }
            methods::NUGET_INSTALLED => {
                let projects = self.lock().nuget.installed(params);
                if params["metadata"] == true
                    && let Some(op) = operation
                {
                    let packages = FakeNuGet::metadata_packages(&projects);
                    if !packages.is_empty() {
                        self.notify(
                            methods::NUGET_UPDATE,
                            json!({"operation": op, "generation": generation, "seq": seq, "kind": "metadata",
                                   "packages": packages}),
                        );
                    }
                }
                reply(json!({"generation": generation, "projects": projects, "elapsedMs": 2.0}));
            }
            methods::NUGET_UPDATES => {
                let (updates, rows) = self.lock().nuget.updates(params);
                reply(
                    json!({"generation": generation, "updates": updates, "sources": rows, "elapsedMs": 2.0}),
                );
            }
            methods::NUGET_CHANGE => {
                let changed = self.lock().nuget.change(params);
                let (written, projects, output) = match changed {
                    Ok(c) => c,
                    Err((code, message, data)) => return error(code, &message, Some(data)),
                };
                lines(&output);
                if projects.is_empty() {
                    lines(&["No change.".into(), "========== Finished ==========".into()]);
                    return reply(
                        json!({"generation": generation, "action": params["action"], "packages": written,
                                        "projects": [], "edited": [], "elapsedMs": 5.0,
                                        "message": "Nothing changed."}),
                    );
                }
                let restore = if params["restore"] == false {
                    None
                } else {
                    let (outcome, restore_lines) = self.lock().nuget.restore();
                    lines(&restore_lines);
                    Some(outcome)
                };
                lines(&["========== Finished ==========".into()]);
                let next = {
                    let mut s = self.lock();
                    s.generation += 1;
                    s.generation
                };
                let count = self.lock().tree.as_array().map_or(0, Vec::len);
                self.notify(
                    methods::SOLUTION_STATUS,
                    json!({"generation": next, "path": solution, "state": "loaded", "elapsedMs": 1.0,
                           "counts": {"projects": count, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
                );
                let edited: Vec<Value> = projects
                    .iter()
                    .map(|p| json!({"path": p, "kind": "project", "changes": ["PackageReference changed"]}))
                    .collect();
                let mut result = json!({"generation": next, "action": params["action"], "packages": written,
                                        "projects": projects, "edited": edited, "elapsedMs": 8.0});
                if let Some(r) = restore {
                    result["restore"] = r;
                }
                reply(result);
            }
            methods::NUGET_SOURCES => {
                let answer = self.lock().nuget.sources_call(params);
                match answer {
                    Ok(r) => reply(r),
                    Err((code, message)) => error(code, &message, None),
                }
            }
            methods::NUGET_RESTORE => {
                let (mut outcome, restore_lines) = self.lock().nuget.restore();
                lines(&restore_lines);
                lines(&["========== Finished ==========".into()]);
                outcome["generation"] = json!(generation);
                reply(outcome);
            }
            other => error(-32601, &format!("{other} not found"), None),
        }
    }

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
        let configuration = params["configuration"].as_str().unwrap_or("Debug");
        let first = format!("Build started...\n> dotnet build {path} -c {configuration}\n");
        s.build = Some(FakeBuild {
            id,
            seq: 1,
            params: params.clone(),
            generation,
            path: path.clone(),
            started: Instant::now(),
            output: first.clone(),
            progress: None,
        });
        drop(s);
        reply(
            json!({"buildId": id, "generation": generation, "path": path, "target": params["target"],
                     "configuration": configuration, "platform": params.get("platform").cloned().unwrap_or(Value::Null),
                     "toolchain": {"kind": "dotnet", "path": "dotnet"}, "binlog": null,
                     "commandLine": format!("dotnet build {path}")}),
        );
        notify(
            methods::BUILD_OUTPUT,
            json!({"buildId": id, "seq": 0, "text": first}),
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
