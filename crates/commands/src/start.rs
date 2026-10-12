//! The start experience and templates (brief 0072): `eludite.workspace.open_project` (File > Open Project/Solution...),
//! `eludite.workspace.recent` and `recent_edit` (the Welcome page's Open recent list), `eludite.project.templates`,
//! `eludite.project.new` (File > New Project...) and `eludite.project.add_item` (Add > New Item... and Existing
//! Item...). The schemas are `protocol/schemas/workspace-open-project.*`, `workspace-recent*.*`,
//! `project-templates.*`, `project-new.*` and `project-add-item.*` (checked in first, CLAUDE.md invariant 4). This
//! module parses input into a typed [`StartRequest`], serializes the typed outputs and registers the commands; the
//! shell implements [`StartCommands`].

use std::collections::BTreeMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const OPEN_PROJECT: &str = "eludite.workspace.open_project";
pub const RECENT: &str = "eludite.workspace.recent";
pub const RECENT_EDIT: &str = "eludite.workspace.recent_edit";
pub const TEMPLATES: &str = "eludite.project.templates";
pub const NEW_PROJECT: &str = "eludite.project.new";
pub const ADD_ITEM: &str = "eludite.project.add_item";

pub const ALL: [&str; 6] = [
    OPEN_PROJECT,
    RECENT,
    RECENT_EDIT,
    TEMPLATES,
    NEW_PROJECT,
    ADD_ITEM,
];

/// The file extensions File > Open Project/Solution... accepts.
pub const PROJECT_OR_SOLUTION: [&str; 6] = ["sln", "slnx", "slnf", "csproj", "vbproj", "fsproj"];

/// True when `path` ends in one of [`PROJECT_OR_SOLUTION`], in any case.
pub fn is_project_or_solution(path: &std::path::Path) -> bool {
    path.extension().and_then(|e| e.to_str()).is_some_and(|e| {
        PROJECT_OR_SOLUTION
            .iter()
            .any(|x| x.eq_ignore_ascii_case(e))
    })
}

/// What `eludite.project.new` does about a solution.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SolutionChoice {
    New,
    Add,
    None,
}

/// `eludite.project.new` with its arguments.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NewProject {
    pub template: String,
    pub name: String,
    pub location: Option<String>,
    pub language: Option<String>,
    pub solution: Option<SolutionChoice>,
    pub solution_name: Option<String>,
    pub framework: Option<String>,
    pub parameters: BTreeMap<String, String>,
    pub open: bool,
}

/// `eludite.project.add_item` with its arguments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddItem {
    /// No template, name or files: the Add New Item dialog, for `project` and `folder` when given (the context menu),
    /// else for the Workspace window's selection.
    Dialog {
        project: Option<String>,
        folder: Option<String>,
    },
    /// `existing: []` from the UI: the file dialog for the selection.
    ExistingDialog {
        project: Option<String>,
        folder: Option<String>,
        link: bool,
    },
    New {
        project: String,
        folder: Option<String>,
        template: String,
        name: String,
        parameters: BTreeMap<String, String>,
    },
    Existing {
        project: String,
        folder: Option<String>,
        files: Vec<String>,
        link: bool,
    },
}

/// A parsed, validated start command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartRequest {
    /// `None`: the file dialog (UI).
    OpenProject {
        path: Option<String>,
    },
    Recent {
        query: Option<String>,
    },
    RecentEdit {
        path: String,
        pinned: Option<bool>,
        remove: bool,
    },
    Templates {
        kind: Option<String>,
        language: Option<String>,
        query: Option<String>,
        refresh: bool,
    },
    /// `None`: the New Project dialog (UI).
    NewProject(Option<NewProject>),
    AddItem(AddItem),
}

impl StartRequest {
    pub fn command(&self) -> &'static str {
        match self {
            StartRequest::OpenProject { .. } => OPEN_PROJECT,
            StartRequest::Recent { .. } => RECENT,
            StartRequest::RecentEdit { .. } => RECENT_EDIT,
            StartRequest::Templates { .. } => TEMPLATES,
            StartRequest::NewProject(_) => NEW_PROJECT,
            StartRequest::AddItem(_) => ADD_ITEM,
        }
    }

    /// True when the request only shows a dialog on the user's screen.
    pub fn is_dialog(&self) -> bool {
        matches!(
            self,
            StartRequest::OpenProject { path: None }
                | StartRequest::NewProject(None)
                | StartRequest::AddItem(AddItem::Dialog { .. } | AddItem::ExistingDialog { .. })
        )
    }
}

/// `workspace-open-project.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OpenProjectOutput {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub root: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
}

