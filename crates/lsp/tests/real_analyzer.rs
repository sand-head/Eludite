//! The generic client against the real rust-analyzer, located through the built-in registration (beside the test
//! binary, `ELUDITE_RUST_ANALYZER`, `PATH`, then `rustup which`): a one-file Cargo project with a type error gets
//! pushed diagnostics and member completion. Skips with a message when rust-analyzer or cargo is missing, so
//! `cargo test` stays green on a machine without them.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eludite_lsp::lsp::{self, DidOpenTextDocumentParams, TextDocumentItem};
use eludite_lsp::{
    ClientInfo, Event, RestartPolicy, ServerClient, ServerCommand, ServerRegistry, ServerSetup,
    StderrMode,
};

const T: Duration = Duration::from_secs(120);

fn next<V>(rx: &Receiver<Event>, what: &str, pick: impl Fn(Event) -> Option<V>) -> V {
    let deadline = Instant::now() + T;
    loop {
        match rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
            Ok(e) => {
                if let Some(v) = pick(e) {
                    return v;
                }
            }
            Err(_) => panic!("timed out waiting for {what}"),
        }
    }
}

#[test]
fn real_rust_analyzer_diagnoses_and_completes() {
    let registry = ServerRegistry::builtin();
    let ra = registry.get("rust-analyzer").unwrap();
    let located = match ra.locate() {
        Ok(l) => l,
        Err(why) => {
            eprintln!("skipped: {why}");
            return;
        }
    };
    if std::process::Command::new("cargo")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: cargo not on PATH");
        return;
    }
    eprintln!("{} from {}", located.version, located.source);
    // Not a dot-folder: rust-analyzer does not load files under hidden folders.
    let dir = tempfile::Builder::new()
        .prefix("eludite-ra-")
        .tempdir()
        .unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let text = "fn f(_a: u8) {}\nfn main() {\n    f();\n    let s = String::new();\n    s.le;\n}\n";
    let main = dir.path().join("src/main.rs");
    std::fs::write(&main, text).unwrap();
    let mut command = ServerCommand::new(located.path.as_os_str()).stderr(StderrMode::Discard);
    command.current_dir = Some(dir.path().to_path_buf());
    let (client, rx) = ServerClient::start(
        command,
        ServerSetup {
            name: ra.id.clone(),
            client: ClientInfo {
                name: "eludite-test".into(),
                version: "0".into(),
            },
            root: dir.path().to_path_buf(),
            // Native diagnostics only: no `cargo check` in a test.
            initialization_options: serde_json::json!({"checkOnSave": false, "cargo": {"targetDir": true}}),
            settings: serde_json::json!({"rust-analyzer": {"checkOnSave": false}}),
        },
        RestartPolicy::default(),
    )
    .expect("rust-analyzer starts");
    let uri = eludite_lsp::path_to_uri(&main);
    client
        .connection()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: ra.language_id.clone(),
                version: 1,
                text: text.into(),
            },
        })
        .unwrap();
    let diagnostics = next(&rx, "rust-analyzer's diagnostics", |e| match e {
        // rust-analyzer computes them on pull, once the crate is loaded (pulled again when it turns quiescent).
        Event::Diagnostics(d)
            if d.params.uri == uri
                && d.params.diagnostics.iter().any(|d| d.range.start.line == 2) =>
        {
            Some(d.params.diagnostics)
        }
        _ => None,
    });
    let arity = diagnostics
        .iter()
        .find(|d| d.range.start.line == 2)
        .unwrap();
    assert_eq!(arity.severity, Some(1), "{arity:?}");
    assert_eq!(arity.code, Some(serde_json::json!("E0107")), "{arity:?}");
    // Completion after `s.le` lists String's methods once the workspace is loaded; retry while it loads.
    let deadline = Instant::now() + T;
    loop {
        let reply = client
            .request::<lsp::Completion>(lsp::CompletionParams {
                text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
                position: lsp::Position {
                    line: 4,
                    character: 8,
                },
                context: None,
            })
            .unwrap()
            .wait_timeout(T)
            .unwrap();
        let labels: Vec<String> = match reply {
            Some(lsp::CompletionResponse::List(l)) => {
                l.items.into_iter().map(|i| i.label).collect()
            }
            Some(lsp::CompletionResponse::Items(i)) => i.into_iter().map(|i| i.label).collect(),
            None => Vec::new(),
        };
        if labels.iter().any(|l| l.starts_with("len")) {
            break;
        }
        assert!(Instant::now() < deadline, "no len in {labels:?}");
        std::thread::sleep(Duration::from_millis(250));
    }
    client.shutdown(Duration::from_secs(10)).unwrap();
}
