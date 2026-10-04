//! Keeps the Rust types, `host-rpc.md` and the JSON schemas in `protocol/schemas/host/` in step.

use std::collections::BTreeSet;
use std::path::PathBuf;

use serde::Serialize;
use serde_json::{Value, json};

use crate::host::{self, methods};
use crate::lsp;

fn schemas_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../schemas")
}

fn load(name: &str) -> Value {
    let path = schemas_dir().join("host").join(name);
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// A deliberately small JSON-schema check: `type`, `const`, `enum`, `required`, `properties`,
/// `additionalProperties: false`, `items`, `minimum`, `minLength`, `maxProperties`, `maxItems`.
fn validate(schema: &Value, value: &Value, at: &str) -> Result<(), String> {
    if let Some(t) = schema.get("type") {
        let types: Vec<&str> = match t {
            Value::String(s) => vec![s.as_str()],
            Value::Array(a) => a.iter().filter_map(Value::as_str).collect(),
            _ => vec![],
        };
        let ok = types.iter().any(|t| match *t {
            "object" => value.is_object(),
            "array" => value.is_array(),
            "string" => value.is_string(),
            "integer" => value.is_i64() || value.is_u64(),
            "number" => value.is_number(),
            "boolean" => value.is_boolean(),
            "null" => value.is_null(),
            _ => false,
        });
        if !ok {
            return Err(format!("{at}: {value} is not {types:?}"));
        }
    }
    if let Some(c) = schema.get("const")
        && c != value
    {
        return Err(format!("{at}: {value} != const {c}"));
    }
    if let Some(Value::Array(e)) = schema.get("enum")
        && !e.contains(value)
    {
        return Err(format!("{at}: {value} not in {e:?}"));
    }
    if let (Some(min), Some(n)) = (
        schema.get("minimum").and_then(Value::as_f64),
        value.as_f64(),
    ) && n < min
    {
        return Err(format!("{at}: {n} < {min}"));
    }
    if let (Some(min), Some(s)) = (
        schema.get("minLength").and_then(Value::as_u64),
        value.as_str(),
    ) && (s.len() as u64) < min
    {
        return Err(format!("{at}: string shorter than {min}"));
    }
    if let Value::Object(obj) = value {
        if let Some(max) = schema.get("maxProperties").and_then(Value::as_u64)
            && obj.len() as u64 > max
        {
            return Err(format!("{at}: more than {max} properties"));
        }
        if let Some(Value::Array(req)) = schema.get("required") {
            for r in req.iter().filter_map(Value::as_str) {
                if !obj.contains_key(r) {
                    return Err(format!("{at}: missing required {r}"));
                }
            }
        }
        let props = schema.get("properties").and_then(Value::as_object);
        for (k, v) in obj {
            match props.and_then(|p| p.get(k)) {
                Some(s) => validate(s, v, &format!("{at}.{k}"))?,
                None if schema.get("additionalProperties") == Some(&Value::Bool(false)) => {
                    return Err(format!("{at}: unexpected property {k}"));
                }
                None => {}
            }
        }
    }
    if let Value::Array(items) = value {
        if let Some(max) = schema.get("maxItems").and_then(Value::as_u64)
            && items.len() as u64 > max
        {
            return Err(format!("{at}: more than {max} items"));
        }
        if let Some(s) = schema.get("items") {
            for (i, v) in items.iter().enumerate() {
                validate(s, v, &format!("{at}[{i}]"))?;
            }
        }
    }
    Ok(())
}

fn conforms<T: Serialize>(file: &str, def: &str, value: &T) {
    let schema = load(file);
    let def_schema = &schema["$defs"][def];
    assert!(!def_schema.is_null(), "{file} has no $defs.{def}");
    let v = serde_json::to_value(value).unwrap();
    // A Rust `()` is "no params" and is sent as an omitted params member.
    let v = if v.is_null() && def == "params" {
        json!({})
    } else {
        v
    };
    validate(def_schema, &v, file).unwrap_or_else(|e| panic!("{e}"));
}

fn rejects(file: &str, def: &str, value: Value) {
    let schema = load(file);
    assert!(
        validate(&schema["$defs"][def], &value, file).is_err(),
        "{file} $defs.{def} should reject {value}"
    );
}

#[test]
fn every_schema_file_names_a_documented_method() {
    let mut documented: BTreeSet<&str> = methods::ELUDITE_ACCEPTED.iter().copied().collect();
    documented.extend(methods::HOST_TO_SHELL);
    // Typed forwarded requests may have a schema for the members Eludite reads (signature-help.json).
    documented.extend(methods::FORWARDED_TYPED_REQUESTS);
    let mut seen = BTreeSet::new();
    for entry in std::fs::read_dir(schemas_dir().join("host")).unwrap() {
        let path = entry.unwrap().path();
        let name = path.file_name().unwrap().to_string_lossy().into_owned();
        let schema = load(&name);
        assert_eq!(
            schema["$id"],
            json!(format!(
                "https://github.com/sand-head/Eludite/protocol/schemas/host/{name}"
            )),
            "$id of {name}"
        );
        let method = schema["x-eludite-method"].as_str().unwrap();
        if method != "*" {
            assert!(
                documented.contains(method),
                "{name}: {method} is not a known method"
            );
            seen.insert(method.to_owned());
        }
    }
    // Every Eludite-specific message has its own schema file.
    for m in methods::ELUDITE_ACCEPTED.iter().chain(&[
        methods::SOLUTION_STATUS,
        methods::LANGUAGE_SERVER_STATUS,
        methods::BUILD_OUTPUT,
        methods::BUILD_PROGRESS,
        methods::BUILD_FINISHED,
        methods::TEST_UPDATE,
        methods::NUGET_UPDATE,
        methods::NUGET_CREDENTIALS,
    ]) {
        assert!(seen.contains(*m), "no schema file for {m}");
    }
}

#[test]
fn host_rpc_md_lists_exactly_the_rust_method_sets() {
    let md = std::fs::read_to_string(schemas_dir().join("host-rpc.md")).unwrap();
    // First-column method names of the markdown tables.
    let mut in_tables = BTreeSet::new();
    for line in md.lines() {
        if let Some(rest) = line.strip_prefix("| `")
            && let Some((name, _)) = rest.split_once('`')
            && (name.contains('/') || name.starts_with('$'))
        {
            in_tables.insert(name.to_owned());
        }
    }
    let mut rust: BTreeSet<String> = BTreeSet::new();
    for set in [
        methods::ELUDITE_ACCEPTED,
        methods::FORWARDED_TYPED_REQUESTS,
        methods::FORWARDED_TYPED_NOTIFICATIONS,
        methods::FORWARDED_UNTYPED_REQUESTS,
        methods::FORWARDED_UNTYPED_NOTIFICATIONS,
        methods::HOST_TO_SHELL,
    ] {
        rust.extend(set.iter().map(|s| s.to_string()));
    }
    // Server-to-client requests the host answers itself are documented in their own table.
    let answered: BTreeSet<String> = [
        "workspace/configuration",
        "client/registerCapability",
        "window/workDoneProgress/create",
        "window/showMessageRequest",
        "workspace/diagnostic/refresh",
        "workspace/semanticTokens/refresh",
    ]
    .iter()
    .map(|s| s.to_string())
    .collect();
    let missing: Vec<_> = rust.difference(&in_tables).collect();
    assert!(missing.is_empty(), "not in host-rpc.md tables: {missing:?}");
    let extra: Vec<_> = in_tables
        .difference(&rust)
        .filter(|m| !answered.contains(*m))
        .collect();
    assert!(
        extra.is_empty(),
        "in host-rpc.md but not in Rust: {extra:?}"
    );
}

#[test]
fn eludite_messages_conform_to_their_schemas() {
    conforms(
        "host-initialize.json",
        "params",
        &host::InitializeParams {
            client_name: "eludite".into(),
            client_version: "0.1.0".into(),
        },
    );
    conforms(
        "host-initialize.json",
        "result",
        &host::InitializeResult {
            host_name: host::HOST_NAME.into(),
            host_version: "0.1.0".into(),
            capabilities: host::HostCapabilities {
                language_server: true,
            },
        },
    );
    rejects(
        "host-initialize.json",
        "result",
        json!({"hostName": "other", "hostVersion": "1", "capabilities": {"languageServer": false}}),
    );
    conforms("ping.json", "params", &());
    conforms(
        "ping.json",
        "result",
        &host::PingResult {
            pong: true,
            timestamp: "2026-10-01T00:00:00Z".into(),
        },
    );
    conforms(
        "host-info.json",
        "result",
        &host::HostInfoResult {
            dotnet_sdks: vec![host::DotnetSdk {
                version: "10.0.302".into(),
                path: "/usr/share/dotnet/sdk".into(),
            }],
            runtime: ".NET 10".into(),
            os: "Linux".into(),
        },
    );
    conforms("host-shutdown.json", "params", &());
    conforms("host-shutdown.json", "result", &());
    conforms("host-exit.json", "params", &());
    rejects("host-exit.json", "params", json!({"x": 1}));
    conforms(
        "solution-open.json",
        "params",
        &host::SolutionOpenParams {
            path: "/src/App.sln".into(),
        },
    );
    rejects("solution-open.json", "params", json!({"path": ""}));
    conforms(
        "solution-open.json",
        "result",
        &host::GenerationResult { generation: 1 },
    );
    conforms("solution-close.json", "params", &());
    conforms(
        "solution-close.json",
        "result",
        &host::GenerationResult { generation: 2 },
    );
    conforms("solution-tree.json", "params", &());
    let project = host::TreeProject {
        name: "Shop".into(),
        path: "/src/Shop/Shop.csproj".into(),
        kind: host::TreeProjectKind::Legacy,
        web: true,
        target_frameworks: vec!["net48".into()],
        files: vec![host::TreeFile {
            path: "/src/Shop/Default.aspx.cs".into(),
            item_type: host::TreeItemType::Compile,
            dependent_upon: Some("/src/Shop/Default.aspx".into()),
            link: None,
        }],
        error: None,
        dependencies: None,
    };
    conforms("solution-tree.json", "project", &project);
    conforms("solution-tree.json", "file", &project.files[0]);
    conforms(
        "solution-tree.json",
        "result",
        &host::SolutionTree {
            generation: 3,
            path: Some("/src/Shop.sln".into()),
            projects: vec![project],
        },
    );
    conforms(
        "solution-tree.json",
        "result",
        &host::SolutionTree {
            generation: 0,
            path: None,
            projects: vec![],
        },
    );
    rejects(
        "solution-tree.json",
        "project",
        json!({"name": "A", "path": "/a.csproj", "kind": "vb", "targetFrameworks": [], "files": []}),
    );
    rejects(
        "solution-tree.json",
        "file",
        json!({"path": "/a.cs", "itemType": "none"}),
    );
    conforms(
        "solution-status.json",
        "params",
        &host::SolutionStatus {
            generation: 1,
            path: "/src/App.sln".into(),
            state: host::SolutionState::Loaded,
            phase: None,
            counts: Some(host::ProjectCounts {
                projects: 3,
                legacy_projects: 1,
                legacy_evaluation_failures: 0,
            }),
            msbuild: Some(host::MsBuildInfo {
                kind: host::MsBuildKind::Mono,
                path: Some("/usr/lib/mono/msbuild/Current/bin/MSBuild.dll".into()),
                source: Some("system".into()),
            }),
            corrections: vec![host::Correction {
                kind: host::CorrectionKind::DesignerPartials,
                project: "/src/Web.csproj".into(),
                count: 16,
            }],
            diagnostics: vec![host::HostDiagnostic {
                severity: host::HostDiagnosticSeverity::Warning,
                code: "ELUDITE0106".into(),
                message: "case".into(),
                project: Some("/src/Core.csproj".into()),
                class: None,
            }],
            elapsed_ms: Some(1.0),
        },
    );
    conforms(
        "solution-status.json",
        "params",
        &host::SolutionStatus {
            generation: 1,
            path: "/src/App.sln".into(),
            state: host::SolutionState::Loading,
            phase: Some(host::LoadPhase::ProjectLoad),
            counts: None,
            msbuild: None,
            corrections: vec![],
            diagnostics: vec![],
            elapsed_ms: None,
        },
    );
    rejects(
        "solution-status.json",
        "params",
        json!({"generation": 1, "path": "/a", "state": "busy"}),
    );
    conforms(
        "language-server-status.json",
        "params",
        &host::LanguageServerStatus {
            state: host::LanguageServerState::Running,
            server_info: Some(host::ServerInfo {
                name: "roslyn".into(),
                version: None,
            }),
            capabilities: Some(json!({})),
            message: None,
        },
    );
    conforms(
        "forwarded-request.json",
        "params",
        &host::WithGeneration {
            params: lsp::WorkspaceSymbolParams { query: "W".into() },
            generation: 0,
        },
    );
    rejects("forwarded-request.json", "params", json!({"query": "W"}));
    rejects(
        "forwarded-request.json",
        "params",
        json!({"query": "W", "eluditeGeneration": -1}),
    );
    conforms(
        "publish-diagnostics.json",
        "params",
        &host::WithGeneration {
            params: lsp::PublishDiagnosticsParams {
                uri: "file:///a.cs".into(),
                version: Some(1),
                diagnostics: vec![],
            },
            generation: 1,
        },
    );
    conforms(
        "errors.json",
        "contentModified",
        &host::ContentModifiedData {
            requested_generation: 0,
            current_generation: 1,
        },
    );
    conforms(
        "errors.json",
        "requestFailed",
        &host::RequestFailedData {
            reason: "languageServerUnavailable".into(),
        },
    );
}

#[test]
fn signature_help_conforms_to_its_schema() {
    use crate::typed::RequestType;
    let schema = load("signature-help.json");
    assert_eq!(
        schema["x-eludite-method"],
        lsp::SignatureHelpRequest::METHOD
    );
    assert!(methods::FORWARDED_TYPED_REQUESTS.contains(&lsp::SignatureHelpRequest::METHOD));
    assert!(!methods::FORWARDED_UNTYPED_REQUESTS.contains(&lsp::SignatureHelpRequest::METHOD));
    conforms(
        "signature-help.json",
        "params",
        &host::WithGeneration {
            params: lsp::SignatureHelpParams {
                text_document: lsp::TextDocumentIdentifier {
                    uri: "file:///a.cs".into(),
                },
                position: lsp::Position {
                    line: 3,
                    character: 9,
                },
                context: Some(lsp::SignatureHelpContext {
                    trigger_kind: 2,
                    trigger_character: Some("(".into()),
                    is_retrigger: false,
                    active_signature_help: None,
                }),
            },
            generation: 1,
        },
    );
    let help = lsp::SignatureHelp {
        signatures: vec![lsp::SignatureInformation {
            label: "void M(int a)".into(),
            documentation: Some(json!({"kind": "markdown", "value": "M"})),
            parameters: Some(vec![lsp::ParameterInformation {
                label: lsp::ParameterLabel::Offsets([7, 12]),
                documentation: None,
                extra: Default::default(),
            }]),
            active_parameter: None,
            extra: Default::default(),
        }],
        active_signature: Some(0),
        active_parameter: Some(0),
        extra: Default::default(),
    };
    conforms("signature-help.json", "result", &Some(help.clone()));
    conforms("signature-help.json", "result", &None::<lsp::SignatureHelp>);
    conforms(
        "signature-help.json",
        "signatureInformation",
        &help.signatures[0],
    );
    rejects(
        "signature-help.json",
        "params",
        json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 0}}),
    );
    rejects(
        "signature-help.json",
        "params",
        json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 0},
               "context": {"triggerKind": 4, "isRetrigger": false}, "eluditeGeneration": 0}),
    );
    rejects(
        "signature-help.json",
        "result",
        json!({"activeSignature": 0}),
    );
}

