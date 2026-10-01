//! The client against [`eludite_lsp::fake::FakeHost`], the in-process fake the shell's tests use: lifecycle, the
//! tree request, generation tracking, notifications recorded by the fake, injected diagnostics, a stall that leaves
//! the client's non-blocking calls fast, and a restart after the in-process host ends.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eludite_lsp::fake::FakeHost;
use eludite_lsp::host::{self, SolutionState, TreeProjectKind};
use eludite_lsp::lsp::{self, DidOpenTextDocumentParams, TextDocumentItem};
use eludite_lsp::{ClientInfo, Event, HostClient, HostEvent, RestartPolicy};
use serde_json::json;

const T: Duration = Duration::from_secs(10);

fn start(fake: &FakeHost, max_restarts: u32) -> (HostClient, Receiver<Event>) {
    HostClient::start_in_process(
        fake.connector(),
        ClientInfo {
            name: "eludite-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts,
            backoff: Duration::ZERO,
        },
    )
    .expect("start in-process fake host")
}

fn next(rx: &Receiver<Event>, pred: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + T;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(e) if pred(&e) => return e,
            Ok(_) => {}
            Err(_) => panic!("timed out waiting for an event"),
        }
    }
}

#[test]
fn tree_status_and_recorded_notifications() {
    let fake = FakeHost::new();
    fake.set_tree(
        json!([{"name": "App", "path": "/src/App/App.csproj", "kind": "sdk",
                          "targetFrameworks": ["net10.0"],
                          "files": [{"path": "/src/App/Program.cs", "itemType": "compile"}]}]),
    );
    let (client, rx) = start(&fake, 0);
    assert_eq!(client.pid(), None);
    assert_eq!(client.open_solution("/src/App.slnx", T).unwrap(), 1);
    let Event::SolutionStatus(loaded) = next(
        &rx,
        |e| matches!(e, Event::SolutionStatus(s) if s.state == SolutionState::Loaded),
    ) else {
        unreachable!()
    };
    assert_eq!(loaded.counts.unwrap().projects, 1);

    let tree = client
        .request::<host::SolutionTreeRequest>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(tree.generation, 1);
    assert_eq!(tree.path.as_deref(), Some("/src/App.slnx"));
    assert_eq!(tree.projects[0].kind, TreeProjectKind::Sdk);
    assert_eq!(tree.projects[0].files[0].path, "/src/App/Program.cs");

    client
        .notify::<lsp::DidOpenTextDocument>(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: "file:///src/App/Program.cs".into(),
                language_id: "csharp".into(),
                version: 1,
                text: "class P {}".into(),
            },
        })
        .unwrap();
    let opened = fake
        .wait_for("textDocument/didOpen", T, |p| {
            p["textDocument"]["version"] == 1
        })
        .expect("didOpen recorded");
    assert_eq!(opened.params["textDocument"]["text"], "class P {}");

    fake.publish_diagnostics(
        "file:///src/App/Program.cs",
        1,
        json!([{"range": {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 7}},
                "severity": 1, "code": "CS0000", "message": "boom"}]),
    );
    let Event::Diagnostics(d) = next(&rx, |e| matches!(e, Event::Diagnostics(_))) else {
        unreachable!()
    };
    assert_eq!(d.generation, 1);
    assert_eq!(d.params.diagnostics[0].message, "boom");
    client.shutdown(T).unwrap();
}

#[test]
fn stalled_host_does_not_block_notify_or_request() {
    let fake = FakeHost::new();
    let (client, _rx) = start(&fake, 0);
    fake.stall_for(Duration::from_millis(600));
    // Writes go into the pipe; nothing waits for the stalled host.
    let t = Instant::now();
    let pending = client.request::<host::Ping>(()).unwrap();
    client
        .notify_untyped(
            "textDocument/didSave",
            json!({"textDocument": {"uri": "file:///a"}}),
        )
        .unwrap();
    assert!(
        t.elapsed() < Duration::from_millis(100),
        "{:?}",
        t.elapsed()
    );
    let pong = pending.wait_timeout(T).unwrap();
    assert!(pong.pong);
    assert!(t.elapsed() >= Duration::from_millis(500));
    client.shutdown(T).unwrap();
}

#[test]
fn killed_in_process_host_restarts() {
    let fake = FakeHost::new();
    let (client, rx) = start(&fake, 1);
    client.open_solution("/src/App.slnx", T).unwrap();
    client.kill().unwrap();
    assert!(matches!(
        next(&rx, |e| matches!(e, Event::Host(HostEvent::Exited { .. }))),
        Event::Host(HostEvent::Exited {
            code: None,
            expected: false
        })
    ));
    next(&rx, |e| {
        matches!(e, Event::Host(HostEvent::Restarted { .. }))
    });
    assert_eq!(fake.connections(), 2);
    assert_eq!(client.generation(), 0);
    client.shutdown(T).unwrap();
}
