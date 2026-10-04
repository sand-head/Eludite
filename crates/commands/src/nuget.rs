//! NuGet's commands (brief 0048): `eludite.nuget.manage` (the Manage NuGet Packages window), `eludite.nuget.search`,
//! `installed` and `updates` (read), `install`, `uninstall`, `update` and `consolidate` (execute, under the solution
//! policy's `nuget.change`), `sources` (read for `list`; execute under `nuget.sources` for add, remove, enable and
//! disable) and `restore` (execute).
//!
//! The schemas are the files in `protocol/schemas/nuget-*.json` (checked in first, CLAUDE.md invariant 4). This module
//! parses input into a typed [`NuGetRequest`], serializes the typed outputs, and registers the commands with their
//! escalation hooks; the shell implements [`NuGetCommands`]. Every change keeps its arguments in the audit log, the
//! user's too ([`CommandRegistry::always_audit_arguments`]).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::policy::{NuGetCall, PolicyView};
use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const MANAGE: &str = "eludite.nuget.manage";
pub const SEARCH: &str = "eludite.nuget.search";
pub const INSTALLED: &str = "eludite.nuget.installed";
pub const UPDATES: &str = "eludite.nuget.updates";
pub const INSTALL: &str = "eludite.nuget.install";
pub const UNINSTALL: &str = "eludite.nuget.uninstall";
pub const UPDATE: &str = "eludite.nuget.update";
pub const CONSOLIDATE: &str = "eludite.nuget.consolidate";
pub const SOURCES: &str = "eludite.nuget.sources";
pub const RESTORE: &str = "eludite.nuget.restore";

pub const ALL: [&str; 10] = [
    MANAGE,
    SEARCH,
    INSTALLED,
    UPDATES,
    INSTALL,
    UNINSTALL,
    UPDATE,
    CONSOLIDATE,
    SOURCES,
    RESTORE,
];

/// The commands that change projects (`nuget.change`).
pub const CHANGES: [&str; 4] = [INSTALL, UNINSTALL, UPDATE, CONSOLIDATE];

/// Results a search answers by default, and at most.
pub const DEFAULT_RESULTS: usize = 50;
pub const MAX_RESULTS: usize = 500;
/// Versions a search row lists at most.
pub const MAX_VERSIONS: usize = 20;
/// Diagnostics a restore lists at most.
pub const MAX_DIAGNOSTICS: usize = 200;

/// The permission class each command declares (title, input, output, class, agent visible).
fn schemas(
    id: &str,
) -> (
    &'static str,
    &'static str,
    &'static str,
    PermissionClass,
    bool,
) {
    use PermissionClass::*;
    match id {
        MANAGE => (
            "Project: Manage NuGet Packages",
            include_str!("../../../protocol/schemas/nuget-manage.input.json"),
            crate::view::TOOL_WINDOW_OUTPUT_SCHEMA,
            Read,
            false,
        ),
        SEARCH => (
            "NuGet: Search",
            include_str!("../../../protocol/schemas/nuget-search.input.json"),
            include_str!("../../../protocol/schemas/nuget-search.output.json"),
            Read,
            true,
        ),
        INSTALLED => (
            "NuGet: Installed Packages",
            include_str!("../../../protocol/schemas/nuget-installed.input.json"),
            include_str!("../../../protocol/schemas/nuget-installed.output.json"),
            Read,
            true,
        ),
        UPDATES => (
            "NuGet: Updates",
            include_str!("../../../protocol/schemas/nuget-updates.input.json"),
            include_str!("../../../protocol/schemas/nuget-updates.output.json"),
            Read,
            true,
        ),
        INSTALL => (
            "NuGet: Install",
            include_str!("../../../protocol/schemas/nuget-install.input.json"),
            include_str!("../../../protocol/schemas/nuget-install.output.json"),
            Execute,
            true,
        ),
        UNINSTALL => (
            "NuGet: Uninstall",
            include_str!("../../../protocol/schemas/nuget-uninstall.input.json"),
            include_str!("../../../protocol/schemas/nuget-uninstall.output.json"),
            Execute,
            true,
        ),
        UPDATE => (
            "NuGet: Update",
            include_str!("../../../protocol/schemas/nuget-update.input.json"),
            include_str!("../../../protocol/schemas/nuget-update.output.json"),
            Execute,
            true,
        ),
        CONSOLIDATE => (
            "NuGet: Consolidate",
            include_str!("../../../protocol/schemas/nuget-consolidate.input.json"),
            include_str!("../../../protocol/schemas/nuget-consolidate.output.json"),
            Execute,
            true,
        ),
        SOURCES => (
            "NuGet: Package Sources",
            include_str!("../../../protocol/schemas/nuget-sources.input.json"),
            include_str!("../../../protocol/schemas/nuget-sources.output.json"),
            Read,
            true,
        ),
        RESTORE => (
            "NuGet: Restore",
            include_str!("../../../protocol/schemas/nuget-restore.input.json"),
            include_str!("../../../protocol/schemas/nuget-restore.output.json"),
            Execute,
            true,
        ),
        other => unreachable!("not a NuGet command: {other}"),
    }
}