#[test]
fn typed_marker_methods_match_the_method_lists() {
    use crate::typed::{NotificationType, RequestType};
    fn req<R: RequestType>() -> (&'static str, bool) {
        (R::METHOD, R::GENERATIONAL)
    }
    let typed = [
        req::<lsp::Completion>(),
        req::<lsp::ResolveCompletionItem>(),
        req::<lsp::HoverRequest>(),
        req::<lsp::SignatureHelpRequest>(),
        req::<lsp::GotoDefinition>(),
        req::<lsp::References>(),
        req::<lsp::PrepareRename>(),
        req::<lsp::Rename>(),
        req::<lsp::CodeActionRequest>(),
        req::<lsp::ResolveCodeAction>(),
        req::<lsp::DocumentSymbolRequest>(),
        req::<lsp::WorkspaceSymbolRequest>(),
        req::<lsp::DocumentDiagnosticRequest>(),
    ];
    let names: BTreeSet<_> = typed.iter().map(|(m, _)| *m).collect();
    let listed: BTreeSet<_> = methods::FORWARDED_TYPED_REQUESTS.iter().copied().collect();
    assert_eq!(names, listed);
    assert!(typed.iter().all(|(_, g)| *g));

    let eludite = [
        req::<host::HostInitialize>(),
        req::<host::Ping>(),
        req::<host::HostInfo>(),
        req::<host::HostShutdown>(),
        req::<host::SolutionOpen>(),
        req::<host::SolutionClose>(),
        req::<host::SolutionTreeRequest>(),
        req::<host::BuildStart>(),
        req::<host::BuildCancel>(),
        req::<host::BuildStatus>(),
        req::<host::TestDiscover>(),
        req::<host::TestRun>(),
        req::<host::TestCancel>(),
        req::<host::TestAttached>(),
        req::<host::TestStatus>(),
        req::<host::NuGetSearch>(),
        req::<host::NuGetInstalled>(),
        req::<host::NuGetUpdates>(),
        req::<host::NuGetChange>(),
        req::<host::NuGetSources>(),
        req::<host::NuGetRestore>(),
        req::<host::NuGetIcon>(),
        req::<host::ProjectProperties>(),
        req::<host::ProjectSetProperty>(),
        req::<host::ProjectLaunchProfiles>(),
        req::<host::ProjectSetLaunchProfile>(),
        req::<host::SolutionConfigurations>(),
        req::<host::SolutionSetConfiguration>(),
    ];
    assert!(
        eludite
            .iter()
            .all(|(m, g)| !*g && methods::ELUDITE_ACCEPTED.contains(m))
    );
    assert_eq!(host::HostExit::METHOD, methods::HOST_EXIT);

    let notes = [
        lsp::DidOpenTextDocument::METHOD,
        lsp::DidChangeTextDocument::METHOD,
        lsp::DidCloseTextDocument::METHOD,
        lsp::Cancel::METHOD,
    ];
    assert_eq!(notes.as_slice(), methods::FORWARDED_TYPED_NOTIFICATIONS);
    assert!(methods::HOST_TO_SHELL.contains(&lsp::PublishDiagnostics::METHOD));
    assert!(methods::HOST_TO_SHELL.contains(&lsp::ApplyEdit::METHOD));
    assert_eq!(
        methods::HOST_TO_SHELL_REQUESTS,
        [lsp::ApplyEdit::METHOD, host::NuGetCredentials::METHOD]
    );
    assert!(methods::HOST_TO_SHELL.contains(&host::NuGetCredentials::METHOD));
    assert!(methods::HOST_TO_SHELL.contains(&host::SolutionStatusNotification::METHOD));
    assert!(methods::HOST_TO_SHELL.contains(&host::LanguageServerStatusNotification::METHOD));
    for m in [
        host::BuildOutputNotification::METHOD,
        host::BuildProgressNotification::METHOD,
        host::BuildFinishedNotification::METHOD,
        host::TestUpdateNotification::METHOD,
        host::NuGetUpdateNotification::METHOD,
    ] {
        assert!(methods::HOST_TO_SHELL.contains(&m), "{m}");
    }
}

