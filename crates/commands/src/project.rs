//! The Workspace window's context menu commands that are not builds (brief 0020): `eludite.workspace.set_startup_project`
//! (Set as Startup Project) and `eludite.workspace.open_containing_folder` (Open Containing Folder). The menu's Build,
//! Rebuild and Clean run `eludite.build.project` with the project and a target. The shell implements
//! [`ProjectTarget`].
//!
//! Brief 0028: `set_startup_project` with `projects` sets Visual Studio's multiple startup projects, each with an
//! action ([`StartupAction`]); from the UI without arguments it opens the Startup Projects dialog
//! ([`ProjectRequest::StartupProjectsDialog`]).

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SET_STARTUP_PROJECT: &str = "eludite.workspace.set_startup_project";
pub const OPEN_CONTAINING_FOLDER: &str = "eludite.workspace.open_containing_folder";

pub const ALL: [&str; 2] = [SET_STARTUP_PROJECT, OPEN_CONTAINING_FOLDER];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectRequest {
    /// A project name or project file path (absolute or relative to the solution folder).
    SetStartupProject {
        project: String,
    },
    /// Multiple startup projects (brief 0028): each project with its action.
    SetStartupProjects {
        projects: Vec<StartupEntry>,
    },
    /// Neither `project` nor `projects`: from the UI, Project > Set Startup Projects... opens the dialog.
    StartupProjectsDialog,
    OpenContainingFolder {
        path: String,
    },
}

/// What a startup project does on F5 (Visual Studio's Action column).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StartupAction {
    Start,
    StartWithoutDebugging,
    None,
}

impl StartupAction {
    pub fn as_str(self) -> &'static str {
        match self {
            StartupAction::Start => "start",
            StartupAction::StartWithoutDebugging => "start_without_debugging",
            StartupAction::None => "none",
        }
    }

    /// How the Startup Projects dialog names it.
    pub fn label(self) -> &'static str {
        match self {
            StartupAction::Start => "Start",
            StartupAction::StartWithoutDebugging => "Start without debugging",
            StartupAction::None => "None",
        }
    }
}

/// One of the multiple startup projects as given (a name or a path).
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupEntry {
    pub project: String,
    pub action: StartupAction,
}

/// One startup project of the output: its name, path and action.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupProjectRow {
    pub project: String,
    pub path: String,
    pub action: StartupAction,
}

/// `workspace-set-startup-project.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupProjectOutput {
    pub project: String,
    pub path: String,
    pub solution: String,
    /// With multiple startup projects: those that start, in solution order (brief 0028).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub projects: Vec<StartupProjectRow>,
}

/// `workspace-open-containing-folder.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ContainingFolderOutput {
    pub folder: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ProjectOutput {
    StartupProject(StartupProjectOutput),
    ContainingFolder(ContainingFolderOutput),
}

impl ProjectOutput {
    pub fn to_json(&self) -> Value {
        match self {
            ProjectOutput::StartupProject(o) => serde_json::to_value(o),
            ProjectOutput::ContainingFolder(o) => serde_json::to_value(o),
        }
        .expect("project outputs serialize")
    }
}

