//! Client tests against a scripted fake `eludite-host` process.
//!
//! This test binary is both the fake host (when `ELUDITE_FAKE_HOST=1`) and the test runner: it re-executes itself as
//! the host, so the client is exercised over a real child process's stdio. It has no libtest harness because the
//! harness would print to stdout, which is the protocol stream.

mod common;

use std::collections::HashMap;
use std::io::{self, BufReader};
use std::panic;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use eludite_lsp::host::{self, LanguageServerState, SolutionState, error_codes};
use eludite_lsp::lsp::{
    self, CompletionContext, CompletionParams, DidOpenTextDocumentParams, Position,
    TextDocumentIdentifier, TextDocumentItem, TextDocumentPositionParams,
};
use eludite_lsp::{
    ClientInfo, Error, Event, HostClient, HostCommand, HostEvent, RestartPolicy, StderrMode,
};
use eludite_protocol::framing;
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn main() {
    if std::env::var_os("ELUDITE_FAKE_HOST").is_some() {
        fake_host();
        return;
    }
    let filter: Vec<String> = std::env::args()
        .skip(1)
        .filter(|a| !a.starts_with('-'))
        .collect();
    let tests: &[(&str, fn())] = &[
        (
            "lifecycle_initialize_ping_shutdown",
            lifecycle_initialize_ping_shutdown,
        ),
        (
            "shutdown_always_reports_the_exit_code",
            shutdown_always_reports_the_exit_code,
        ),
        (
            "status_notifications_track_generation",
            status_notifications_track_generation,
        ),
        (
            "forwarded_requests_carry_the_current_generation",
            forwarded_requests_carry_the_current_generation,
        ),
        (
            "host_rejects_stale_generation",
            host_rejects_stale_generation,
        ),
        (
            "late_result_for_old_generation_is_dropped",
            late_result_for_old_generation_is_dropped,
        ),
        ("cancel_returns_within_50ms", cancel_returns_within_50ms),
        (
            "cancel_discards_a_result_already_sent",
            cancel_discards_a_result_already_sent,
        ),
        (
            "stale_diagnostics_are_dropped",
            stale_diagnostics_are_dropped,
        ),
        (
            "crash_restarts_and_resets_generation",
            crash_restarts_and_resets_generation,
        ),
        ("restart_budget_gives_up", restart_budget_gives_up),
        (
            "stderr_is_captured_not_mixed_into_protocol",
            stderr_is_captured_not_mixed_into_protocol,
        ),
        (
            "unknown_method_is_an_rpc_error",
            unknown_method_is_an_rpc_error,
        ),
    ];
    let mut failed = Vec::new();
    let mut ran = 0;
    for (name, test) in tests {
        if !filter.is_empty() && !filter.iter().any(|f| name.contains(f.as_str())) {
            continue;
        }
        ran += 1;
        let started = Instant::now();
        match panic::catch_unwind(test) {
            Ok(()) => println!("test {name} ... ok ({} ms)", started.elapsed().as_millis()),
            Err(_) => {
                println!("test {name} ... FAILED");
                failed.push(*name);
            }
        }
    }
    println!(
        "\ntest result: {}. {} passed; {} failed",
        if failed.is_empty() { "ok" } else { "FAILED" },
        ran - failed.len(),
        failed.len()
    );
    if !failed.is_empty() {
        std::process::exit(101);
    }
}

// ---------------------------------------------------------------------------------------------- client side

fn start_with(policy: RestartPolicy, stderr: StderrMode) -> (HostClient, Receiver<Event>) {
    let exe = std::env::current_exe().unwrap();
    let command = HostCommand::new(exe)
        .env("ELUDITE_FAKE_HOST", "1")
        .stderr(stderr);
    HostClient::start(
        command,
        ClientInfo {
            name: "eludite-test".into(),
            version: "0".into(),
        },
        policy,
    )
    .expect("start fake host")
}

fn start() -> (HostClient, Receiver<Event>) {
    start_with(
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
        StderrMode::Discard,
    )
}

