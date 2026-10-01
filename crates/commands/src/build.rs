//! Build and Output commands (brief 0017): `eludite.build.solution`, `eludite.build.project`, `eludite.build.rebuild`,
//! `eludite.build.clean`, `eludite.build.cancel`, `eludite.output.show` and `eludite.output.clear`.
//!
//! The schemas are the files in `protocol/schemas/` (checked in first, CLAUDE.md invariant 4). This module parses
//! input into a typed [`BuildRequest`] and serializes the typed [`BuildCommandOutput`]; the shell implements
//! [`BuildCommands`]. A build command invoked off the UI thread (an agent through MCP) waits for the build to finish
//! and returns its summary unless `wait` is false; on the UI thread (menus, keys) it never waits.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SOLUTION: &str = "eludite.build.solution";
pub const PROJECT: &str = "eludite.build.project";
pub const REBUILD: &str = "eludite.build.rebuild";
pub const CLEAN: &str = "eludite.build.clean";
pub const CANCEL: &str = "eludite.build.cancel";
pub const OUTPUT_SHOW: &str = "eludite.output.show";
pub const OUTPUT_CLEAR: &str = "eludite.output.clear";

/// The commands that start a build (disabled in the menus while one runs).
pub const STARTS: [&str; 4] = [SOLUTION, PROJECT, REBUILD, CLEAN];

pub const ALL: [&str; 7] = [
    SOLUTION,
    PROJECT,
    REBUILD,
    CLEAN,
    CANCEL,
    OUTPUT_SHOW,
    OUTPUT_CLEAR,
];

/// Diagnostics a build command's output lists at most (`build-result.output.json`).
pub const MAX_RESULT_DIAGNOSTICS: usize = 200;

/// Lines `eludite.output.show` returns at most.
pub const MAX_TAIL: u32 = 2000;

const RESULT: &str = include_str!("../../../protocol/schemas/build-result.output.json");

/// (title, input schema, output schema, permission, agent visible)
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
        // Builds run processes: execute (PLAN.md 5.3).
        SOLUTION => (
            "Build: Build Solution",
            include_str!("../../../protocol/schemas/build-solution.input.json"),
            RESULT,
            Execute,
            true,
        ),
        PROJECT => (
            "Build: Build Project",
            include_str!("../../../protocol/schemas/build-project.input.json"),
            RESULT,
            Execute,
            true,
        ),
        REBUILD => (
            "Build: Rebuild Solution",
            include_str!("../../../protocol/schemas/build-rebuild.input.json"),
            RESULT,
            Execute,
            true,
        ),
        CLEAN => (
            "Build: Clean Solution",
            include_str!("../../../protocol/schemas/build-clean.input.json"),
            RESULT,
            Execute,
            true,
        ),
        // Stopping a build only ends a process the IDE started; like canceling an agent's turn, it is not gated.
        CANCEL => (
            "Build: Cancel",
            include_str!("../../../protocol/schemas/build-cancel.input.json"),
            include_str!("../../../protocol/schemas/build-cancel.output.json"),
            Read,
            true,
        ),
        // Agents read the build log through it (PLAN.md 5.4, build output).
        OUTPUT_SHOW => (
            "View: Output",
            include_str!("../../../protocol/schemas/output-show.input.json"),
            include_str!("../../../protocol/schemas/output-show.output.json"),
            Read,
            true,
        ),
        // Clearing the window is a view action, like the Error List's filters.
        OUTPUT_CLEAR => (
            "Output: Clear All",
            include_str!("../../../protocol/schemas/output-clear.input.json"),
            include_str!("../../../protocol/schemas/output-clear.output.json"),
            Read,
            false,
        ),
        other => unreachable!("not a build command: {other}"),
    }
}

/// What a build does (`build`, `rebuild`, `clean`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BuildKind {
    Build,
    Rebuild,
    Clean,
}