/// What the shell implements.
pub trait ProjectTarget: Send + Sync {
    fn apply(&self, request: ProjectRequest) -> Result<ProjectOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct StartupInput {
    project: Option<String>,
    projects: Option<Vec<StartupEntry>>,
}

/// The most projects `projects` lists.
pub const MAX_STARTUP_PROJECTS: usize = 100;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderInput {
    path: String,
}

pub fn parse(id: &str, input: Value) -> Result<ProjectRequest, CommandError> {
    let bad = |e: serde_json::Error| CommandError::InvalidInput(e.to_string());
    let empty = |s: &str, what: &str| {
        if s.is_empty() {
            Err(CommandError::InvalidInput(format!("{what} is empty")))
        } else {
            Ok(())
        }
    };
    match id {
        SET_STARTUP_PROJECT => {
            let i: StartupInput = if input.is_null() {
                StartupInput::default()
            } else {
                serde_json::from_value(input).map_err(bad)?
            };
            match (i.project, i.projects) {
                (Some(_), Some(_)) => Err(CommandError::InvalidInput(
                    "give `project` (one startup project) or `projects` (multiple), not both"
                        .into(),
                )),
                (Some(project), None) => {
                    empty(&project, "project")?;
                    Ok(ProjectRequest::SetStartupProject { project })
                }
                (None, Some(projects)) => {
                    if projects.is_empty() || projects.len() > MAX_STARTUP_PROJECTS {
                        return Err(CommandError::InvalidInput(format!(
                            "`projects` lists 1 to {MAX_STARTUP_PROJECTS} projects"
                        )));
                    }
                    for p in &projects {
                        empty(&p.project, "a project")?;
                    }
                    if projects.iter().all(|p| p.action == StartupAction::None) {
                        return Err(CommandError::InvalidInput(
                            "at least one project must start (`start` or `start_without_debugging`)".into(),
                        ));
                    }
                    Ok(ProjectRequest::SetStartupProjects { projects })
                }
                (None, None) => Ok(ProjectRequest::StartupProjectsDialog),
            }
        }
        OPEN_CONTAINING_FOLDER => {
            let i: FolderInput = serde_json::from_value(input).map_err(bad)?;
            empty(&i.path, "path")?;
            if !std::path::Path::new(&i.path).is_absolute() {
                return Err(CommandError::InvalidInput(format!(
                    "{} is not an absolute path",
                    i.path
                )));
            }
            Ok(ProjectRequest::OpenContainingFolder { path: i.path })
        }
        other => Err(CommandError::InvalidInput(format!(
            "not a workspace project command: {other}"
        ))),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission, agent_visible) = match id {
        // It changes what F5 builds and runs (kept per solution): the edit class.
        SET_STARTUP_PROJECT => (
            "Workspace: Set as Startup Project",
            include_str!("../../../protocol/schemas/workspace-set-startup-project.input.json"),
            include_str!("../../../protocol/schemas/workspace-set-startup-project.output.json"),
            PermissionClass::EditBuffer,
            true,
        ),
        // It starts the system's file manager on the user's screen: not for agents.
        OPEN_CONTAINING_FOLDER => (
            "Workspace: Open Containing Folder",
            include_str!("../../../protocol/schemas/workspace-open-containing-folder.input.json"),
            include_str!("../../../protocol/schemas/workspace-open-containing-folder.output.json"),
            PermissionClass::Execute,
            false,
        ),
        other => unreachable!("not a workspace project command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible,
    }
}

pub fn register(registry: &CommandRegistry, target: Arc<dyn ProjectTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|o| o.to_json())
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_and_specs_follow_the_schemas() {
        assert_eq!(
            parse(SET_STARTUP_PROJECT, json!({"project": "Eludite.Host"})).unwrap(),
            ProjectRequest::SetStartupProject {
                project: "Eludite.Host".into()
            }
        );
        // Neither: the dialog (the shell refuses it for agents).
        assert_eq!(
            parse(SET_STARTUP_PROJECT, json!({})).unwrap(),
            ProjectRequest::StartupProjectsDialog
        );
        assert!(parse(SET_STARTUP_PROJECT, json!({"project": ""})).is_err());
        assert!(parse(OPEN_CONTAINING_FOLDER, json!({"path": "relative/a.cs"})).is_err());
        let abs = if cfg!(windows) {
            "C:\\s\\a.cs"
        } else {
            "/s/a.cs"
        };
        assert!(parse(OPEN_CONTAINING_FOLDER, json!({"path": abs})).is_ok());
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
        }
        let out = ProjectOutput::StartupProject(StartupProjectOutput {
            project: "A".into(),
            path: "/s/A/A.csproj".into(),
            solution: "/s/S.slnx".into(),
            projects: Vec::new(),
        })
        .to_json();
        let schema = spec(SET_STARTUP_PROJECT).output_schema;
        for k in out.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
    }

    /// Brief 0028: multiple startup projects with their actions, and the output's list.
    #[test]
    fn multiple_startup_projects_parse_and_follow_the_schemas() {
        let r = parse(
            SET_STARTUP_PROJECT,
            json!({"projects": [
                {"project": "App", "action": "start"},
                {"project": "src/Web/Web.csproj", "action": "start_without_debugging"},
                {"project": "Tool", "action": "none"}
            ]}),
        )
        .unwrap();
        assert_eq!(
            r,
            ProjectRequest::SetStartupProjects {
                projects: vec![
                    StartupEntry {
                        project: "App".into(),
                        action: StartupAction::Start
                    },
                    StartupEntry {
                        project: "src/Web/Web.csproj".into(),
                        action: StartupAction::StartWithoutDebugging
                    },
                    StartupEntry {
                        project: "Tool".into(),
                        action: StartupAction::None
                    },
                ]
            }
        );
        for bad in [
            json!({"projects": []}),
            json!({"projects": [{"project": "App", "action": "none"}]}),
            json!({"projects": [{"project": "App", "action": "run"}]}),
            json!({"projects": [{"project": "", "action": "start"}]}),
            json!({"projects": [{"project": "App"}]}),
            json!({"project": "App", "projects": [{"project": "App", "action": "start"}]}),
        ] {
            assert!(parse(SET_STARTUP_PROJECT, bad.clone()).is_err(), "{bad}");
        }
        let schema = spec(SET_STARTUP_PROJECT).input_schema;
        assert!(schema.get("required").is_none());
        assert_eq!(
            schema["properties"]["projects"]["items"]["properties"]["action"]["enum"],
            json!(["start", "start_without_debugging", "none"])
        );
        let out = ProjectOutput::StartupProject(StartupProjectOutput {
            project: "App".into(),
            path: "/s/App/App.csproj".into(),
            solution: "/s/S.slnx".into(),
            projects: vec![
                StartupProjectRow {
                    project: "App".into(),
                    path: "/s/App/App.csproj".into(),
                    action: StartupAction::Start,
                },
                StartupProjectRow {
                    project: "Web".into(),
                    path: "/s/Web/Web.csproj".into(),
                    action: StartupAction::StartWithoutDebugging,
                },
            ],
        })
        .to_json();
        let schema = spec(SET_STARTUP_PROJECT).output_schema;
        for k in out.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
        let actions = &schema["properties"]["projects"]["items"]["properties"]["action"]["enum"];
        for p in out["projects"].as_array().unwrap() {
            assert!(actions.as_array().unwrap().contains(&p["action"]));
            for k in p.as_object().unwrap().keys() {
                assert!(
                    schema["properties"]["projects"]["items"]["properties"]
                        .get(k)
                        .is_some(),
                    "{k}"
                );
            }
        }
        assert_eq!(
            StartupAction::StartWithoutDebugging.label(),
            "Start without debugging"
        );
    }
}