fn next_event(rx: &Receiver<Event>, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + T;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match rx.recv_timeout(left) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => continue,
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

fn completion_at(line: u32) -> CompletionParams {
    CompletionParams {
        text_document: TextDocumentIdentifier {
            uri: "file:///a.cs".into(),
        },
        position: Position { line, character: 4 },
        context: Some(CompletionContext {
            trigger_kind: 2,
            trigger_character: Some(".".into()),
        }),
    }
}

fn hover(uri: &str) -> TextDocumentPositionParams {
    TextDocumentPositionParams {
        text_document: TextDocumentIdentifier { uri: uri.into() },
        position: Position {
            line: 0,
            character: 0,
        },
    }
}

/// The reader thread records the exit code before it drops the process entry, so a shutdown racing it never reads
/// "exited, code unknown". The race showed as `None` with the real host under heavy load; with the fake host the
/// window is narrow, so this guards the contract over many cycles rather than reproducing the race on demand.
fn shutdown_always_reports_the_exit_code() {
    for _ in 0..40 {
        let (client, _rx) = start();
        client.initialize_result().unwrap();
        assert_eq!(client.shutdown(T).unwrap(), Some(0));
    }
}

fn lifecycle_initialize_ping_shutdown() {
    let (client, rx) = start();
    let init = client.initialize_result().unwrap();
    assert_eq!(init.host_name, host::HOST_NAME);
    assert!(init.capabilities.language_server);
    let pong = client
        .request::<host::Ping>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(pong.pong);
    assert_eq!(client.shutdown(T).unwrap(), Some(0));
    let exited = next_event(&rx, "exit", |e| matches!(e, Event::Host(_)));
    assert_eq!(
        exited,
        Event::Host(HostEvent::Exited {
            code: Some(0),
            expected: true
        })
    );
}

fn status_notifications_track_generation() {
    let (client, rx) = start();
    let running = next_event(&rx, "server status", |e| {
        matches!(e, Event::LanguageServerStatus(_))
    });
    assert!(
        matches!(running, Event::LanguageServerStatus(s) if s.state == LanguageServerState::Running)
    );
    assert_eq!(client.generation(), 0);
    assert_eq!(client.open_solution("/src/App.sln", T).unwrap(), 1);
    assert_eq!(client.generation(), 1);
    let Event::SolutionStatus(loading) =
        next_event(&rx, "loading", |e| matches!(e, Event::SolutionStatus(_)))
    else {
        unreachable!()
    };
    assert_eq!(loading.state, SolutionState::Loading);
    let Event::SolutionStatus(loaded) =
        next_event(&rx, "loaded", |e| matches!(e, Event::SolutionStatus(_)))
    else {
        unreachable!()
    };
    assert_eq!(loaded.state, SolutionState::Loaded);
    assert_eq!(loaded.generation, 1);
    assert_eq!(loaded.counts.unwrap().projects, 3);
    assert_eq!(client.close_solution(T).unwrap(), 2);
    let Event::SolutionStatus(closed) =
        next_event(&rx, "closed", |e| matches!(e, Event::SolutionStatus(_)))
    else {
        unreachable!()
    };
    assert_eq!(closed.state, SolutionState::Closed);
    // A status notification alone also moves the client's generation forward.
    assert_eq!(client.generation(), 2);
    client.shutdown(T).unwrap();
}

fn forwarded_requests_carry_the_current_generation() {
    let (client, _rx) = start();
    client.open_solution("/src/App.sln", T).unwrap();
    let result = client
        .request::<lsp::Completion>(completion_at(1))
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .unwrap();
    let item = &result.items()[0];
    assert_eq!(item.label, "Compute");
    // The fake echoes the eluditeGeneration it received.
    assert_eq!(item.detail.as_deref(), Some("generation 1"));
    // Untyped forwarded requests are pinned too.
    let sig = client
        .request_untyped(
            "textDocument/signatureHelp",
            json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 0}}),
        )
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(sig["generation"], json!(1));
    client.shutdown(T).unwrap();
}

