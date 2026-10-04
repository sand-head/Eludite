//! The client against the real `eludite-host` (built by `dotnet build dotnet/Eludite.slnx`), without a language
//! server: proves the Rust types and the .NET host agree on the wire. Skips with a message when the host is not
//! built or `dotnet` is missing, so `cargo test` stays green on a machine without .NET.

use std::path::PathBuf;
use std::sync::mpsc::Receiver;
use std::time::{Duration, Instant};

use eludite_lsp::host::{self, HostDiagnosticSeverity, LanguageServerState, SolutionState};
use eludite_lsp::lsp::{self, Position, TextDocumentIdentifier, TextDocumentPositionParams};
use eludite_lsp::{ClientInfo, Error, Event, HostClient, HostCommand, RestartPolicy, StderrMode};

const T: Duration = Duration::from_secs(30);

fn host_dll() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ELUDITE_HOST_DLL") {
        return Some(PathBuf::from(p));
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    ["Release", "Debug"]
        .iter()
        .map(|c| {
            root.join(format!(
                "dotnet/src/Eludite.Host/bin/{c}/net10.0/eludite-host.dll"
            ))
        })
        .find(|p| p.exists())
}

/// The temp directory in the long form the host reports: Windows' `TEMP` is often an 8.3 short path.
fn temp_dir() -> PathBuf {
    let t = std::fs::canonicalize(std::env::temp_dir()).unwrap();
    match t.to_string_lossy().strip_prefix(r"\\?\") {
        Some(plain) => PathBuf::from(plain),
        None => t,
    }
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
            "skipped: eludite-host.dll not built (dotnet build dotnet/Eludite.slnx) and ELUDITE_HOST_DLL unset"
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
            name: "eludite-lsp-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
    )
    .expect("start eludite-host");

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

    let dir = temp_dir().join(format!("eludite-lsp-real-host-{}", std::process::id()));
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
    assert_eq!(failed.diagnostics[0].code, "ELUDITE0001");
    assert_eq!(
        failed.diagnostics[0].severity,
        HostDiagnosticSeverity::Error
    );

    // The tree is the host's own evaluation and needs no language server; this solution lists no projects.
    let tree = client
        .request::<host::SolutionTreeRequest>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    assert_eq!(tree.generation, 1);
    assert_eq!(tree.path.as_deref(), sln.to_str());
    assert!(tree.projects.is_empty());

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