/// A recent workspace.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentWorkspace {
    pub path: String,
    pub name: String,
    pub kind: String,
    pub pinned: bool,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<u64>,
}

/// A recent file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentFile {
    pub path: String,
    pub name: String,
    pub exists: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub opened: Option<u64>,
}

/// `workspace-recent.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentOutput {
    pub workspaces: Vec<RecentWorkspace>,
    pub files: Vec<RecentFile>,
}

/// `workspace-recent-edit.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecentEditOutput {
    pub path: String,
    pub pinned: bool,
    pub removed: bool,
}

/// A template parameter as `eludite.project.templates` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateParameterOut {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_name: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub data_type: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_value: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub choices: Vec<String>,
}

/// A template as `eludite.project.templates` lists it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplateOut {
    pub identity: String,
    pub name: String,
    pub short_name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub language: Option<String>,
    #[serde(rename = "type")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub classifications: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub default_name: Option<String>,
    pub source: String,
    pub parameters: Vec<TemplateParameterOut>,
}

/// `project-templates.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TemplatesOutput {
    pub templates: Vec<TemplateOut>,
}

/// `project-new.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NewProjectOutput {
    pub state: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub solution: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub primary_file: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub exit_code: Option<i32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
}

/// `project-add-item.output.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AddItemOutput {
    pub state: String,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub files: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub namespace: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub project_written: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartOutput {
    OpenProject(OpenProjectOutput),
    Recent(RecentOutput),
    RecentEdit(RecentEditOutput),
    Templates(TemplatesOutput),
    NewProject(NewProjectOutput),
    AddItem(AddItemOutput),
}

impl StartOutput {
    pub fn to_json(&self) -> Value {
        match self {
            StartOutput::OpenProject(o) => serde_json::to_value(o),
            StartOutput::Recent(o) => serde_json::to_value(o),
            StartOutput::RecentEdit(o) => serde_json::to_value(o),
            StartOutput::Templates(o) => serde_json::to_value(o),
            StartOutput::NewProject(o) => serde_json::to_value(o),
            StartOutput::AddItem(o) => serde_json::to_value(o),
        }
        .expect("start outputs serialize")
    }
}

