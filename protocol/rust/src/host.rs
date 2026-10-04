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
    /// The running build and its output so far, for a shell that (re)connects (brief 0020).
    pub const BUILD_STATUS: &str = "eludite/build/status";
    /// Host-to-shell notification: a chunk of the build log.
    pub const BUILD_OUTPUT: &str = "eludite/build/output";
    /// Host-to-shell notification.
    pub const BUILD_PROGRESS: &str = "eludite/build/progress";
    /// Host-to-shell notification, one per accepted build.
    pub const BUILD_FINISHED: &str = "eludite/build/finished";
    /// Discover tests (brief 0035).
    pub const TEST_DISCOVER: &str = "eludite/test/discover";
    /// Run tests, or debug them (brief 0035).
    pub const TEST_RUN: &str = "eludite/test/run";
    pub const TEST_CANCEL: &str = "eludite/test/cancel";
    /// The shell's answer to a debug run's `attach` update.
    pub const TEST_ATTACHED: &str = "eludite/test/attached";
    /// The discoveries and runs going, for a shell that (re)connects.
    pub const TEST_STATUS: &str = "eludite/test/status";
    /// Host-to-shell notification: the next piece of a discovery or run.
    pub const TEST_UPDATE: &str = "eludite/test/update";
    /// NuGet (brief 0048): search the package sources.
    pub const NUGET_SEARCH: &str = "eludite/nuget/search";
    /// The projects' packages.
    pub const NUGET_INSTALLED: &str = "eludite/nuget/installed";
    /// Newer versions of the projects' packages.
    pub const NUGET_UPDATES: &str = "eludite/nuget/updates";
    /// Install, uninstall, update or consolidate.
    pub const NUGET_CHANGE: &str = "eludite/nuget/change";
    /// The package sources, and changes to the user's NuGet.config.
    pub const NUGET_SOURCES: &str = "eludite/nuget/sources";
    pub const NUGET_RESTORE: &str = "eludite/nuget/restore";
    /// Fetch a package's icon into the host's cache.
    pub const NUGET_ICON: &str = "eludite/nuget/icon";
    /// Host-to-shell notification: output, progress and metadata of a NuGet call.
    pub const NUGET_UPDATE: &str = "eludite/nuget/update";
    /// Host-to-shell request: credentials for a private feed, during an interactive call.
    pub const NUGET_CREDENTIALS: &str = "eludite/nuget/credentials";

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
        BUILD_STATUS,
        TEST_DISCOVER,
        TEST_RUN,
        TEST_CANCEL,
        TEST_ATTACHED,
        TEST_STATUS,
        NUGET_SEARCH,
        NUGET_INSTALLED,
        NUGET_UPDATES,
        NUGET_CHANGE,
        NUGET_SOURCES,
        NUGET_RESTORE,
        NUGET_ICON,
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
        TEST_UPDATE,
        NUGET_UPDATE,
        NUGET_CREDENTIALS,
    ];

    /// The requests among [`HOST_TO_SHELL`]: the shell answers them.
    pub const HOST_TO_SHELL_REQUESTS: &[&str] = &[APPLY_EDIT, NUGET_CREDENTIALS];
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
    /// `eludite/test/run` naming a container a run is running (data: [`super::TestRunInProgressData`]).
    pub const TEST_RUN_IN_PROGRESS: i64 = -32012;
    /// An `eludite/nuget/*` call failed as a whole (data: [`super::NuGetFailedData`]).
    pub const NUGET_FAILED: i64 = -32014;
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
    /// Visual Studio's Dependencies node (brief 0048).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<TreeDependencies>,
}

/// A project's Dependencies node: packages, project references and frameworks, read from disk.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeDependencies {
    pub restored: bool,
    pub packages: Vec<TreePackage>,
    pub projects: Vec<TreeProjectReference>,
    pub frameworks: Vec<TreeFramework>,
}

/// A top-level package of [`TreeDependencies`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreePackage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_referenced: bool,
    /// The packages it brings in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transitive: Option<Vec<TreeTransitive>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<Vec<NuGetVulnerability>>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeTransitive {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeProjectReference {
    pub name: String,
    pub path: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TreeFramework {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target_framework: Option<String>,
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

/// `eludite/build/status` (brief 0020): a running build's output so far, the chunks `first_seq` to `next_seq - 1`
/// concatenated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusOutput {
    pub first_seq: u64,
    /// The seq of the next `eludite/build/output` chunk: replay `text`, then apply chunks from this seq on.
    pub next_seq: u64,
    pub text: String,
    pub truncated: bool,
}