#[test]
fn build_status_conforms_to_its_schema() {
    use host::*;
    conforms("build-status.json", "params", &());
    conforms("build-status.json", "result", &BuildStatusResult::default());
    rejects("build-status.json", "result", json!({}));
    rejects("build-status.json", "params", json!({"buildId": 1}));
    let running = BuildStatusRunning {
        build_id: 3,
        generation: 2,
        system: None,
        path: "/s/A.slnx".into(),
        target: BuildTarget::Rebuild,
        configuration: "Debug".into(),
        platform: Some("x64".into()),
        toolchain: Toolchain {
            kind: ToolchainKind::Dotnet,
            path: Some("/usr/bin/dotnet".into()),
            source: None,
        },
        binlog: Some("/tmp/b.binlog".into()),
        command_line: "dotnet build -t:Rebuild /s/A.slnx".into(),
        elapsed_ms: 812.5,
        progress: Some(BuildProgress {
            build_id: 3,
            elapsed_ms: 800.0,
            projects_total: 8,
            projects_completed: 2,
            errors: 1,
            warnings: 0,
            current_project: Some("Eludite.Host".into()),
        }),
        output: BuildStatusOutput {
            first_seq: 0,
            next_seq: 4,
            text: "Rebuild All started at 10:00:00...\n".into(),
            truncated: false,
        },
    };
    let last = BuildStatusLast {
        build_id: 2,
        generation: 2,
        target: BuildTarget::Build,
        path: "/s/A.slnx".into(),
        result: BuildResult::Failed,
        elapsed_ms: 1500.0,
        summary: BuildSummary {
            projects_succeeded: 7,
            projects_failed: 1,
            errors: 1,
            warnings: 0,
        },
    };
    let full = BuildStatusResult {
        running: Some(running.clone()),
        last: Some(last),
    };
    conforms("build-status.json", "result", &full);
    let v = serde_json::to_value(&full).unwrap();
    assert_eq!(v["running"]["output"]["nextSeq"], 4);
    assert_eq!(
        v["running"]["commandLine"],
        "dotnet build -t:Rebuild /s/A.slnx"
    );
    let back: BuildStatusResult = serde_json::from_value(v).unwrap();
    assert_eq!(back, full);
    // What the C# host sends when nothing runs: `running` is present and null, `last` omitted.
    let idle: BuildStatusResult = serde_json::from_value(json!({"running": null})).unwrap();
    assert_eq!(idle, BuildStatusResult::default());
    assert_eq!(running.start_result().build_id, 3);
    assert_eq!(running.start_result().platform.as_deref(), Some("x64"));
    // A canceled build in the past without one running, and a running build without the optional members.
    rejects(
        "build-status.json",
        "result",
        json!({"running": {"buildId": 1, "generation": 0, "path": "/a", "target": "build",
                           "configuration": "Debug", "toolchain": {"kind": "dotnet"}, "commandLine": "x",
                           "elapsedMs": 1.0}}),
    );
}

