//! Solution structure, references, item properties and unloaded projects (brief 0081).
//!
//! - **The solution file:** `eludite.solution.add_project`, `remove_project`, `add_folder`, `rename_folder`,
//!   `remove_folder`, `add_item`, `remove_item` and `set_project_dependencies` (execute: they write the `.sln` or
//!   `.slnx`), answered with [`SolutionEditOutput`].
//! - **File Properties:** `eludite.project.item_properties` (read) and `eludite.project.set_item_properties`
//!   (execute), answered with [`ItemPropertiesOutput`].
//! - **References:** `eludite.project.add_reference` and `remove_reference` (execute) and
//!   `eludite.project.reference_manager` (the dialog; not for agents), answered with [`ReferencesOutput`].
//! - **Unload and Reload Project:** `eludite.project.unload` and `eludite.project.reload` (execute), answered with
//!   [`UnloadOutput`].
//!
//! The shell implements [`StructureTarget`]; it resolves names to paths and talks to the host.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const ADD_PROJECT: &str = "eludite.solution.add_project";
pub const REMOVE_PROJECT: &str = "eludite.solution.remove_project";
pub const ADD_FOLDER: &str = "eludite.solution.add_folder";
pub const RENAME_FOLDER: &str = "eludite.solution.rename_folder";
pub const REMOVE_FOLDER: &str = "eludite.solution.remove_folder";
pub const ADD_ITEM: &str = "eludite.solution.add_item";
pub const REMOVE_ITEM: &str = "eludite.solution.remove_item";
pub const SET_PROJECT_DEPENDENCIES: &str = "eludite.solution.set_project_dependencies";
pub const ITEM_PROPERTIES: &str = "eludite.project.item_properties";
pub const SET_ITEM_PROPERTIES: &str = "eludite.project.set_item_properties";
pub const ADD_REFERENCE: &str = "eludite.project.add_reference";
pub const REMOVE_REFERENCE: &str = "eludite.project.remove_reference";
pub const REFERENCE_MANAGER: &str = "eludite.project.reference_manager";
pub const UNLOAD: &str = "eludite.project.unload";
pub const RELOAD: &str = "eludite.project.reload";

/// Every command of brief 0081.
pub const ALL: [&str; 15] = [
    ADD_PROJECT,
    REMOVE_PROJECT,
    ADD_FOLDER,
    RENAME_FOLDER,
    REMOVE_FOLDER,
    ADD_ITEM,
    REMOVE_ITEM,
    SET_PROJECT_DEPENDENCIES,
    ITEM_PROPERTIES,
    SET_ITEM_PROPERTIES,
    ADD_REFERENCE,
    REMOVE_REFERENCE,
    REFERENCE_MANAGER,
    UNLOAD,
    RELOAD,
];

/// Characters a solution folder name cannot hold (Visual Studio's rule).
pub const FOLDER_NAME_FORBIDDEN: &[char] = &['/', '\\', ':', '*', '?', '"', '<', '>', '|'];

/// Copy to Output Directory, as the commands spell it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CopyToOutput {
    #[default]
    Never,
    Always,
    PreserveNewest,
}

impl CopyToOutput {
    /// Visual Studio's label.
    pub fn label(self) -> &'static str {
        match self {
            CopyToOutput::Never => "Do not copy",
            CopyToOutput::Always => "Copy always",
            CopyToOutput::PreserveNewest => "Copy if newer",
        }
    }

    pub const ALL: [CopyToOutput; 3] = [
        CopyToOutput::Never,
        CopyToOutput::Always,
        CopyToOutput::PreserveNewest,
    ];
}

/// A project or an assembly reference.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferenceKind {
    Project,
    Assembly,
}