/// Brief 0017: a real build through the real host, typed end to end: the reply, ordered output chunks, progress
/// and a failed result whose diagnostic has its file, position, code and project (from the binary log).
#[test]
fn real_host_builds_a_project() {
    let Some(dll) = host_dll() else {
        eprintln!(
            "skipped: eludite-host.dll not built (dotnet build dotnet/Eludite.slnx) and ELUDITE_HOST_DLL unset"
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
            name: "eludite-lsp-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
    )
    .expect("start eludite-host");
    let dir = temp_dir().join(format!("eludite-lsp-real-build-{}", std::process::id()));
    std::fs::create_dir_all(dir.join("App")).unwrap();
    let sln = dir.join("App.slnx");
    std::fs::write(
        &sln,
        "<Solution>\n  <Project Path=\"App/App.csproj\" />\n</Solution>\n",
    )
    .unwrap();
    let project = dir.join("App").join("App.csproj");
    std::fs::write(
        &project,
        "<Project Sdk=\"Microsoft.NET.Sdk\">\n  <PropertyGroup>\n    <TargetFramework>net10.0</TargetFramework>\n  </PropertyGroup>\n</Project>\n",
    )
    .unwrap();
    let source = dir.join("App").join("Broken.cs");
    std::fs::write(&source, "class Broken\n{\n    int x = y;\n}\n").unwrap();
    client.open_solution(sln.to_str().unwrap(), T).unwrap();

    let started = client
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
    assert_eq!(started.toolchain.kind, host::ToolchainKind::Dotnet);
    assert_eq!(started.platform, None);
    let mut seq = 0;
    let mut text = String::new();
    let deadline = Instant::now() + Duration::from_secs(180);
    let finished = loop {
        match rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .expect("build events")
        {
            Event::BuildOutput(o) => {
                assert_eq!((o.build_id, o.seq), (started.build_id, seq));
                seq += 1;
                text.push_str(&o.text);
            }
            Event::BuildFinished(f) => break f,
            _ => {}
        }
    };
    assert!(text.starts_with("Build started at "), "{text}");
    assert_eq!(finished.result, host::BuildResult::Failed);
    let error = finished
        .diagnostics
        .iter()
        .find(|d| d.code == "CS0103")
        .unwrap_or_else(|| panic!("{:?}", finished.diagnostics));
    assert_eq!(error.severity, host::BuildDiagnosticSeverity::Error);
    assert_eq!(
        std::path::Path::new(error.file.as_deref().unwrap()),
        source.as_path()
    );
    assert_eq!((error.line, error.column), (Some(3), Some(13)));
    assert_eq!(
        std::path::Path::new(error.project.as_deref().unwrap()),
        project.as_path()
    );
    assert_eq!(finished.projects[0].name, "App");
    assert_eq!(finished.projects[0].result, host::BuildResult::Failed);
    assert!(finished.binlog.is_some());

    assert_eq!(client.shutdown(T).unwrap(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}

/// The test corpus's MTP project (corpus/tests, built by `corpus/tests/build.sh`), its discovery and its run through
/// the real host (brief 0035). Skips when the host or the corpus is not built or `dotnet` is not on PATH.
#[test]
fn real_host_discovers_and_runs_the_mtp_corpus_project() {
    let Some(dll) = host_dll() else {
        eprintln!(
            "skipped: eludite-host.dll not built (dotnet build dotnet/Eludite.slnx) and ELUDITE_HOST_DLL unset"
        );
        return;
    };
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/tests");
    let project = corpus.join("Corpus.XunitV3").join("Corpus.XunitV3.csproj");
    let built = corpus.join("Corpus.XunitV3/bin/Debug/net10.0/Corpus.XunitV3.dll");
    if !built.exists() {
        eprintln!("skipped: corpus/tests is not built (corpus/tests/build.sh)");
        return;
    }
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
            name: "eludite-lsp-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
    )
    .expect("start eludite-host");
    let project = std::fs::canonicalize(project).unwrap();
    client.open_solution(project.to_str().unwrap(), T).unwrap();
    let found = client
        .request::<host::TestDiscover>(host::TestDiscoverParams::default())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let net10 = found
        .containers
        .iter()
        .find(|c| c.target_framework == "net10.0")
        .expect("the net10.0 container");
    assert_eq!(net10.protocol, host::TestProtocol::Mtp);
    let wait_finished = |run_id: u64| {
        let mut updates = Vec::new();
        let deadline = Instant::now() + Duration::from_secs(120);
        loop {
            if let Event::TestUpdate(u) = rx
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("test updates")
                && u.run_id == run_id
            {
                let done = u.kind == host::TestUpdateKind::Finished;
                updates.push(*u);
                if done {
                    return updates;
                }
            }
        }
    };
    let updates = wait_finished(found.run_id);
    let tests: Vec<host::TestItem> = updates
        .iter()
        .filter(|u| u.container.as_deref() == Some(net10.id.as_str()))
        .filter_map(|u| u.tests.clone())
        .flatten()
        .collect();
    assert_eq!(tests.len(), 7, "{tests:?}");
    let subtracts = tests
        .iter()
        .find(|t| t.method.as_deref() == Some("Subtracts"))
        .unwrap();
    assert_eq!(
        subtracts.fully_qualified_name,
        "Corpus.XunitV3.CalculatorTests.Subtracts"
    );
    let run = client
        .request::<host::TestRun>(host::TestRunParams {
            containers: Some(vec![host::TestRunContainer {
                id: net10.id.clone(),
                tests: None,
            }]),
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let updates = wait_finished(run.run_id);
    let finished = updates.last().unwrap();
    let summary = finished.summary.unwrap();
    assert_eq!(
        (
            summary.total,
            summary.passed,
            summary.failed,
            summary.skipped
        ),
        (7, 5, 1, 1)
    );
    let failed: Vec<host::TestResultItem> = updates
        .iter()
        .filter_map(|u| u.results.clone())
        .flatten()
        .filter(|r| r.outcome == host::TestOutcome::Failed)
        .collect();
    assert_eq!(failed[0].id, subtracts.id);
    assert!(
        failed[0]
            .stack_trace
            .as_deref()
            .unwrap()
            .contains("CalculatorTests.cs:line 25")
    );
    client.shutdown(Duration::from_secs(10)).unwrap();
}

fn copy_tree(from: &std::path::Path, to: &std::path::Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        if [".packages", "bin", "obj"].iter().any(|n| name == *n) {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).unwrap();
        }
    }
}

/// Brief 0048: the NuGet corpus (corpus/nuget, its local feed packed by `corpus/nuget/build.sh`) through the real
/// host, typed end to end: the sources of its NuGet.config, a search of the local feed, the installed packages before
/// a restore, an install into `Shared` that edits the project and restores it with ordered output updates and a new
/// generation, the installed packages after, and the tree's Dependencies node. Skips when the host is not built,
/// `dotnet` is not on PATH, or the feed cannot be packed.
#[test]
fn real_host_installs_a_package_from_the_nuget_corpus() {
    let Some(dll) = host_dll() else {
        eprintln!(
            "skipped: eludite-host.dll not built (dotnet build dotnet/Eludite.slnx) and ELUDITE_HOST_DLL unset"
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
    let corpus = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../corpus/nuget");
    let dir = temp_dir().join(format!("eludite-lsp-real-nuget-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(&corpus, &dir);
    if !dir.join("feed/Eludite.Corpus.Greeter.1.1.0.nupkg").exists() {
        let packed = std::process::Command::new("bash")
            .arg(corpus.join("build.sh"))
            .arg(&dir)
            .output();
        if !packed.as_ref().is_ok_and(|o| o.status.success()) {
            eprintln!(
                "skipped: the NuGet corpus's feed did not pack (corpus/nuget/build.sh): {packed:?}"
            );
            let _ = std::fs::remove_dir_all(&dir);
            return;
        }
    }
    let (client, rx) = HostClient::start(
        HostCommand::dotnet_host(&dll)
            .arg("--no-roslyn")
            .stderr(StderrMode::Discard),
        ClientInfo {
            name: "eludite-lsp-test".into(),
            version: "0".into(),
        },
        RestartPolicy {
            max_restarts: 0,
            backoff: Duration::ZERO,
        },
    )
    .expect("start eludite-host");
    let sln = dir.join("Corpus.slnx");
    let generation = client.open_solution(sln.to_str().unwrap(), T).unwrap();

    let sources = client
        .request::<host::NuGetSources>(host::NuGetSourcesParams {
            generation,
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let corpus_source = sources
        .sources
        .iter()
        .find(|s| s.name == "corpus")
        .unwrap_or_else(|| panic!("{sources:?}"));
    assert!(corpus_source.enabled && corpus_source.local);
    assert_eq!(corpus_source.scope, host::NuGetSourceScope::Solution);
    assert!(!sources.changed);

    let search = client
        .request::<host::NuGetSearch>(host::NuGetSearchParams {
            generation,
            query: Some("Corpus".into()),
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    // A local feed answers in its own order.
    let mut ids: Vec<_> = search
        .results
        .iter()
        .map(|p| (p.id.as_str(), p.version.as_str()))
        .collect();
    ids.sort();
    assert_eq!(
        ids,
        [
            ("Eludite.Corpus.Greeter", "1.1.0"),
            ("Eludite.Corpus.Logging", "1.0.0")
        ]
    );
    assert_eq!(search.sources[0].name, "corpus");
    assert_eq!(search.sources[0].error, None);

    let before = client
        .request::<host::NuGetInstalled>(host::NuGetInstalledParams {
            generation,
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let app = before.projects.iter().find(|p| p.name == "App").unwrap();
    assert_eq!(app.format, host::NuGetProjectFormat::PackageReference);
    let greeter = app
        .packages
        .iter()
        .find(|p| p.id == "Eludite.Corpus.Greeter")
        .unwrap();
    assert_eq!(greeter.requested.as_deref(), Some("1.0.0"));

    let shared = dir.join("Shared").join("Shared.csproj");
    let changed = client
        .request::<host::NuGetChange>(host::NuGetChangeParams {
            generation,
            operation: Some(7),
            action: host::NuGetAction::Install,
            packages: vec![host::NuGetPackageArg {
                id: "Eludite.Corpus.Logging".into(),
                version: None,
            }],
            projects: Some(vec![shared.to_str().unwrap().to_owned()]),
            prerelease: None,
            source: None,
            include_transitive: None,
            restore: Some(true),
            lock_files: None,
            interactive: None,
        })
        .unwrap()
        .wait_timeout(Duration::from_secs(180))
        .unwrap();
    assert!(changed.generation > generation, "{changed:?}");
    assert_eq!(changed.packages[0].version.as_deref(), Some("1.0.0"));
    assert_eq!(
        std::path::Path::new(&changed.edited[0].path),
        shared.as_path()
    );
    let restore = changed.restore.as_ref().unwrap();
    assert_eq!(
        restore.result,
        host::NuGetRestoreState::Succeeded,
        "{restore:?}"
    );
    let text = std::fs::read_to_string(&shared).unwrap();
    assert!(
        text.contains(r#"<PackageReference Include="Eludite.Corpus.Logging" Version="1.0.0" />"#),
        "{text}"
    );
    // The restore's output came as ordered updates of operation 7.
    let mut seqs = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let Event::NuGetUpdate(u) = event
            && u.operation == 7
            && u.kind == host::NuGetUpdateKind::Output
        {
            seqs.push(u.seq);
        }
    }
    assert!(!seqs.is_empty());
    assert!(seqs.windows(2).all(|w| w[0] < w[1]), "{seqs:?}");

    let generation = changed.generation;
    let after = client
        .request::<host::NuGetInstalled>(host::NuGetInstalledParams {
            generation,
            include_transitive: Some(true),
            ..Default::default()
        })
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let shared_after = after.projects.iter().find(|p| p.name == "Shared").unwrap();
    assert!(shared_after.restored);
    assert_eq!(shared_after.packages[0].id, "Eludite.Corpus.Logging");
    assert_eq!(shared_after.packages[0].version.as_deref(), Some("1.0.0"));

    // The tree's Dependencies node reads the same files.
    let tree = client
        .request::<host::SolutionTreeRequest>(())
        .unwrap()
        .wait_timeout(T)
        .unwrap();
    let shared_tree = tree.projects.iter().find(|p| p.name == "Shared").unwrap();
    let deps = shared_tree.dependencies.as_ref().unwrap();
    assert!(deps.restored);
    assert_eq!(deps.packages[0].id, "Eludite.Corpus.Logging");
    let app_tree = tree.projects.iter().find(|p| p.name == "App").unwrap();
    let app_deps = app_tree.dependencies.as_ref().unwrap();
    assert_eq!(app_deps.projects[0].name, "Shared");
    assert_eq!(app_deps.frameworks[0].name, "Microsoft.NETCore.App");

    assert_eq!(client.shutdown(T).unwrap(), Some(0));
    let _ = std::fs::remove_dir_all(&dir);
}
