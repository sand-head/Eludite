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
    assert_eq!(methods::HOST_TO_SHELL_REQUESTS, [lsp::ApplyEdit::METHOD]);
    assert!(methods::HOST_TO_SHELL.contains(&host::SolutionStatusNotification::METHOD));
    assert!(methods::HOST_TO_SHELL.contains(&host::LanguageServerStatusNotification::METHOD));
    for m in [
        host::BuildOutputNotification::METHOD,
        host::BuildProgressNotification::METHOD,
        host::BuildFinishedNotification::METHOD,
    ] {
        assert!(methods::HOST_TO_SHELL.contains(&m), "{m}");
    }
}

#[test]
fn build_messages_conform_to_their_schemas() {
    use host::*;
    conforms(
        "build-start.json",
        "params",
        &BuildStartParams {
            target: BuildTarget::Build,
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