/// A parsed, validated request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureRequest {
    /// `project: None`: from the UI, the file dialog asks first.
    AddProject {
        project: Option<String>,
        folder: Option<String>,
    },
    RemoveProject {
        project: String,
    },
    /// `name: None`: Visual Studio's `NewFolder1`.
    AddFolder {
        name: Option<String>,
        parent: Option<String>,
    },
    RenameFolder {
        folder: String,
        name: String,
    },
    RemoveFolder {
        folder: String,
    },
    /// `path: None`: from the UI, the file dialog asks first.
    AddItem {
        folder: String,
        path: Option<String>,
    },
    RemoveItem {
        folder: String,
        path: String,
    },
    SetProjectDependencies {
        project: String,
        depends_on: Vec<String>,
    },
    ItemProperties {
        project: Option<String>,
        path: String,
    },
    SetItemProperties(SetItemPropertiesInput),
    AddReference {
        project: String,
        reference: String,
        kind: Option<ReferenceKind>,
    },
    RemoveReference {
        project: String,
        reference: String,
        kind: Option<ReferenceKind>,
    },
    ReferenceManager {
        project: Option<String>,
    },
    Unload {
        project: String,
    },
    Reload {
        project: String,
    },
}

impl StructureRequest {
    pub fn command(&self) -> &'static str {
        match self {
            StructureRequest::AddProject { .. } => ADD_PROJECT,
            StructureRequest::RemoveProject { .. } => REMOVE_PROJECT,
            StructureRequest::AddFolder { .. } => ADD_FOLDER,
            StructureRequest::RenameFolder { .. } => RENAME_FOLDER,
            StructureRequest::RemoveFolder { .. } => REMOVE_FOLDER,
            StructureRequest::AddItem { .. } => ADD_ITEM,
            StructureRequest::RemoveItem { .. } => REMOVE_ITEM,
            StructureRequest::SetProjectDependencies { .. } => SET_PROJECT_DEPENDENCIES,
            StructureRequest::ItemProperties { .. } => ITEM_PROPERTIES,
            StructureRequest::SetItemProperties(_) => SET_ITEM_PROPERTIES,
            StructureRequest::AddReference { .. } => ADD_REFERENCE,
            StructureRequest::RemoveReference { .. } => REMOVE_REFERENCE,
            StructureRequest::ReferenceManager { .. } => REFERENCE_MANAGER,
            StructureRequest::Unload { .. } => UNLOAD,
            StructureRequest::Reload { .. } => RELOAD,
        }
    }
}

/// `eludite.project.set_item_properties` input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SetItemPropertiesInput {
    pub path: String,
    pub project: Option<String>,
    pub build_action: Option<String>,
    pub copy_to_output: Option<CopyToOutput>,
    pub custom_tool: Option<String>,
    pub custom_tool_namespace: Option<String>,
}

/// `solution-edit.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SolutionEditOutput {
    pub path: String,
    pub written: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// `project-item-properties.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ItemPropertiesOutput {
    pub project: String,
    pub project_path: String,
    pub path: String,
    #[serde(default)]
    pub listed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub build_action: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub build_actions: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub copy_to_output: Option<CopyToOutput>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_tool: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub custom_tool_namespace: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub implicit: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub written: Option<bool>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// A refused reference.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefusedRow {
    pub reference: String,
    pub reason: String,
}

/// `project-references.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReferencesOutput {
    pub project: String,
    pub path: String,
    pub written: bool,
    pub refused: Vec<RefusedRow>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub opened: bool,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// `project-unload.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UnloadOutput {
    pub project: String,
    pub path: String,
    pub unloaded: Vec<String>,
    pub generation: u64,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub pending: bool,
}

/// The answer to a [`StructureRequest`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StructureOutput {
    Solution(SolutionEditOutput),
    Item(ItemPropertiesOutput),
    References(ReferencesOutput),
    Unload(UnloadOutput),
}

impl StructureOutput {
    pub fn to_json(&self) -> Value {
        match self {
            StructureOutput::Solution(o) => serde_json::to_value(o),
            StructureOutput::Item(o) => serde_json::to_value(o),
            StructureOutput::References(o) => serde_json::to_value(o),
            StructureOutput::Unload(o) => serde_json::to_value(o),
        }
        .expect("structure outputs serialize")
    }
}

/// What the shell implements, for the fifteen commands.
pub trait StructureTarget: Send + Sync {
    fn apply(&self, request: StructureRequest) -> Result<StructureOutput, CommandError>;
}

