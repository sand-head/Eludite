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

#[test]
fn typed_signature_help_scripted_replies_and_cancel() {
    use eludite_lsp::fake::FakeReply;
    use eludite_lsp::lsp::{Position, SignatureHelpParams, TextDocumentIdentifier};
    let fake = FakeHost::new();
    fake.respond("textDocument/signatureHelp", |p| {
        assert_eq!(p["position"]["character"], 9);
        FakeReply::Result(json!({"signatures": [{"label": "void M(int a, int b)",
            "parameters": [{"label": "int a"}, {"label": "int b"}]}], "activeSignature": 0, "activeParameter": 1}))
    });
    fake.respond("textDocument/completion", |_| FakeReply::Hold);
    let (client, _rx) = start(&fake, 0);
    client.open_solution("/src/App.slnx", T).unwrap();
    let params = SignatureHelpParams {
        text_document: TextDocumentIdentifier {
            uri: "file:///a.cs".into(),
        },
        position: Position {
            line: 0,
            character: 9,
        },
        context: None,
    };
    let help = client
        .request::<lsp::SignatureHelpRequest>(params)
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("a signature");
    let s = &help.signatures[0];
    assert_eq!(help.active_parameter, Some(1));
    assert_eq!(&s.label[s.parameter_range(1).unwrap()], "int b");
    let sent = fake.received_params("textDocument/signatureHelp");
    assert_eq!(sent[0]["eluditeGeneration"], 1);

    // A held request ends with RequestCancelled once canceled.
    let completion = client
        .request::<lsp::Completion>(lsp::CompletionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///a.cs".into(),
            },
            position: Position {
                line: 0,
                character: 1,
            },
            context: None,
        })
        .unwrap();
    let deadline = Instant::now() + T;
    while fake.inflight() == 0 {
        assert!(Instant::now() < deadline);
        std::thread::sleep(Duration::from_millis(1));
    }
    completion.cancel();
    assert!(matches!(
        completion.wait_timeout(T),
        Err(eludite_lsp::Error::Canceled)
    ));
    assert_eq!(fake.inflight(), 0);
    assert_eq!(fake.received_params("$/cancelRequest").len(), 1);

    // With cancels ignored, a delayed result still arrives after a raw `$/cancelRequest`.
    fake.set_ignore_cancel(true);
    fake.respond("textDocument/completion", |_| {
        FakeReply::After(
            Duration::from_millis(20),
            json!({"isIncomplete": false, "items": [{"label": "Late"}]}),
        )
    });
    let late = client
        .request::<lsp::Completion>(lsp::CompletionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///a.cs".into(),
            },
            position: Position {
                line: 0,
                character: 1,
            },
            context: None,
        })
        .unwrap();
    client
        .notify::<lsp::Cancel>(lsp::CancelParams { id: late.id() })
        .unwrap();
    let items = late.wait_timeout(T).unwrap().unwrap();
    assert_eq!(items.items()[0].label, "Late");
}

#[test]
fn held_load_stays_loading_until_finished() {
    let fake = FakeHost::new();
    fake.set_hold_load(true);
    let (client, rx) = start(&fake, 0);
    client.open_solution("/src/App.slnx", T).unwrap();
    next(
        &rx,
        |e| matches!(e, Event::SolutionStatus(s) if s.state == SolutionState::Loading),
    );
    assert!(rx.recv_timeout(Duration::from_millis(50)).map_or(
        true,
        |e| !matches!(e, Event::SolutionStatus(s) if s.state == SolutionState::Loaded)
    ));
    fake.finish_load();
    next(
        &rx,
        |e| matches!(e, Event::SolutionStatus(s) if s.state == SolutionState::Loaded),
    );
}