fn host_rejects_stale_generation() {
    let (client, _rx) = start();
    client.open_solution("/src/App.sln", T).unwrap();
    // The fake bumps its generation without telling the client.
    client
        .request_untyped("eludite/solution/open", json!({"path": "/src/silent.sln"}))
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let err = client
        .request::<lsp::HoverRequest>(hover("file:///a.cs"))
        .unwrap()
        .wait_timeout(T)
        .unwrap_err();
    assert!(
        matches!(
            err,
            Error::Stale {
                requested: 1,
                current: 2
            }
        ),
        "{err:?}"
    );
    client.shutdown(T).unwrap();
}

fn late_result_for_old_generation_is_dropped() {
    let (client, _rx) = start();
    client.open_solution("/src/App.sln", T).unwrap();
    // Line 998: the fake answers after 300 ms and does not enforce generations in flight.
    let slow = client
        .request::<lsp::Completion>(completion_at(998))
        .unwrap();
    assert_eq!(client.open_solution("/src/Other.sln", T).unwrap(), 2);
    let err = slow.wait_timeout(T).unwrap_err();
    assert!(
        matches!(
            err,
            Error::Stale {
                requested: 1,
                current: 2
            }
        ),
        "{err:?}"
    );
    client.shutdown(T).unwrap();
}

fn cancel_returns_within_50ms() {
    let (client, _rx) = start();
    let mut worst = Duration::ZERO;
    for _ in 0..20 {
        // Line 999: the fake parks the request until $/cancelRequest.
        let pending = client
            .request::<lsp::Completion>(completion_at(999))
            .unwrap();
        thread::sleep(Duration::from_millis(5));
        let t = Instant::now();
        pending.cancel();
        let err = pending.wait_timeout(T).unwrap_err();
        worst = worst.max(t.elapsed());
        assert!(matches!(err, Error::Canceled), "{err:?}");
    }
    eprintln!("cancel -> return, worst of 20: {worst:?}");
    common::assert_budget(
        "cancel to return, worst of 20",
        worst,
        Duration::from_millis(50),
    );
    client.shutdown(T).unwrap();
}

fn cancel_discards_a_result_already_sent() {
    let (client, _rx) = start();
    // Line 996: the fake ignores the cancel and sends a result after 100 ms anyway.
    let pending = client
        .request::<lsp::Completion>(completion_at(996))
        .unwrap();
    pending.cancel();
    assert!(matches!(pending.wait_timeout(T), Err(Error::Canceled)));
    // The connection is still usable and ids still correlate.
    let ok = client
        .request::<lsp::Completion>(completion_at(1))
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(ok.is_some());
    client.shutdown(T).unwrap();
}

fn stale_diagnostics_are_dropped() {
    let (client, rx) = start();
    client.open_solution("/src/App.sln", T).unwrap();
    client
        .notify::<lsp::DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: "file:///a.cs".into(),
                language_id: "csharp".into(),
                version: 1,
                text: "class A {}".into(),
            },
        })
        .unwrap();
    // The fake publishes one set for generation 0 (stale) and then one for generation 1.
    let Event::Diagnostics(d) =
        next_event(&rx, "diagnostics", |e| matches!(e, Event::Diagnostics(_)))
    else {
        unreachable!()
    };
    assert_eq!(d.generation, 1);
    assert_eq!(d.params.version, Some(1));
    assert_eq!(d.params.diagnostics[0].message, "current");
    client.shutdown(T).unwrap();
}