fn bad(e: impl std::fmt::Display) -> CommandError {
    CommandError::InvalidInput(e.to_string())
}

fn object<T: serde::de::DeserializeOwned + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(bad)
}

fn required<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CommandError> {
    serde_json::from_value(value).map_err(bad)
}

fn text(what: &str, s: String) -> Result<String, CommandError> {
    if s.trim().is_empty() {
        Err(bad(format!("`{what}` is empty")))
    } else {
        Ok(s)
    }
}

fn opt_text(what: &str, s: Option<String>) -> Result<Option<String>, CommandError> {
    s.map(|s| text(what, s)).transpose()
}

/// Visual Studio's rule for a solution folder name.
pub fn check_folder_name(name: &str) -> Result<(), CommandError> {
    if name.trim().is_empty() {
        return Err(bad("the folder name is empty"));
    }
    if name.contains(FOLDER_NAME_FORBIDDEN) {
        return Err(bad(format!(
            "a solution folder name cannot hold any of {}",
            FOLDER_NAME_FORBIDDEN.iter().collect::<String>()
        )));
    }
    if name.trim() != name || name == "." || name == ".." {
        return Err(bad(format!("`{name}` is not a valid folder name")));
    }
    Ok(())
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AddProjectIn {
    project: Option<String>,
    folder: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectIn {
    project: String,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OptProjectIn {
    project: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AddFolderIn {
    name: Option<String>,
    parent: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RenameFolderIn {
    folder: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FolderIn {
    folder: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemIn {
    folder: String,
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DependenciesIn {
    project: String,
    depends_on: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathIn {
    path: String,
    project: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ReferenceIn {
    project: String,
    reference: String,
    kind: Option<ReferenceKind>,
}

/// Parse and validate command `id` of brief 0081.
pub fn parse(id: &str, value: Value) -> Result<StructureRequest, CommandError> {
    Ok(match id {
        ADD_PROJECT => {
            let i: AddProjectIn = object(value)?;
            StructureRequest::AddProject {
                project: opt_text("project", i.project)?,
                folder: opt_text("folder", i.folder)?,
            }
        }
        REMOVE_PROJECT => {
            let i: ProjectIn = required(value)?;
            StructureRequest::RemoveProject {
                project: text("project", i.project)?,
            }
        }
        ADD_FOLDER => {
            let i: AddFolderIn = object(value)?;
            if let Some(n) = &i.name {
                check_folder_name(n)?;
            }
            StructureRequest::AddFolder {
                name: i.name,
                parent: opt_text("parent", i.parent)?,
            }
        }
        RENAME_FOLDER => {
            let i: RenameFolderIn = required(value)?;
            check_folder_name(&i.name)?;
            StructureRequest::RenameFolder {
                folder: text("folder", i.folder)?,
                name: i.name,
            }
        }
        REMOVE_FOLDER => {
            let i: FolderIn = required(value)?;
            StructureRequest::RemoveFolder {
                folder: text("folder", i.folder)?,
            }
        }
        ADD_ITEM => {
            let i: ItemIn = required(value)?;
            StructureRequest::AddItem {
                folder: text("folder", i.folder)?,
                path: opt_text("path", i.path)?,
            }
        }
        REMOVE_ITEM => {
            let i: ItemIn = required(value)?;
            StructureRequest::RemoveItem {
                folder: text("folder", i.folder)?,
                path: i
                    .path
                    .ok_or_else(|| bad("`path` is required"))
                    .and_then(|p| text("path", p))?,
            }
        }
        SET_PROJECT_DEPENDENCIES => {
            let i: DependenciesIn = required(value)?;
            if i.depends_on.len() > 1000 {
                return Err(bad("`depends_on` lists at most 1000 projects"));
            }
            StructureRequest::SetProjectDependencies {
                project: text("project", i.project)?,
                depends_on: i
                    .depends_on
                    .into_iter()
                    .map(|d| text("a project of depends_on", d))
                    .collect::<Result<_, _>>()?,
            }
        }
        ITEM_PROPERTIES => {
            let i: PathIn = required(value)?;
            StructureRequest::ItemProperties {
                project: opt_text("project", i.project)?,
                path: text("path", i.path)?,
            }
        }
        SET_ITEM_PROPERTIES => {
            let i: SetItemPropertiesInput = required(value)?;
            if i.build_action.is_none()
                && i.copy_to_output.is_none()
                && i.custom_tool.is_none()
                && i.custom_tool_namespace.is_none()
            {
                return Err(bad(
                    "give `build_action`, `copy_to_output`, `custom_tool` or `custom_tool_namespace`",
                ));
            }
            if let Some(a) = &i.build_action
                && (a.trim().is_empty()
                    || !a.chars().all(|c| c.is_ascii_alphanumeric() || c == '_'))
            {
                return Err(bad(format!("`{a}` is not an item type")));
            }
            StructureRequest::SetItemProperties(SetItemPropertiesInput {
                path: text("path", i.path)?,
                project: opt_text("project", i.project)?,
                ..i
            })
        }
        ADD_REFERENCE | REMOVE_REFERENCE => {
            let i: ReferenceIn = required(value)?;
            let project = text("project", i.project)?;
            let reference = text("reference", i.reference)?;
            if id == ADD_REFERENCE {
                StructureRequest::AddReference {
                    project,
                    reference,
                    kind: i.kind,
                }
            } else {
                StructureRequest::RemoveReference {
                    project,
                    reference,
                    kind: i.kind,
                }
            }
        }
        REFERENCE_MANAGER => {
            let i: OptProjectIn = object(value)?;
            StructureRequest::ReferenceManager {
                project: opt_text("project", i.project)?,
            }
        }
        UNLOAD | RELOAD => {
            let i: ProjectIn = required(value)?;
            let project = text("project", i.project)?;
            if id == UNLOAD {
                StructureRequest::Unload { project }
            } else {
                StructureRequest::Reload { project }
            }
        }
        other => return Err(bad(format!("not a brief 0081 command: {other}"))),
    })
}

/// The kind a reference is when the input does not say: a project file's extension or a project of the solution
/// (`is_project`) makes it a project reference.
pub fn reference_kind(reference: &str, is_project: impl Fn(&str) -> bool) -> ReferenceKind {
    let lower = reference.to_ascii_lowercase();
    if [".csproj", ".vbproj", ".fsproj"]
        .iter()
        .any(|e| lower.ends_with(e))
        || is_project(reference)
    {
        ReferenceKind::Project
    } else {
        ReferenceKind::Assembly
    }
}

pub fn spec(id: &str) -> CommandSpec {
    macro_rules! schema {
        ($name:literal) => {
            include_str!(concat!("../../../../protocol/schemas/", $name))
        };
    }
    let solution_out = schema!("solution-edit.output.json");
    let item_out = schema!("project-item-properties.output.json");
    let refs_out = schema!("project-references.output.json");
    let unload_out = schema!("project-unload.output.json");
    let (title, input, output, permission, agent_visible) = match id {
        ADD_PROJECT => (
            "Workspace: Add Existing Project",
            schema!("solution-add-project.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        REMOVE_PROJECT => (
            "Workspace: Remove Project",
            schema!("solution-remove-project.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        ADD_FOLDER => (
            "Workspace: New Solution Folder",
            schema!("solution-add-folder.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        RENAME_FOLDER => (
            "Workspace: Rename Solution Folder",
            schema!("solution-rename-folder.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        REMOVE_FOLDER => (
            "Workspace: Remove Solution Folder",
            schema!("solution-remove-folder.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        ADD_ITEM => (
            "Workspace: Add Existing Item to Solution Folder",
            schema!("solution-add-item.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        REMOVE_ITEM => (
            "Workspace: Remove Solution Item",
            schema!("solution-remove-item.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        SET_PROJECT_DEPENDENCIES => (
            "Project: Project Dependencies",
            schema!("solution-set-project-dependencies.input.json"),
            solution_out,
            PermissionClass::Execute,
            true,
        ),
        ITEM_PROPERTIES => (
            "Project: File Properties",
            schema!("project-item-properties.input.json"),
            item_out,
            PermissionClass::Read,
            true,
        ),
        SET_ITEM_PROPERTIES => (
            "Project: Set File Properties",
            schema!("project-set-item-properties.input.json"),
            item_out,
            PermissionClass::Execute,
            true,
        ),
        ADD_REFERENCE => (
            "Project: Add Reference",
            schema!("project-add-reference.input.json"),
            refs_out,
            PermissionClass::Execute,
            true,
        ),
        REMOVE_REFERENCE => (
            "Project: Remove Reference",
            schema!("project-remove-reference.input.json"),
            refs_out,
            PermissionClass::Execute,
            true,
        ),
        // A dialog on the person's screen: agents use add_reference and remove_reference.
        REFERENCE_MANAGER => (
            "Project: Add Project Reference",
            schema!("project-reference-manager.input.json"),
            refs_out,
            PermissionClass::Execute,
            false,
        ),
        UNLOAD => (
            "Project: Unload Project",
            schema!("project-unload.input.json"),
            unload_out,
            PermissionClass::Execute,
            true,
        ),
        RELOAD => (
            "Project: Reload Project",
            schema!("project-reload.input.json"),
            unload_out,
            PermissionClass::Execute,
            true,
        ),
        other => unreachable!("not a brief 0081 command: {other}"),
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

/// Register the fifteen commands, answered by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn StructureTarget>) {
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

    fn keys_in_schema(value: &Value, schema: &Value) {
        for (k, v) in value.as_object().unwrap() {
            let s = &schema["properties"][k];
            assert!(!s.is_null(), "{k} is not in the schema");
            if let Some(e) = s.get("enum") {
                assert!(e.as_array().unwrap().contains(v), "{k}: {v} not in {e}");
            }
        }
        for r in schema["required"].as_array().into_iter().flatten() {
            assert!(value.get(r.as_str().unwrap()).is_some(), "{r} is missing");
        }
    }

    #[test]
    fn inputs_parse_and_are_validated() {
        assert_eq!(
            parse(ADD_PROJECT, Value::Null).unwrap(),
            StructureRequest::AddProject {
                project: None,
                folder: None
            }
        );
        assert_eq!(
            parse(
                ADD_PROJECT,
                json!({"project": "Lib/Lib.csproj", "folder": "/src/"})
            )
            .unwrap(),
            StructureRequest::AddProject {
                project: Some("Lib/Lib.csproj".into()),
                folder: Some("/src/".into())
            }
        );
        assert!(parse(ADD_PROJECT, json!({"project": ""})).is_err());
        assert!(parse(REMOVE_PROJECT, json!({})).is_err());
        assert_eq!(
            parse(ADD_FOLDER, json!({})).unwrap(),
            StructureRequest::AddFolder {
                name: None,
                parent: None
            }
        );
        for bad in ["a/b", "x:y", "", " lead", "..", "q?"] {
            assert!(parse(ADD_FOLDER, json!({ "name": bad })).is_err(), "{bad}");
            assert!(
                parse(RENAME_FOLDER, json!({"folder": "/a/", "name": bad})).is_err(),
                "{bad}"
            );
        }
        assert!(parse(RENAME_FOLDER, json!({"folder": "/a/", "name": "Tests"})).is_ok());
        assert_eq!(
            parse(ADD_ITEM, json!({"folder": "/a/"})).unwrap(),
            StructureRequest::AddItem {
                folder: "/a/".into(),
                path: None
            }
        );
        assert!(parse(REMOVE_ITEM, json!({"folder": "/a/"})).is_err());
        assert!(
            parse(
                SET_PROJECT_DEPENDENCIES,
                json!({"project": "App", "depends_on": ["Lib"]})
            )
            .is_ok()
        );
        assert!(parse(SET_PROJECT_DEPENDENCIES, json!({"project": "App"})).is_err());
        assert!(parse(ITEM_PROPERTIES, json!({"path": "a.txt"})).is_ok());
        assert!(parse(SET_ITEM_PROPERTIES, json!({"path": "a.txt"})).is_err());
        let StructureRequest::SetItemProperties(s) = parse(
            SET_ITEM_PROPERTIES,
            json!({"path": "a.txt", "copy_to_output": "preserve_newest", "build_action": "Content"}),
        )
        .unwrap() else {
            panic!()
        };
        assert_eq!(s.copy_to_output, Some(CopyToOutput::PreserveNewest));
        assert!(
            parse(
                SET_ITEM_PROPERTIES,
                json!({"path": "a.txt", "build_action": "Con tent"})
            )
            .is_err()
        );
        assert!(
            parse(
                SET_ITEM_PROPERTIES,
                json!({"path": "a.txt", "copy_to_output": "sometimes"})
            )
            .is_err()
        );
        assert_eq!(
            parse(
                ADD_REFERENCE,
                json!({"project": "App", "reference": "Lib", "kind": "project"})
            )
            .unwrap(),
            StructureRequest::AddReference {
                project: "App".into(),
                reference: "Lib".into(),
                kind: Some(ReferenceKind::Project)
            }
        );
        assert!(parse(REMOVE_REFERENCE, json!({"project": "App"})).is_err());
        assert!(parse(REFERENCE_MANAGER, Value::Null).is_ok());
        assert_eq!(
            parse(UNLOAD, json!({"project": "App"})).unwrap().command(),
            UNLOAD
        );
        assert_eq!(
            parse(RELOAD, json!({"project": "App"})).unwrap().command(),
            RELOAD
        );
        assert!(parse(UNLOAD, json!({"project": "App", "x": 1})).is_err());
    }

    #[test]
    fn reference_kinds_follow_the_extension_or_the_solution() {
        assert_eq!(
            reference_kind("../Lib/Lib.CSPROJ", |_| false),
            ReferenceKind::Project
        );
        assert_eq!(
            reference_kind("Lib", |n| n == "Lib"),
            ReferenceKind::Project
        );
        assert_eq!(
            reference_kind("System.Xml", |_| false),
            ReferenceKind::Assembly
        );
    }

    #[test]
    fn specs_and_outputs_follow_the_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"), "{id}");
            assert_eq!(s.input_schema["type"], "object");
            assert_eq!(s.agent_visible, id != REFERENCE_MANAGER, "{id}");
        }
        assert_eq!(spec(ITEM_PROPERTIES).permission, PermissionClass::Read);
        assert_eq!(spec(ADD_PROJECT).permission, PermissionClass::Execute);
        keys_in_schema(
            &StructureOutput::Solution(SolutionEditOutput {
                path: "/s/S.slnx".into(),
                written: true,
                folder: Some("/NewFolder1/".into()),
                message: Some("x".into()),
                generation: 3,
                pending: true,
            })
            .to_json(),
            &spec(ADD_FOLDER).output_schema,
        );
        keys_in_schema(
            &StructureOutput::Item(ItemPropertiesOutput {
                project: "App".into(),
                project_path: "/s/App/App.csproj".into(),
                path: "/s/App/a.json".into(),
                listed: true,
                build_action: Some("None".into()),
                build_actions: vec!["None".into()],
                copy_to_output: Some(CopyToOutput::PreserveNewest),
                custom_tool: Some(String::new()),
                custom_tool_namespace: Some(String::new()),
                implicit: true,
                written: Some(true),
                generation: 2,
                pending: true,
            })
            .to_json(),
            &spec(SET_ITEM_PROPERTIES).output_schema,
        );
        keys_in_schema(
            &StructureOutput::References(ReferencesOutput {
                project: "App".into(),
                path: "/s/App/App.csproj".into(),
                written: false,
                refused: vec![RefusedRow {
                    reference: "Lib".into(),
                    reason: "cycle: Lib -> App -> Lib".into(),
                }],
                opened: true,
                generation: 2,
                pending: true,
            })
            .to_json(),
            &spec(ADD_REFERENCE).output_schema,
        );
        keys_in_schema(
            &StructureOutput::Unload(UnloadOutput {
                project: "App".into(),
                path: "/s/App/App.csproj".into(),
                unloaded: vec!["/s/App/App.csproj".into()],
                generation: 2,
                pending: false,
            })
            .to_json(),
            &spec(UNLOAD).output_schema,
        );
    }
}
