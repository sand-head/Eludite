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
    pub const SOLUTION_TREE: &str = "eludite/solution/tree";
    /// Host-to-shell notification.
    pub const SOLUTION_STATUS: &str = "eludite/solution/status";
    /// Host-to-shell notification.
    pub const LANGUAGE_SERVER_STATUS: &str = "eludite/languageServer/status";
    /// Host-to-shell notification (LSP shape plus `eluditeGeneration`).
    pub const PUBLISH_DIAGNOSTICS: &str = "textDocument/publishDiagnostics";
    pub const CANCEL_REQUEST: &str = "$/cancelRequest";
    /// Host-to-shell request (relayed from the language server).
    pub const APPLY_EDIT: &str = "workspace/applyEdit";
    /// Start a build (brief 0017).
    pub const BUILD_START: &str = "eludite/build/start";
    pub const BUILD_CANCEL: &str = "eludite/build/cancel";
    /// Host-to-shell notification: a chunk of the build log.
    pub const BUILD_OUTPUT: &str = "eludite/build/output";
    /// Host-to-shell notification.
    pub const BUILD_PROGRESS: &str = "eludite/build/progress";
    /// Host-to-shell notification, one per accepted build.
    pub const BUILD_FINISHED: &str = "eludite/build/finished";

    /// Eludite requests and notifications the host accepts.
    pub const ELUDITE_ACCEPTED: &[&str] = &[
        HOST_INITIALIZE,
        PING,
        HOST_INFO,
        HOST_SHUTDOWN,
        HOST_EXIT,
        SOLUTION_OPEN,
        SOLUTION_CLOSE,
        SOLUTION_TREE,
        BUILD_START,
        BUILD_CANCEL,
    ];

    /// Forwarded LSP requests typed in [`crate::lsp`].
    pub const FORWARDED_TYPED_REQUESTS: &[&str] = &[
        "textDocument/completion",
        "completionItem/resolve",
        "textDocument/hover",
        "textDocument/signatureHelp",
        "textDocument/definition",
        "textDocument/references",
        "textDocument/prepareRename",
        "textDocument/rename",
        "textDocument/codeAction",
        "codeAction/resolve",
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
        "textDocument/typeDefinition",
        "textDocument/implementation",
        "textDocument/documentHighlight",
        "textDocument/semanticTokens/full",
        "textDocument/semanticTokens/range",
        "textDocument/formatting",
    ];

    /// Forwarded LSP notifications passed through as raw JSON.
    pub const FORWARDED_UNTYPED_NOTIFICATIONS: &[&str] =
        &["textDocument/didSave", "workspace/didChangeWatchedFiles"];

    /// Messages the host sends to the shell: notifications, and the requests in [`HOST_TO_SHELL_REQUESTS`].
    pub const HOST_TO_SHELL: &[&str] = &[
        SOLUTION_STATUS,
        LANGUAGE_SERVER_STATUS,
        PUBLISH_DIAGNOSTICS,
        "window/showMessage",
        "$/progress",
        APPLY_EDIT,
        BUILD_OUTPUT,
        BUILD_PROGRESS,
        BUILD_FINISHED,
    ];

    /// The requests among [`HOST_TO_SHELL`]: the shell answers them.
    pub const HOST_TO_SHELL_REQUESTS: &[&str] = &[APPLY_EDIT];
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
    /// `eludite/build/start` while a build runs (data: [`super::BuildInProgressData`]).
    pub const BUILD_IN_PROGRESS: i64 = -32010;
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

/// `eludite/solution/tree` result: the projects of the open solution and their source files.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SolutionTree {
    /// The generation the tree was computed under.
    pub generation: Generation,
    /// The open solution or project file; `None` when no solution is open.
    #[serde(default)]
    pub path: Option<String>,
    pub projects: Vec<TreeProject>,
}

/// `sdk` or `legacy` (non-SDK MSBuild 2003 format).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TreeProjectKind {
    Sdk,
    Legacy,
}

/// One project of [`SolutionTree`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeProject {
    pub name: String,
    pub path: String,
    pub kind: TreeProjectKind,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub web: bool,
    /// Short monikers (`net10.0`, `net48`).
    pub target_frameworks: Vec<String>,
    pub files: Vec<TreeFile>,
    /// Why the project did not evaluate; `files` is then empty.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The MSBuild item type of a [`TreeFile`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TreeItemType {
    Compile,
    Content,
}