/// The window's tabs.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Tab {
    #[default]
    Browse,
    Installed,
    Updates,
    Consolidate,
}

/// What `eludite.nuget.sources` does.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SourcesAction {
    #[default]
    List,
    Add,
    Remove,
    Enable,
    Disable,
}

/// What a change does (the four change commands share one shape).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChangeAction {
    Install,
    Uninstall,
    Update,
    Consolidate,
}

impl ChangeAction {
    pub fn command(self) -> &'static str {
        match self {
            ChangeAction::Install => INSTALL,
            ChangeAction::Uninstall => UNINSTALL,
            ChangeAction::Update => UPDATE,
            ChangeAction::Consolidate => CONSOLIDATE,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            ChangeAction::Install => "install",
            ChangeAction::Uninstall => "uninstall",
            ChangeAction::Update => "update",
            ChangeAction::Consolidate => "consolidate",
        }
    }
}

/// A parsed change request.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Change {
    /// None only for `update` with `all`.
    pub package: Option<String>,
    pub version: Option<String>,
    /// Projects by name or path (`project` and `projects` together).
    pub projects: Vec<String>,
    pub prerelease: Option<bool>,
    pub source: Option<String>,
    /// `update`: every update eludite.nuget.updates lists.
    pub all: bool,
}

/// A parsed, validated NuGet command.
#[derive(Debug, Clone, PartialEq)]
pub enum NuGetRequest {
    Manage {
        project: Option<String>,
        solution: bool,
        tab: Option<Tab>,
        query: Option<String>,
    },
    Search {
        query: String,
        source: Option<String>,
        prerelease: Option<bool>,
        max_items: usize,
        skip: usize,
    },
    Installed {
        project: Option<String>,
        include_transitive: bool,
        vulnerabilities: bool,
    },
    Updates {
        project: Option<String>,
        prerelease: Option<bool>,
        source: Option<String>,
    },
    Change {
        action: ChangeAction,
        change: Change,
    },
    Sources {
        action: SourcesAction,
        name: Option<String>,
        url: Option<String>,
    },
    Restore {
        project: Option<String>,
        force: bool,
    },
}

impl NuGetRequest {
    pub fn command(&self) -> &'static str {
        match self {
            NuGetRequest::Manage { .. } => MANAGE,
            NuGetRequest::Search { .. } => SEARCH,
            NuGetRequest::Installed { .. } => INSTALLED,
            NuGetRequest::Updates { .. } => UPDATES,
            NuGetRequest::Change { action, .. } => action.command(),
            NuGetRequest::Sources { .. } => SOURCES,
            NuGetRequest::Restore { .. } => RESTORE,
        }
    }
}

/// A known vulnerability, as the commands name it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Vulnerability {
    pub severity: String,
    pub advisory_url: String,
}

