//! Brief 0050: one document, two servers. Two scripted in-process [`FakeServer`]s stand in for TypeScript and ESLint
//! behind real [`ServerClient`]s, and [`fanout::dispatch`] sends each request to the servers that offer it and merges
//! the answers: completion lists concatenated with each server named, resolve routed back to the item's server,
//! code actions concatenated with ESLint's command run on ESLint, hover the first non-empty answer, formatting from
//! the first server that offers it only, references deduplicated, and one server failing leaves the other's answer.

use std::sync::Arc;
use std::time::Duration;

use eludite_lsp::fake::FakeReply;
use eludite_lsp::fake_server::{FakeServer, analyzer_capabilities};
use eludite_lsp::fanout::{self, Member, Sent};
use eludite_lsp::lsp::{DidOpenTextDocumentParams, TextDocumentItem};
use eludite_lsp::{ClientInfo, RestartPolicy, ServerClient, ServerSetup};
use serde_json::{Value, json};

const T: Duration = Duration::from_secs(10);
const URI: &str = "file:///w/src/main.ts";

/// A started server under a name of its own (the fakes all call themselves `fake-analyzer`).
struct Named(&'static str, ServerClient);

impl Member for Named {
    fn name(&self) -> String {
        self.0.to_owned()
    }
    fn capabilities(&self) -> Option<Value> {
        self.1.capabilities()
    }
    fn send(&self, method: &str, params: Value) -> Sent {
        Member::send(&self.1, method, params)
    }
}

fn start(name: &'static str, fake: &FakeServer) -> Arc<dyn Member> {
    let (client, _events) = ServerClient::start_in_process(
        fake.connector(),
        ServerSetup {
            name: name.into(),
            client: ClientInfo {
                name: "eludite-test".into(),
                version: "0".into(),
            },
            root: std::env::temp_dir().join("w"),
            initialization_options: Value::Null,
            settings: Value::Null,
            push_settings: false,
        },
        RestartPolicy::default(),
    )
    .expect("start the fake server");
    client
        .connection()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: URI.into(),
                language_id: "typescript".into(),
                version: 1,
                text: "const x = 1;\n".into(),
            },
        })
        .unwrap();
    Arc::new(Named(name, client))
}

fn two_servers() -> (FakeServer, FakeServer, Vec<Arc<dyn Member>>) {
    let ts = FakeServer::new();
    let mut caps = analyzer_capabilities();
    caps["documentFormattingProvider"] = json!(true);
    ts.set_capabilities(caps);
    let eslint = FakeServer::new();
    eslint.set_capabilities(json!({
        "textDocumentSync": {"openClose": true, "change": 2},
        "codeActionProvider": {"codeActionKinds": ["quickfix", "source.fixAll.eslint"]},
        "executeCommandProvider": {"commands": ["eslint.applySingleFix", "eslint.applyAllFixes"]},
        "documentFormattingProvider": true
    }));
    let members = vec![start("TypeScript", &ts), start("ESLint", &eslint)];
    (ts, eslint, members)
}

fn dispatch(members: &[Arc<dyn Member>], method: &str, params: Value) -> Result<Value, String> {
    fanout::dispatch(members, method, params, &|_| {})
}

fn position() -> Value {
    json!({"textDocument": {"uri": URI}, "position": {"line": 0, "character": 6}})
}

#[test]
fn completion_concatenates_and_resolve_goes_back_to_the_items_server() {
    let (ts, eslint, members) = two_servers();
    ts.respond("textDocument/completion", |_| {
        FakeReply::Result(
            json!({"isIncomplete": false, "items": [{"label": "log", "data": {"ts": 1}}]}),
        )
    });
    ts.respond("completionItem/resolve", |item| {
        let mut item = item.clone();
        item["documentation"] = json!("Prints.");
        FakeReply::Result(item)
    });
    let merged = dispatch(&members, "textDocument/completion", position()).unwrap();
    let items = merged["items"].as_array().unwrap();
    assert_eq!(
        items.len(),
        1,
        "ESLint offers no completion and is not asked"
    );
    assert_eq!(items[0]["labelDetails"]["description"], "TypeScript");
    assert!(eslint.received_params("textDocument/completion").is_empty());
    let resolved = dispatch(&members, "completionItem/resolve", items[0].clone()).unwrap();
    assert_eq!(resolved["documentation"], "Prints.");
    // The server saw its own data, and the answer carries the origin again.
    let sent = ts.received_params("completionItem/resolve");
    assert_eq!(sent[0]["data"], json!({"ts": 1}));
    assert_eq!(resolved["data"][fanout::ORIGIN], 0);
}