fn crash_restarts_and_resets_generation() {
    let (client, rx) = start_with(
        RestartPolicy {
            max_restarts: 2,
            backoff: Duration::from_millis(10),
        },
        StderrMode::Discard,
    );
    let first_pid = client.pid().unwrap();
    client.open_solution("/src/App.sln", T).unwrap();
    // The fake exits with code 3 on a hover for file:///crash.
    let err = client
        .request::<lsp::HoverRequest>(hover("file:///crash"))
        .unwrap()
        .wait_timeout(T)
        .unwrap_err();
    assert!(matches!(err, Error::HostExited), "{err:?}");
    let exited = next_event(&rx, "exit", |e| {
        matches!(e, Event::Host(HostEvent::Exited { .. }))
    });
    assert_eq!(
        exited,
        Event::Host(HostEvent::Exited {
            code: Some(3),
            expected: false
        })
    );
    let restarted = next_event(&rx, "restart", |e| {
        matches!(e, Event::Host(HostEvent::Restarted { .. }))
    });
    let Event::Host(HostEvent::Restarted { attempt, pid }) = restarted else {
        unreachable!()
    };
    assert_eq!(attempt, 1);
    assert_ne!(pid, first_pid);
    assert_eq!(client.generation(), 0);
    assert!(
        client
            .request::<host::Ping>(())
            .unwrap()
            .wait_timeout(T)
            .unwrap()
            .pong
    );
    client.shutdown(T).unwrap();
}

fn restart_budget_gives_up() {
    let (client, rx) = start();
    let _ = client
        .request::<lsp::HoverRequest>(hover("file:///crash"))
        .unwrap()
        .wait_timeout(T);
    let gave_up = next_event(&rx, "give up", |e| {
        matches!(e, Event::Host(HostEvent::GaveUp { .. }))
    });
    assert!(matches!(gave_up, Event::Host(HostEvent::GaveUp { .. })));
    assert!(matches!(
        client.request::<host::Ping>(()),
        Err(Error::HostExited)
    ));
}

fn stderr_is_captured_not_mixed_into_protocol() {
    let (client, rx) = start_with(
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
        StderrMode::Capture,
    );
    let log = next_event(&rx, "log line", |e| matches!(e, Event::Log(_)));
    assert_eq!(log, Event::Log("fake-host: listening on stdio".into()));
    client.shutdown(T).unwrap();
}

fn unknown_method_is_an_rpc_error() {
    let (client, _rx) = start();
    let err = client
        .request_untyped("eludite/nope", Value::Null)
        .unwrap()
        .wait_timeout(T)
        .unwrap_err();
    assert!(
        matches!(err, Error::Rpc(ref e) if e.code == -32601),
        "{err:?}"
    );
    client.shutdown(T).unwrap();
}

// ---------------------------------------------------------------------------------------------- fake host

type Out = Arc<Mutex<io::Stdout>>;

fn send(out: &Out, message: Value) {
    let body = serde_json::to_vec(&message).unwrap();
    let mut out = out.lock().unwrap();
    framing::write_message(&mut *out, &body).unwrap();
}

fn reply(out: &Out, id: &Value, result: Value) {
    send(out, json!({"jsonrpc": "2.0", "id": id, "result": result}));
}

fn error(out: &Out, id: &Value, code: i64, message: &str, data: Option<Value>) {
    let mut e = json!({"code": code, "message": message});
    if let Some(d) = data {
        e["data"] = d;
    }
    send(out, json!({"jsonrpc": "2.0", "id": id, "error": e}));
}

fn notify(out: &Out, method: &str, params: Value) {
    send(
        out,
        json!({"jsonrpc": "2.0", "method": method, "params": params}),
    );
}