#[test]
fn build_messages_conform_to_their_schemas() {
    use host::*;
    conforms(
        "build-start.json",
        "params",
        &BuildStartParams {
            target: BuildTarget::Build,
            system: None,
            project: None,
            configuration: None,
            platform: None,
        },
    );
    conforms(
        "build-start.json",
        "params",
        &BuildStartParams {
            target: BuildTarget::Clean,
            system: Some(BuildSystem::Msbuild),
            project: Some("/s/A/A.csproj".into()),
            configuration: Some("Release".into()),
            platform: Some("Any CPU".into()),
        },
    );
    rejects("build-start.json", "params", json!({}));
    rejects("build-start.json", "params", json!({"target": "publish"}));
    rejects(
        "build-start.json",
        "params",
        json!({"target": "build", "solution": "/a.sln"}),
    );
    let toolchain = Toolchain {
        kind: ToolchainKind::Dotnet,
        path: Some("dotnet".into()),
        source: None,
    };
    conforms("build-start.json", "toolchain", &toolchain);
    conforms(
        "build-start.json",
        "result",
        &BuildStartResult {
            build_id: 1,
            generation: 2,
            system: None,
            path: "/s/A.slnx".into(),
            target: BuildTarget::Build,
            configuration: "Debug".into(),
            platform: None,
            toolchain,
            binlog: None,
            command_line: "dotnet build /s/A.slnx".into(),
        },
    );
    rejects(
        "build-start.json",
        "result",
        json!({"buildId": 0, "generation": 0, "path": "/a", "target": "build", "configuration": "Debug",
               "toolchain": {"kind": "dotnet"}, "commandLine": "x"}),
    );
    // The shell's own Cargo builds share the shape (brief 0019).
    conforms(
        "build-start.json",
        "result",
        &BuildStartResult {
            build_id: 1 << 32,
            generation: 0,
            system: Some(BuildSystem::Cargo),
            path: "/w/Cargo.toml".into(),
            target: BuildTarget::Build,
            configuration: "Debug".into(),
            platform: None,
            toolchain: Toolchain {
                kind: ToolchainKind::Cargo,
                path: Some("cargo".into()),
                source: Some("PATH".into()),
            },
            binlog: None,
            command_line: "cargo build --message-format=json-diagnostic-rendered-ansi".into(),
        },
    );
    rejects(
        "build-start.json",
        "params",
        json!({"target": "build", "system": "make"}),
    );
    conforms(
        "errors.json",
        "buildInProgress",
        &BuildInProgressData { build_id: 4 },
    );
    conforms("build-cancel.json", "params", &BuildCancelParams::default());
    conforms(
        "build-cancel.json",
        "params",
        &BuildCancelParams { build_id: Some(2) },
    );
    conforms(
        "build-cancel.json",
        "result",
        &BuildCancelResult {
            canceled: false,
            build_id: None,
        },
    );
    conforms(
        "build-output.json",
        "params",
        &BuildOutput {
            build_id: 1,
            seq: 0,
            text: "a\nb\n".into(),
        },
    );
    rejects(
        "build-output.json",
        "params",
        json!({"buildId": 1, "text": "a\n"}),
    );
    conforms(
        "build-progress.json",
        "params",
        &BuildProgress {
            build_id: 1,
            elapsed_ms: 10.0,
            projects_total: 8,
            projects_completed: 1,
            errors: 0,
            warnings: 2,
            current_project: Some("A".into()),
        },
    );
    let diagnostic = BuildDiagnostic {
        severity: BuildDiagnosticSeverity::Warning,
        code: "ELUDITE0101".into(),
        message: "COM reference skipped".into(),
        file: None,
        line: None,
        column: None,
        end_line: None,
        end_column: None,
        project: Some("/s/A/A.csproj".into()),
    };
    conforms("build-finished.json", "diagnostic", &diagnostic);
    let project = BuildProjectResult {
        name: "A".into(),
        path: "/s/A/A.csproj".into(),
        result: BuildResult::Succeeded,
        elapsed_ms: Some(10.0),
        errors: 0,
        warnings: 1,
    };
    conforms("build-finished.json", "project", &project);
    conforms(
        "build-finished.json",
        "params",
        &BuildFinished {
            build_id: 1,
            generation: 2,
            target: BuildTarget::Rebuild,
            path: "/s/A.slnx".into(),
            result: BuildResult::Canceled,
            exit_code: None,
            elapsed_ms: 1200.0,
            summary: BuildSummary::default(),
            projects: vec![project],
            diagnostics: vec![diagnostic],
            diagnostics_truncated: true,
            binlog: None,
            message: Some("canceled".into()),
        },
    );
    rejects(
        "build-finished.json",
        "project",
        json!({"name": "A", "path": "/a", "result": "skipped", "errors": 0, "warnings": 0}),
    );
    rejects(
        "build-finished.json",
        "diagnostic",
        json!({"severity": "fatal", "code": "", "message": ""}),
    );
}

fn sample_edit() -> lsp::WorkspaceEdit {
    let range = lsp::Range {
        start: lsp::Position {
            line: 1,
            character: 2,
        },
        end: lsp::Position {
            line: 1,
            character: 6,
        },
    };
    lsp::WorkspaceEdit {
        changes: None,
        document_changes: Some(vec![
            lsp::DocumentChange::Edit(lsp::TextDocumentEdit {
                text_document: lsp::OptionalVersionedTextDocumentIdentifier {
                    uri: "file:///s/A.cs".into(),
                    version: None,
                },
                edits: vec![lsp::TextEdit::new(range, "Pong")],
            }),
            lsp::DocumentChange::Operation(lsp::ResourceOperation::Create {
                uri: "file:///s/New.cs".into(),
                options: None,
                annotation_id: None,
            }),
            lsp::DocumentChange::Operation(lsp::ResourceOperation::Rename {
                old_uri: "file:///s/Old.cs".into(),
                new_uri: "file:///s/Renamed.cs".into(),
                options: Some(lsp::CreateFileOptions {
                    overwrite: Some(true),
                    ignore_if_exists: None,
                }),
                annotation_id: None,
            }),
            lsp::DocumentChange::Operation(lsp::ResourceOperation::Delete {
                uri: "file:///s/Gone.cs".into(),
                options: None,
                annotation_id: None,
            }),
        ]),
        extra: Default::default(),
    }
}