impl BuildKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BuildKind::Build => "build",
            BuildKind::Rebuild => "rebuild",
            BuildKind::Clean => "clean",
        }
    }
}

/// The Output window's sources ("Show output from").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutputSource {
    Build,
    Host,
}

impl OutputSource {
    pub fn as_str(self) -> &'static str {
        match self {
            OutputSource::Build => "build",
            OutputSource::Host => "host",
        }
    }
}

/// A parsed, validated build or output command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BuildRequest {
    Start {
        kind: BuildKind,
        /// `None`: the solution. `Some(None)`: the active document's project. `Some(Some(p))`: that project (a name
        /// or a path).
        project: Option<Option<String>>,
        configuration: Option<String>,
        platform: Option<String>,
        /// `None`: wait when invoked off the UI thread.
        wait: Option<bool>,
    },
    Cancel,
    OutputShow {
        source: Option<OutputSource>,
        tail: u32,
    },
    OutputClear {
        source: Option<OutputSource>,
    },
}

impl BuildRequest {
    pub fn command(&self) -> &'static str {
        match self {
            BuildRequest::Start {
                project: Some(_), ..
            } => PROJECT,
            BuildRequest::Start {
                kind: BuildKind::Rebuild,
                ..
            } => REBUILD,
            BuildRequest::Start {
                kind: BuildKind::Clean,
                ..
            } => CLEAN,
            BuildRequest::Start { .. } => SOLUTION,
            BuildRequest::Cancel => CANCEL,
            BuildRequest::OutputShow { .. } => OUTPUT_SHOW,
            BuildRequest::OutputClear { .. } => OUTPUT_CLEAR,
        }
    }
}

/// A diagnostic of `build-result.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultDiagnostic {
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
}

/// A project of `build-result.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ResultProject {
    pub name: String,
    /// `succeeded`, `failed` or `canceled`.
    pub result: String,
    pub errors: u32,
    pub warnings: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
}

/// `build-result.output.json`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildResultOutput {
    /// `running`, `succeeded`, `failed` or `canceled`.
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<u64>,
    pub target: BuildKind,
    pub path: String,
    pub configuration: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub toolchain: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub errors: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub warnings: Option<u32>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<ResultProject>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub diagnostics: Vec<ResultDiagnostic>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// `build-cancel.output.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CancelOutput {
    pub canceled: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_id: Option<u64>,
}

/// `output-show.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputShowOutput {
    pub source: OutputSource,
    pub line_count: u64,
    pub following: bool,
    pub tail: Vec<String>,
}

/// `output-clear.output.json`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OutputClearOutput {
    pub source: OutputSource,
    pub cleared: u64,
}

#[derive(Debug, Clone, PartialEq)]
pub enum BuildCommandOutput {
    Result(Box<BuildResultOutput>),
    Cancel(CancelOutput),
    OutputShow(OutputShowOutput),
    OutputClear(OutputClearOutput),
}

impl BuildCommandOutput {
    pub fn to_json(&self) -> Value {
        match self {
            BuildCommandOutput::Result(o) => serde_json::to_value(o),
            BuildCommandOutput::Cancel(o) => serde_json::to_value(o),
            BuildCommandOutput::OutputShow(o) => serde_json::to_value(o),
            BuildCommandOutput::OutputClear(o) => serde_json::to_value(o),
        }
        .expect("build outputs serialize")
    }
}

