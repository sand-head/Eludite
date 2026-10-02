//! The generic client ([`eludite_lsp::ServerClient`], brief 0019) against the scripted in-process
//! [`FakeServer`]: the LSP handshake with the shared client capabilities and the registration's initialization
//! options, document notifications through the shared connection, work-done progress and `serverStatus` as events,
//! pushed diagnostics, a typed completion, cancellation, `workspace/configuration` from the registration's settings,
//! `workspace/applyEdit` relayed as an event, a crash restart with a new generation, and the restart budget.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eludite_lsp::fake::FakeReply;
use eludite_lsp::fake_server::FakeServer;
use eludite_lsp::lsp::{self, DidOpenTextDocumentParams, TextDocumentItem};
use eludite_lsp::{
    ClientInfo, Error, Event, HostEvent, RestartPolicy, ServerClient, ServerRegistry, ServerSetup,
};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);

fn setup() -> ServerSetup {
    let registry = ServerRegistry::builtin();
    let ra = registry.get("rust-analyzer").unwrap();
    ServerSetup {
        name: ra.id.clone(),
        client: ClientInfo {
            name: "eludite-test".into(),
            version: "0".into(),
        },
        root: std::env::temp_dir().join("ws"),
        initialization_options: ra.initialization_options.clone(),
        settings: ra.settings.clone(),
    }
}

fn start(fake: &FakeServer, max_restarts: u32) -> (ServerClient, Receiver<Event>) {
    ServerClient::start_in_process(
        fake.connector(),
        setup(),
        RestartPolicy {
            max_restarts,
            backoff: Duration::ZERO,
        },
    )
    .expect("start the fake server")
}

fn next(rx: &Receiver<Event>, what: &str, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + T;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

fn open(client: &ServerClient, uri: &str, text: &str) {
    client
        .connection()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.into(),
                language_id: "rust".into(),
                version: 1,
                text: text.into(),
            },
        })
        .unwrap();
}

#[test]
fn handshake_sends_lsp_initialize_with_registration_options_then_initialized() {
    let fake = FakeServer::new();
    let (client, _rx) = start(&fake, 0);
    let init = fake.wait_for("initialize", T, |_| true).unwrap();
    assert_eq!(init.params["clientInfo"]["name"], "eludite-test");
    assert!(
        init.params["rootUri"]
            .as_str()
            .unwrap()
            .starts_with("file:///")
    );
    assert_eq!(init.params["workspaceFolders"][0]["name"], "ws");
    assert_eq!(
        init.params["initializationOptions"]["cargo"]["targetDir"],
        true
    );
    assert_eq!(
        init.params["capabilities"]["experimental"]["serverStatusNotification"],
        true
    );
    // No Eludite vocabulary on a generic connection.
    assert!(init.params.get("eluditeGeneration").is_none());
    assert!(fake.wait_for("initialized", T, |_| true).is_some());
    assert_eq!(client.generation(), 1);
    assert_eq!(
        client.server_info(),
        Some(("fake-analyzer".into(), Some("0.0.0-fake".into())))
    );
    assert_eq!(client.capabilities().unwrap()["hoverProvider"], true);
    assert_eq!(client.shutdown(T).unwrap(), None);
    assert!(fake.wait_for("shutdown", T, |_| true).is_some());
    assert!(fake.wait_for("exit", T, |_| true).is_some());
}

#[test]
fn progress_and_server_status_arrive_as_events() {
    let fake = FakeServer::new();
    fake.set_quiescent_after_init(false);
    let (_client, rx) = start(&fake, 0);
    let Event::Progress(begin) = next(
        &rx,
        "progress begin",
        |e| matches!(e, Event::Progress(p) if p.kind == "begin"),
    ) else {
        unreachable!()
    };
    assert_eq!(begin.title.as_deref(), Some("Indexing"));
    let Event::Progress(report) = next(
        &rx,
        "progress report",
        |e| matches!(e, Event::Progress(p) if p.kind == "report"),
    ) else {
        unreachable!()
    };
    assert_eq!(
        (report.message.as_deref(), report.percentage),
        (Some("1/2 (core)"), Some(50))
    );
    let Event::ServerStatus(busy) = next(&rx, "status", |e| matches!(e, Event::ServerStatus(_)))
    else {
        unreachable!()
    };
    assert!(!busy.quiescent);
    fake.finish_indexing();
    next(
        &rx,
        "progress end",
        |e| matches!(e, Event::Progress(p) if p.kind == "end"),
    );
    let Event::ServerStatus(ready) = next(&rx, "status", |e| matches!(e, Event::ServerStatus(_)))
    else {
        unreachable!()
    };
    assert!(ready.quiescent);
    assert_eq!(ready.health, "ok");
}

