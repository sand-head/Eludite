//! The generic client against the real FsAutoComplete (brief 0063), located through the built-in registration
//! (beside the test binary, `ELUDITE_FSAUTOCOMPLETE`, the pinned cache of `tools/fsautocomplete/fetch.sh`,
//! `~/.dotnet/tools`, then `PATH`) and spawned with the environment the registry adds (`DOTNET_ROOT` for a user-local
//! SDK): a one-file F# project with a type error, restored with the `dotnet` on `PATH`, gets pushed diagnostics and
//! hover text on a function. Skips with a message unless `ELUDITE_FSAUTOCOMPLETE` names a server (the install is
//! heavy and the first load of FSharp.Core takes a while), or when `dotnet` is missing, so `cargo test` stays green
//! on a machine without them.

use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eludite_lsp::lsp::{self, DidOpenTextDocumentParams, TextDocumentItem};
use eludite_lsp::{
    ClientInfo, Event, RestartPolicy, ServerClient, ServerCommand, ServerRegistry, ServerSetup,
    StderrMode,
};

/// The first load of FSharp.Core and the project can take 30 to 90 s on a cold machine.
const T: Duration = Duration::from_secs(240);

const PROGRAM_FS: &str = "module Program\n\nlet add (a: int) (b: int) = a + b\n\nlet total: int = \"two\"\n\n[<EntryPoint>]\nlet main _ =\n    printfn \"%d\" (add 1 2)\n    0\n";

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

/// Every text of a hover's `contents` (`MarkupContent`, `MarkedString` or `MarkedString[]`), joined.
fn hover_text(contents: &serde_json::Value) -> String {
    match contents {
        serde_json::Value::String(s) => s.clone(),
        serde_json::Value::Array(items) => {
            items.iter().map(hover_text).collect::<Vec<_>>().join("\n")
        }
        serde_json::Value::Object(o) => o
            .get("value")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        _ => String::new(),
    }
}

