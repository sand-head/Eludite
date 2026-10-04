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
    let located = match ra.locate(None, None, &|| Err("not needed".into())) {
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
            push_settings: false,
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

/// Brief 0052: the lenses the real rust-analyzer offers this client (it requires the client commands the client lists
/// in `experimental.commands`): Run and Debug on a test and on its module, as Cargo test runnables the shell maps to
/// Run Test and Debug Test, and the implementations lens of a trait, resolved to the locations of its impls.
#[test]
fn real_rust_analyzer_offers_run_debug_and_implementations_lenses() {
    use eludite_lsp::codelens::{self, LensKind, LensTarget};
    let registry = ServerRegistry::builtin();
    let ra = registry.get("rust-analyzer").unwrap();
    let located = match ra.locate(None, None, &|| Err("not needed".into())) {
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
    let dir = tempfile::Builder::new()
        .prefix("eludite-ra-lens-")
        .tempdir()
        .unwrap();
    std::fs::write(
        dir.path().join("Cargo.toml"),
        "[package]\nname = \"probe\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n[workspace]\n",
    )
    .unwrap();
    std::fs::create_dir(dir.path().join("src")).unwrap();
    let text = "pub trait Shape {\n    fn area(&self) -> u32;\n}\n\npub struct Square(pub u32);\n\nimpl Shape for Square {\n    fn area(&self) -> u32 {\n        self.0 * self.0\n    }\n}\n\n#[cfg(test)]\nmod tests {\n    use super::*;\n\n    #[test]\n    fn squares() {\n        assert_eq!(Square(3).area(), 9);\n    }\n}\n";
    let lib = dir.path().join("src/lib.rs");
    std::fs::write(&lib, text).unwrap();
    let mut command = ServerCommand::new(located.path.as_os_str()).stderr(StderrMode::Discard);
    command.current_dir = Some(dir.path().to_path_buf());
    let (client, _rx) = ServerClient::start(
        command,
        ServerSetup {
            name: ra.id.clone(),
            client: ClientInfo {
                name: "eludite-test".into(),
                version: "0".into(),
            },
            root: dir.path().to_path_buf(),
            initialization_options: serde_json::json!({"checkOnSave": false, "cargo": {"targetDir": true}}),
            settings: serde_json::json!({"rust-analyzer": {"checkOnSave": false}}),
            push_settings: false,
        },
        RestartPolicy::default(),
    )
    .expect("rust-analyzer starts");
    assert!(
        client
            .capabilities()
            .is_some_and(|c| c["codeLensProvider"].is_object()),
        "rust-analyzer advertises codeLensProvider"
    );
    let uri = eludite_lsp::path_to_uri(&lib);
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
    // The lenses come once the crate is loaded; ask until the test's runnable is there.
    let deadline = Instant::now() + T;
    let lenses = loop {
        // While it loads, rust-analyzer answers ContentModified (Stale here).
        let lenses = match client
            .request::<lsp::CodeLensRequest>(lsp::CodeLensParams {
                text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
            })
            .unwrap()
            .wait_timeout(T)
        {
            Ok(l) => l.unwrap_or_default(),
            Err(eludite_lsp::Error::Stale { .. }) => Vec::new(),
            Err(e) => panic!("textDocument/codeLens: {e:?}"),
        };
        if lenses.iter().any(|l| l.range.start.line == 17) {
            break lenses;
        }
        assert!(Instant::now() < deadline, "no test lens in {lenses:?}");
        std::thread::sleep(Duration::from_millis(250));
    };
    let mut kinds = Vec::new();
    for lens in &lenses {
        let lens = match &lens.command {
            Some(_) => lens.clone(),
            None => client
                .request::<lsp::ResolveCodeLens>(lens.clone())
                .unwrap()
                .wait_timeout(T)
                .unwrap(),
        };
        let command = lens.command.as_ref().expect("resolved");
        let classified = codelens::classify(command);
        eprintln!(
            "rust-analyzer lens on line {}: {:?} {:?} -> {:?}",
            lens.range.start.line,
            command.title,
            command.command,
            classified.as_ref().map(|c| c.kind)
        );
        if let Some(c) = classified {
            kinds.push((lens.range.start.line, c));
        }
    }
    let at = |line: u32, kind: LensKind| kinds.iter().find(|(l, c)| *l == line && c.kind == kind);
    let run = at(17, LensKind::RunTest).expect("Run Test above the test");
    assert!(
        matches!(&run.1.target, LensTarget::CargoTest { name, exact: true, .. } if name == "tests::squares"),
        "{run:?}"
    );
    assert!(at(17, LensKind::DebugTest).is_some(), "{kinds:?}");
    assert!(
        at(13, LensKind::RunTest).is_some(),
        "Run Tests above the module: {kinds:?}"
    );
    let imps = at(0, LensKind::Implementations).expect("the trait's implementations lens");
    assert!(
        matches!(&imps.1.target, LensTarget::References { locations: Some(l), .. } if l.len() == 1),
        "{imps:?}"
    );
    client.shutdown(Duration::from_secs(10)).unwrap();
}
