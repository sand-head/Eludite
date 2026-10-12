//! Headless GPUI tests of brief 0079, Razor through the host, against the in-process fake `eludite-host` and
//! `eludite-lsp`'s scripted `FakeServer` standing in for vscode-html-language-server, through the real `servers.json`
//! registrations: a `.razor` document is the host's, opened with `languageId` `aspnetcorerazor`, with no generic server
//! started for it; the host's `eludite/razor/html` opens the document's virtual HTML document
//! (`<path>__virtual.html`, `languageId` `html`) in the HTML server with the projection, sends the request there with
//! the virtual uri, and returns the HTML server's answer; the same checksum sends nothing, a new one the new text;
//! the virtual uri in an answer is mapped back to the Razor document's; a stale generation and a missing HTML server
//! answer `null`. The real Razor's test is `dotnet/tests/Eludite.Host.Tests/RazorTests.cs`.

use std::path::PathBuf;

use eludite_commands::workspace;
use eludite_lsp::fake::{FakeHost, FakeReply};
use eludite_lsp::fake_server::FakeServer;
use eludite_lsp::host::methods;
use serde_json::{Value, json};

use super::documents::{normalize_path, path_to_uri};
use super::servers::ServerKey;
use super::tests::{T, Ws};

const HOME_RAZOR: &str =
    "@page \"/\"\n<h1>Hello, @name!</h1>\n<Counter IncrementAmount=\"2\" />\n\n@code {\n    private string name = \"world\";\n}\n";
/// Razor's projection of [`HOME_RAZOR`]: the C# and Razor parts blanked out, lines and lengths kept.
const HOME_HTML: &str =
    "         \n<h1>Hello,      !</h1>\n                               \n\n       \n                                    \n \n";

/// The HTML server's capabilities, as vscode-html-language-server advertises them.
fn html_capabilities() -> Value {
    json!({
        "textDocumentSync": {"openClose": true, "change": 2},
        "completionProvider": {"triggerCharacters": [".", ":", "<", "\"", "=", "/"], "resolveProvider": true},
        "hoverProvider": true,
        "documentHighlightProvider": true,
        "definitionProvider": true,
        "documentFormattingProvider": true
    })
}

/// The shell with the fake host and, when `html` is set, the fake HTML server behind the `html` registration (else
/// that server is looked for and not found: the web servers' cache is an empty folder).
fn setup(cx: &mut gpui::TestAppContext, html: Option<&FakeServer>) -> (Ws, tempfile::TempDir) {
    let cache = tempfile::tempdir().unwrap();
    let cache_path = cache.path().to_path_buf();
    let connector = html.map(FakeServer::connector);
    let w = super::tests::setup_services(
        cx,
        |_| {},
        None,
        None,
        move |services| {
            if let Some(c) = connector {
                services.launches.in_process.insert("html".into(), c);
            }
            services.launches.cache = Some(Some(cache_path));
        },
    );
    (w, cache)
}

/// Writes and opens `src/App/Components/Pages/Home.razor` after opening the solution; its path.
fn open_home(w: &mut Ws) -> PathBuf {
    w.open_solution();
    let path = w.dir.path().join("src/App/Components/Pages/Home.razor");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, HOME_RAZOR).unwrap();
    let path = normalize_path(&path);
    w.shell
        .update_in(&mut w.vcx, |s, window, cx| {
            s.invoke(
                workspace::FILE_OPEN,
                json!({"path": path.to_string_lossy()}),
                window,
                cx,
            )
        })
        .unwrap();
    w.editor(&path);
    path
}

/// Sends the shell `eludite/razor/html` from the fake host (on a thread: the fake waits for the answer) and pumps the
/// shell until the answer comes: the whole JSON-RPC response.
fn razor_html(w: &mut Ws, method: &str, uri: &str, checksum: &str, params: Value, generation: Option<u64>) -> Value {
    let fake: FakeHost = w.fake.clone();
    let request = json!({
        "method": method,
        "uri": uri,
        "checksum": checksum,
        // Another checksum is another projection (the session sends a didChange only for changed text).
        "text": if checksum == "c1" { HOME_HTML.to_owned() } else { HOME_HTML.replace("<h1>", "<h2>") },
        "params": params,
        "eluditeGeneration": generation.unwrap_or_else(|| fake.generation()),
    });
    let handle = std::thread::spawn(move || fake.request_shell(methods::RAZOR_HTML, request, T));
    w.wait("the shell's answer", |_| handle.is_finished());
    handle
        .join()
        .unwrap()
        .expect("the shell answered eludite/razor/html")
}

fn completion_params(uri: &str) -> Value {
    json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 1}, "context": {"triggerKind": 1}})
}

/// A `.razor` document goes to the host as `aspnetcorerazor`; no generic server starts for it.
#[gpui::test]
fn a_razor_document_is_served_by_the_host_as_aspnetcorerazor(cx: &mut gpui::TestAppContext) {
    let html = FakeServer::new();
    html.set_capabilities(html_capabilities());
    let (mut w, _cache) = setup(cx, Some(&html));
    let path = open_home(&mut w);
    let uri = path_to_uri(&path);
    let opened = w
        .fake
        .wait_for("textDocument/didOpen", T, |p| p["textDocument"]["uri"] == uri)
        .expect("the host got the Razor document");
    assert_eq!(opened.params["textDocument"]["languageId"], "aspnetcorerazor");
    assert_eq!(opened.params["textDocument"]["text"], HOME_RAZOR);
    let id = path.to_string_lossy().into_owned();
    let (key, generic) = w
        .shell
        .read_with(&w.vcx, |s, _| (s.doc_key(&id), s.generic_servers().len()));
    assert_eq!(key, ServerKey::Host);
    assert_eq!(generic, 0, "no generic server for a Razor document");
    assert_eq!(html.connections(), 0, "the HTML server never sees Razor source");
}