/// `eludite/build/status`: the running build (the members of its start result, the time since it started, its last
/// progress and its output so far).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusRunning {
    pub build_id: u64,
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub system: Option<BuildSystem>,
    pub path: String,
    pub target: BuildTarget,
    pub configuration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    pub toolchain: Toolchain,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binlog: Option<String>,
    pub command_line: String,
    pub elapsed_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub progress: Option<BuildProgress>,
    pub output: BuildStatusOutput,
}

impl BuildStatusRunning {
    /// The start result this build answered with.
    pub fn start_result(&self) -> BuildStartResult {
        BuildStartResult {
            build_id: self.build_id,
            generation: self.generation,
            system: self.system,
            path: self.path.clone(),
            target: self.target,
            configuration: self.configuration.clone(),
            platform: self.platform.clone(),
            toolchain: self.toolchain.clone(),
            binlog: self.binlog.clone(),
            command_line: self.command_line.clone(),
        }
    }
}

/// `eludite/build/status`: the last finished build, without its diagnostics.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusLast {
    pub build_id: u64,
    pub generation: Generation,
    pub target: BuildTarget,
    pub path: String,
    pub result: BuildResult,
    pub elapsed_ms: f64,
    pub summary: BuildSummary,
}

/// `eludite/build/status` result.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BuildStatusResult {
    pub running: Option<BuildStatusRunning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<BuildStatusLast>,
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
request!(
    /// `eludite/build/status` (brief 0020).
    BuildStatus,
    methods::BUILD_STATUS,
    (),
    BuildStatusResult
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

// Tests (brief 0035): `eludite/test/*`, see host-rpc.md "Tests".

/// How a test container is discovered and run.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestProtocol {
    /// Microsoft.Testing.Platform's server mode.
    Mtp,
    /// The VSTest translation-layer protocol.
    Vstest,
}

/// What runs a container's tests.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestRuntime {
    Dotnet,
    Mono,
    Netfx,
}

/// One test container: a test project built for one target framework.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestContainer {
    /// `<absolute project path>|<target framework>`.
    pub id: String,
    pub name: String,
    pub project: String,
    pub target_framework: String,
    pub protocol: TestProtocol,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<TestRuntime>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub program: Option<String>,
    /// Why it cannot be discovered or run (not built, no Mono).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `eludite/test/discover` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestDiscoverParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_settings: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vstest_console_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestDiscoverResult {
    pub run_id: u64,
    pub generation: Generation,
    pub containers: Vec<TestContainer>,
}

/// A container of `eludite/test/run` and, optionally, which of its tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunContainer {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<String>>,
}

/// `eludite/test/run` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub containers: Option<Vec<TestRunContainer>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub parallel: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub configuration: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_settings: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vstest_console_path: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunResult {
    pub run_id: u64,
    pub generation: Generation,
    pub containers: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCancelParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestCancelResult {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<u64>,
}

/// `eludite/test/attached` params: the shell's answer to an `attach` update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestAttachedParams {
    pub run_id: u64,
    pub process_id: u32,
    pub attached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestAttachedResult {
    pub accepted: bool,
}

/// `data` of a -32012 TestRunInProgress error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestRunInProgressData {
    pub run_id: u64,
    pub container: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestTrait {
    pub name: String,
    pub value: String,
}

/// A discovered test (host-rpc.md, "Tests", the model).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestItem {
    /// The runner's id, unique in its container.
    pub id: String,
    pub display_name: String,
    /// `Namespace.Class.Method`, without a data row's arguments.
    pub fully_qualified_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub class_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub traits: Vec<TestTrait>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestOutcome {
    Running,
    Passed,
    Failed,
    Skipped,
    NotRun,
}

/// A test's state in a run.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestResultItem {
    pub id: String,
    /// `eludite/test/status` only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    pub outcome: TestOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stack_trace: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    /// Only for a test discovery did not list.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fully_qualified_name: Option<String>,
}