#[test]
fn typed_definition_and_references_through_the_fake_host() {
    use eludite_lsp::fake::FakeReply;
    use eludite_lsp::lsp::{
        Position, ReferenceContext, ReferenceParams, TextDocumentIdentifier,
        TextDocumentPositionParams,
    };
    let fake = FakeHost::new();
    let metadata =
        "file:///tmp/MetadataAsSource/1/DecompilationMetadataAsSourceFileProvider/2/JsonRpc.cs";
    fake.respond("textDocument/definition", move |_| {
        FakeReply::Result(json!([{"uri": metadata,
            "range": {"start": {"line": 30, "character": 13}, "end": {"line": 30, "character": 20}}}]))
    });
    fake.respond("textDocument/references", |p| {
        assert_eq!(p["context"]["includeDeclaration"], true);
        let at = |line| json!({"uri": "file:///a.cs", "range": {"start": {"line": line, "character": 4}, "end": {"line": line, "character": 9}}});
        FakeReply::Result(json!([at(1), at(7)]))
    });
    let (client, _rx) = start(&fake, 0);
    client.open_solution("/src/App.slnx", T).unwrap();
    let doc = TextDocumentIdentifier {
        uri: "file:///a.cs".into(),
    };
    let position = Position {
        line: 1,
        character: 5,
    };
    let targets = client
        .request::<lsp::GotoDefinition>(TextDocumentPositionParams {
            text_document: doc.clone(),
            position,
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("a definition")
        .into_locations();
    assert_eq!(targets[0].uri, metadata);
    assert_eq!(targets[0].range.start.line, 30);
    let refs = client
        .request::<lsp::References>(ReferenceParams {
            text_document: doc,
            position,
            context: ReferenceContext {
                include_declaration: true,
            },
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("references");
    assert_eq!(
        refs.iter().map(|l| l.range.start.line).collect::<Vec<_>>(),
        [1, 7]
    );
    for method in ["textDocument/definition", "textDocument/references"] {
        assert_eq!(fake.received_params(method)[0]["eluditeGeneration"], 1);
    }
}

#[test]
fn typed_rename_and_code_actions_through_the_fake_host() {
    use eludite_lsp::fake::FakeReply;
    use eludite_lsp::lsp::{
        CodeActionContext, CodeActionOrCommand, CodeActionParams, DocumentChange, Position,
        PrepareRenameResponse, Range, RenameParams, TextDocumentIdentifier,
        TextDocumentPositionParams,
    };
    let fake = FakeHost::new();
    let r = json!({"start": {"line": 2, "character": 4}, "end": {"line": 2, "character": 8}});
    let r2 = r.clone();
    fake.respond("textDocument/prepareRename", move |_| {
        FakeReply::Result(r2.clone())
    });
    fake.respond("textDocument/rename", move |p| {
        FakeReply::Result(json!({"documentChanges": [{"textDocument": {"uri": p["textDocument"]["uri"], "version": null},
            "edits": [{"range": r, "newText": p["newName"]}]}]}))
    });
    fake.respond("textDocument/codeAction", |p| {
        assert_eq!(p["context"]["triggerKind"], 2);
        FakeReply::Result(
            json!([{"title": "Use primary constructor", "kind": "quickfix", "data": {"id": 1}}]),
        )
    });
    fake.respond("codeAction/resolve", |p| {
        assert_eq!(p["data"]["id"], 1);
        FakeReply::Result(json!({"title": p["title"], "data": p["data"], "edit": {"changes": {}}}))
    });
    let (client, _rx) = start(&fake, 0);
    client.open_solution("/src/App.slnx", T).unwrap();
    let doc = TextDocumentIdentifier {
        uri: "file:///a.cs".into(),
    };
    let position = Position {
        line: 2,
        character: 5,
    };
    let prepared = client
        .request::<lsp::PrepareRename>(TextDocumentPositionParams {
            text_document: doc.clone(),
            position,
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(matches!(prepared, Some(PrepareRenameResponse::Range(r)) if r.start.character == 4));
    let edit = client
        .request::<lsp::Rename>(RenameParams {
            text_document: doc.clone(),
            position,
            new_name: "Pong".into(),
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("an edit");
    assert!(
        matches!(&edit.document_changes.as_ref().unwrap()[0], DocumentChange::Edit(e) if e.edits[0].new_text == "Pong")
    );
    let actions = client
        .request::<lsp::CodeActionRequest>(CodeActionParams {
            text_document: doc,
            range: Range {
                start: position,
                end: position,
            },
            context: CodeActionContext {
                diagnostics: vec![],
                only: None,
                trigger_kind: Some(2),
            },
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("actions");
    let CodeActionOrCommand::Action(action) = &actions[0] else {
        panic!("{actions:?}")
    };
    let resolved = client
        .request::<lsp::ResolveCodeAction>((**action).clone())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(resolved.edit.is_some());
    for method in [
        "textDocument/prepareRename",
        "textDocument/rename",
        "textDocument/codeAction",
        "codeAction/resolve",
    ] {
        assert_eq!(
            fake.received_params(method)[0]["eluditeGeneration"],
            1,
            "{method}"
        );
    }
}

#[test]
fn apply_edit_requests_from_the_host_are_events_and_answered() {
    let fake = FakeHost::new();
    let (client, rx) = start(&fake, 0);
    client.open_solution("/src/App.slnx", T).unwrap();
    let edit = json!({"changes": {"file:///a.cs": [
        {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// x\n"}]}});
    let shell = {
        let client = client.clone();
        std::thread::spawn(move || {
            let Event::ApplyEdit { id, params } =
                next(&rx, |e| matches!(e, Event::ApplyEdit { .. }))
            else {
                unreachable!()
            };
            assert_eq!(params.generation, 1);
            assert_eq!(params.params.label.as_deref(), Some("Fix"));
            assert!(params.params.edit.changes.is_some());
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
            rx
        })
    };
    let response = fake
        .apply_edit(json!({"label": "Fix", "edit": edit}), T)
        .expect("the shell answers");
    assert_eq!(response["result"], json!({"applied": true}));
    let _rx = shell.join().unwrap();
    // A malformed request is InvalidParams; any other method is MethodNotFound.
    let bad = fake.apply_edit(json!({"edit": 3}), T).unwrap();
    assert_eq!(bad["error"]["code"], -32602);
    let other = fake
        .request_shell("window/showDocument", json!({}), T)
        .unwrap();
    assert_eq!(other["error"]["code"], -32601);
}