#[test]
fn rename_code_action_and_apply_edit_conform_to_their_schemas() {
    use crate::typed::RequestType;
    for (file, method) in [
        ("prepare-rename.json", lsp::PrepareRename::METHOD),
        ("rename.json", lsp::Rename::METHOD),
        ("code-action.json", lsp::CodeActionRequest::METHOD),
        ("code-action-resolve.json", lsp::ResolveCodeAction::METHOD),
    ] {
        assert_eq!(load(file)["x-eludite-method"], method, "{file}");
        assert!(methods::FORWARDED_TYPED_REQUESTS.contains(&method));
        assert!(!methods::FORWARDED_UNTYPED_REQUESTS.contains(&method));
    }
    assert_eq!(
        load("apply-edit.json")["x-eludite-method"],
        lsp::ApplyEdit::METHOD
    );
    let doc = lsp::TextDocumentIdentifier {
        uri: "file:///s/A.cs".into(),
    };
    let position = lsp::Position {
        line: 1,
        character: 3,
    };
    let range = lsp::Range {
        start: position,
        end: position,
    };
    conforms(
        "prepare-rename.json",
        "params",
        &host::WithGeneration {
            params: lsp::TextDocumentPositionParams {
                text_document: doc.clone(),
                position,
            },
            generation: 1,
        },
    );
    conforms(
        "prepare-rename.json",
        "result",
        &Some(lsp::PrepareRenameResponse::Range(range)),
    );
    conforms(
        "prepare-rename.json",
        "result",
        &Some(lsp::PrepareRenameResponse::RangeWithPlaceholder {
            range,
            placeholder: "Ping".into(),
        }),
    );
    conforms(
        "prepare-rename.json",
        "result",
        &None::<lsp::PrepareRenameResponse>,
    );
    rejects(
        "prepare-rename.json",
        "params",
        json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 0}}),
    );
    conforms(
        "rename.json",
        "params",
        &host::WithGeneration {
            params: lsp::RenameParams {
                text_document: doc.clone(),
                position,
                new_name: "Pong".into(),
            },
            generation: 1,
        },
    );
    conforms("rename.json", "result", &Some(sample_edit()));
    conforms("rename.json", "result", &None::<lsp::WorkspaceEdit>);
    rejects(
        "rename.json",
        "params",
        json!({"textDocument": {"uri": "file:///a.cs"}, "position": {"line": 0, "character": 0},
               "newName": "", "eluditeGeneration": 0}),
    );
    conforms(
        "code-action.json",
        "params",
        &host::WithGeneration {
            params: lsp::CodeActionParams {
                text_document: doc,
                range,
                context: lsp::CodeActionContext {
                    diagnostics: vec![],
                    only: None,
                    trigger_kind: Some(1),
                },
            },
            generation: 1,
        },
    );
    rejects(
        "code-action.json",
        "params",
        json!({"textDocument": {"uri": "file:///a.cs"},
               "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
               "context": {"triggerKind": 1}, "eluditeGeneration": 0}),
    );
    let action = lsp::CodeAction {
        title: "Use primary constructor".into(),
        kind: Some("quickfix".into()),
        data: Some(json!({"UniqueIdentifier": "Use primary constructor"})),
        ..Default::default()
    };
    conforms(
        "code-action.json",
        "result",
        &Some(vec![
            lsp::CodeActionOrCommand::Action(Box::new(action.clone())),
            lsp::CodeActionOrCommand::Command(lsp::Command {
                title: "Organize".into(),
                command: "x.organize".into(),
                arguments: None,
            }),
        ]),
    );
    conforms("code-action.json", "codeAction", &action);
    conforms(
        "code-action-resolve.json",
        "params",
        &host::WithGeneration {
            params: action.clone(),
            generation: 2,
        },
    );
    let resolved = lsp::CodeAction {
        edit: Some(sample_edit()),
        ..action
    };
    conforms("code-action-resolve.json", "result", &resolved);
    rejects(
        "code-action-resolve.json",
        "params",
        json!({"kind": "quickfix", "eluditeGeneration": 0}),
    );
    conforms(
        "apply-edit.json",
        "params",
        &host::WithGeneration {
            params: lsp::ApplyWorkspaceEditParams {
                label: Some("Use primary constructor".into()),
                edit: sample_edit(),
            },
            generation: 1,
        },
    );
    conforms(
        "apply-edit.json",
        "result",
        &lsp::ApplyWorkspaceEditResult {
            applied: true,
            failure_reason: None,
            failed_change: None,
        },
    );
    rejects("apply-edit.json", "result", json!({"failureReason": "x"}));
    rejects("apply-edit.json", "params", json!({"edit": {}}));
    conforms("apply-edit.json", "workspaceEdit", &sample_edit());
    let edit = serde_json::to_value(sample_edit()).unwrap();
    for (i, def) in ["textDocumentEdit", "createFile", "renameFile", "deleteFile"]
        .iter()
        .enumerate()
    {
        let schema = load("apply-edit.json");
        validate(&schema["$defs"][*def], &edit["documentChanges"][i], def)
            .unwrap_or_else(|e| panic!("{e}"));
    }
    rejects(
        "apply-edit.json",
        "createFile",
        json!({"kind": "rename", "uri": "file:///a.cs"}),
    );
}

#[test]
fn test_messages_conform_to_their_schemas() {
    use host::*;
    let container = TestContainer {
        id: "/s/T/T.csproj|net10.0".into(),
        name: "T (net10.0)".into(),
        project: "/s/T/T.csproj".into(),
        target_framework: "net10.0".into(),
        protocol: TestProtocol::Mtp,
        runtime: Some(TestRuntime::Dotnet),
        program: Some("/s/T/bin/Debug/net10.0/T.dll".into()),
        error: None,
    };
    conforms("test-discover.json", "container", &container);
    conforms(
        "test-discover.json",
        "params",
        &TestDiscoverParams {
            projects: Some(vec!["/s/T/T.csproj".into()]),
            configuration: Some("Debug".into()),
            run_settings: None,
            vstest_console_path: Some("/sdk/vstest.console.dll".into()),
        },
    );
    conforms(
        "test-discover.json",
        "params",
        &TestDiscoverParams::default(),
    );
    rejects("test-discover.json", "params", json!({"project": "/a"}));
    conforms(
        "test-discover.json",
        "result",
        &TestDiscoverResult {
            run_id: 1,
            generation: 2,
            containers: vec![container.clone()],
        },
    );
    rejects(
        "test-discover.json",
        "container",
        json!({"id": "a", "name": "a", "project": "a", "targetFramework": "net10.0", "protocol": "nunit"}),
    );
    conforms(
        "test-run.json",
        "params",
        &TestRunParams {
            containers: Some(vec![TestRunContainer {
                id: container.id.clone(),
                tests: Some(vec!["u1".into()]),
            }]),
            debug: Some(true),
            parallel: Some(false),
            ..Default::default()
        },
    );
    conforms(
        "test-run.json",
        "result",
        &TestRunResult {
            run_id: 3,
            generation: 2,
            containers: vec![container.id.clone()],
            debug: Some(true),
        },
    );
    conforms(
        "test-run.json",
        "testRunInProgress",
        &TestRunInProgressData {
            run_id: 3,
            container: container.id.clone(),
        },
    );
    conforms("test-cancel.json", "params", &TestCancelParams::default());
    conforms(
        "test-cancel.json",
        "result",
        &TestCancelResult {
            canceled: true,
            run_id: Some(3),
        },
    );
    conforms(
        "test-attached.json",
        "params",
        &TestAttachedParams {
            run_id: 3,
            process_id: 4242,
            attached: false,
            message: Some("no adapter".into()),
        },
    );
    conforms(
        "test-attached.json",
        "result",
        &TestAttachedResult { accepted: true },
    );
    rejects(
        "test-attached.json",
        "params",
        json!({"runId": 3, "attached": true}),
    );

    let item = TestItem {
        id: "u1".into(),
        display_name: "N.C.AddsPairs(a: 1)".into(),
        fully_qualified_name: "N.C.AddsPairs".into(),
        namespace: Some("N".into()),
        class_name: Some("C".into()),
        method: Some("AddsPairs".into()),
        source: Some("/s/T/C.cs".into()),
        line: Some(14),
        traits: vec![TestTrait {
            name: "Category".into(),
            value: "Math".into(),
        }],
    };
    let result = TestResultItem {
        id: "u1".into(),
        container: None,
        outcome: TestOutcome::Failed,
        duration_ms: Some(2.5),
        message: Some("boom".into()),
        stack_trace: Some("at N.C.M() in /s/T/C.cs:line 20".into()),
        output: Some("out\n".into()),
        display_name: None,
        fully_qualified_name: None,
    };
    let mut discovered = TestUpdate::new(1, 2, 0, TestUpdateKind::Discovered);
    discovered.container = Some(container.id.clone());
    discovered.tests = Some(vec![item.clone()]);
    conforms("test-update.json", "params", &discovered);
    let mut results = TestUpdate::new(3, 2, 1, TestUpdateKind::Results);
    results.container = Some(container.id.clone());
    results.results = Some(vec![result.clone()]);
    conforms("test-update.json", "params", &results);
    let mut launch = TestUpdate::new(3, 2, 2, TestUpdateKind::Launch);
    launch.launch = Some(TestLaunch {
        program: "/s/T/bin/Debug/net10.0/T.dll".into(),
        args: vec!["--server".into()],
        cwd: "/s/T".into(),
        env: [(
            "TESTINGPLATFORM_TELEMETRY_OPTOUT".to_owned(),
            "1".to_owned(),
        )]
        .into(),
        runtime: Some(TestRuntime::Dotnet),
    });
    conforms("test-update.json", "params", &launch);
    let mut finished = TestUpdate::new(3, 2, 3, TestUpdateKind::Finished);
    finished.state = Some(TestState::Canceled);
    finished.summary = Some(TestSummary {
        total: 2,
        passed: 1,
        failed: 0,
        skipped: 0,
        not_run: 1,
    });
    finished.elapsed_ms = Some(12.0);
    conforms("test-update.json", "params", &finished);
    let v = serde_json::to_value(&finished).unwrap();
    assert_eq!(v["summary"]["notRun"], 1);
    assert_eq!(v["kind"], "finished");
    assert_eq!(
        serde_json::to_value(TestUpdateKind::ContainerFinished).unwrap(),
        json!("containerFinished")
    );
    assert_eq!(
        serde_json::to_value(TestOutcome::NotRun).unwrap(),
        json!("notRun")
    );
    rejects(
        "test-update.json",
        "params",
        json!({"runId": 1, "generation": 0, "seq": 0, "kind": "progress"}),
    );

    conforms("test-status.json", "params", &());
    conforms("test-status.json", "result", &TestStatusResult::default());
    let status = TestStatusResult {
        running: vec![TestStatusRunning {
            run_id: 3,
            kind: TestJobKind::Run,
            generation: 2,
            debug: None,
            containers: vec![container.clone()],
            elapsed_ms: 40.0,
            next_seq: 2,
            tests: vec![TestStatusTest {
                container: container.id.clone(),
                test: item,
            }],
            results: vec![TestResultItem {
                container: Some(container.id.clone()),
                ..result
            }],
        }],
        last: Some(TestStatusLast {
            run_id: 1,
            kind: TestJobKind::Discover,
            generation: 2,
            state: TestState::Completed,
            summary: TestSummary::default(),
            elapsed_ms: 300.0,
        }),
    };
    conforms("test-status.json", "result", &status);
    let back: TestStatusResult =
        serde_json::from_value(serde_json::to_value(&status).unwrap()).unwrap();
    assert_eq!(back, status);
    rejects("test-status.json", "result", json!({}));
}