/// How to start a test application under a debug adapter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestLaunch {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: String,
    pub env: std::collections::BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runtime: Option<TestRuntime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestUpdateKind {
    Discovered,
    Results,
    Output,
    Launch,
    Attach,
    ContainerFinished,
    Finished,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestState {
    Completed,
    Failed,
    Canceled,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestSummary {
    pub total: u32,
    pub passed: u32,
    pub failed: u32,
    pub skipped: u32,
    pub not_run: u32,
}

/// `eludite/test/update` params: `kind` says which members are present.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestUpdate {
    pub run_id: u64,
    pub generation: Generation,
    pub seq: u64,
    pub kind: TestUpdateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub container: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tests: Option<Vec<TestItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub results: Option<Vec<TestResultItem>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub launch: Option<TestLaunch>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub process_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub state: Option<TestState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub count: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<TestSummary>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl TestUpdate {
    /// An update of `kind` with no optional member (tests fill them in).
    pub fn new(run_id: u64, generation: Generation, seq: u64, kind: TestUpdateKind) -> Self {
        Self {
            run_id,
            generation,
            seq,
            kind,
            container: None,
            tests: None,
            results: None,
            text: None,
            launch: None,
            process_id: None,
            state: None,
            count: None,
            summary: None,
            elapsed_ms: None,
            message: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TestJobKind {
    Discover,
    Run,
}

/// A running discovery's test with its container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStatusTest {
    pub container: String,
    pub test: TestItem,
}

/// `eludite/test/status`: a discovery or run that is going, with what it reported so far.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStatusRunning {
    pub run_id: u64,
    pub kind: TestJobKind,
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub debug: Option<bool>,
    pub containers: Vec<TestContainer>,
    pub elapsed_ms: f64,
    /// The seq of its next update: apply updates from this seq on.
    pub next_seq: u64,
    pub tests: Vec<TestStatusTest>,
    pub results: Vec<TestResultItem>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStatusLast {
    pub run_id: u64,
    pub kind: TestJobKind,
    pub generation: Generation,
    pub state: TestState,
    pub summary: TestSummary,
    pub elapsed_ms: f64,
}

/// `eludite/test/status` result.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TestStatusResult {
    pub running: Vec<TestStatusRunning>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last: Option<TestStatusLast>,
}

request!(
    /// `eludite/test/discover` (brief 0035).
    TestDiscover,
    methods::TEST_DISCOVER,
    TestDiscoverParams,
    TestDiscoverResult
);
request!(
    /// `eludite/test/run` (brief 0035).
    TestRun,
    methods::TEST_RUN,
    TestRunParams,
    TestRunResult
);
request!(
    /// `eludite/test/cancel` (brief 0035).
    TestCancel,
    methods::TEST_CANCEL,
    TestCancelParams,
    TestCancelResult
);
request!(
    /// `eludite/test/attached` (brief 0035).
    TestAttached,
    methods::TEST_ATTACHED,
    TestAttachedParams,
    TestAttachedResult
);
request!(
    /// `eludite/test/status` (brief 0035).
    TestStatus,
    methods::TEST_STATUS,
    (),
    TestStatusResult
);

/// `eludite/test/update` (host to shell).
#[derive(Debug)]
pub enum TestUpdateNotification {}
impl NotificationType for TestUpdateNotification {
    const METHOD: &'static str = methods::TEST_UPDATE;
    type Params = TestUpdate;
}

// NuGet (brief 0048): `eludite/nuget/*`, see host-rpc.md "NuGet".

/// A known vulnerability of a package version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetVulnerability {
    /// `low`, `moderate`, `high` or `critical`.
    pub severity: String,
    pub advisory_url: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetAlternatePackage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

/// Why a version is deprecated.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetDeprecation {
    /// `legacy`, `criticalBugs`, `other`.
    pub reasons: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub alternate_package: Option<NuGetAlternatePackage>,
}

/// How one source answered.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSourceResult {
    pub name: String,
    pub url: String,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub cached: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// `eludite/nuget/search` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSearchParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub query: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub skip: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
}

/// One package of a search.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSearchPackage {
    pub id: String,
    pub version: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license_expression: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<Vec<NuGetVulnerability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<NuGetDeprecation>,
}

