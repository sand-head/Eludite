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

/// Brief 0017: the build messages arrive typed and in order; a second start is refused with BuildInProgress; cancel
/// ends the build as canceled.
#[test]
fn build_messages_are_typed_and_a_concurrent_build_is_refused() {
    let fake = FakeHost::new();
    fake.set_tree(
        json!([{"name": "App", "path": "/src/App/App.csproj", "kind": "sdk",
                          "targetFrameworks": ["net10.0"], "files": []}]),
    );
    let (client, rx) = start(&fake, 0);
    let start_build = |target| {
        client
            .request::<host::BuildStart>(host::BuildStartParams {
                target,
                system: None,
                project: None,
                configuration: None,
                platform: None,
            })
            .unwrap()
            .wait_timeout(T)
    };
    // No solution open yet.
    assert!(matches!(
        start_build(host::BuildTarget::Build),
        Err(eludite_lsp::Error::Rpc(e)) if e.code == -32602
    ));
    client.open_solution("/src/App.slnx", T).unwrap();
    let started = start_build(host::BuildTarget::Build).unwrap();
    assert_eq!(started.build_id, 1);
    assert_eq!(started.configuration, "Debug");
    assert_eq!(started.toolchain.kind, host::ToolchainKind::Dotnet);
    let Event::BuildOutput(first) = next(&rx, |e| matches!(e, Event::BuildOutput(_))) else {
        unreachable!()
    };
    assert_eq!((first.build_id, first.seq), (1, 0));
    assert!(first.text.starts_with("Build started"));

    let refused = start_build(host::BuildTarget::Rebuild);
    let Err(eludite_lsp::Error::Rpc(e)) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(e.code, host::error_codes::BUILD_IN_PROGRESS);
    let data: host::BuildInProgressData = serde_json::from_value(e.data.unwrap()).unwrap();
    assert_eq!(data.build_id, 1);

    for i in 0..50 {
        fake.build_output(&format!("line {i}\n"));
    }
    fake.build_progress(0, 1, 0, 0);
    let mut seqs = Vec::new();
    while seqs.len() < 50 {
        if let Event::BuildOutput(o) = next(&rx, |e| matches!(e, Event::BuildOutput(_))) {
            assert_eq!(o.text, format!("line {}\n", seqs.len()));
            seqs.push(o.seq);
        }
    }
    assert_eq!(seqs, (1..=50).collect::<Vec<_>>(), "chunks arrive in order");
    let Event::BuildProgress(p) = next(&rx, |e| matches!(e, Event::BuildProgress(_))) else {
        unreachable!()
    };
    assert_eq!(p.projects_total, 1);

    let cancel = client
        .request::<host::BuildCancel>(host::BuildCancelParams::default())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(
        cancel,
        host::BuildCancelResult {
            canceled: true,
            build_id: Some(1)
        }
    );
    let Event::BuildFinished(f) = next(&rx, |e| matches!(e, Event::BuildFinished(_))) else {
        unreachable!()
    };
    assert_eq!(f.result, host::BuildResult::Canceled);
    assert_eq!(f.projects[0].result, host::BuildResult::Canceled);
    assert_eq!(fake.running_build(), None);

    // A new build after the cancel; it fails with one error.
    let again = start_build(host::BuildTarget::Build).unwrap();
    assert_eq!(again.build_id, 2);
    fake.finish_build(
        "failed",
        json!([{"severity": "error", "code": "CS0103", "message": "x", "file": "/src/App/Program.cs",
                "line": 3, "column": 9, "project": "/src/App/App.csproj"}]),
    );
    let Event::BuildFinished(f) = next(&rx, |e| matches!(e, Event::BuildFinished(_))) else {
        unreachable!()
    };
    assert_eq!(f.build_id, 2);
    assert_eq!(f.result, host::BuildResult::Failed);
    assert_eq!(f.summary.errors, 1);
    assert_eq!(f.diagnostics[0].line, Some(3));
    let cancel = client
        .request::<host::BuildCancel>(host::BuildCancelParams::default())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(!cancel.canceled);
}