/// A source file of a [`TreeProject`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeFile {
    pub path: String,
    pub item_type: TreeItemType,
    /// Absolute path of the file this one nests under.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependent_upon: Option<String>,
    /// Project-relative display path of a linked file.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub link: Option<String>,
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
request!(
    /// `eludite/solution/tree`.
    SolutionTreeRequest,
    methods::SOLUTION_TREE,
    (),
    SolutionTree
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

// Build (brief 0017): `eludite/build/*`, see host-rpc.md "Build".

/// What a build does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildTarget {
    Build,
    Rebuild,
    Clean,
}

impl BuildTarget {
    /// The wire name (`build`, `rebuild`, `clean`).
    pub fn as_str(self) -> &'static str {
        match self {
            BuildTarget::Build => "build",
            BuildTarget::Rebuild => "rebuild",
            BuildTarget::Clean => "clean",
        }
    }
}

/// Which build system builds (brief 0019): the host runs `msbuild`; `cargo` builds run in the shell and only share
/// the `eludite/build/*` shapes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildSystem {
    Msbuild,
    Cargo,
}

/// `eludite/build/start` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStartParams {
    pub target: BuildTarget,
    /// `None` (or `msbuild`): the host's build. The shell never sends `cargo` to the host.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<BuildSystem>,
    /// One project file of the open solution; `None` builds the solution.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
}

/// Which MSBuild runs the build.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ToolchainKind {
    /// The .NET SDK's `dotnet build`.
    Dotnet,
    /// Mono's MSBuild (located, brief 0003).
    Mono,
    /// Build Tools' `MSBuild.exe` (Windows).
    BuildTools,
    /// `cargo`: the shell's own Cargo builds (brief 0019), never the host's.
    Cargo,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Toolchain {
    pub kind: ToolchainKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
}

/// `eludite/build/start` result: the build has started.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStartResult {
    pub build_id: u64,
    pub generation: Generation,
    /// `None`: `msbuild` (the host). `cargo` for the shell's Cargo builds.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<BuildSystem>,
    pub path: String,
    pub target: BuildTarget,
    pub configuration: String,
    #[serde(default)]
    pub platform: Option<String>,
    pub toolchain: Toolchain,
    #[serde(default)]
    pub binlog: Option<String>,
    pub command_line: String,
}

/// `eludite/build/cancel` params: `None` cancels the running build.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildCancelParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildCancelResult {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<u64>,
}

/// `data` of a -32010 BuildInProgress error.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildInProgressData {
    pub build_id: u64,
}

/// `eludite/build/output` params: the next chunk of whole lines (`seq` from 0, in order).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildOutput {
    pub build_id: u64,
    pub seq: u64,
    pub text: String,
}