/// `eludite/nuget/search` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSearchResult {
    pub generation: Generation,
    pub results: Vec<NuGetSearchPackage>,
    pub sources: Vec<NuGetSourceResult>,
    pub elapsed_ms: f64,
}

/// `eludite/nuget/installed` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetInstalledParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_transitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub metadata: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetPackageDependency {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub range: Option<String>,
}

/// A package of a project.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetInstalledPackage {
    pub id: String,
    pub target_frameworks: Vec<String>,
    pub transitive: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_referenced: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub dependencies: Option<Vec<NuGetPackageDependency>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<Vec<NuGetVulnerability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<NuGetDeprecation>,
}

/// How a project references packages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetProjectFormat {
    PackageReference,
    PackagesConfig,
    None,
}

/// A project's packages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetInstalledProject {
    pub path: String,
    pub name: String,
    pub format: NuGetProjectFormat,
    pub restored: bool,
    pub central_package_management: bool,
    pub target_frameworks: Vec<String>,
    pub packages: Vec<NuGetInstalledPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub assets_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `eludite/nuget/installed` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetInstalledResult {
    pub generation: Generation,
    pub projects: Vec<NuGetInstalledProject>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sources: Option<Vec<NuGetSourceResult>>,
    pub elapsed_ms: f64,
}

/// `eludite/nuget/updates` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetUpdatesParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
}

/// One available update.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetUpdateRow {
    pub project: String,
    pub id: String,
    pub installed: String,
    pub latest: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub versions: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<Vec<NuGetVulnerability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<NuGetDeprecation>,
}

/// `eludite/nuget/updates` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetUpdatesResult {
    pub generation: Generation,
    pub updates: Vec<NuGetUpdateRow>,
    pub sources: Vec<NuGetSourceResult>,
    pub elapsed_ms: f64,
}

/// What `eludite/nuget/change` does.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetAction {
    Install,
    Uninstall,
    Update,
    Consolidate,
}

impl NuGetAction {
    pub fn as_str(self) -> &'static str {
        match self {
            NuGetAction::Install => "install",
            NuGetAction::Uninstall => "uninstall",
            NuGetAction::Update => "update",
            NuGetAction::Consolidate => "consolidate",
        }
    }
}

/// The setting `nuget.lockFiles`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetLockFiles {
    #[default]
    Respect,
    Ignore,
}

/// A package to change, and the version (when given).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetPackageArg {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// `eludite/nuget/change` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetChangeParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    pub action: NuGetAction,
    pub packages: Vec<NuGetPackageArg>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub prerelease: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub include_transitive: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_files: Option<NuGetLockFiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetEditedKind {
    Project,
    CentralPackageVersions,
}

/// A file a change wrote.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetEditedFile {
    pub path: String,
    pub kind: NuGetEditedKind,
    pub changes: Vec<String>,
}

/// A restore error or warning.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetDiagnostic {
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
    pub project: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetRestoreState {
    Succeeded,
    Failed,
    Canceled,
}

/// How a restore went.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetRestoreOutcome {
    pub result: NuGetRestoreState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    pub elapsed_ms: f64,
    pub command_line: String,
    pub locked_mode: bool,
    pub lock_files: Vec<String>,
    pub diagnostics: Vec<NuGetDiagnostic>,
}

/// `eludite/nuget/change` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetChangeResult {
    /// The generation after the change.
    pub generation: Generation,
    pub action: NuGetAction,
    pub packages: Vec<NuGetPackageArg>,
    pub projects: Vec<String>,
    pub edited: Vec<NuGetEditedFile>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<NuGetRestoreOutcome>,
    pub elapsed_ms: f64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetSourcesAction {
    #[default]
    List,
    Add,
    Remove,
    Enable,
    Disable,
}

/// `eludite/nuget/sources` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSourcesParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub action: Option<NuGetSourcesAction>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetSourceScope {
    Machine,
    User,
    Solution,
    Other,
}

/// A package source of the NuGet.config chain.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSourceInfo {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub local: bool,
    pub scope: NuGetSourceScope,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_file: Option<String>,
}

