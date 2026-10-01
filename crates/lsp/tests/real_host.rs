//! The client against the real `niello-host` (built by `dotnet build dotnet/Niello.slnx`), without a language
//! server: proves the Rust types and the .NET host agree on the wire. Skips with a message when the host is not
//! built or `dotnet` is missing, so `cargo test` stays green on a machine without .NET.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use niello_lsp::host::{self, HostDiagnosticSeverity, LanguageServerState, SolutionState};
use niello_lsp::lsp::{self, Position, TextDocumentIdentifier, TextDocumentPositionParams};
use niello_lsp::{ClientInfo, Error, Event, HostClient, HostCommand, RestartPolicy, StderrMode};

const T: Duration = Duration::from_secs(30);

fn host_dll() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("NIELLO_HOST_DLL") {
        return Some(PathBuf::from(p));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    ["Release", "Debug"]
        .iter()
        .map(|c| {
            root.join(format!(
                "dotnet/src/Niello.Host/bin/{c}/net10.0/niello-host.dll"
            ))
        })
        .find(|p| p.exists())
}

fn next<T>(rx: &Receiver<Event>, pick: impl Fn(Event) -> Option<T>) -> T {
    let deadline = Instant::now() + T;
    loop {
        let event = rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("event");
        if let Some(v) = pick(event) {
            return v;
        }
    }
}

#[test]
fn real_host_without_language_server() {
    let Some(dll) = host_dll() else {
        eprintln!(
            "skipped: niello-host.dll not built (dotnet build dotnet/Niello.slnx) and NIELLO_HOST_DLL unset"
        );
        return;
    };
    if std::process::Command::new("dotnet")
        .arg("--version")
        .output()
        .is_err()
    {
        eprintln!("skipped: dotnet not on PATH");
        return;
    }
    let (client, rx) = HostClient::start(
        HostCommand::dotnet_host(&dll)
            .arg("--no-roslyn")
            .stderr(StderrMode::Discard),
        ClientInfo {
            name: "niello-lsp-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
    )
    .expect("start niello-host");

    let init = client.initialize_result().unwrap();
    assert_eq!(init.host_name, host::HOST_NAME);
    assert!(!init.capabilities.language_server);
    let status = next(&rx, |e| match e {
        Event::LanguageServerStatus(s) => Some(s),
        _ => None,
    });
    assert_eq!(status.state, LanguageServerState::Unavailable);
    let info = client
        .request::<host::HostInfo>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert!(!info.runtime.is_empty());

    let dir = std::env::temp_dir().join(format!("niello-lsp-real-host-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let sln = dir.join("App.slnx");
    std::fs::write(&sln, "<Solution />").unwrap();
    let generation = client.open_solution(sln.to_str().unwrap(), T).unwrap();
    assert_eq!(generation, 1);
    let failed = next(&rx, |e| match e {
        Event::SolutionStatus(s) if s.state == SolutionState::Failed => Some(s),
        _ => None,
    });
    assert_eq!(failed.generation, 1);
    assert_eq!(failed.diagnostics[0].code, "NIELLO0001");
    assert_eq!(
        failed.diagnostics[0].severity,
        HostDiagnosticSeverity::Error
    );

    let err = client
        .request::<lsp::HoverRequest>(TextDocumentPositionParams {
            text_document: TextDocumentIdentifier {
                uri: "file:///a.cs".into(),
            },
            position: Position {
                line: 0,
                character: 0,
            },
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap_err();
    assert!(
        matches!(err, Error::Rpc(ref e) if e.code == host::error_codes::REQUEST_FAILED),
        "{err:?}"
    );

    assert_eq!(client.shutdown(T).unwrap(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}