/// `eludite/build/progress` params.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProgress {
    pub build_id: u64,
    pub elapsed_ms: f64,
    pub projects_total: u32,
    pub projects_completed: u32,
    pub errors: u32,
    pub warnings: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildResult {
    Succeeded,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildSummary {
    pub projects_succeeded: u32,
    pub projects_failed: u32,
    pub errors: u32,
    pub warnings: u32,
}

/// One project of [`BuildFinished`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildProjectResult {
    pub name: String,
    pub path: String,
    pub result: BuildResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    pub errors: u32,
    pub warnings: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BuildDiagnosticSeverity {
    Error,
    Warning,
    Message,
}

/// A build diagnostic. Line and column are 1-based; `None` (or 0) when unknown.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildDiagnostic {
    pub severity: BuildDiagnosticSeverity,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub end_column: Option<u32>,
    /// The project file that reported it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// `eludite/build/finished` params: one per accepted build, after its last output chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildFinished {
    pub build_id: u64,
    pub generation: Generation,
    pub target: BuildTarget,
    pub path: String,
    pub result: BuildResult,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub elapsed_ms: f64,
    pub summary: BuildSummary,
    pub projects: Vec<BuildProjectResult>,
    pub diagnostics: Vec<BuildDiagnostic>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub diagnostics_truncated: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binlog: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

request!(
    /// `eludite/build/start`.
    BuildStart,
    methods::BUILD_START,
    BuildStartParams,
    BuildStartResult
);
request!(
    /// `eludite/build/cancel`.
    BuildCancel,
    methods::BUILD_CANCEL,
    BuildCancelParams,
    BuildCancelResult
);

/// `eludite/build/output` (host to shell).
#[derive(Debug)]
pub enum BuildOutputNotification {}
impl NotificationType for BuildOutputNotification {
    const METHOD: &'static str = methods::BUILD_OUTPUT;
    type Params = BuildOutput;
}

/// `eludite/build/progress` (host to shell).
#[derive(Debug)]
pub enum BuildProgressNotification {}
impl NotificationType for BuildProgressNotification {
    const METHOD: &'static str = methods::BUILD_PROGRESS;
    type Params = BuildProgress;
}

/// `eludite/build/finished` (host to shell).
#[derive(Debug)]
pub enum BuildFinishedNotification {}
impl NotificationType for BuildFinishedNotification {
    const METHOD: &'static str = methods::BUILD_FINISHED;
    type Params = BuildFinished;
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
    fn solution_tree() {
        round_trip(
            &SolutionTree {
                generation: 2,
                path: Some("/src/App.slnx".into()),
                projects: vec![
                    TreeProject {
                        name: "App".into(),
                        path: "/src/App/App.csproj".into(),
                        kind: TreeProjectKind::Sdk,
                        web: false,
                        target_frameworks: vec!["net8.0".into(), "net10.0".into()],
                        files: vec![TreeFile {
                            path: "/src/App/Program.cs".into(),
                            item_type: TreeItemType::Compile,
                            dependent_upon: None,
                            link: None,
                        }],
                        error: None,
                    },
                    TreeProject {
                        name: "Shop".into(),
                        path: "/src/Shop/Shop.csproj".into(),
                        kind: TreeProjectKind::Legacy,
                        web: true,
                        target_frameworks: vec!["net48".into()],
                        files: vec![
                            TreeFile {
                                path: "/src/Shop/Default.aspx".into(),
                                item_type: TreeItemType::Content,
                                dependent_upon: None,
                                link: None,
                            },
                            TreeFile {
                                path: "/src/Shop/Default.aspx.cs".into(),
                                item_type: TreeItemType::Compile,
                                dependent_upon: Some("/src/Shop/Default.aspx".into()),
                                link: None,
                            },
                            TreeFile {
                                path: "/src/Shared/Version.cs".into(),
                                item_type: TreeItemType::Compile,
                                dependent_upon: None,
                                link: Some("Properties/Version.cs".into()),
                            },
                        ],
                        error: None,
                    },
                    TreeProject {
                        name: "Broken".into(),
                        path: "/src/Broken/Broken.csproj".into(),
                        kind: TreeProjectKind::Sdk,
                        web: false,
                        target_frameworks: vec![],
                        files: vec![],
                        error: Some("MSB4025: invalid XML".into()),
                    },
                ],
            },
            json!({
                "generation": 2,
                "path": "/src/App.slnx",
                "projects": [
                    {"name": "App", "path": "/src/App/App.csproj", "kind": "sdk", "targetFrameworks": ["net8.0", "net10.0"],
                     "files": [{"path": "/src/App/Program.cs", "itemType": "compile"}]},
                    {"name": "Shop", "path": "/src/Shop/Shop.csproj", "kind": "legacy", "web": true, "targetFrameworks": ["net48"],
                     "files": [
                        {"path": "/src/Shop/Default.aspx", "itemType": "content"},
                        {"path": "/src/Shop/Default.aspx.cs", "itemType": "compile", "dependentUpon": "/src/Shop/Default.aspx"},
                        {"path": "/src/Shared/Version.cs", "itemType": "compile", "link": "Properties/Version.cs"}
                     ]},
                    {"name": "Broken", "path": "/src/Broken/Broken.csproj", "kind": "sdk", "targetFrameworks": [], "files": [],
                     "error": "MSB4025: invalid XML"}
                ]
            }),
        );
        // No solution open: path is null on the wire.
        let empty = SolutionTree {
            generation: 0,
            path: None,
            projects: vec![],
        };
        round_trip(
            &empty,
            json!({"generation": 0, "path": null, "projects": []}),
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

    #[test]
    fn build_messages() {
        round_trip(
            &BuildStartParams {
                target: BuildTarget::Rebuild,
                system: None,
                project: Some("/s/App/App.csproj".into()),
                configuration: Some("Release".into()),
                platform: None,
            },
            json!({"target": "rebuild", "project": "/s/App/App.csproj", "configuration": "Release"}),
        );
        round_trip(
            &BuildStartResult {
                build_id: 3,
                generation: 1,
                system: None,
                path: "/s/App.slnx".into(),
                target: BuildTarget::Build,
                configuration: "Debug".into(),
                platform: None,
                toolchain: Toolchain {
                    kind: ToolchainKind::Mono,
                    path: Some("/usr/lib/mono/msbuild/Current/bin/MSBuild.dll".into()),
                    source: Some("PATH".into()),
                },
                binlog: Some("/tmp/b.binlog".into()),
                command_line: "mono MSBuild.dll /s/App.slnx".into(),
            },
            json!({"buildId": 3, "generation": 1, "path": "/s/App.slnx", "target": "build", "configuration": "Debug",
                   "platform": null,
                   "toolchain": {"kind": "mono", "path": "/usr/lib/mono/msbuild/Current/bin/MSBuild.dll", "source": "PATH"},
                   "binlog": "/tmp/b.binlog", "commandLine": "mono MSBuild.dll /s/App.slnx"}),
        );
        round_trip(&BuildCancelParams::default(), json!({}));
        round_trip(
            &BuildCancelResult {
                canceled: true,
                build_id: Some(3),
            },
            json!({"canceled": true, "buildId": 3}),
        );
        round_trip(
            &BuildOutput {
                build_id: 3,
                seq: 0,
                text: "Build started\n".into(),
            },
            json!({"buildId": 3, "seq": 0, "text": "Build started\n"}),
        );
        round_trip(
            &BuildProgress {
                build_id: 3,
                elapsed_ms: 120.5,
                projects_total: 8,
                projects_completed: 2,
                errors: 1,
                warnings: 0,
                current_project: Some("Eludite.Web".into()),
            },
            json!({"buildId": 3, "elapsedMs": 120.5, "projectsTotal": 8, "projectsCompleted": 2, "errors": 1,
                   "warnings": 0, "currentProject": "Eludite.Web"}),
        );
        round_trip(
            &BuildFinished {
                build_id: 3,
                generation: 1,
                target: BuildTarget::Build,
                path: "/s/App.slnx".into(),
                result: BuildResult::Failed,
                exit_code: Some(1),
                elapsed_ms: 2500.0,
                summary: BuildSummary {
                    projects_succeeded: 1,
                    projects_failed: 1,
                    errors: 1,
                    warnings: 0,
                },
                projects: vec![BuildProjectResult {
                    name: "App".into(),
                    path: "/s/App/App.csproj".into(),
                    result: BuildResult::Failed,
                    elapsed_ms: Some(900.0),
                    errors: 1,
                    warnings: 0,
                }],
                diagnostics: vec![BuildDiagnostic {
                    severity: BuildDiagnosticSeverity::Error,
                    code: "CS0103".into(),
                    message: "The name 'x' does not exist in the current context".into(),
                    file: Some("/s/App/Program.cs".into()),
                    line: Some(3),
                    column: Some(9),
                    end_line: None,
                    end_column: None,
                    project: Some("/s/App/App.csproj".into()),
                }],
                diagnostics_truncated: false,
                binlog: Some("/tmp/b.binlog".into()),
                message: None,
            },
            json!({"buildId": 3, "generation": 1, "target": "build", "path": "/s/App.slnx", "result": "failed",
                   "exitCode": 1, "elapsedMs": 2500.0,
                   "summary": {"projectsSucceeded": 1, "projectsFailed": 1, "errors": 1, "warnings": 0},
                   "projects": [{"name": "App", "path": "/s/App/App.csproj", "result": "failed", "elapsedMs": 900.0,
                                 "errors": 1, "warnings": 0}],
                   "diagnostics": [{"severity": "error", "code": "CS0103",
                                    "message": "The name 'x' does not exist in the current context",
                                    "file": "/s/App/Program.cs", "line": 3, "column": 9, "project": "/s/App/App.csproj"}],
                   "binlog": "/tmp/b.binlog"}),
        );
        assert_eq!(
            serde_json::to_value(ToolchainKind::BuildTools).unwrap(),
            json!("buildTools")
        );
        assert_eq!(BuildTarget::Clean.as_str(), "clean");
        assert_eq!(
            serde_json::to_value(BuildResult::Canceled).unwrap(),
            json!("canceled")
        );
        round_trip(&BuildInProgressData { build_id: 2 }, json!({"buildId": 2}));
    }
}