#[test]
fn code_actions_concatenate_and_eslints_command_runs_on_eslint() {
    let (ts, eslint, members) = two_servers();
    ts.respond("textDocument/codeAction", |_| {
        FakeReply::Result(
            json!([{"title": "Add missing import", "kind": "quickfix", "edit": {"changes": {}}}]),
        )
    });
    eslint.respond("textDocument/codeAction", |_| {
        FakeReply::Result(json!([
            {"title": "Fix this prefer-const problem", "kind": "quickfix",
             "command": {"title": "Fix this prefer-const problem", "command": "eslint.applySingleFix",
                         "arguments": [{"uri": URI, "version": 1, "ruleId": "prefer-const"}]}},
            {"title": "Fix all auto-fixable problems", "kind": "quickfix",
             "command": {"title": "Fix all auto-fixable problems", "command": "eslint.applyAllFixes",
                         "arguments": [{"uri": URI, "version": 1}]}}
        ]))
    });
    let range = json!({"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 5}});
    let merged = dispatch(
        &members,
        "textDocument/codeAction",
        json!({"textDocument": {"uri": URI}, "range": range, "context": {"diagnostics": []}}),
    )
    .unwrap();
    let titles: Vec<&str> = merged
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["title"].as_str().unwrap())
        .collect();
    assert_eq!(
        titles,
        [
            "Add missing import",
            "Fix this prefer-const problem",
            "Fix all auto-fixable problems"
        ]
    );
    let command = merged[2]["command"].clone();
    dispatch(&members, "workspace/executeCommand", command).unwrap();
    assert!(
        eslint
            .wait_for("workspace/executeCommand", T, |p| p["command"]
                == "eslint.applyAllFixes")
            .is_some()
    );
    assert!(ts.received_params("workspace/executeCommand").is_empty());
    // ESLint resolves nothing: its action comes back as it was.
    let same = dispatch(&members, "codeAction/resolve", merged[1].clone()).unwrap();
    assert_eq!(same["command"]["command"], "eslint.applySingleFix");
    assert!(eslint.received_params("codeAction/resolve").is_empty());
}

#[test]
fn hover_is_the_first_non_empty_and_formatting_comes_from_the_first_server_only() {
    let (ts, eslint, members) = two_servers();
    // ESLint answers hover here (a server that offers it), TypeScript with nothing.
    ts.respond("textDocument/hover", |_| FakeReply::Result(Value::Null));
    ts.respond("textDocument/formatting", |_| {
        FakeReply::Result(json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": "// ts\n"}]))
    });
    eslint.respond("textDocument/formatting", |_| {
        FakeReply::Result(json!([{"newText": "eslint"}]))
    });
    let hover = dispatch(&members, "textDocument/hover", position()).unwrap();
    assert_eq!(hover, Value::Null, "no server had anything to show");
    let edits = dispatch(
        &members,
        "textDocument/formatting",
        json!({"textDocument": {"uri": URI}, "options": {"tabSize": 4, "insertSpaces": true}}),
    )
    .unwrap();
    assert_eq!(edits[0]["newText"], "// ts\n");
    assert!(
        eslint.received_params("textDocument/formatting").is_empty(),
        "formatting never runs twice"
    );
    // When the first fails, the next one formats.
    ts.respond("textDocument/formatting", |_| {
        FakeReply::Error(-32603, "boom".into())
    });
    let edits = dispatch(
        &members,
        "textDocument/formatting",
        json!({"textDocument": {"uri": URI}, "options": {"tabSize": 4, "insertSpaces": true}}),
    )
    .unwrap();
    assert_eq!(edits[0]["newText"], "eslint");
}

#[test]
fn hover_takes_the_first_server_with_something_to_show() {
    let a = FakeServer::new();
    let b = FakeServer::new();
    a.respond("textDocument/hover", |_| {
        FakeReply::Result(json!({"contents": {"kind": "markdown", "value": ""}}))
    });
    b.respond("textDocument/hover", |_| {
        FakeReply::Result(json!({"contents": {"kind": "markdown", "value": "const x: 1"}}))
    });
    let members = vec![start("A", &a), start("B", &b)];
    let hover = dispatch(&members, "textDocument/hover", position()).unwrap();
    assert_eq!(hover["contents"]["value"], "const x: 1");
}

#[test]
fn references_merge_without_duplicates_and_a_failing_server_is_left_out() {
    let a = FakeServer::new();
    let b = FakeServer::new();
    let loc = json!({"uri": URI, "range": {"start": {"line": 0, "character": 6}, "end": {"line": 0, "character": 7}}});
    let other = json!({"uri": "file:///w/src/b.ts", "range": {"start": {"line": 2, "character": 0}, "end": {"line": 2, "character": 1}}});
    let (l1, l2) = (loc.clone(), other.clone());
    a.respond("textDocument/references", move |_| {
        FakeReply::Result(json!([l1.clone()]))
    });
    b.respond("textDocument/references", move |_| {
        FakeReply::Result(json!([loc.clone(), l2.clone()]))
    });
    let members = vec![start("A", &a), start("B", &b)];
    let refs = dispatch(
        &members,
        "textDocument/references",
        json!({"textDocument": {"uri": URI}, "position": {"line": 0, "character": 6}, "context": {"includeDeclaration": true}}),
    )
    .unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 2);
    b.respond("textDocument/references", |_| {
        FakeReply::Error(-32603, "crashed".into())
    });
    let refs = dispatch(
        &members,
        "textDocument/references",
        json!({"textDocument": {"uri": URI}, "position": {"line": 0, "character": 6}, "context": {"includeDeclaration": true}}),
    )
    .unwrap();
    assert_eq!(refs.as_array().unwrap().len(), 1);
    a.respond("textDocument/references", |_| {
        FakeReply::Error(-32603, "also".into())
    });
    let e = dispatch(
        &members,
        "textDocument/references",
        json!({"textDocument": {"uri": URI}, "position": {"line": 0, "character": 6}, "context": {"includeDeclaration": true}}),
    )
    .unwrap_err();
    assert!(e.contains("A:") && e.contains("B:"), "{e}");
}