#[test]
fn documents_diagnostics_and_completion_through_the_shared_connection() {
    let fake = FakeServer::new();
    fake.diagnose_on_open(
        "/src/main.rs",
        json!([{"range": {"start": {"line": 0, "character": 12}, "end": {"line": 0, "character": 15}},
                "severity": 1, "code": "E0308", "source": "rustc", "message": "mismatched types"}]),
    );
    fake.respond("textDocument/completion", |p| {
        assert!(p.get("eluditeGeneration").is_none());
        FakeReply::Result(json!({"isIncomplete": false, "items": [{"label": "len", "kind": 2}]}))
    });
    let (client, rx) = start(&fake, 0);
    let uri = "file:///ws/src/main.rs";
    open(&client, uri, "fn main() { 1u8 + \"\"; }\n");
    let Event::Diagnostics(d) = next(&rx, "diagnostics", |e| matches!(e, Event::Diagnostics(_)))
    else {
        unreachable!()
    };
    assert_eq!(d.params.uri, uri);
    assert_eq!(d.generation, client.generation());
    assert_eq!(d.params.diagnostics[0].message, "mismatched types");

    let completion = client
        .request::<lsp::Completion>(lsp::CompletionParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.into() },
            position: lsp::Position {
                line: 0,
                character: 4,
            },
            context: None,
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let Some(lsp::CompletionResponse::List(list)) = completion else {
        panic!("{completion:?}")
    };
    assert_eq!(list.items[0].label, "len");

    client.connection().did_save(uri).unwrap();
    client
        .connection()
        .did_close(lsp::DidCloseTextDocumentParams {
            text_document: lsp::TextDocumentIdentifier { uri: uri.into() },
        })
        .unwrap();
    assert!(fake.wait_for("textDocument/didSave", T, |_| true).is_some());
    assert!(
        fake.wait_for("textDocument/didClose", T, |p| p["textDocument"]["uri"]
            == uri)
            .is_some()
    );
}

#[test]
fn cancel_ends_a_held_request() {
    let fake = FakeServer::new();
    fake.respond("textDocument/hover", |_| FakeReply::Hold);
    let (client, _rx) = start(&fake, 0);
    let pending = client
        .request_untyped(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///ws/a.rs"}, "position": {"line": 0, "character": 0}}),
        )
        .unwrap();
    fake.wait_for("textDocument/hover", T, |_| true).unwrap();
    let started = Instant::now();
    pending.cancel();
    assert!(matches!(pending.wait_timeout(T), Err(Error::Canceled)));
    assert!(started.elapsed() < Duration::from_millis(500));
    assert!(fake.wait_for("$/cancelRequest", T, |_| true).is_some());
}

#[test]
fn configuration_and_apply_edit_requests_from_the_server() {
    let fake = FakeServer::new();
    let (client, rx) = start(&fake, 0);
    fake.wait_for("initialized", T, |_| true).unwrap();
    let answer = fake
        .request_client(
            "workspace/configuration",
            json!({"items": [{"section": "rust-analyzer"}, {"section": "editor"}]}),
            T,
        )
        .unwrap();
    assert_eq!(answer["result"][0]["cargo"]["targetDir"], true);
    assert_eq!(answer["result"][1], Value::Null);
    let created = fake
        .request_client("window/workDoneProgress/create", json!({"token": "x"}), T)
        .unwrap();
    assert_eq!(created["result"], Value::Null);
    let unknown = fake.request_client("custom/thing", json!({}), T).unwrap();
    assert_eq!(unknown["error"]["code"], -32601);

    let fake2 = fake.clone();
    let waiter = std::thread::spawn(move || {
        fake2.request_client(
            "workspace/applyEdit",
            json!({"label": "fix", "edit": {"changes": {}}}),
            T,
        )
    });
    let Event::ApplyEdit { id, params } =
        next(&rx, "applyEdit", |e| matches!(e, Event::ApplyEdit { .. }))
    else {
        unreachable!()
    };
    assert_eq!(params.generation, client.generation());
    assert_eq!(params.params.label.as_deref(), Some("fix"));
    client
        .respond_apply_edit(
            id,
            lsp::ApplyWorkspaceEditResult {
                applied: true,
                failure_reason: None,
                failed_change: None,
            },
        )
        .unwrap();
    assert_eq!(waiter.join().unwrap().unwrap()["result"]["applied"], true);
}

