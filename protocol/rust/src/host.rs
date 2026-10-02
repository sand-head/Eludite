//! Eludite-specific messages between the shell and `eludite-host`.
//!
//! This is the Rust mirror of `protocol/schemas/host-rpc.md` and the JSON schemas in `protocol/schemas/host/`;
//! field names are camelCase on the wire. Forwarded LSP messages are in [`crate::lsp`].

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::typed::{NotificationType, RequestType};

/// A solution generation (host-rpc.md, "Solution generation").
pub type Generation = u64;

/// Params member every forwarded LSP request carries.
pub const GENERATION_FIELD: &str = "eluditeGeneration";

/// Method name constants.
pub mod methods {
    pub const HOST_INITIALIZE: &str = "eludite/host/initialize";
    pub const PING: &str = "eludite/ping";
    pub const HOST_INFO: &str = "eludite/host/info";
    pub const HOST_SHUTDOWN: &str = "eludite/host/shutdown";
    /// Notification, no response.
    pub const HOST_EXIT: &str = "eludite/host/exit";
    pub const SOLUTION_OPEN: &str = "eludite/solution/open";
    pub const SOLUTION_CLOSE: &str = "eludite/solution/close";
    /// Host-to-shell notification.
    pub const SOLUTION_STATUS: &str = "eludite/solution/status";
    /// Host-to-shell notification.
    pub const LANGUAGE_SERVER_STATUS: &str = "eludite/languageServer/status";
    /// Host-to-shell notification (LSP shape plus `eluditeGeneration`).
    pub const PUBLISH_DIAGNOSTICS: &str = "textDocument/publishDiagnostics";
    pub const CANCEL_REQUEST: &str = "$/cancelRequest";

    /// Eludite requests and notifications the host accepts.
    pub const ELUDITE_ACCEPTED: &[&str] = &[
        HOST_INITIALIZE,
        PING,
        HOST_INFO,
        HOST_SHUTDOWN,
        HOST_EXIT,
        SOLUTION_OPEN,
        SOLUTION_CLOSE,
    ];

    /// Forwarded LSP requests typed in [`crate::lsp`].
    pub const FORWARDED_TYPED_REQUESTS: &[&str] = &[
        "textDocument/completion",
        "completionItem/resolve",
        "textDocument/hover",
        "textDocument/definition",
        "textDocument/references",
        "textDocument/documentSymbol",
        "workspace/symbol",
        "textDocument/diagnostic",
    ];

    /// Forwarded LSP notifications typed in [`crate::lsp`] (`$/cancelRequest` is handled by the host itself).
    pub const FORWARDED_TYPED_NOTIFICATIONS: &[&str] = &[
        "textDocument/didOpen",
        "textDocument/didChange",
        "textDocument/didClose",
        CANCEL_REQUEST,
    ];

    /// Forwarded LSP requests passed through as raw JSON ("forwarded, untyped").
    pub const FORWARDED_UNTYPED_REQUESTS: &[&str] = &[
        "textDocument/signatureHelp",
        "textDocument/typeDefinition",
        "textDocument/implementation",
        "textDocument/documentHighlight",
        "textDocument/semanticTokens/full",
        "textDocument/semanticTokens/range",
        "textDocument/codeAction",
        "textDocument/formatting",
        "textDocument/rename",
    ];

    /// Forwarded LSP notifications passed through as raw JSON.
    pub const FORWARDED_UNTYPED_NOTIFICATIONS: &[&str] =
        &["textDocument/didSave", "workspace/didChangeWatchedFiles"];

    /// Notifications the host sends to the shell.
    pub const HOST_TO_SHELL: &[&str] = &[
        SOLUTION_STATUS,
        LANGUAGE_SERVER_STATUS,
        PUBLISH_DIAGNOSTICS,
        "window/showMessage",
        "$/progress",
    ];
}

/// Error codes the host returns (host-rpc.md, "Error codes").
pub mod error_codes {
    /// `eludite/solution/*` or a forwarded request before `eludite/host/initialize`.
    pub const SERVER_NOT_INITIALIZED: i64 = -32002;
    /// The request was canceled with `$/cancelRequest`.
    pub const REQUEST_CANCELLED: i64 = -32800;
    /// Stale `eluditeGeneration`, or the generation changed while the request was in flight.
    pub const CONTENT_MODIFIED: i64 = -32801;
    /// The language server is unavailable.
    pub const REQUEST_FAILED: i64 = -32803;
}