/// Brief 0020: `eludite/build/status` reports the running build with every chunk so far (also one the client never
/// received) and, after a restart, either the build that survived it or none, with the last finished build.
#[test]
fn build_status_replays_the_running_build_across_a_restart() {
    let fake = FakeHost::new();
    let (client, rx) = start(&fake, 2);
    client.open_solution("/src/App.slnx", T).unwrap();
    let status = |client: &HostClient| {
        client
            .request::<host::BuildStatus>(())
            .unwrap()
            .wait_timeout(T)
            .unwrap()
    };
    assert_eq!(status(&client), host::BuildStatusResult::default());
    let started = client
        .request::<host::BuildStart>(host::BuildStartParams {
            target: host::BuildTarget::Build,
            system: None,
            project: None,
            configuration: Some("Release".into()),
            platform: None,
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    fake.build_output("one\n");
    fake.build_progress(1, 2, 0, 0);
    let running = status(&client).running.expect("a running build");
    assert_eq!(running.build_id, started.build_id);
    assert_eq!(running.configuration, "Release");
    assert_eq!(running.output.next_seq, 2);
    assert!(running.output.text.ends_with("one\n"), "{running:?}");
    assert_eq!(
        running.progress.as_ref().map(|p| p.projects_completed),
        Some(1)
    );

    // The build survives the restart (a host the client reattaches to) and goes on while nobody listens.
    fake.set_build_survives_restart(true);
    client.kill().unwrap();
    next(&rx, |e| {
        matches!(e, Event::Host(HostEvent::Restarted { .. }))
    });
    fake.build_output_unsent("two\n");
    let after = status(&client).running.expect("the build survived");
    assert_eq!(after.output.next_seq, 3);
    assert!(after.output.text.ends_with("one\ntwo\n"));
    fake.finish_build("succeeded", json!([]));
    let done = status(&client);
    assert!(done.running.is_none());
    let last = done.last.expect("the last build");
    assert_eq!(
        (last.build_id, last.result),
        (started.build_id, host::BuildResult::Succeeded)
    );

    // By default a restart ends the running build, as a real host's does.
    fake.set_build_survives_restart(false);
    client.open_solution("/src/App.slnx", T).unwrap();
    client
        .request::<host::BuildStart>(host::BuildStartParams {
            target: host::BuildTarget::Build,
            system: None,
            project: None,
            configuration: None,
            platform: None,
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(status(&client).running.is_some());
    client.kill().unwrap();
    next(&rx, |e| {
        matches!(e, Event::Host(HostEvent::Restarted { .. }))
    });
    assert!(status(&client).running.is_none());
    client.shutdown(T).unwrap();
}

/// Brief 0035: `eludite/test/*` through the fake host: typed updates in order, a second run of a running container
/// refused with -32012, the status of a held run, and cancel.
#[test]
fn test_messages_are_typed_and_a_running_container_is_refused() {
    let fake = FakeHost::new();
    fake.set_tests(json!([{
        "container": {"id": "/s/T/T.csproj|net10.0", "name": "T", "project": "/s/T/T.csproj",
                      "targetFramework": "net10.0", "protocol": "mtp", "runtime": "dotnet"},
        "tests": [
            {"id": "t1", "displayName": "T.C.Adds", "fullyQualifiedName": "T.C.Adds"},
            {"id": "t2", "displayName": "T.C.Subtracts", "fullyQualifiedName": "T.C.Subtracts"}
        ]
    }]));
    let (client, rx) = start(&fake, 0);
    let discover = || {
        client
            .request::<host::TestDiscover>(host::TestDiscoverParams::default())
            .unwrap()
            .wait_timeout(T)
    };
    assert!(matches!(discover(), Err(eludite_lsp::Error::Rpc(e)) if e.code == -32602));
    client.open_solution("/s/T.slnx", T).unwrap();
    let found = discover().unwrap();
    assert_eq!(found.containers[0].protocol, host::TestProtocol::Mtp);
    let mut kinds = Vec::new();
    loop {
        let Event::TestUpdate(u) = next(&rx, |e| matches!(e, Event::TestUpdate(_))) else {
            unreachable!()
        };
        assert_eq!((u.run_id, u.seq), (found.run_id, kinds.len() as u64));
        kinds.push(u.kind);
        if u.kind == host::TestUpdateKind::Discovered {
            assert_eq!(u.tests.as_ref().unwrap().len(), 2);
        }
        if u.kind == host::TestUpdateKind::Finished {
            assert_eq!(u.summary.unwrap().total, 2);
            break;
        }
    }
    assert!(kinds.contains(&host::TestUpdateKind::ContainerFinished));

    fake.set_hold_test_runs(true);
    let run = |tests: Option<Vec<String>>| {
        client
            .request::<host::TestRun>(host::TestRunParams {
                containers: Some(vec![host::TestRunContainer {
                    id: "/s/T/T.csproj|net10.0".into(),
                    tests,
                }]),
                ..Default::default()
            })
            .unwrap()
            .wait_timeout(T)
    };
    let started = run(Some(vec!["t1".into()])).unwrap();
    let refused = run(None);
    let Err(eludite_lsp::Error::Rpc(e)) = refused else {
        panic!("{refused:?}")
    };
    assert_eq!(e.code, host::error_codes::TEST_RUN_IN_PROGRESS);
    let data: host::TestRunInProgressData = serde_json::from_value(e.data.unwrap()).unwrap();
    assert_eq!(data.run_id, started.run_id);
    fake.test_results(
        "/s/T/T.csproj|net10.0",
        json!([{"id": "t1", "outcome": "failed", "message": "boom"}]),
    );
    let Event::TestUpdate(u) = next(
        &rx,
        |e| matches!(e, Event::TestUpdate(u) if u.kind == host::TestUpdateKind::Results),
    ) else {
        unreachable!()
    };
    assert_eq!(u.results.unwrap()[0].outcome, host::TestOutcome::Failed);
    let status = client
        .request::<host::TestStatus>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(status.running[0].next_seq, 1);
    assert_eq!(
        status.running[0].results[0].container.as_deref(),
        Some("/s/T/T.csproj|net10.0")
    );
    let canceled = client
        .request::<host::TestCancel>(host::TestCancelParams::default())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(canceled.canceled);
    let Event::TestUpdate(f) = next(
        &rx,
        |e| matches!(e, Event::TestUpdate(u) if u.kind == host::TestUpdateKind::Finished),
    ) else {
        unreachable!()
    };
    assert_eq!(f.state, Some(host::TestState::Canceled));
    assert_eq!(f.summary.unwrap().failed, 1);
}

/// Brief 0049: the project properties, launch profiles and solution configuration messages are typed end to end, a
/// write reloads the solution (the generation moves on and the client follows it), and a stale write is refused.
#[test]
fn project_property_messages_are_typed_and_writes_follow_the_generation() {
    let fake = FakeHost::new();
    let project = "/src/App/App.csproj";
    fake.set_project_properties(
        project,
        FakeHost::sample_project_properties(project, &["net10.0"]),
    );
    fake.set_launch_profiles(
        project,
        json!([{"name": "App", "commandName": "Project", "environmentVariables": [{"name": "B", "value": "2"},
                                                                                   {"name": "A", "value": "1"}]}]),
    );
    fake.set_solution_configurations(json!({
        "format": "slnx", "configurations": ["Debug", "Release"], "platforms": ["Any CPU", "x64"],
        "projects": [{"name": "App", "path": project, "configurations": ["Debug", "Release"],
                      "platforms": ["Any CPU"], "mappings": [
            {"solutionConfiguration": "Release", "solutionPlatform": "Any CPU", "configuration": "Release",
             "platform": "Any CPU", "build": true}]}]
    }));
    let (client, rx) = start(&fake, 0);
    let generation = client.open_solution("/src/App.slnx", T).unwrap();

    let props = client
        .request::<host::ProjectProperties>(host::ProjectPropertiesParams {
            project: project.into(),
            configuration: Some("Release".into()),
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(
        (props.configuration.as_str(), props.generation),
        ("Release", generation)
    );
    let lang = props
        .properties
        .iter()
        .find(|p| p.name == "LangVersion")
        .unwrap();
    assert_eq!(lang.source, host::PropertySource::Inherited);
    assert!(
        lang.inherited_from
            .as_deref()
            .unwrap()
            .ends_with("Directory.Build.props")
    );

    let edit = |generation, name: &str, value: Option<&str>, configuration: Option<&str>| {
        client
            .request::<host::ProjectSetProperty>(host::ProjectSetPropertyParams {
                project: project.into(),
                generation,
                edits: vec![host::PropertyEdit {
                    name: name.into(),
                    value: value.map(str::to_owned),
                    configuration: configuration.map(str::to_owned),
                    platform: configuration.map(|_| "AnyCPU".to_owned()),
                    ..Default::default()
                }],
            })
            .unwrap()
            .wait_timeout(T)
    };
    let inherited = edit(generation, "LangVersion", Some("13.0"), None).unwrap();
    assert_eq!(inherited.results[0].status, host::EditStatus::Inherited);
    assert!(!inherited.written);
    let set = edit(
        generation,
        "DefineConstants",
        Some("TRACE;RELEASE"),
        Some("Release"),
    )
    .unwrap();
    assert!(set.written);
    assert_eq!(set.generation, generation + 1);
    next(
        &rx,
        |e| matches!(e, Event::SolutionStatus(s) if s.state == SolutionState::Loaded && s.generation == generation + 1),
    );
    assert_eq!(client.generation(), generation + 1);
    assert_eq!(
        fake.project_property(project, "DefineConstants", Some(("Release", "Any CPU"))),
        Some("TRACE;RELEASE".into())
    );
    let stale = edit(generation, "Version", Some("2.0.0"), None).unwrap_err();
    assert!(
        matches!(
            stale,
            eludite_lsp::Error::Stale {
                requested: 1,
                current: 2
            }
        ),
        "{stale:?}"
    );

    let profiles = client
        .request::<host::ProjectLaunchProfiles>(host::LaunchProfilesParams {
            project: project.into(),
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(
        profiles.profiles[0]
            .environment_variables
            .iter()
            .map(|e| e.name.as_str())
            .collect::<Vec<_>>(),
        ["B", "A"]
    );
    let created = client
        .request::<host::ProjectSetLaunchProfile>(host::SetLaunchProfileParams {
            project: project.into(),
            generation: generation + 1,
            action: host::LaunchProfileAction::Create,
            profile: "Profile 1".into(),
            new_name: None,
            values: Some(json!({"commandLineArgs": "--x"})),
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(created.profiles.len(), 2);
    assert_eq!(
        created.profiles[1].command_line_args.as_deref(),
        Some("--x")
    );
    assert_eq!(
        created.generation,
        generation + 1,
        "launch profiles do not reload"
    );

    let configurations = client
        .request::<host::SolutionConfigurations>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(configurations.platforms, ["Any CPU", "x64"]);
    assert_eq!(configurations.format, Some(host::SolutionFormat::Slnx));
    let selected = client
        .request::<host::SolutionSetConfiguration>(host::SolutionSetConfigurationParams {
            generation: generation + 1,
            select: Some(host::Selection {
                configuration: "Release".into(),
                platform: "x64".into(),
            }),
            mappings: Some(vec![host::MappingEdit {
                project: project.into(),
                solution_configuration: "Release".into(),
                solution_platform: "Any CPU".into(),
                build: Some(false),
                ..Default::default()
            }]),
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(selected.written);
    assert_eq!(selected.generation, generation + 2);
    assert_eq!(selected.active.platform, "x64");
}

/// Brief 0052: typed `textDocument/codeLens` and `codeLens/resolve` carry the generation through the fake host, and
/// the host's `eludite/codeLens/refresh` becomes an event (one for an older generation is dropped).
#[test]
fn typed_code_lens_and_the_relayed_refresh() {
    use eludite_lsp::fake::FakeReply;
    let fake = FakeHost::new();
    let range = json!({"start": {"line": 2, "character": 13}, "end": {"line": 2, "character": 28}});
    let r = range.clone();
    fake.respond("textDocument/codeLens", move |_| {
        FakeReply::Result(json!([{"range": r, "data": {"listIndex": 0}}]))
    });
    let r = range.clone();
    fake.respond("codeLens/resolve", move |p| {
        FakeReply::Result(json!({"range": r, "data": p["data"], "command": {"title": "3 references",
            "command": "eludite.editor.find_references",
            "arguments": [{"uri": "file:///a.cs", "path": "/a.cs", "position": {"line": 2, "character": 13}}]}}))
    });
    let (client, rx) = start(&fake, 0);
    let generation = client.open_solution("/src/App.slnx", T).unwrap();
    let lenses = client
        .request::<lsp::CodeLensRequest>(lsp::CodeLensParams {
            text_document: lsp::TextDocumentIdentifier {
                uri: "file:///a.cs".into(),
            },
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap()
        .expect("lenses");
    let resolved = client
        .request::<lsp::ResolveCodeLens>(lenses[0].clone())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(resolved.command.as_ref().unwrap().title, "3 references");
    for method in ["textDocument/codeLens", "codeLens/resolve"] {
        assert_eq!(
            fake.received_params(method)[0]["eluditeGeneration"],
            generation
        );
    }
    assert_eq!(
        fake.received_params("codeLens/resolve")[0]["data"]["listIndex"],
        0
    );

    fake.notify(
        host::methods::CODE_LENS_REFRESH,
        json!({"eluditeGeneration": generation - 1}),
    );
    fake.notify(
        host::methods::CODE_LENS_REFRESH,
        json!({"eluditeGeneration": generation}),
    );
    let Event::CodeLensRefresh {
        generation: g,
        lens_generation,
    } = next(&rx, |e| matches!(e, Event::CodeLensRefresh { .. }))
    else {
        unreachable!()
    };
    assert_eq!((g, lens_generation), (generation, 1));
    assert_eq!(client.connection().code_lens_generation(), 1);
}