/// How one source answered.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceRow {
    pub name: String,
    pub count: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchRow {
    pub id: String,
    pub version: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub versions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub authors: Option<String>,
    pub source: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub downloads: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_url: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon_url: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub vulnerabilities: Vec<Vulnerability>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
}

/// `nuget-search.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SearchOutput {
    pub results: Vec<SearchRow>,
    pub sources: Vec<SourceRow>,
    pub truncated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledPackage {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub requested: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub transitive: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_referenced: bool,
    pub vulnerabilities: Vec<Vulnerability>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub deprecated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledProject {
    pub name: String,
    pub path: String,
    pub restored: bool,
    pub central_package_management: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub props_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lock_file: Option<String>,
    pub packages: Vec<InstalledPackage>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// `nuget-installed.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct InstalledOutput {
    pub projects: Vec<InstalledProject>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdateRow {
    pub project: String,
    pub id: String,
    pub installed: String,
    pub latest: String,
    pub source: String,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub vulnerable: bool,
}

/// `nuget-updates.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct UpdatesOutput {
    pub updates: Vec<UpdateRow>,
    pub sources: Vec<SourceRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreDiagnostic {
    /// `error`, `warning` or `message`.
    pub severity: String,
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
}

/// `nuget-restore.output.json` (and a change's `restore`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RestoreOutput {
    /// `succeeded`, `failed` or `canceled`.
    pub result: String,
    pub errors: u32,
    pub warnings: u32,
    pub elapsed_ms: f64,
    pub locked_mode: bool,
    pub lock_files: Vec<String>,
    pub diagnostics: Vec<RestoreDiagnostic>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PackageVersion {
    pub id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

/// The change commands' output (`nuget-install.output.json` and its three twins).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ChangeOutput {
    pub action: ChangeAction,
    pub packages: Vec<PackageVersion>,
    pub projects: Vec<String>,
    pub edited: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub restore: Option<RestoreOutput>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourceOut {
    pub name: String,
    pub url: String,
    pub enabled: bool,
    pub local: bool,
    /// `machine`, `user`, `solution` or `other`.
    pub scope: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub config_file: Option<String>,
}

/// `nuget-sources.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SourcesOutput {
    pub sources: Vec<SourceOut>,
    pub user_config: String,
    pub changed: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum NuGetOutput {
    /// `eludite.nuget.manage`: the window's place (view-tool-window.output.json).
    Manage(Value),
    Search(Box<SearchOutput>),
    Installed(Box<InstalledOutput>),
    Updates(Box<UpdatesOutput>),
    Change(Box<ChangeOutput>),
    Sources(Box<SourcesOutput>),
    Restore(Box<RestoreOutput>),
}

impl NuGetOutput {
    pub fn to_json(&self) -> Value {
        match self {
            NuGetOutput::Manage(v) => Ok(v.clone()),
            NuGetOutput::Search(o) => serde_json::to_value(o),
            NuGetOutput::Installed(o) => serde_json::to_value(o),
            NuGetOutput::Updates(o) => serde_json::to_value(o),
            NuGetOutput::Change(o) => serde_json::to_value(o),
            NuGetOutput::Sources(o) => serde_json::to_value(o),
            NuGetOutput::Restore(o) => serde_json::to_value(o),
        }
        .expect("NuGet outputs serialize")
    }
}

/// Whatever owns the NuGet window and the host connection (the shell). Called on the invoking thread.
pub trait NuGetCommands: Send + Sync {
    fn apply(&self, request: NuGetRequest) -> Result<NuGetOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ManageIn {
    project: Option<String>,
    solution: Option<bool>,
    tab: Option<Tab>,
    query: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SearchIn {
    query: Option<String>,
    source: Option<String>,
    prerelease: Option<bool>,
    max_items: Option<u64>,
    skip: Option<u64>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct InstalledIn {
    project: Option<String>,
    include_transitive: Option<bool>,
    vulnerabilities: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct UpdatesIn {
    project: Option<String>,
    prerelease: Option<bool>,
    source: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct InstallIn {
    package: Option<String>,
    version: Option<String>,
    project: Option<String>,
    projects: Option<Vec<String>>,
    prerelease: Option<bool>,
    source: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct UninstallIn {
    package: Option<String>,
    project: Option<String>,
    projects: Option<Vec<String>>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct UpdateIn {
    package: Option<String>,
    version: Option<String>,
    project: Option<String>,
    all: Option<bool>,
    prerelease: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ConsolidateIn {
    package: Option<String>,
    version: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SourcesIn {
    action: Option<SourcesAction>,
    name: Option<String>,
    url: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RestoreIn {
    project: Option<String>,
    force: Option<bool>,
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(invalid(format!("`{name}` must not be empty"))),
        Some(s) => Ok(Some(s.trim().to_owned())),
        None => Ok(None),
    }
}

fn required(name: &str, v: Option<String>) -> Result<String, CommandError> {
    non_empty(name, v)?.ok_or_else(|| invalid(format!("`{name}` is required")))
}

fn projects(
    project: Option<String>,
    many: Option<Vec<String>>,
) -> Result<Vec<String>, CommandError> {
    let mut all = Vec::new();
    if let Some(p) = non_empty("project", project)? {
        all.push(p);
    }
    for p in many.unwrap_or_default() {
        all.push(non_empty("projects", Some(p))?.expect("non_empty keeps Some"));
    }
    Ok(all)
}

/// Parse and validate the input of NuGet command `id`.
pub fn parse(id: &str, value: Value) -> Result<NuGetRequest, CommandError> {
    Ok(match id {
        MANAGE => {
            let i: ManageIn = input(value)?;
            let solution = i.solution.unwrap_or(false);
            let project = non_empty("project", i.project)?;
            if solution && project.is_some() {
                return Err(invalid("`project` and `solution` exclude each other"));
            }
            NuGetRequest::Manage {
                project,
                solution,
                tab: i.tab,
                query: i.query,
            }
        }
        SEARCH => {
            let i: SearchIn = input(value)?;
            let max_items = match i.max_items {
                None => DEFAULT_RESULTS,
                Some(n) if (1..=MAX_RESULTS as u64).contains(&n) => n as usize,
                Some(_) => return Err(invalid(format!("`max_items` is 1 to {MAX_RESULTS}"))),
            };
            NuGetRequest::Search {
                query: i.query.unwrap_or_default().trim().to_owned(),
                source: non_empty("source", i.source)?,
                prerelease: i.prerelease,
                max_items,
                skip: i.skip.unwrap_or(0).min(100_000) as usize,
            }
        }
        INSTALLED => {
            let i: InstalledIn = input(value)?;
            NuGetRequest::Installed {
                project: non_empty("project", i.project)?,
                include_transitive: i.include_transitive.unwrap_or(false),
                vulnerabilities: i.vulnerabilities.unwrap_or(true),
            }
        }
        UPDATES => {
            let i: UpdatesIn = input(value)?;
            NuGetRequest::Updates {
                project: non_empty("project", i.project)?,
                prerelease: i.prerelease,
                source: non_empty("source", i.source)?,
            }
        }
        INSTALL => {
            let i: InstallIn = input(value)?;
            NuGetRequest::Change {
                action: ChangeAction::Install,
                change: Change {
                    package: Some(required("package", i.package)?),
                    version: non_empty("version", i.version)?,
                    projects: projects(i.project, i.projects)?,
                    prerelease: i.prerelease,
                    source: non_empty("source", i.source)?,
                    all: false,
                },
            }
        }
        UNINSTALL => {
            let i: UninstallIn = input(value)?;
            NuGetRequest::Change {
                action: ChangeAction::Uninstall,
                change: Change {
                    package: Some(required("package", i.package)?),
                    projects: projects(i.project, i.projects)?,
                    ..Change::default()
                },
            }
        }
        UPDATE => {
            let i: UpdateIn = input(value)?;
            let all = i.all.unwrap_or(false);
            let package = non_empty("package", i.package)?;
            if package.is_none() && !all {
                return Err(invalid("`package` is required unless `all` is true"));
            }
            if package.is_some() && all {
                return Err(invalid("`package` and `all` exclude each other"));
            }
            let version = non_empty("version", i.version)?;
            if version.is_some() && all {
                return Err(invalid(
                    "`version` names one package's version; `all` takes each newest",
                ));
            }
            NuGetRequest::Change {
                action: ChangeAction::Update,
                change: Change {
                    package,
                    version,
                    projects: projects(i.project, None)?,
                    prerelease: i.prerelease,
                    source: None,
                    all,
                },
            }
        }
        CONSOLIDATE => {
            let i: ConsolidateIn = input(value)?;
            NuGetRequest::Change {
                action: ChangeAction::Consolidate,
                change: Change {
                    package: Some(required("package", i.package)?),
                    version: non_empty("version", i.version)?,
                    ..Change::default()
                },
            }
        }
        SOURCES => {
            let i: SourcesIn = input(value)?;
            let action = i.action.unwrap_or_default();
            let name = non_empty("name", i.name)?;
            let url = non_empty("url", i.url)?;
            match action {
                SourcesAction::List => {}
                SourcesAction::Add if name.is_none() || url.is_none() => {
                    return Err(invalid("`add` needs `name` and `url`"));
                }
                SourcesAction::Remove | SourcesAction::Enable | SourcesAction::Disable
                    if name.is_none() =>
                {
                    return Err(invalid("`name` is required"));
                }
                _ => {}
            }
            NuGetRequest::Sources { action, name, url }
        }
        RESTORE => {
            let i: RestoreIn = input(value)?;
            NuGetRequest::Restore {
                project: non_empty("project", i.project)?,
                force: i.force.unwrap_or(false),
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

/// The public description of command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission, agent_visible) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible,
    }
}

/// The escalation hook of NuGet command `id`, if its calls can escalate: the change commands under `nuget.change`;
/// `sources` with add, remove, enable or disable raised to execute and put under `nuget.sources`.
pub fn escalation(id: &'static str) -> Option<EscalationHook> {
    let change = CHANGES.contains(&id);
    if !change && id != SOURCES {
        return None;
    }
    let tool = id.replace('.', "-");
    Some(Arc::new(move |input: &Value, view: &PolicyView| {
        let sources = id == SOURCES
            && input
                .get("action")
                .and_then(Value::as_str)
                .is_some_and(|a| a != "list");
        if !change && !sources {
            return None;
        }
        let policy = view.nuget().decide_for(
            NuGetCall { change, sources },
            &view.policy().rules,
            &tool,
            input,
        );
        match policy {
            Some(e) => Some(e),
            // `allow`: the call keeps its class, raised to execute for a sources change.
            None if sources => Some(Escalation::raise(
                PermissionClass::Execute,
                "it changes the user's NuGet.config",
            )),
            None => None,
        }
    }))
}

/// Register every NuGet command, applying them to `target`, with their escalation hooks; the change commands keep
/// their arguments in the audit log for every caller.
pub fn register(registry: &CommandRegistry, target: Arc<dyn NuGetCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
    for id in CHANGES {
        registry.always_audit_arguments(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policy::{AgentPolicy, NuGetChangePolicy, NuGetPolicy, PolicySnapshot};
    use serde_json::json;

    fn schema(text: &str) -> Value {
        serde_json::from_str(text).unwrap()
    }

    /// Every member of `value` is a property of `schema`, and every required one is there.
    fn fits(value: &Value, schema: &Value) {
        for r in schema["required"].as_array().into_iter().flatten() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "missing {r}");
        }
        for k in value.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "unexpected {k}");
        }
    }

    #[test]
    fn parses_and_validates_every_command() {
        assert_eq!(
            parse(MANAGE, json!({"project": "App", "tab": "updates"})).unwrap(),
            NuGetRequest::Manage {
                project: Some("App".into()),
                solution: false,
                tab: Some(Tab::Updates),
                query: None
            }
        );
        assert!(parse(MANAGE, json!({"project": "App", "solution": true})).is_err());
        assert!(parse(MANAGE, json!({"tab": "nope"})).is_err());
        assert_eq!(
            parse(SEARCH, Value::Null).unwrap(),
            NuGetRequest::Search {
                query: String::new(),
                source: None,
                prerelease: None,
                max_items: DEFAULT_RESULTS,
                skip: 0
            }
        );
        assert!(parse(SEARCH, json!({"max_items": 0})).is_err());
        assert!(parse(SEARCH, json!({"max_items": 501})).is_err());
        assert!(parse(SEARCH, json!({"source": " "})).is_err());
        assert_eq!(
            parse(INSTALLED, json!({})).unwrap(),
            NuGetRequest::Installed {
                project: None,
                include_transitive: false,
                vulnerabilities: true
            }
        );
        let install = parse(
            INSTALL,
            json!({"package": " Newtonsoft.Json ", "project": "App", "projects": ["Lib"]}),
        )
        .unwrap();
        assert_eq!(
            install,
            NuGetRequest::Change {
                action: ChangeAction::Install,
                change: Change {
                    package: Some("Newtonsoft.Json".into()),
                    projects: vec!["App".into(), "Lib".into()],
                    ..Change::default()
                }
            }
        );
        assert_eq!(install.command(), INSTALL);
        assert!(parse(INSTALL, json!({})).is_err());
        assert!(parse(INSTALL, json!({"package": "P", "all": true})).is_err());
        assert!(parse(UNINSTALL, json!({"package": ""})).is_err());
        assert!(parse(UPDATE, json!({})).is_err());
        assert!(parse(UPDATE, json!({"package": "P", "all": true})).is_err());
        assert!(parse(UPDATE, json!({"all": true, "version": "1.0.0"})).is_err());
        let all = parse(UPDATE, json!({"all": true, "project": "App"})).unwrap();
        assert!(
            matches!(all, NuGetRequest::Change { change: Change { all: true, ref projects, .. }, .. } if projects == &["App".to_owned()])
        );
        assert!(parse(CONSOLIDATE, json!({"version": "1.0.0"})).is_err());
        assert_eq!(
            parse(SOURCES, json!({})).unwrap(),
            NuGetRequest::Sources {
                action: SourcesAction::List,
                name: None,
                url: None
            }
        );
        assert!(parse(SOURCES, json!({"action": "add", "name": "x"})).is_err());
        assert!(parse(SOURCES, json!({"action": "remove"})).is_err());
        assert!(parse(SOURCES, json!({"action": "add", "name": "x", "url": "u"})).is_ok());
        assert_eq!(
            parse(RESTORE, json!({"force": true})).unwrap(),
            NuGetRequest::Restore {
                project: None,
                force: true
            }
        );
        assert!(parse("eludite.nuget.nope", json!({})).is_err());
    }

    #[test]
    fn specs_and_outputs_follow_the_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
        }
        for id in [SEARCH, INSTALLED, UPDATES, SOURCES] {
            assert_eq!(spec(id).permission, PermissionClass::Read, "{id}");
        }
        for id in [INSTALL, UNINSTALL, UPDATE, CONSOLIDATE, RESTORE] {
            assert_eq!(spec(id).permission, PermissionClass::Execute, "{id}");
        }
        assert!(!spec(MANAGE).agent_visible);
        assert!(spec(INSTALL).agent_visible);

        let vuln = Vulnerability {
            severity: "high".into(),
            advisory_url: "https://github.com/advisories/GHSA-x".into(),
        };
        let search = NuGetOutput::Search(Box::new(SearchOutput {
            results: vec![SearchRow {
                id: "P".into(),
                version: "1.1.0".into(),
                versions: vec!["1.1.0".into(), "1.0.0".into()],
                description: Some("d".into()),
                authors: None,
                source: "corpus".into(),
                downloads: Some(3),
                license: Some("MIT".into()),
                project_url: None,
                icon_url: Some("https://example.invalid/i.png".into()),
                vulnerabilities: vec![vuln.clone()],
                deprecated: true,
            }],
            sources: vec![SourceRow {
                name: "down".into(),
                count: 0,
                error: Some("unreachable".into()),
            }],
            truncated: false,
        }))
        .to_json();
        let s = schema(schemas(SEARCH).2);
        fits(&search, &s);
        fits(&search["results"][0], &s["properties"]["results"]["items"]);
        fits(&search["sources"][0], &s["properties"]["sources"]["items"]);
        let installed = NuGetOutput::Installed(Box::new(InstalledOutput {
            projects: vec![InstalledProject {
                name: "App".into(),
                path: "/c/App/App.csproj".into(),
                restored: true,
                central_package_management: false,
                props_file: None,
                lock_file: Some("/c/App/packages.lock.json".into()),
                packages: vec![InstalledPackage {
                    id: "P".into(),
                    version: Some("1.0.0".into()),
                    requested: Some("1.0.0".into()),
                    source: Some("/c/feed".into()),
                    transitive: false,
                    auto_referenced: false,
                    vulnerabilities: vec![vuln],
                    deprecated: false,
                }],
                error: None,
                note: None,
            }],
        }))
        .to_json();
        let s = schema(schemas(INSTALLED).2);
        fits(&installed, &s);
        let project_schema = &s["properties"]["projects"]["items"];
        fits(&installed["projects"][0], project_schema);
        fits(
            &installed["projects"][0]["packages"][0],
            &project_schema["properties"]["packages"]["items"],
        );
        let restore = RestoreOutput {
            result: "failed".into(),
            errors: 1,
            warnings: 0,
            elapsed_ms: 900.0,
            locked_mode: false,
            lock_files: vec![],
            diagnostics: vec![RestoreDiagnostic {
                severity: "error".into(),
                code: "NU1102".into(),
                message: "Unable to find package".into(),
                path: Some("/c/Lib/Lib.csproj".into()),
                line: None,
                column: None,
            }],
        };
        let change = NuGetOutput::Change(Box::new(ChangeOutput {
            action: ChangeAction::Install,
            packages: vec![PackageVersion {
                id: "P".into(),
                version: Some("1.0.0".into()),
            }],
            projects: vec!["/c/App/App.csproj".into()],
            edited: vec!["/c/App/App.csproj".into()],
            restore: Some(restore.clone()),
            generation: 3,
            message: None,
        }))
        .to_json();
        for id in CHANGES {
            let s = schema(schemas(id).2);
            fits(&change, &s);
            fits(&change["restore"], &s["properties"]["restore"]);
        }
        assert_eq!(change["action"], "install");
        let r = NuGetOutput::Restore(Box::new(restore)).to_json();
        fits(&r, &schema(schemas(RESTORE).2));
        let updates = NuGetOutput::Updates(Box::new(UpdatesOutput {
            updates: vec![UpdateRow {
                project: "/c/App/App.csproj".into(),
                id: "P".into(),
                installed: "1.0.0".into(),
                latest: "1.1.0".into(),
                source: "corpus".into(),
                vulnerable: true,
            }],
            sources: vec![],
        }))
        .to_json();
        let s = schema(schemas(UPDATES).2);
        fits(&updates, &s);
        fits(&updates["updates"][0], &s["properties"]["updates"]["items"]);
        let sources = NuGetOutput::Sources(Box::new(SourcesOutput {
            sources: vec![SourceOut {
                name: "corpus".into(),
                url: "/c/feed".into(),
                enabled: true,
                local: true,
                scope: "solution".into(),
                config_file: Some("/c/NuGet.config".into()),
            }],
            user_config: "/u/NuGet.Config".into(),
            changed: false,
        }))
        .to_json();
        let s = schema(schemas(SOURCES).2);
        fits(&sources, &s);
        fits(&sources["sources"][0], &s["properties"]["sources"]["items"]);
    }

    fn view(policy: AgentPolicy) -> PolicyView {
        PolicyView::of(PolicySnapshot {
            policy,
            ..Default::default()
        })
    }

    #[test]
    fn the_hooks_apply_the_nuget_policy() {
        let hook = |id: &'static str, input: Value, policy: AgentPolicy| {
            escalation(id).expect("a hook")(&input, &view(policy))
        };
        assert!(escalation(SEARCH).is_none());
        assert!(escalation(RESTORE).is_none());
        // Defaults: a change asks (dangerous), a sources change too; listing sources does not escalate.
        assert!(matches!(
            hook(INSTALL, json!({"package": "P"}), AgentPolicy::default()),
            Some(Escalation::Raise { class: PermissionClass::Dangerous, reason, .. }) if reason.contains("nuget.change: prompt")
        ));
        assert!(matches!(
            hook(SOURCES, json!({"action": "add"}), AgentPolicy::default()),
            Some(Escalation::Raise { class: PermissionClass::Dangerous, reason, .. }) if reason.contains("nuget.sources: prompt")
        ));
        assert_eq!(hook(SOURCES, json!({}), AgentPolicy::default()), None);
        let allow = AgentPolicy {
            nuget: Some(NuGetPolicy {
                change: Some(NuGetChangePolicy::Allow),
                sources: None,
            }),
            ..Default::default()
        };
        assert_eq!(hook(UPDATE, json!({"all": true}), allow.clone()), None);
        let deny = AgentPolicy {
            nuget: Some(NuGetPolicy {
                change: Some(NuGetChangePolicy::Deny),
                sources: Some(crate::policy::GuardPolicy::Deny),
            }),
            ..Default::default()
        };
        assert!(matches!(
            hook(UNINSTALL, json!({"package": "P"}), deny.clone()),
            Some(Escalation::Refuse(r)) if r.contains("nuget.change")
        ));
        assert!(matches!(
            hook(SOURCES, json!({"action": "remove", "name": "x"}), deny),
            Some(Escalation::Refuse(r)) if r.contains("nuget.sources")
        ));
    }

    struct Echo;
    impl NuGetCommands for Echo {
        fn apply(&self, request: NuGetRequest) -> Result<NuGetOutput, CommandError> {
            match request {
                NuGetRequest::Sources { .. } => Ok(NuGetOutput::Sources(Box::new(SourcesOutput {
                    sources: vec![],
                    user_config: "/u".into(),
                    changed: false,
                }))),
                other => Err(CommandError::Failed(format!("{other:?}"))),
            }
        }
    }

    #[test]
    fn registers_routes_and_audits_changes() {
        let r = CommandRegistry::new();
        register(&r, Arc::new(Echo));
        assert_eq!(
            r.invoke(SOURCES, json!({})).unwrap(),
            json!({"sources": [], "user_config": "/u", "changed": false})
        );
        assert!(matches!(
            r.invoke(INSTALL, json!({"nope": 1})),
            Err(CommandError::InvalidInput(_))
        ));
        let _ = r.invoke(
            INSTALL,
            json!({"package": "P", "version": "1.0.0", "project": "App"}),
        );
        let entry = r.audit_log().entries().pop().unwrap();
        assert_eq!(entry.command, INSTALL);
        assert_eq!(
            entry.arguments,
            Some(json!({"package": "P", "version": "1.0.0", "project": "App"}))
        );
        assert!(r.has_escalation(SOURCES));
        assert!(r.has_escalation(CONSOLIDATE));
        assert!(!r.has_escalation(INSTALLED));
    }
}