/// Whatever runs builds and owns the Output window (the shell). Called on the invoking thread.
pub trait BuildCommands: Send + Sync {
    fn apply(&self, request: BuildRequest) -> Result<BuildCommandOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct SolutionIn {
    configuration: Option<String>,
    platform: Option<String>,
    wait: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ProjectIn {
    project: Option<String>,
    target: Option<BuildKind>,
    configuration: Option<String>,
    platform: Option<String>,
    wait: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ShowIn {
    source: Option<OutputSource>,
    tail: Option<u32>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct ClearIn {
    source: Option<OutputSource>,
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn non_empty(name: &str, v: Option<String>) -> Result<Option<String>, CommandError> {
    match v {
        Some(s) if s.trim().is_empty() => Err(CommandError::InvalidInput(format!(
            "`{name}` must not be empty"
        ))),
        other => Ok(other),
    }
}

/// Parse and validate the input of build or output command `id`.
pub fn parse(id: &str, value: Value) -> Result<BuildRequest, CommandError> {
    let solution = |kind, value| -> Result<BuildRequest, CommandError> {
        let i: SolutionIn = input(value)?;
        Ok(BuildRequest::Start {
            kind,
            project: None,
            configuration: non_empty("configuration", i.configuration)?,
            platform: non_empty("platform", i.platform)?,
            wait: i.wait,
        })
    };
    Ok(match id {
        SOLUTION => solution(BuildKind::Build, value)?,
        REBUILD => solution(BuildKind::Rebuild, value)?,
        CLEAN => solution(BuildKind::Clean, value)?,
        PROJECT => {
            let i: ProjectIn = input(value)?;
            BuildRequest::Start {
                kind: i.target.unwrap_or(BuildKind::Build),
                project: Some(non_empty("project", i.project)?),
                configuration: non_empty("configuration", i.configuration)?,
                platform: non_empty("platform", i.platform)?,
                wait: i.wait,
            }
        }
        CANCEL => {
            let _: Empty = input(value)?;
            BuildRequest::Cancel
        }
        OUTPUT_SHOW => {
            let i: ShowIn = input(value)?;
            let tail = i.tail.unwrap_or(50);
            if tail > MAX_TAIL {
                return Err(CommandError::InvalidInput(format!(
                    "`tail` is at most {MAX_TAIL}"
                )));
            }
            BuildRequest::OutputShow {
                source: i.source,
                tail,
            }
        }
        OUTPUT_CLEAR => {
            let i: ClearIn = input(value)?;
            BuildRequest::OutputClear { source: i.source }
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

/// Register the build and output commands, applying them to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn BuildCommands>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_validates_and_specs_follow_the_schemas() {
        assert_eq!(
            parse(SOLUTION, json!({})).unwrap(),
            BuildRequest::Start {
                kind: BuildKind::Build,
                project: None,
                configuration: None,
                platform: None,
                wait: None
            }
        );
        assert_eq!(
            parse(
                REBUILD,
                json!({"configuration": "Release", "platform": "Any CPU", "wait": false})
            )
            .unwrap(),
            BuildRequest::Start {
                kind: BuildKind::Rebuild,
                project: None,
                configuration: Some("Release".into()),
                platform: Some("Any CPU".into()),
                wait: Some(false)
            }
        );
        assert_eq!(parse(CLEAN, Value::Null).unwrap().command(), CLEAN);
        assert_eq!(
            parse(
                PROJECT,
                json!({"project": "Eludite.Host", "target": "clean"})
            )
            .unwrap(),
            BuildRequest::Start {
                kind: BuildKind::Clean,
                project: Some(Some("Eludite.Host".into())),
                configuration: None,
                platform: None,
                wait: None
            }
        );
        assert_eq!(parse(PROJECT, json!({})).unwrap().command(), PROJECT);
        assert!(parse(SOLUTION, json!({"configuration": ""})).is_err());
        assert!(parse(SOLUTION, json!({"project": "A"})).is_err());
        assert!(parse(PROJECT, json!({"target": "publish"})).is_err());
        assert_eq!(parse(CANCEL, json!({})).unwrap(), BuildRequest::Cancel);
        assert!(parse(CANCEL, json!({"id": 1})).is_err());
        assert_eq!(
            parse(OUTPUT_SHOW, json!({"source": "host"})).unwrap(),
            BuildRequest::OutputShow {
                source: Some(OutputSource::Host),
                tail: 50
            }
        );
        assert!(parse(OUTPUT_SHOW, json!({"tail": 2001})).is_err());
        assert!(parse(OUTPUT_SHOW, json!({"source": "debug"})).is_err());
        assert_eq!(
            parse(OUTPUT_CLEAR, json!({})).unwrap(),
            BuildRequest::OutputClear { source: None }
        );

        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.input_schema["type"], "object");
        }
        for id in STARTS {
            assert_eq!(spec(id).permission, PermissionClass::Execute);
            assert!(spec(id).agent_visible);
        }
        assert!(!spec(OUTPUT_CLEAR).agent_visible);
        assert!(spec(OUTPUT_SHOW).agent_visible);

        // Outputs serialize to their schemas' members.
        let result = BuildCommandOutput::Result(Box::new(BuildResultOutput {
            state: "failed".into(),
            build_id: Some(1),
            target: BuildKind::Build,
            path: "/s/A.slnx".into(),
            configuration: "Debug".into(),
            platform: None,
            toolchain: Some("dotnet".into()),
            elapsed_ms: Some(1.0),
            errors: Some(1),
            warnings: Some(0),
            projects: vec![ResultProject {
                name: "A".into(),
                result: "failed".into(),
                errors: 1,
                warnings: 0,
                elapsed_ms: None,
            }],
            diagnostics: vec![ResultDiagnostic {
                severity: "error".into(),
                code: "CS0103".into(),
                message: "x".into(),
                path: Some("A/Program.cs".into()),
                line: Some(1),
                column: Some(2),
                project: Some("A".into()),
            }],
            message: None,
        }))
        .to_json();
        let schema: Value = serde_json::from_str(RESULT).unwrap();
        for r in schema["required"].as_array().unwrap() {
            assert!(result.get(r.as_str().unwrap()).is_some(), "{r}");
        }
        for k in result.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let diag_props = &schema["properties"]["diagnostics"]["items"]["properties"];
        for k in result["diagnostics"][0].as_object().unwrap().keys() {
            assert!(diag_props.get(k).is_some(), "{k}");
        }
        let show = BuildCommandOutput::OutputShow(OutputShowOutput {
            source: OutputSource::Build,
            line_count: 2,
            following: true,
            tail: vec!["a".into()],
        })
        .to_json();
        assert_eq!(show["source"], "build");
        let schema: Value = serde_json::from_str(schemas(OUTPUT_SHOW).2).unwrap();
        for r in schema["required"].as_array().unwrap() {
            assert!(show.get(r.as_str().unwrap()).is_some(), "{r}");
        }
    }

    struct Echo;
    impl BuildCommands for Echo {
        fn apply(&self, request: BuildRequest) -> Result<BuildCommandOutput, CommandError> {
            Ok(match request {
                BuildRequest::Cancel => BuildCommandOutput::Cancel(CancelOutput {
                    canceled: false,
                    build_id: None,
                }),
                BuildRequest::OutputClear { source } => {
                    BuildCommandOutput::OutputClear(OutputClearOutput {
                        source: source.unwrap_or(OutputSource::Build),
                        cleared: 3,
                    })
                }
                other => return Err(CommandError::Failed(format!("{other:?}"))),
            })
        }
    }

    #[test]
    fn registers_and_routes_to_the_target() {
        let r = CommandRegistry::new();
        register(&r, Arc::new(Echo));
        assert_eq!(
            r.invoke(CANCEL, json!({})).unwrap(),
            json!({"canceled": false})
        );
        assert_eq!(
            r.invoke(OUTPUT_CLEAR, json!({"source": "host"})).unwrap(),
            json!({"source": "host", "cleared": 3})
        );
        assert!(matches!(
            r.invoke(SOLUTION, json!({"nope": 1})),
            Err(CommandError::InvalidInput(_))
        ));
        assert_eq!(
            r.lookup(SOLUTION).unwrap().permission,
            PermissionClass::Execute
        );
    }
}