#[test]
fn nuget_messages_conform_to_their_schemas() {
    use host::*;
    let vuln = NuGetVulnerability {
        severity: "high".into(),
        advisory_url: "https://github.com/advisories/GHSA-test".into(),
    };
    let deprecation = NuGetDeprecation {
        reasons: vec!["legacy".into()],
        message: Some("Use 1.1.0".into()),
        alternate_package: Some(NuGetAlternatePackage {
            id: "Other".into(),
            range: Some("[1.0.0, )".into()),
        }),
    };
    let rows = vec![
        NuGetSourceResult {
            name: "corpus".into(),
            url: "/c/feed".into(),
            count: 2,
            elapsed_ms: Some(4.5),
            cached: true,
            error: None,
        },
        NuGetSourceResult {
            name: "down".into(),
            url: "http://127.0.0.1:9/v3/index.json".into(),
            count: 0,
            elapsed_ms: Some(1.0),
            cached: false,
            error: Some("Unable to load the service index".into()),
        },
    ];
    conforms(
        "nuget-search.json",
        "params",
        &NuGetSearchParams {
            generation: 2,
            operation: Some(4),
            query: Some("Greeter".into()),
            source: Some("corpus".into()),
            prerelease: Some(true),
            skip: Some(0),
            take: Some(50),
            interactive: Some(true),
        },
    );
    rejects("nuget-search.json", "params", json!({"query": "x"}));
    rejects(
        "nuget-search.json",
        "params",
        json!({"generation": 1, "take": 0}),
    );
    let package = NuGetSearchPackage {
        id: "Eludite.Corpus.Greeter".into(),
        version: "1.1.0".into(),
        source: "corpus".into(),
        versions: Some(vec!["1.1.0".into(), "1.0.0".into()]),
        title: Some("Greeter".into()),
        description: Some("Greets".into()),
        authors: Some("The Eludite Authors".into()),
        icon_url: Some("https://example.invalid/icon.png".into()),
        license_url: None,
        license_expression: Some("MIT".into()),
        project_url: None,
        downloads: Some(42),
        vulnerabilities: Some(vec![vuln.clone()]),
        deprecation: Some(deprecation.clone()),
    };
    conforms(
        "nuget-search.json",
        "result",
        &NuGetSearchResult {
            generation: 2,
            results: vec![package],
            sources: rows.clone(),
            elapsed_ms: 6.0,
        },
    );
    conforms(
        "nuget-installed.json",
        "params",
        &NuGetInstalledParams {
            generation: 2,
            operation: None,
            projects: Some(vec!["/c/App/App.csproj".into()]),
            include_transitive: Some(true),
            metadata: Some(true),
            interactive: None,
        },
    );
    let installed = NuGetInstalledPackage {
        id: "Eludite.Corpus.Greeter".into(),
        target_frameworks: vec!["net10.0".into()],
        transitive: false,
        requested: Some("1.0.0".into()),
        version: Some("1.0.0".into()),
        source: Some("/c/feed".into()),
        auto_referenced: false,
        dependencies: Some(vec![NuGetPackageDependency {
            id: "Eludite.Corpus.Logging".into(),
            range: Some("[1.0.0, )".into()),
        }]),
        vulnerabilities: Some(vec![vuln.clone()]),
        deprecation: Some(deprecation.clone()),
    };
    conforms(
        "nuget-installed.json",
        "result",
        &NuGetInstalledResult {
            generation: 2,
            projects: vec![
                NuGetInstalledProject {
                    path: "/c/App/App.csproj".into(),
                    name: "App".into(),
                    format: NuGetProjectFormat::PackageReference,
                    restored: true,
                    central_package_management: true,
                    target_frameworks: vec!["net10.0".into()],
                    packages: vec![installed.clone()],
                    assets_file: Some("/c/App/obj/project.assets.json".into()),
                    props_file: Some("/c/Directory.Packages.props".into()),
                    lock_file: Some("/c/App/packages.lock.json".into()),
                    error: None,
                    note: None,
                },
                NuGetInstalledProject {
                    path: "/c/Old/Old.csproj".into(),
                    name: "Old".into(),
                    format: NuGetProjectFormat::PackagesConfig,
                    restored: false,
                    central_package_management: false,
                    target_frameworks: vec![],
                    packages: vec![],
                    assets_file: None,
                    props_file: None,
                    lock_file: None,
                    error: None,
                    note: Some("packages.config is listed read-only".into()),
                },
            ],
            sources: Some(rows.clone()),
            elapsed_ms: 12.0,
        },
    );
    conforms(
        "nuget-updates.json",
        "params",
        &NuGetUpdatesParams {
            generation: 2,
            prerelease: Some(false),
            ..Default::default()
        },
    );
    conforms(
        "nuget-updates.json",
        "result",
        &NuGetUpdatesResult {
            generation: 2,
            updates: vec![NuGetUpdateRow {
                project: "/c/App/App.csproj".into(),
                id: "Eludite.Corpus.Greeter".into(),
                installed: "1.0.0".into(),
                latest: "1.1.0".into(),
                source: "corpus".into(),
                requested: Some("1.0.0".into()),
                versions: Some(vec!["1.1.0".into(), "1.0.0".into()]),
                vulnerabilities: Some(vec![vuln.clone()]),
                deprecation: None,
            }],
            sources: rows.clone(),
            elapsed_ms: 3.0,
        },
    );
    conforms(
        "nuget-change.json",
        "params",
        &NuGetChangeParams {
            generation: 2,
            operation: Some(9),
            action: NuGetAction::Install,
            packages: vec![NuGetPackageArg {
                id: "Eludite.Corpus.Logging".into(),
                version: Some("1.0.0".into()),
            }],
            projects: Some(vec!["/c/App/App.csproj".into()]),
            prerelease: None,
            source: None,
            include_transitive: Some(false),
            restore: Some(true),
            lock_files: Some(NuGetLockFiles::Respect),
            interactive: Some(true),
        },
    );
    rejects(
        "nuget-change.json",
        "params",
        json!({"generation": 1, "action": "reinstall", "packages": [{"id": "X"}]}),
    );
    let outcome = NuGetRestoreOutcome {
        result: NuGetRestoreState::Failed,
        exit_code: Some(1),
        elapsed_ms: 900.0,
        command_line: "dotnet restore /c/Corpus.slnx".into(),
        locked_mode: false,
        lock_files: vec!["/c/Lib/packages.lock.json".into()],
        diagnostics: vec![NuGetDiagnostic {
            severity: BuildDiagnosticSeverity::Error,
            code: "NU1102".into(),
            message: "Unable to find package with version (>= 7.0.0)".into(),
            file: Some("/c/Lib/Lib.csproj".into()),
            line: None,
            column: None,
            project: Some("/c/Corpus.slnx".into()),
        }],
    };
    conforms(
        "nuget-change.json",
        "result",
        &NuGetChangeResult {
            generation: 3,
            action: NuGetAction::Install,
            packages: vec![NuGetPackageArg {
                id: "Eludite.Corpus.Logging".into(),
                version: Some("1.0.0".into()),
            }],
            projects: vec!["/c/App/App.csproj".into()],
            edited: vec![NuGetEditedFile {
                path: "/c/App/App.csproj".into(),
                kind: NuGetEditedKind::Project,
                changes: vec!["PackageReference Eludite.Corpus.Logging 1.0.0 added".into()],
            }],
            restore: Some(outcome.clone()),
            elapsed_ms: 2500.0,
            message: None,
        },
    );
    conforms(
        "nuget-sources.json",
        "params",
        &NuGetSourcesParams {
            generation: 2,
            operation: None,
            action: Some(NuGetSourcesAction::Add),
            name: Some("mine".into()),
            url: Some("https://example.invalid/v3/index.json".into()),
        },
    );
    conforms(
        "nuget-sources.json",
        "result",
        &NuGetSourcesResult {
            sources: vec![NuGetSourceInfo {
                name: "corpus".into(),
                url: "/c/feed".into(),
                enabled: true,
                local: true,
                scope: NuGetSourceScope::Solution,
                config_file: Some("/c/NuGet.config".into()),
            }],
            config_files: vec!["/c/NuGet.config".into()],
            user_config: "/home/u/.nuget/NuGet/NuGet.Config".into(),
            changed: false,
        },
    );
    conforms(
        "nuget-restore.json",
        "params",
        &NuGetRestoreParams {
            generation: 2,
            force: Some(true),
            lock_files: Some(NuGetLockFiles::Ignore),
            ..Default::default()
        },
    );
    let restore = NuGetRestoreResult {
        generation: 2,
        outcome,
    };
    conforms("nuget-restore.json", "result", &restore);
    let back: NuGetRestoreResult =
        serde_json::from_value(serde_json::to_value(&restore).unwrap()).unwrap();
    assert_eq!(back, restore);
    for update in [
        NuGetUpdate {
            operation: 9,
            generation: 2,
            seq: 0,
            kind: NuGetUpdateKind::Output,
            text: Some("Installing NuGet package Eludite.Corpus.Logging 1.0.0 in App.\n".into()),
            message: None,
            packages: None,
        },
        NuGetUpdate {
            operation: 9,
            generation: 2,
            seq: 1,
            kind: NuGetUpdateKind::Progress,
            text: None,
            message: Some("Restoring Corpus.slnx".into()),
            packages: None,
        },
        NuGetUpdate {
            operation: 9,
            generation: 2,
            seq: 2,
            kind: NuGetUpdateKind::Metadata,
            text: None,
            message: None,
            packages: Some(vec![NuGetPackageMetadata {
                id: "Eludite.Corpus.Greeter".into(),
                version: "1.0.0".into(),
                vulnerabilities: Some(vec![vuln.clone()]),
                deprecation: Some(deprecation.clone()),
            }]),
        },
    ] {
        conforms("nuget-update.json", "params", &update);
    }
    conforms(
        "nuget-credentials.json",
        "params",
        &NuGetCredentialsParams {
            operation: Some(9),
            source: "private".into(),
            url: "https://feed.example/v3/index.json".into(),
            host: "feed.example".into(),
            proxy: false,
            is_retry: false,
            message: None,
        },
    );
    conforms(
        "nuget-credentials.json",
        "result",
        &Some(NuGetCredentialsAnswer {
            username: Some("alice".into()),
            password: Some("secret".into()),
            remember: Some(true),
            canceled: None,
        }),
    );
    conforms(
        "nuget-credentials.json",
        "result",
        &None::<NuGetCredentialsAnswer>,
    );
    conforms(
        "nuget-icon.json",
        "params",
        &NuGetIconParams {
            url: "https://example.invalid/icon.png".into(),
        },
    );
    for path in [
        Some("/home/u/.cache/eludite/nuget-icons/ab.png".to_owned()),
        None,
    ] {
        conforms("nuget-icon.json", "result", &NuGetIconResult { path });
    }
    // The tree's Dependencies member.
    let schema = load("solution-tree.json");
    let project = TreeProject {
        name: "App".into(),
        path: "/c/App/App.csproj".into(),
        kind: TreeProjectKind::Sdk,
        web: false,
        target_frameworks: vec!["net10.0".into()],
        files: vec![],
        error: None,
        dependencies: Some(TreeDependencies {
            restored: true,
            packages: vec![TreePackage {
                id: "Eludite.Corpus.Greeter".into(),
                requested: Some("1.0.0".into()),
                version: Some("1.0.0".into()),
                auto_referenced: false,
                transitive: Some(vec![TreeTransitive {
                    id: "Eludite.Corpus.Logging".into(),
                    version: Some("[1.0.0, )".into()),
                }]),
                vulnerabilities: Some(vec![vuln]),
                deprecated: true,
            }],
            projects: vec![TreeProjectReference {
                name: "Shared".into(),
                path: "/c/Shared/Shared.csproj".into(),
            }],
            frameworks: vec![TreeFramework {
                name: "Microsoft.NETCore.App".into(),
                target_framework: Some("net10.0".into()),
            }],
        }),
    };
    validate(
        &schema["$defs"]["project"],
        &serde_json::to_value(&project).unwrap(),
        "solution-tree.json",
    )
    .unwrap();
    let data = NuGetFailedData {
        reason: "credentialsRequired".into(),
        source: Some("private".into()),
        host: Some("feed.example".into()),
        package: None,
    };
    validate(
        &load("errors.json")["$defs"]["nugetFailed"],
        &serde_json::to_value(&data).unwrap(),
        "errors.json",
    )
    .unwrap();
}