/// The `hostName` value every conforming host reports.
pub const HOST_NAME: &str = "eludite-host";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeParams {
    pub client_name: String,
    pub client_version: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostCapabilities {
    /// True when a language server is configured.
    pub language_server: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct InitializeResult {
    /// Always `"eludite-host"`.
    pub host_name: String,
    pub host_version: String,
    pub capabilities: HostCapabilities,
}

/// `eludite/ping` takes no params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResult {
    /// Always `true`.
    pub pong: bool,
    /// ISO-8601 timestamp produced by the host.
    pub timestamp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DotnetSdk {
    pub version: String,
    pub path: String,
}

/// `eludite/host/info` takes no params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostInfoResult {
    pub dotnet_sdks: Vec<DotnetSdk>,
    pub runtime: String,
    pub os: String,
}

/// `eludite/host/shutdown` takes no params and returns `null`.
pub type ShutdownResult = ();

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionOpenParams {
    /// A `.sln`, `.slnx` or project file.
    pub path: String,
}

/// Result of `eludite/solution/open` and `eludite/solution/close`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GenerationResult {
    pub generation: Generation,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SolutionState {
    Loading,
    Loaded,
    Failed,
    Closed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LoadPhase {
    LegacyEvaluation,
    ProjectLoad,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProjectCounts {
    pub projects: u32,
    pub legacy_projects: u32,
    pub legacy_evaluation_failures: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum MsBuildKind {
    Mono,
    BuildTools,
    Sdk,
}

/// The MSBuild used for legacy (non-SDK) projects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MsBuildInfo {
    pub kind: MsBuildKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CorrectionKind {
    DesignerPartials,
    CaseFixups,
    ComReferencesRemoved,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Correction {
    pub kind: CorrectionKind,
    pub project: String,
    pub count: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostDiagnosticSeverity {
    Error,
    Warning,
    Info,
}

/// A project-load diagnostic (codes `ELUDITE0001`.. in host-rpc.md).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HostDiagnostic {
    pub severity: HostDiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class: Option<String>,
}

/// `eludite/solution/status` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionStatus {
    pub generation: Generation,
    pub path: String,
    pub state: SolutionState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<LoadPhase>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub counts: Option<ProjectCounts>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub msbuild: Option<MsBuildInfo>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub corrections: Vec<Correction>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<HostDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum LanguageServerState {
    Starting,
    Running,
    Restarting,
    Unavailable,
    Exited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerInfo {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `eludite/languageServer/status` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageServerStatus {
    pub state: LanguageServerState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub server_info: Option<ServerInfo>,
    /// LSP `ServerCapabilities`, untyped.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub capabilities: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `data` of a -32801 ContentModified error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ContentModifiedData {
    pub requested_generation: Generation,
    pub current_generation: Generation,
}

/// `data` of a -32803 RequestFailed error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RequestFailedData {
    /// `"languageServerUnavailable"`.
    pub reason: String,
}

/// LSP params plus the generation they were issued or computed under.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WithGeneration<T> {
    #[serde(flatten)]
    pub params: T,
    #[serde(rename = "eluditeGeneration")]
    pub generation: Generation,
}

macro_rules! request {
    ($(#[$m:meta])* $name:ident, $method:expr, $params:ty, $result:ty) => {
        $(#[$m])*
        #[derive(Debug)]
        pub enum $name {}
        impl RequestType for $name {
            const METHOD: &'static str = $method;
            const GENERATIONAL: bool = false;
            type Params = $params;
            type Result = $result;
        }
    };
}

request!(
    /// `eludite/host/initialize`.
    HostInitialize,
    methods::HOST_INITIALIZE,
    InitializeParams,
    InitializeResult
);
request!(
    /// `eludite/ping`.
    Ping,
    methods::PING,
    (),
    PingResult
);
request!(
    /// `eludite/host/info`.
    HostInfo,
    methods::HOST_INFO,
    (),
    HostInfoResult
);
request!(
    /// `eludite/host/shutdown`.
    HostShutdown,
    methods::HOST_SHUTDOWN,
    (),
    ShutdownResult
);
request!(
    /// `eludite/solution/open`.
    SolutionOpen,
    methods::SOLUTION_OPEN,
    SolutionOpenParams,
    GenerationResult
);
request!(
    /// `eludite/solution/close`.
    SolutionClose,
    methods::SOLUTION_CLOSE,
    (),
    GenerationResult
);

/// `eludite/host/exit`.
#[derive(Debug)]
pub enum HostExit {}
impl NotificationType for HostExit {
    const METHOD: &'static str = methods::HOST_EXIT;
    type Params = ();
}

/// `eludite/solution/status` (host to shell).
#[derive(Debug)]
pub enum SolutionStatusNotification {}
impl NotificationType for SolutionStatusNotification {
    const METHOD: &'static str = methods::SOLUTION_STATUS;
    type Params = SolutionStatus;
}

/// `eludite/languageServer/status` (host to shell).
#[derive(Debug)]
pub enum LanguageServerStatusNotification {}
impl NotificationType for LanguageServerStatusNotification {
    const METHOD: &'static str = methods::LANGUAGE_SERVER_STATUS;
    type Params = LanguageServerStatus;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_util::round_trip;
    use serde_json::json;

    #[test]
    fn initialize() {
        round_trip(
            &InitializeParams {
                client_name: "eludite".into(),
                client_version: "0.1.0".into(),
            },
            json!({"clientName": "eludite", "clientVersion": "0.1.0"}),
        );
        round_trip(
            &InitializeResult {
                host_name: HOST_NAME.into(),
                host_version: "0.1.0".into(),
                capabilities: HostCapabilities {
                    language_server: true,
                },
            },
            json!({"hostName": "eludite-host", "hostVersion": "0.1.0", "capabilities": {"languageServer": true}}),
        );
    }

    #[test]
    fn ping() {
        round_trip(
            &PingResult {
                pong: true,
                timestamp: "2026-10-01T12:00:00Z".into(),
            },
            json!({"pong": true, "timestamp": "2026-10-01T12:00:00Z"}),
        );
    }

    #[test]
    fn host_info() {
        round_trip(
            &HostInfoResult {
                dotnet_sdks: vec![DotnetSdk {
                    version: "9.0.100".into(),
                    path: "/usr/share/dotnet/sdk".into(),
                }],
                runtime: ".NET 9.0.0".into(),
                os: "Linux".into(),
            },
            json!({
                "dotnetSdks": [{"version": "9.0.100", "path": "/usr/share/dotnet/sdk"}],
                "runtime": ".NET 9.0.0",
                "os": "Linux"
            }),
        );
    }

    #[test]
    fn shutdown_is_null() {
        round_trip(&(), Value::Null);
    }

    #[test]
    fn solution_open_and_close() {
        round_trip(
            &SolutionOpenParams {
                path: "/src/App.slnx".into(),
            },
            json!({"path": "/src/App.slnx"}),
        );
        round_trip(
            &GenerationResult { generation: 3 },
            json!({"generation": 3}),
        );
    }

    #[test]
    fn solution_status_loading_and_loaded() {
        round_trip(
            &SolutionStatus {
                generation: 1,
                path: "/src/App.sln".into(),
                state: SolutionState::Loading,
                phase: Some(LoadPhase::LegacyEvaluation),
                counts: None,
                msbuild: None,
                corrections: vec![],
                diagnostics: vec![],
                elapsed_ms: Some(12.5),
            },
            json!({"generation": 1, "path": "/src/App.sln", "state": "loading", "phase": "legacyEvaluation", "elapsedMs": 12.5}),
        );
        round_trip(
            &SolutionStatus {
                generation: 1,
                path: "/src/App.sln".into(),
                state: SolutionState::Loaded,
                phase: None,
                counts: Some(ProjectCounts {
                    projects: 15,
                    legacy_projects: 15,
                    legacy_evaluation_failures: 1,
                }),
                msbuild: Some(MsBuildInfo {
                    kind: MsBuildKind::Mono,
                    path: Some("/usr/lib/mono/msbuild/Current/bin/MSBuild.dll".into()),
                    source: Some("PATH".into()),
                }),
                corrections: vec![Correction {
                    kind: CorrectionKind::CaseFixups,
                    project: "/src/Core.csproj".into(),
                    count: 9,
                }],
                diagnostics: vec![HostDiagnostic {
                    severity: HostDiagnosticSeverity::Warning,
                    code: "ELUDITE0003".into(),
                    message: "did not evaluate".into(),
                    project: Some("/src/Web.UI.csproj".into()),
                    class: Some("webTargets".into()),
                }],
                elapsed_ms: Some(7000.0),
            },
            json!({
                "generation": 1, "path": "/src/App.sln", "state": "loaded",
                "counts": {"projects": 15, "legacyProjects": 15, "legacyEvaluationFailures": 1},
                "msbuild": {"kind": "mono", "path": "/usr/lib/mono/msbuild/Current/bin/MSBuild.dll", "source": "PATH"},
                "corrections": [{"kind": "caseFixups", "project": "/src/Core.csproj", "count": 9}],
                "diagnostics": [{"severity": "warning", "code": "ELUDITE0003", "message": "did not evaluate",
                                 "project": "/src/Web.UI.csproj", "class": "webTargets"}],
                "elapsedMs": 7000.0
            }),
        );
    }

    #[test]
    fn solution_status_failed_and_closed() {
        round_trip(
            &SolutionStatus {
                generation: 2,
                path: "/src/App.sln".into(),
                state: SolutionState::Failed,
                phase: None,
                counts: None,
                msbuild: Some(MsBuildInfo {
                    kind: MsBuildKind::Sdk,
                    path: None,
                    source: None,
                }),
                corrections: vec![],
                diagnostics: vec![HostDiagnostic {
                    severity: HostDiagnosticSeverity::Error,
                    code: "ELUDITE0001".into(),
                    message: "no language server".into(),
                    project: None,
                    class: None,
                }],
                elapsed_ms: None,
            },
            json!({"generation": 2, "path": "/src/App.sln", "state": "failed", "msbuild": {"kind": "sdk"},
                   "diagnostics": [{"severity": "error", "code": "ELUDITE0001", "message": "no language server"}]}),
        );
        for (state, s) in [
            (SolutionState::Closed, "closed"),
            (SolutionState::Loaded, "loaded"),
        ] {
            let v = serde_json::to_value(state).unwrap();
            assert_eq!(v, json!(s));
        }
        let kinds = [
            (MsBuildKind::BuildTools, "buildTools"),
            (MsBuildKind::Mono, "mono"),
        ];
        for (k, s) in kinds {
            assert_eq!(serde_json::to_value(k).unwrap(), json!(s));
        }
        assert_eq!(
            serde_json::to_value(CorrectionKind::ComReferencesRemoved).unwrap(),
            json!("comReferencesRemoved")
        );
        assert_eq!(
            serde_json::to_value(LoadPhase::ProjectLoad).unwrap(),
            json!("projectLoad")
        );
    }

    #[test]
    fn language_server_status() {
        round_trip(
            &LanguageServerStatus {
                state: LanguageServerState::Running,
                server_info: Some(ServerInfo {
                    name: "Microsoft.CodeAnalysis.LanguageServer".into(),
                    version: Some("5.3.0".into()),
                }),
                capabilities: Some(json!({"completionProvider": {"resolveProvider": true}})),
                message: None,
            },
            json!({"state": "running",
                   "serverInfo": {"name": "Microsoft.CodeAnalysis.LanguageServer", "version": "5.3.0"},
                   "capabilities": {"completionProvider": {"resolveProvider": true}}}),
        );
        round_trip(
            &LanguageServerStatus {
                state: LanguageServerState::Exited,
                server_info: None,
                capabilities: None,
                message: Some("exit code 134".into()),
            },
            json!({"state": "exited", "message": "exit code 134"}),
        );
        for (s, w) in [
            (LanguageServerState::Starting, "starting"),
            (LanguageServerState::Restarting, "restarting"),
            (LanguageServerState::Unavailable, "unavailable"),
        ] {
            assert_eq!(serde_json::to_value(s).unwrap(), json!(w));
        }
    }

    #[test]
    fn error_data() {
        round_trip(
            &ContentModifiedData {
                requested_generation: 1,
                current_generation: 2,
            },
            json!({"requestedGeneration": 1, "currentGeneration": 2}),
        );
        round_trip(
            &RequestFailedData {
                reason: "languageServerUnavailable".into(),
            },
            json!({"reason": "languageServerUnavailable"}),
        );
    }

    #[test]
    fn with_generation_flattens() {
        round_trip(
            &WithGeneration {
                params: SolutionOpenParams { path: "/a".into() },
                generation: 4,
            },
            json!({"path": "/a", "eluditeGeneration": 4}),
        );
    }
}