/// `eludite/nuget/sources` result.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetSourcesResult {
    pub sources: Vec<NuGetSourceInfo>,
    pub config_files: Vec<String>,
    pub user_config: String,
    pub changed: bool,
}

/// `eludite/nuget/restore` params.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetRestoreParams {
    pub generation: Generation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_files: Option<NuGetLockFiles>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub force: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interactive: Option<bool>,
}

/// `eludite/nuget/restore` result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetRestoreResult {
    pub generation: Generation,
    #[serde(flatten)]
    pub outcome: NuGetRestoreOutcome,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NuGetUpdateKind {
    Output,
    Progress,
    Metadata,
}

/// A package version's vulnerability and deprecation data (a `metadata` update).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetPackageMetadata {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vulnerabilities: Option<Vec<NuGetVulnerability>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deprecation: Option<NuGetDeprecation>,
}

/// `eludite/nuget/update` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetUpdate {
    pub operation: u64,
    pub generation: Generation,
    pub seq: u64,
    pub kind: NuGetUpdateKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub packages: Option<Vec<NuGetPackageMetadata>>,
}

/// `eludite/nuget/credentials` params (host to shell).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetCredentialsParams {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub operation: Option<u64>,
    pub source: String,
    pub url: String,
    pub host: String,
    pub proxy: bool,
    pub is_retry: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// The shell's answer to `eludite/nuget/credentials` (`null` gives up too).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetCredentialsAnswer {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub remember: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub canceled: Option<bool>,
}

/// `eludite/nuget/icon` params.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetIconParams {
    pub url: String,
}

/// `eludite/nuget/icon` result: the cached file, or `None` when it could not be fetched.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetIconResult {
    pub path: Option<String>,
}

/// `data` of a -32014 NuGetFailed error.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NuGetFailedData {
    /// `notFound`, `credentialsRequired`, `sourceFailed`, `invalidProject`, `noSources`.
    pub reason: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub package: Option<String>,
}

request!(
    /// `eludite/nuget/search` (brief 0048).
    NuGetSearch,
    methods::NUGET_SEARCH,
    NuGetSearchParams,
    NuGetSearchResult
);
request!(
    /// `eludite/nuget/installed` (brief 0048).
    NuGetInstalled,
    methods::NUGET_INSTALLED,
    NuGetInstalledParams,
    NuGetInstalledResult
);
request!(
    /// `eludite/nuget/updates` (brief 0048).
    NuGetUpdates,
    methods::NUGET_UPDATES,
    NuGetUpdatesParams,
    NuGetUpdatesResult
);
request!(
    /// `eludite/nuget/change` (brief 0048).
    NuGetChange,
    methods::NUGET_CHANGE,
    NuGetChangeParams,
    NuGetChangeResult
);
request!(
    /// `eludite/nuget/sources` (brief 0048).
    NuGetSources,
    methods::NUGET_SOURCES,
    NuGetSourcesParams,
    NuGetSourcesResult
);
request!(
    /// `eludite/nuget/restore` (brief 0048).
    NuGetRestore,
    methods::NUGET_RESTORE,
    NuGetRestoreParams,
    NuGetRestoreResult
);
request!(
    /// `eludite/nuget/icon` (brief 0048).
    NuGetIcon,
    methods::NUGET_ICON,
    NuGetIconParams,
    NuGetIconResult
);
request!(
    /// `eludite/nuget/credentials` (brief 0048): the host asks the shell.
    NuGetCredentials,
    methods::NUGET_CREDENTIALS,
    NuGetCredentialsParams,
    Option<NuGetCredentialsAnswer>
);

/// `eludite/nuget/update` (host to shell).
#[derive(Debug)]
pub enum NuGetUpdateNotification {}
impl NotificationType for NuGetUpdateNotification {
    const METHOD: &'static str = methods::NUGET_UPDATE;
    type Params = NuGetUpdate;
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
                        dependencies: None,
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
                        dependencies: None,
                    },
                    TreeProject {
                        name: "Broken".into(),
                        path: "/src/Broken/Broken.csproj".into(),
                        kind: TreeProjectKind::Sdk,
                        web: false,
                        target_frameworks: vec![],
                        files: vec![],
                        error: Some("MSB4025: invalid XML".into()),
                        dependencies: None,
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