/// What the shell implements.
pub trait StartCommands: Send + Sync {
    fn apply(&self, request: StartRequest) -> Result<StartOutput, CommandError>;
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct OpenProjectInput {
    path: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct RecentInput {
    query: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RecentEditInput {
    path: String,
    pinned: Option<bool>,
    #[serde(default)]
    remove: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct TemplatesInput {
    #[serde(rename = "type")]
    kind: Option<String>,
    language: Option<String>,
    query: Option<String>,
    #[serde(default)]
    refresh: bool,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct NewProjectInput {
    template: Option<String>,
    name: Option<String>,
    location: Option<String>,
    language: Option<String>,
    solution: Option<SolutionChoice>,
    solution_name: Option<String>,
    framework: Option<String>,
    #[serde(default)]
    parameters: BTreeMap<String, String>,
    open: Option<bool>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct AddItemInput {
    project: Option<String>,
    folder: Option<String>,
    template: Option<String>,
    name: Option<String>,
    existing: Option<Vec<String>>,
    #[serde(default)]
    link: bool,
    #[serde(default)]
    parameters: BTreeMap<String, String>,
}

fn object<T: for<'de> Deserialize<'de> + Default>(input: Value) -> Result<T, CommandError> {
    if input.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn nonempty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

fn absolute(path: &str, what: &str) -> Result<(), CommandError> {
    if std::path::Path::new(path).is_absolute() {
        Ok(())
    } else {
        Err(CommandError::InvalidInput(format!(
            "{what} must be an absolute path: {path}"
        )))
    }
}

/// True when `name` can name a project or an item: not empty, no path separators, no characters Windows refuses.
pub fn valid_name(name: &str) -> bool {
    let name = name.trim();
    !name.is_empty()
        && name != "."
        && name != ".."
        && !name.chars().any(|c| {
            matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c.is_control()
        })
}

pub fn parse(id: &str, input: Value) -> Result<StartRequest, CommandError> {
    let invalid = |m: String| CommandError::InvalidInput(m);
    match id {
        OPEN_PROJECT => {
            let i: OpenProjectInput = object(input)?;
            if let Some(p) = &i.path {
                absolute(p, "path")?;
                if !is_project_or_solution(std::path::Path::new(p)) {
                    return Err(invalid(format!(
                        "{p}: expected a .sln, .slnx, .slnf, .csproj, .vbproj or .fsproj file"
                    )));
                }
            }
            Ok(StartRequest::OpenProject { path: i.path })
        }
        RECENT => {
            let i: RecentInput = object(input)?;
            Ok(StartRequest::Recent {
                query: nonempty(i.query),
            })
        }
        RECENT_EDIT => {
            let i: RecentEditInput =
                serde_json::from_value(input).map_err(|e| invalid(e.to_string()))?;
            if i.path.is_empty() {
                return Err(invalid("path is empty".into()));
            }
            if i.pinned.is_none() && !i.remove {
                return Err(invalid("give `pinned` or `remove`".into()));
            }
            Ok(StartRequest::RecentEdit {
                path: i.path,
                pinned: i.pinned,
                remove: i.remove,
            })
        }
        TEMPLATES => {
            let i: TemplatesInput = object(input)?;
            if let Some(k) = &i.kind
                && !matches!(k.as_str(), "project" | "item" | "solution")
            {
                return Err(invalid(format!(
                    "type must be project, item or solution, not `{k}`"
                )));
            }
            Ok(StartRequest::Templates {
                kind: i.kind,
                language: nonempty(i.language),
                query: nonempty(i.query),
                refresh: i.refresh,
            })
        }
        NEW_PROJECT => {
            let i: NewProjectInput = object(input)?;
            let empty = i.template.is_none()
                && i.name.is_none()
                && i.location.is_none()
                && i.language.is_none()
                && i.solution.is_none()
                && i.solution_name.is_none()
                && i.framework.is_none()
                && i.parameters.is_empty()
                && i.open.is_none();
            if empty {
                return Ok(StartRequest::NewProject(None));
            }
            let template = nonempty(i.template).ok_or_else(|| {
                invalid("template is required (see eludite.project.templates)".into())
            })?;
            let name = i.name.ok_or_else(|| invalid("name is required".into()))?;
            if !valid_name(&name) {
                return Err(invalid(format!("`{name}` is not a valid project name")));
            }
            if let Some(l) = &i.location {
                absolute(l, "location")?;
            }
            if let Some(s) = &i.solution_name
                && !valid_name(s)
            {
                return Err(invalid(format!("`{s}` is not a valid solution name")));
            }
            Ok(StartRequest::NewProject(Some(NewProject {
                template,
                name: name.trim().to_owned(),
                location: i.location,
                language: nonempty(i.language),
                solution: i.solution,
                solution_name: nonempty(i.solution_name),
                framework: nonempty(i.framework),
                parameters: i.parameters,
                open: i.open.unwrap_or(true),
            })))
        }
        ADD_ITEM => {
            let i: AddItemInput = object(input)?;
            if let Some(f) = &i.folder {
                absolute(f, "folder")?;
            }
            match (i.existing, i.template) {
                (Some(_), Some(_)) => Err(invalid(
                    "give `template` (a new item) or `existing` (files), not both".into(),
                )),
                (Some(files), None) if files.is_empty() => {
                    Ok(StartRequest::AddItem(AddItem::ExistingDialog {
                        project: nonempty(i.project),
                        folder: i.folder,
                        link: i.link,
                    }))
                }
                (Some(files), None) => {
                    let project =
                        nonempty(i.project).ok_or_else(|| invalid("project is required".into()))?;
                    for f in &files {
                        absolute(f, "a file of `existing`")?;
                    }
                    Ok(StartRequest::AddItem(AddItem::Existing {
                        project,
                        folder: i.folder,
                        files,
                        link: i.link,
                    }))
                }
                (None, Some(template)) => {
                    let project =
                        nonempty(i.project).ok_or_else(|| invalid("project is required".into()))?;
                    let name = i.name.ok_or_else(|| invalid("name is required".into()))?;
                    if !valid_name(&name) {
                        return Err(invalid(format!("`{name}` is not a valid item name")));
                    }
                    Ok(StartRequest::AddItem(AddItem::New {
                        project,
                        folder: i.folder,
                        template,
                        name: name.trim().to_owned(),
                        parameters: i.parameters,
                    }))
                }
                (None, None) if i.name.is_none() => Ok(StartRequest::AddItem(AddItem::Dialog {
                    project: nonempty(i.project),
                    folder: i.folder,
                })),
                (None, None) => Err(invalid(
                    "give `template` and `name` (a new item) or `existing` (files)".into(),
                )),
            }
        }
        other => Err(invalid(format!("not a start command: {other}"))),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    use PermissionClass::*;
    let (title, input, output, permission, agent_visible) = match id {
        // Like eludite.workspace.open_folder: loading runs MSBuild evaluations.
        OPEN_PROJECT => (
            "File: Open Project/Solution",
            include_str!("../../../protocol/schemas/workspace-open-project.input.json"),
            include_str!("../../../protocol/schemas/workspace-open-project.output.json"),
            Execute,
            true,
        ),
        RECENT => (
            "File: Recent Workspaces",
            include_str!("../../../protocol/schemas/workspace-recent.input.json"),
            include_str!("../../../protocol/schemas/workspace-recent.output.json"),
            Read,
            true,
        ),
        // It changes the user's own list: the edit class.
        RECENT_EDIT => (
            "File: Pin or Remove a Recent Workspace",
            include_str!("../../../protocol/schemas/workspace-recent-edit.input.json"),
            include_str!("../../../protocol/schemas/workspace-recent-edit.output.json"),
            EditBuffer,
            true,
        ),
        TEMPLATES => (
            "Project: Templates",
            include_str!("../../../protocol/schemas/project-templates.input.json"),
            include_str!("../../../protocol/schemas/project-templates.output.json"),
            Read,
            true,
        ),
        // It runs `dotnet new` and `dotnet sln`: the execute class.
        NEW_PROJECT => (
            "File: New Project",
            include_str!("../../../protocol/schemas/project-new.input.json"),
            include_str!("../../../protocol/schemas/project-new.output.json"),
            Execute,
            true,
        ),
        // It writes files and project files (and may run `dotnet new`).
        ADD_ITEM => (
            "Project: Add Item",
            include_str!("../../../protocol/schemas/project-add-item.input.json"),
            include_str!("../../../protocol/schemas/project-add-item.output.json"),
            Execute,
            true,
        ),
        other => unreachable!("not a start command: {other}"),
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

pub fn register(registry: &CommandRegistry, target: Arc<dyn StartCommands>) {
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

    fn abs(p: &str) -> String {
        if cfg!(windows) {
            format!("C:\\w\\{}", p.replace('/', "\\"))
        } else {
            format!("/w/{p}")
        }
    }

    fn keys_in_schema(out: &Value, schema: &Value) {
        for (k, v) in out.as_object().unwrap() {
            let prop = &schema["properties"][k];
            assert!(!prop.is_null(), "{k} is not in the schema");
            if let (Some(items), Some(first)) =
                (v.as_array(), prop["items"]["properties"].as_object())
            {
                for item in items {
                    for ik in item.as_object().unwrap().keys() {
                        assert!(first.contains_key(ik), "{k}[].{ik} is not in the schema");
                    }
                }
            }
        }
    }

    #[test]
    fn parses_and_specs_follow_the_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
        }
        // From the UI without arguments: the dialogs.
        assert!(parse(OPEN_PROJECT, json!({})).unwrap().is_dialog());
        assert!(parse(NEW_PROJECT, json!({})).unwrap().is_dialog());
        assert!(parse(ADD_ITEM, json!({})).unwrap().is_dialog());
        assert!(
            parse(ADD_ITEM, json!({"existing": []}))
                .unwrap()
                .is_dialog()
        );
        assert!(parse(OPEN_PROJECT, json!({"path": "App.sln"})).is_err());
        assert!(parse(OPEN_PROJECT, json!({"path": abs("notes.txt")})).is_err());
        assert_eq!(
            parse(OPEN_PROJECT, json!({"path": abs("App.slnf")})).unwrap(),
            StartRequest::OpenProject {
                path: Some(abs("App.slnf"))
            }
        );
        assert!(parse(RECENT_EDIT, json!({"path": "/w"})).is_err());
        assert_eq!(
            parse(RECENT_EDIT, json!({"path": "/w", "pinned": true})).unwrap(),
            StartRequest::RecentEdit {
                path: "/w".into(),
                pinned: Some(true),
                remove: false
            }
        );
        assert!(parse(TEMPLATES, json!({"type": "page"})).is_err());
        let r = parse(
            NEW_PROJECT,
            json!({"template": "console", "name": "Hello", "location": abs(""), "solution": "new", "framework": "net10.0"}),
        )
        .unwrap();
        let StartRequest::NewProject(Some(p)) = r else {
            panic!("{r:?}")
        };
        assert_eq!(p.template, "console");
        assert_eq!(p.solution, Some(SolutionChoice::New));
        assert!(p.open);
        assert!(parse(NEW_PROJECT, json!({"template": "console"})).is_err());
        assert!(parse(NEW_PROJECT, json!({"template": "console", "name": "a/b"})).is_err());
        assert!(parse(NEW_PROJECT, json!({"name": "A"})).is_err());
        assert!(
            parse(
                NEW_PROJECT,
                json!({"template": "console", "name": "A", "solution": "maybe"})
            )
            .is_err()
        );
        assert_eq!(
            parse(
                ADD_ITEM,
                json!({"project": "App", "template": "class", "name": "Customer.cs"})
            )
            .unwrap(),
            StartRequest::AddItem(AddItem::New {
                project: "App".into(),
                folder: None,
                template: "class".into(),
                name: "Customer.cs".into(),
                parameters: BTreeMap::new()
            })
        );
        assert!(parse(ADD_ITEM, json!({"project": "App", "name": "X"})).is_err());
        assert_eq!(
            parse(ADD_ITEM, json!({"project": "App"})).unwrap(),
            StartRequest::AddItem(AddItem::Dialog {
                project: Some("App".into()),
                folder: None
            })
        );
        assert!(
            parse(
                ADD_ITEM,
                json!({"project": "App", "template": "class", "existing": [abs("a.cs")]})
            )
            .is_err()
        );
        assert!(parse(ADD_ITEM, json!({"project": "App", "existing": ["a.cs"]})).is_err());
        assert_eq!(
            parse(
                ADD_ITEM,
                json!({"project": "App", "existing": [abs("a.cs")], "link": true})
            )
            .unwrap(),
            StartRequest::AddItem(AddItem::Existing {
                project: "App".into(),
                folder: None,
                files: vec![abs("a.cs")],
                link: true
            })
        );
        assert!(valid_name("My.App"));
        assert!(!valid_name("a:b") && !valid_name(" ") && !valid_name(".."));
    }

    #[test]
    fn outputs_follow_the_schemas() {
        let cases = [
            (
                OPEN_PROJECT,
                StartOutput::OpenProject(OpenProjectOutput {
                    state: "loading".into(),
                    root: Some("/w".into()),
                    solution: Some("/w/App.slnf".into()),
                }),
            ),
            (
                RECENT,
                StartOutput::Recent(RecentOutput {
                    workspaces: vec![RecentWorkspace {
                        path: "/w".into(),
                        name: "w".into(),
                        kind: "folder".into(),
                        pinned: true,
                        exists: true,
                        opened: Some(1),
                    }],
                    files: vec![RecentFile {
                        path: "/w/a.cs".into(),
                        name: "a.cs".into(),
                        exists: false,
                        opened: None,
                    }],
                }),
            ),
            (
                RECENT_EDIT,
                StartOutput::RecentEdit(RecentEditOutput {
                    path: "/w".into(),
                    pinned: false,
                    removed: true,
                }),
            ),
            (
                TEMPLATES,
                StartOutput::Templates(TemplatesOutput {
                    templates: vec![TemplateOut {
                        identity: "Microsoft.Common.Console.CSharp".into(),
                        name: "Console App".into(),
                        short_name: "console".into(),
                        language: Some("C#".into()),
                        kind: "project".into(),
                        classifications: vec!["Common".into()],
                        description: None,
                        default_name: Some("ConsoleApp1".into()),
                        source: "sdk".into(),
                        parameters: vec![TemplateParameterOut {
                            name: "Framework".into(),
                            display_name: None,
                            description: None,
                            data_type: "choice".into(),
                            default_value: Some("net10.0".into()),
                            choices: vec!["net10.0".into()],
                        }],
                    }],
                }),
            ),
            (
                NEW_PROJECT,
                StartOutput::NewProject(NewProjectOutput {
                    state: "created".into(),
                    project: Some("/w/A/A.csproj".into()),
                    files: vec!["/w/A/A.csproj".into()],
                    solution: Some("/w/A.slnx".into()),
                    primary_file: Some("/w/A/Program.cs".into()),
                    exit_code: Some(0),
                    output: Some(String::new()),
                }),
            ),
            (
                ADD_ITEM,
                StartOutput::AddItem(AddItemOutput {
                    state: "added".into(),
                    files: vec!["/w/A/B.cs".into()],
                    namespace: Some("A".into()),
                    project_written: true,
                }),
            ),
        ];
        for (id, out) in cases {
            let schema = spec(id).output_schema;
            let v = out.to_json();
            keys_in_schema(&v, &schema);
            for r in schema["required"].as_array().into_iter().flatten() {
                assert!(v.get(r.as_str().unwrap()).is_some(), "{id}: {r}");
            }
            if let Some(states) = schema["properties"]["state"]["enum"].as_array() {
                assert!(states.contains(&v["state"]), "{id}: {}", v["state"]);
            }
        }
    }
}
