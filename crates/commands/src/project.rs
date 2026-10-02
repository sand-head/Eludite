//! The Workspace window's context menu commands that are not builds (brief 0020): `eludite.workspace.set_startup_project`
//! (Set as Startup Project) and `eludite.workspace.open_containing_folder` (Open Containing Folder). The menu's Build,
//! Rebuild and Clean run `eludite.build.project` with the project and a target. The shell implements
//! [`ProjectTarget`].

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
    OpenContainingFolder {
        path: String,
    },
}

/// `workspace-set-startup-project.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StartupProjectOutput {
    pub project: String,
    pub path: String,
    pub solution: String,
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

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartupInput {
    project: String,
}

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
            let i: StartupInput = serde_json::from_value(input).map_err(bad)?;
            empty(&i.project, "project")?;
            Ok(ProjectRequest::SetStartupProject { project: i.project })
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
        assert!(parse(SET_STARTUP_PROJECT, json!({})).is_err());
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
        })
        .to_json();
        let schema = spec(SET_STARTUP_PROJECT).output_schema;
        for k in out.as_object().unwrap().keys() {
            assert!(schema["properties"].get(k).is_some(), "{k}");
        }
    }
}