/// Brief 0049: the project properties, launch profiles and solution configuration messages.
#[test]
fn project_property_messages_conform_to_their_schemas() {
    use host::*;
    conforms(
        "project-properties.json",
        "params",
        &ProjectPropertiesParams {
            project: "/s/App/App.csproj".into(),
            configuration: Some("Release".into()),
            platform: Some("AnyCPU".into()),
            framework: None,
        },
    );
    rejects("project-properties.json", "params", json!({}));
    let property = ProjectProperty {
        name: "DefineConstants".into(),
        page: "build".into(),
        section: Some("General".into()),
        label: "Conditional compilation symbols".into(),
        description: Some("Symbols.".into()),
        kind: PropertyType::List,
        values: None,
        true_value: None,
        false_value: None,
        per_configuration: true,
        target: None,
        value: "TRACE;RELEASE_ONLY".into(),
        raw: Some("$(DefineConstants);RELEASE_ONLY".into()),
        source: PropertySource::Conditioned,
        defined_in: Some(DefinedIn {
            file: "/s/App/App.csproj".into(),
            line: 13,
            condition: Some("'$(Configuration)|$(Platform)'=='Release|AnyCPU'".into()),
        }),
        conditioned: true,
        inherited: false,
        inherited_from: None,
        conditions: Some(vec![
            "'$(Configuration)|$(Platform)'=='Release|AnyCPU'".into(),
        ]),
        read_only: false,
        read_only_reason: None,
    };
    conforms("project-properties.json", "property", &property);
    let nullable = ProjectProperty {
        name: "ImplicitUsings".into(),
        kind: PropertyType::Bool,
        true_value: Some("enable".into()),
        false_value: Some("disable".into()),
        values: Some(vec![CatalogValue {
            value: "enable".into(),
            label: "Enable".into(),
        }]),
        source: PropertySource::Inherited,
        inherited: true,
        inherited_from: Some("/s/Directory.Build.props".into()),
        read_only: true,
        read_only_reason: Some("Legacy".into()),
        ..property.clone()
    };
    conforms("project-properties.json", "property", &nullable);
    let wire = serde_json::to_value(&nullable).unwrap();
    assert_eq!(wire["type"], "bool");
    assert_eq!(wire["source"], "inherited");
    assert_eq!(wire["readOnly"], true);
    assert!(
        serde_json::to_value(&property)
            .unwrap()
            .get("readOnly")
            .is_none()
    );
    let page = PropertyPage {
        id: "resources".into(),
        title: "Resources".into(),
        state: "notYet".into(),
        note: Some("Not yet.".into()),
    };
    conforms("project-properties.json", "page", &page);
    let result = ProjectPropertiesResult {
        generation: 3,
        project: "/s/App/App.csproj".into(),
        kind: "sdk".into(),
        configuration: "Release".into(),
        platform: "AnyCPU".into(),
        framework: None,
        configurations: vec!["Debug".into(), "Release".into()],
        platforms: vec!["AnyCPU".into()],
        frameworks: vec!["net10.0".into()],
        pages: vec![page],
        properties: vec![property, nullable],
    };
    conforms("project-properties.json", "result", &result);
    let back: ProjectPropertiesResult =
        serde_json::from_value(serde_json::to_value(&result).unwrap()).unwrap();
    assert_eq!(back, result);

    let edit = PropertyEdit {
        name: "DefineConstants".into(),
        value: None,
        configuration: Some("Debug".into()),
        platform: Some("AnyCPU".into()),
        framework: None,
        all_configurations: false,
        override_inherited: true,
    };
    let edit_wire = serde_json::to_value(&edit).unwrap();
    assert_eq!(
        edit_wire["value"],
        Value::Null,
        "null removes: sent, not omitted"
    );
    assert_eq!(edit_wire["override"], true);
    conforms("project-set-property.json", "edit", &edit);
    conforms(
        "project-set-property.json",
        "params",
        &ProjectSetPropertyParams {
            project: "/s/App/App.csproj".into(),
            generation: 3,
            edits: vec![edit],
        },
    );
    rejects(
        "project-set-property.json",
        "params",
        json!({"project": "/s/App/App.csproj", "edits": []}),
    );
    let set = ProjectSetPropertyResult {
        generation: 4,
        project: "/s/App/App.csproj".into(),
        written: true,
        results: vec![PropertyEditResult {
            name: "LangVersion".into(),
            status: EditStatus::Inherited,
            condition: None,
            line: None,
            inherited_from: Some("/s/Directory.Build.props".into()),
            removed_conditions: None,
        }],
    };
    conforms("project-set-property.json", "result", &set);
    conforms("project-set-property.json", "editResult", &set.results[0]);

    let profile = LaunchProfile {
        name: "https".into(),
        command_name: "Project".into(),
        environment_variables: vec![EnvironmentVariable {
            name: "ASPNETCORE_ENVIRONMENT".into(),
            value: "Development".into(),
        }],
        launch_browser: Some(true),
        application_url: Some("https://localhost:7080".into()),
        unknown: Some(vec!["x-note".into()]),
        ..LaunchProfile::default()
    };
    conforms("launch-profiles.json", "profile", &profile);
    let profiles = LaunchProfilesResult {
        generation: 3,
        project: "/s/App/App.csproj".into(),
        file: "/s/App/Properties/launchSettings.json".into(),
        exists: true,
        profiles: vec![profile],
    };
    conforms("launch-profiles.json", "result", &profiles);
    conforms("launch-profile-set.json", "result", &profiles);
    conforms(
        "launch-profiles.json",
        "params",
        &LaunchProfilesParams {
            project: "/s/App/App.csproj".into(),
        },
    );
    let set_profile = SetLaunchProfileParams {
        project: "/s/App/App.csproj".into(),
        generation: 3,
        action: LaunchProfileAction::Set,
        profile: "https".into(),
        new_name: None,
        values: Some(json!({"commandLineArgs": "--x", "launchUrl": null,
                            "environmentVariables": [{"name": "A", "value": "1"}]})),
    };
    conforms("launch-profile-set.json", "params", &set_profile);
    conforms(
        "launch-profile-set.json",
        "values",
        set_profile.values.as_ref().unwrap(),
    );
    rejects(
        "launch-profile-set.json",
        "values",
        json!({"commandLineArgs": "x", "other": 1}),
    );

    let mapping = ConfigurationMapping {
        solution_configuration: "Release".into(),
        solution_platform: "Any CPU".into(),
        configuration: "Release".into(),
        platform: "Any CPU".into(),
        build: false,
        deploy: false,
    };
    conforms("solution-configurations.json", "mapping", &mapping);
    let configurations = SolutionConfigurationsResult {
        generation: 3,
        path: Some("/s/S.sln".into()),
        format: Some(SolutionFormat::Sln),
        configurations: vec!["Debug".into(), "Release".into()],
        platforms: vec!["Any CPU".into(), "x64".into()],
        active: Selection {
            configuration: "Debug".into(),
            platform: "Any CPU".into(),
        },
        projects: vec![SolutionProjectConfigurations {
            name: "Lib".into(),
            path: "/s/Lib/Lib.csproj".into(),
            configurations: vec!["Debug".into(), "Release".into()],
            platforms: vec!["Any CPU".into()],
            mappings: vec![mapping],
        }],
    };
    conforms("solution-configurations.json", "result", &configurations);
    conforms(
        "solution-configurations.json",
        "project",
        &configurations.projects[0],
    );
    conforms("solution-configurations.json", "params", &());
    let edit = MappingEdit {
        project: "/s/Lib/Lib.csproj".into(),
        solution_configuration: "Release".into(),
        solution_platform: "Any CPU".into(),
        configuration: None,
        platform: None,
        build: Some(true),
    };
    conforms("solution-set-configuration.json", "mappingEdit", &edit);
    conforms(
        "solution-set-configuration.json",
        "params",
        &SolutionSetConfigurationParams {
            generation: 3,
            select: Some(configurations.active.clone()),
            mappings: Some(vec![edit]),
        },
    );
    rejects("solution-set-configuration.json", "params", json!({}));
    conforms(
        "solution-set-configuration.json",
        "result",
        &SolutionSetConfigurationResult {
            generation: 4,
            path: Some("/s/S.sln".into()),
            written: true,
            active: configurations.active,
        },
    );
}