#[test]
fn a_crash_restarts_with_a_new_generation_and_drops_old_results() {
    let fake = FakeServer::new();
    fake.respond("textDocument/hover", |_| FakeReply::Hold);
    let (client, rx) = start(&fake, 2);
    assert_eq!(client.generation(), 1);
    let pending = client
        .request_untyped(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///ws/a.rs"}, "position": {"line": 0, "character": 0}}),
        )
        .unwrap();
    fake.wait_for("textDocument/hover", T, |_| true).unwrap();
    fake.crash();
    assert!(matches!(pending.wait_timeout(T), Err(Error::HostExited)));
    next(&rx, "exit", |e| {
        matches!(
            e,
            Event::Host(HostEvent::Exited {
                expected: false,
                ..
            })
        )
    });
    next(&rx, "restart", |e| {
        matches!(e, Event::Host(HostEvent::Restarted { attempt: 1, .. }))
    });
    assert_eq!(fake.connections(), 2);
    assert_eq!(client.generation(), 2);
    assert_eq!(fake.received_params("initialize").len(), 2);
    // The new process answers.
    fake.respond("textDocument/hover", |_| {
        FakeReply::Result(json!({"contents": "fn main()"}))
    });
    let hover = client
        .request_untyped(
            "textDocument/hover",
            json!({"textDocument": {"uri": "file:///ws/a.rs"}, "position": {"line": 0, "character": 0}}),
        )
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(hover["contents"], "fn main()");
}

#[test]
fn the_restart_budget_gives_up() {
    let fake = FakeServer::new();
    let (_client, rx) = start(&fake, 0);
    fake.wait_for("initialized", T, |_| true).unwrap();
    fake.crash();
    next(
        &rx,
        "gave up",
        |e| matches!(e, Event::Host(HostEvent::GaveUp { reason }) if reason.contains("rust-analyzer")),
    );
    assert_eq!(fake.connections(), 1);
}

#[test]
fn pulled_and_pushed_diagnostics_merge_per_document() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, Ordering};

    let fake = FakeServer::new();
    let mut caps = eludite_lsp::fake_server::analyzer_capabilities();
    caps["diagnosticProvider"] = json!({"identifier": "fake", "interFileDependencies": true,
                                        "workspaceDiagnostics": false});
    fake.set_capabilities(caps);
    let fixed = Arc::new(AtomicBool::new(false));
    let answer = fixed.clone();
    fake.respond("textDocument/diagnostic", move |_| {
        let items = if answer.load(Ordering::SeqCst) {
            json!([])
        } else {
            json!([{"range": {"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 7}},
                    "severity": 1, "code": "E0107", "message": "expected 1 argument, found 0"}])
        };
        FakeReply::Result(json!({"kind": "full", "items": items}))
    });
    let (client, rx) = start(&fake, 0);
    let uri = "file:///ws/src/lib.rs";
    open(&client, uri, "fn f(_a: u8) {}\nfn g() {\n    f();\n}\n");
    let pulled = |e: &Event| matches!(e, Event::Diagnostics(d) if d.params.uri == uri);
    let Event::Diagnostics(first) = next(&rx, "pulled diagnostics", pulled) else {
        unreachable!()
    };
    assert_eq!(first.params.version, Some(1));
    assert_eq!(first.params.diagnostics.len(), 1);
    assert_eq!(first.params.diagnostics[0].code, Some(json!("E0107")));

    // A push (a `cargo check` result) joins the pulled list instead of replacing it.
    fake.publish_diagnostics(
        uri,
        None,
        json!([{"range": {"start": {"line": 0, "character": 3}, "end": {"line": 0, "character": 4}},
                "severity": 2, "code": "dead_code", "message": "function `f` is never used"}]),
    );
    let Event::Diagnostics(both) = next(&rx, "merged diagnostics", pulled) else {
        unreachable!()
    };
    assert_eq!(both.params.diagnostics.len(), 2);

    // An edit is pulled again after the debounce; the fixed error leaves only the pushed warning.
    fixed.store(true, Ordering::SeqCst);
    client
        .connection()
        .did_change(lsp::DidChangeTextDocumentParams {
            text_document: lsp::VersionedTextDocumentIdentifier {
                uri: uri.into(),
                version: 2,
            },
            content_changes: vec![lsp::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: "fn f(_a: u8) {}\nfn g() {\n    f(1);\n}\n".into(),
            }],
        })
        .unwrap();
    let Event::Diagnostics(after) = next(
        &rx,
        "diagnostics after the edit",
        |e| matches!(e, Event::Diagnostics(d) if d.params.uri == uri && d.params.version == Some(2)),
    ) else {
        unreachable!()
    };
    assert_eq!(after.params.diagnostics.len(), 1);
    assert_eq!(after.params.diagnostics[0].code, Some(json!("dead_code")));
    // Open, the edit, and possibly the pull of every open document when the server turned quiescent.
    assert!(fake.received_params("textDocument/diagnostic").len() >= 2);
}