#[test]
fn real_fsautocomplete_diagnoses_and_hovers() {
    if std::env::var_os("ELUDITE_FSAUTOCOMPLETE").is_none_or(|v| v.is_empty()) {
        eprintln!("skipped: ELUDITE_FSAUTOCOMPLETE is not set (run tools/fsautocomplete/fetch.sh)");
        return;
    }
    let registry = ServerRegistry::builtin();
    let fsac = registry.get("fsautocomplete").unwrap();
    let located = match fsac.locate(None, None, &|| Err("not needed".into())) {
        Ok(l) => l,
        Err(why) => {
            eprintln!("skipped: {why}");
            return;
        }
    };
    if std::process::Command::new("dotnet")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: dotnet not on PATH");
        return;
    }
    eprintln!(
        "{} from {} ({}){}",
        located.version,
        located.source,
        located.path.display(),
        located
            .envs
            .iter()
            .map(|(k, v)| format!(" with {k}={v}"))
            .collect::<String>()
    );
    let dir = tempfile::Builder::new()
        .prefix("eludite-fsac-")
        .tempdir()
        .unwrap();
    std::fs::write(
        dir.path().join("Probe.fsproj"),
        "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <OutputType>Exe</OutputType>\n    <TargetFramework>net10.0</TargetFramework>\n  </PropertyGroup>\n  <ItemGroup>\n    <Compile Include=\"Program.fs\" />\n  </ItemGroup>\n</Project>\n",
    )
    .unwrap();
    let program = dir.path().join("Program.fs");
    std::fs::write(&program, PROGRAM_FS).unwrap();
    // FsAutoComplete needs a restored project (the assets file names FSharp.Core).
    let started = Instant::now();
    let restore = std::process::Command::new("dotnet")
        .args(["restore", "Probe.fsproj"])
        .current_dir(dir.path())
        .env("DOTNET_CLI_TELEMETRY_OPTOUT", "1")
        .env("DOTNET_NOLOGO", "1")
        .output()
        .expect("dotnet restore runs");
    assert!(
        restore.status.success(),
        "dotnet restore: {}{}",
        String::from_utf8_lossy(&restore.stdout),
        String::from_utf8_lossy(&restore.stderr)
    );
    eprintln!("dotnet restore took {:?}", started.elapsed());

    let started = Instant::now();
    let mut command = ServerCommand::new(located.path.as_os_str()).stderr(StderrMode::Discard);
    for (k, v) in &located.envs {
        command = command.env(k, v);
    }
    command.current_dir = Some(dir.path().to_path_buf());
    let (client, rx) = ServerClient::start(
        command,
        ServerSetup {
            name: fsac.id.clone(),
            client: ClientInfo {
                name: "eludite-test".into(),
                version: "0".into(),
            },
            root: dir.path().to_path_buf(),
            initialization_options: fsac.initialization_options.clone(),
            settings: fsac.settings.clone(),
            push_settings: fsac.push_settings,
        },
        RestartPolicy::default(),
    )
    .expect("FsAutoComplete starts");
    eprintln!("initialize answered after {:?}", started.elapsed());
    assert!(
        client.capabilities().is_some_and(
            |c| c["hoverProvider"].as_bool() == Some(true) || c["hoverProvider"].is_object()
        ),
        "FsAutoComplete advertises hover: {:?}",
        client.capabilities()
    );
    let uri = eludite_lsp::path_to_uri(&program);
    client
        .connection()
        .did_open(DidOpenTextDocumentParams {
            text_document: TextDocumentItem {
                uri: uri.clone(),
                language_id: fsac.language_id.clone(),
                version: 1,
                text: PROGRAM_FS.into(),
            },
        })
        .unwrap();
    // The type error on `let total: int = "two"` (line 4), once the project is loaded and checked.
    let diagnostics = next(&rx, "FsAutoComplete's diagnostics", |e| match e {
        Event::Diagnostics(d)
            if d.params.uri == uri
                && d.params.diagnostics.iter().any(|d| d.range.start.line == 4) =>
        {
            Some(d.params.diagnostics)
        }
        _ => None,
    });
    eprintln!(
        "first diagnostics after {:?}: {:?}",
        started.elapsed(),
        diagnostics
            .iter()
            .map(|d| (d.range.start.line, d.code.clone(), d.message.clone()))
            .collect::<Vec<_>>()
    );
    let mismatch = diagnostics
        .iter()
        .find(|d| d.range.start.line == 4)
        .unwrap();
    assert_eq!(mismatch.severity, Some(1), "{mismatch:?}");
    assert!(
        mismatch.message.contains("string") && mismatch.message.contains("int"),
        "{mismatch:?}"
    );
    // Hover on `add` at its definition (line 2, after `let `).
    let hovering = Instant::now();
    let deadline = Instant::now() + T;
    let text = loop {
        let reply = client
            .request::<lsp::HoverRequest>(lsp::TextDocumentPositionParams {
                text_document: lsp::TextDocumentIdentifier { uri: uri.clone() },
                position: lsp::Position {
                    line: 2,
                    character: 5,
                },
            })
            .unwrap()
            .wait_timeout(T);
        let text = match reply {
            Ok(Some(h)) => hover_text(&h.contents),
            Ok(None) => String::new(),
            Err(eludite_lsp::Error::Stale { .. }) => String::new(),
            Err(e) => panic!("textDocument/hover: {e:?}"),
        };
        if text.contains("add") && text.contains("int") {
            break text;
        }
        assert!(Instant::now() < deadline, "no hover for add: {text:?}");
        std::thread::sleep(Duration::from_millis(250));
    };
    eprintln!(
        "hover answered after {:?} ({:?} since start): {}",
        hovering.elapsed(),
        started.elapsed(),
        text.lines().next().unwrap_or_default()
    );
    client.shutdown(Duration::from_secs(20)).unwrap();
}