/// A scripted host: enough of host-rpc.md to exercise the client.
fn fake_host() {
    eprintln!("fake-host: listening on stdio");
    let out: Out = Arc::new(Mutex::new(io::stdout()));
    let mut input = BufReader::new(io::stdin());
    let mut generation: u64 = 0;
    let mut shutdown = false;
    let mut parked: HashMap<String, Value> = HashMap::new();
    while let Ok(Some(body)) = framing::read_message(&mut input) {
        let msg: Value = serde_json::from_slice(&body).unwrap();
        let method = msg["method"].as_str().unwrap_or_default().to_owned();
        let id = msg.get("id").cloned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        let Some(id) = id else {
            // Notifications.
            match method.as_str() {
                "eludite/host/exit" => std::process::exit(if shutdown { 0 } else { 1 }),
                "$/cancelRequest" => {
                    let key = params["id"].to_string();
                    if let Some(parked_id) = parked.remove(&key) {
                        error(
                            &out,
                            &parked_id,
                            error_codes::REQUEST_CANCELLED,
                            "canceled",
                            None,
                        );
                    }
                }
                "textDocument/didOpen" => {
                    let uri = params["textDocument"]["uri"].clone();
                    let version = params["textDocument"]["version"].clone();
                    let diag = |message: &str, g: u64| {
                        json!({"uri": uri, "version": version, "eluditeGeneration": g, "diagnostics": [
                            {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}}, "message": message}]})
                    };
                    if generation > 0 {
                        notify(
                            &out,
                            "textDocument/publishDiagnostics",
                            diag("stale", generation - 1),
                        );
                    }
                    notify(
                        &out,
                        "textDocument/publishDiagnostics",
                        diag("current", generation),
                    );
                }
                _ => {}
            }
            continue;
        };
        match method.as_str() {
            "eludite/host/initialize" => {
                reply(
                    &out,
                    &id,
                    json!({"hostName": "eludite-host", "hostVersion": "fake", "capabilities": {"languageServer": true}}),
                );
                notify(
                    &out,
                    "eludite/languageServer/status",
                    json!({"state": "running", "serverInfo": {"name": "fake-ls"}}),
                );
            }
            "eludite/ping" => reply(
                &out,
                &id,
                json!({"pong": true, "timestamp": "2026-10-01T00:00:00Z"}),
            ),
            "eludite/host/shutdown" => {
                shutdown = true;
                reply(&out, &id, Value::Null);
            }
            "eludite/solution/open" => {
                generation += 1;
                let path = params["path"].as_str().unwrap_or_default().to_owned();
                reply(&out, &id, json!({"generation": generation}));
                if !path.contains("silent") {
                    notify(
                        &out,
                        "eludite/solution/status",
                        json!({"generation": generation, "path": path, "state": "loading", "phase": "projectLoad"}),
                    );
                    notify(
                        &out,
                        "eludite/solution/status",
                        json!({"generation": generation, "path": path, "state": "loaded",
                               "counts": {"projects": 3, "legacyProjects": 0, "legacyEvaluationFailures": 0}}),
                    );
                }
            }
            "eludite/solution/close" => {
                generation += 1;
                reply(&out, &id, json!({"generation": generation}));
                notify(
                    &out,
                    "eludite/solution/status",
                    json!({"generation": generation, "path": "/src/App.sln", "state": "closed"}),
                );
            }
            m if host::methods::FORWARDED_TYPED_REQUESTS.contains(&m)
                || host::methods::FORWARDED_UNTYPED_REQUESTS.contains(&m) =>
            {
                let Some(g) = params[host::GENERATION_FIELD].as_u64() else {
                    error(&out, &id, -32602, "eluditeGeneration is required", None);
                    continue;
                };
                if g != generation {
                    error(
                        &out,
                        &id,
                        error_codes::CONTENT_MODIFIED,
                        "stale",
                        Some(json!({"requestedGeneration": g, "currentGeneration": generation})),
                    );
                    continue;
                }
                match m {
                    "textDocument/hover" if params["textDocument"]["uri"] == "file:///crash" => {
                        std::process::exit(3)
                    }
                    "textDocument/hover" => reply(
                        &out,
                        &id,
                        json!({"contents": {"kind": "markdown", "value": "x"}}),
                    ),
                    "textDocument/completion" => {
                        let list = json!({"isIncomplete": false, "items": [{"label": "Compute", "detail": format!("generation {g}")}]});
                        match params["position"]["line"].as_u64() {
                            Some(999) => {
                                parked.insert(id.to_string(), id);
                            }
                            Some(line @ (996 | 998)) => {
                                let out = out.clone();
                                let delay = if line == 996 { 100 } else { 300 };
                                thread::spawn(move || {
                                    thread::sleep(Duration::from_millis(delay));
                                    reply(&out, &id, list);
                                });
                            }
                            _ => reply(&out, &id, list),
                        }
                    }
                    _ => reply(&out, &id, json!({"generation": g})),
                }
            }
            _ => error(&out, &id, -32601, &format!("{method} not found"), None),
        }
    }
    std::process::exit(if shutdown { 0 } else { 1 });
}