/// The host's `eludite/razor/html` is answered by the HTML server on the virtual document, opened with the projection;
/// the same checksum sends nothing, a new checksum the new text; a definition into the virtual document comes back
/// as the Razor document.
#[gpui::test]
fn razor_html_requests_are_answered_by_the_html_server_on_the_virtual_document(
    cx: &mut gpui::TestAppContext,
) {
    let html = FakeServer::new();
    html.set_capabilities(html_capabilities());
    html.respond("textDocument/completion", |_| {
        FakeReply::Result(json!({"isIncomplete": false, "items": [{"label": "section", "kind": 10}]}))
    });
    let (mut w, _cache) = setup(cx, Some(&html));
    let path = open_home(&mut w);
    let uri = path_to_uri(&path);
    let virtual_uri = path_to_uri(&super::razor::virtual_path(&path));
    assert!(virtual_uri.ends_with("Home.razor__virtual.html"));
    let definition_target = virtual_uri.clone();
    html.respond("textDocument/definition", move |_| {
        FakeReply::Result(json!([{"uri": definition_target, "range": {
            "start": {"line": 1, "character": 1}, "end": {"line": 1, "character": 3}}}]))
    });

    let answer = razor_html(&mut w, "textDocument/completion", &uri, "c1", completion_params(&uri), None);
    assert_eq!(answer["result"]["items"][0]["label"], "section", "{answer}");
    let opened = html
        .wait_for("textDocument/didOpen", T, |_| true)
        .expect("the virtual document was opened");
    assert_eq!(opened.params["textDocument"]["uri"], virtual_uri);
    assert_eq!(opened.params["textDocument"]["languageId"], "html");
    assert_eq!(opened.params["textDocument"]["text"], HOME_HTML);
    let asked = html.received_params("textDocument/completion");
    assert_eq!(asked.len(), 1);
    assert_eq!(asked[0]["textDocument"]["uri"], virtual_uri);
    assert_eq!(asked[0]["position"], json!({"line": 1, "character": 1}));

    // The same projection again: no didChange.
    let answer = razor_html(&mut w, "textDocument/completion", &uri, "c1", completion_params(&uri), None);
    assert_eq!(answer["result"]["items"][0]["label"], "section");
    assert!(html.received_params("textDocument/didChange").is_empty());

    // A new projection: its text, as the next version.
    let answer = razor_html(
        &mut w,
        "textDocument/definition",
        &uri,
        "c2",
        json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 2}}),
        None,
    );
    let changed = html
        .wait_for("textDocument/didChange", T, |_| true)
        .expect("the new projection was sent");
    assert_eq!(changed.params["textDocument"]["uri"], virtual_uri);
    assert_eq!(changed.params["textDocument"]["version"], 2);
    // The definition pointed into the virtual document: Razor gets its own document's uri.
    assert_eq!(answer["result"][0]["uri"], uri, "{answer}");
    let answered = w.shell.read_with(&w.vcx, |s, _| s.razor_html().answered);
    assert_eq!(answered, 3);

    // The HTML server's diagnostics for the virtual document never reach the Error List; a real file's after them do.
    html.publish_diagnostics(
        &virtual_uri,
        None,
        json!([{"range": {"start": {"line": 1, "character": 0}, "end": {"line": 1, "character": 4}},
                "severity": 1, "code": "virtual", "message": "from the projection"}]),
    );
    let page = path_to_uri(&w.dir.path().join("wwwroot/index.html"));
    html.publish_diagnostics(
        &page,
        None,
        json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                "severity": 1, "code": "real", "message": "from a page"}]),
    );
    w.wait("the page's row", |w| w.error_rows().iter().any(|r| r.code == "real"));
    assert!(!w.error_rows().iter().any(|r| r.code == "virtual"));
}

/// A request computed under an older generation is answered `null` without asking the HTML server.
#[gpui::test]
fn a_stale_razor_html_request_is_answered_null(cx: &mut gpui::TestAppContext) {
    let html = FakeServer::new();
    html.set_capabilities(html_capabilities());
    let (mut w, _cache) = setup(cx, Some(&html));
    let path = open_home(&mut w);
    let uri = path_to_uri(&path);
    let current = w.fake.generation();
    let answer = razor_html(
        &mut w,
        "textDocument/hover",
        &uri,
        "c1",
        json!({"textDocument": {"uri": uri}, "position": {"line": 1, "character": 1}}),
        Some(current + 7),
    );
    assert_eq!(answer["result"], Value::Null, "{answer}");
    w.vcx.run_until_parked();
    assert_eq!(html.connections(), 0, "the HTML server was not asked");
}

/// Without an HTML server (not installed) the answer is `null`, so Razor's C# features go on alone.
#[gpui::test]
fn without_an_html_server_razor_html_is_answered_null(cx: &mut gpui::TestAppContext) {
    let (mut w, _cache) = setup(cx, None);
    let path = open_home(&mut w);
    let uri = path_to_uri(&path);
    let answer = razor_html(&mut w, "textDocument/completion", &uri, "c1", completion_params(&uri), None);
    assert_eq!(answer["result"], Value::Null, "{answer}");
    assert!(answer.get("error").is_none(), "{answer}");
    // Asked again (the server is now known to be missing): still null, at once.
    let answer = razor_html(&mut w, "textDocument/hover", &uri, "c1", completion_params(&uri), None);
    assert_eq!(answer["result"], Value::Null);
}
