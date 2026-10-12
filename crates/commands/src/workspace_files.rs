//! The Workspace window's file operations and switches (brief 0073): `eludite.workspace.rename`, `move`, `copy`,
//! `delete`, `new_folder`, `include`, `exclude` and `fsharp_move` (execute; `delete` with `permanent` is raised to
//! dangerous), and `view`, `copy_path` and `edit_project_file` (read). The schemas are
//! `protocol/schemas/workspace-files-*.json` (checked in first, CLAUDE.md invariant 4). This module parses input into
//! a typed [`FilesRequest`], serializes the typed outputs and registers the commands; the shell implements
//! [`WorkspaceFilesTarget`] (the Workspace window and agents run the same code). Writes keep their arguments in the
//! audit log for every caller.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{
    CommandError, CommandId, CommandRegistry, CommandSpec, Escalation, EscalationHook,
    PermissionClass,
};

pub const RENAME: &str = "eludite.workspace.rename";
pub const MOVE: &str = "eludite.workspace.move";
pub const COPY: &str = "eludite.workspace.copy";
pub const DELETE: &str = "eludite.workspace.delete";
pub const NEW_FOLDER: &str = "eludite.workspace.new_folder";
pub const INCLUDE: &str = "eludite.workspace.include";
pub const EXCLUDE: &str = "eludite.workspace.exclude";
pub const FSHARP_MOVE: &str = "eludite.workspace.fsharp_move";
pub const VIEW: &str = "eludite.workspace.view";
pub const COPY_PATH: &str = "eludite.workspace.copy_path";
pub const EDIT_PROJECT_FILE: &str = "eludite.workspace.edit_project_file";

pub const ALL: [&str; 11] = [
    RENAME,
    MOVE,
    COPY,
    DELETE,
    NEW_FOLDER,
    INCLUDE,
    EXCLUDE,
    FSHARP_MOVE,
    VIEW,
    COPY_PATH,
    EDIT_PROJECT_FILE,
];

/// The commands that change files; their arguments are always audited.
pub const WRITES: [&str; 8] = [
    RENAME,
    MOVE,
    COPY,
    DELETE,
    NEW_FOLDER,
    INCLUDE,
    EXCLUDE,
    FSHARP_MOVE,
];

/// Paths one call takes at most.
pub const MAX_PATHS: usize = 1000;

macro_rules! schema {
    ($name:literal) => {
        include_str!(concat!("../../../protocol/schemas/", $name))
    };
}

/// Title, input schema, output schema and class of each command.
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    let files = schema!("workspace-files.output.json");
    match id {
        RENAME => (
            "Workspace: Rename",
            schema!("workspace-files-rename.input.json"),
            files,
            Execute,
        ),
        MOVE => (
            "Workspace: Move",
            schema!("workspace-files-move.input.json"),
            files,
            Execute,
        ),
        COPY => (
            "Workspace: Copy",
            schema!("workspace-files-copy.input.json"),
            files,
            Execute,
        ),
        DELETE => (
            "Workspace: Delete",
            schema!("workspace-files-delete.input.json"),
            files,
            Execute,
        ),
        NEW_FOLDER => (
            "Workspace: New Folder",
            schema!("workspace-files-new-folder.input.json"),
            files,
            Execute,
        ),
        INCLUDE => (
            "Workspace: Include In Project",
            schema!("workspace-files-include.input.json"),
            files,
            Execute,
        ),
        EXCLUDE => (
            "Workspace: Exclude From Project",
            schema!("workspace-files-exclude.input.json"),
            files,
            Execute,
        ),
        FSHARP_MOVE => (
            "Workspace: Move Up or Down (F#)",
            schema!("workspace-files-fsharp-move.input.json"),
            files,
            Execute,
        ),
        VIEW => (
            "Workspace: View",
            schema!("workspace-files-view.input.json"),
            schema!("workspace-files-view.output.json"),
            Read,
        ),
        COPY_PATH => (
            "Workspace: Copy Path",
            schema!("workspace-files-copy-path.input.json"),
            schema!("workspace-files-copy-path.output.json"),
            Read,
        ),
        EDIT_PROJECT_FILE => (
            "Workspace: Edit Project File",
            schema!("workspace-files-edit-project-file.input.json"),
            schema!("workspace-files-edit-project-file.output.json"),
            Read,
        ),
        other => unreachable!("not a workspace files command: {other}"),
    }
}

/// The public description of command `id`.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("the schema is JSON"),
        output_schema: serde_json::from_str(output).expect("the schema is JSON"),
        permission,
        agent_visible: true,
    }
}

/// `eludite.workspace.view`'s input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewInput {
    pub show_all_files: Option<bool>,
    pub file_nesting: Option<bool>,
    pub track_active_item: Option<bool>,
    #[serde(default)]
    pub collapse_all: bool,
    #[serde(default)]
    pub sync_active: bool,
    #[serde(default)]
    pub refresh: bool,
}

/// A parsed call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesRequest {
    Rename {
        path: String,
        name: String,
        rename_symbol: bool,
    },
    Move {
        paths: Vec<String>,
        to: String,
    },
    Copy {
        paths: Vec<String>,
        to: String,
    },
    Delete {
        paths: Vec<String>,
        permanent: bool,
    },
    NewFolder {
        parent: String,
        name: String,
    },
    Include {
        paths: Vec<String>,
    },
    Exclude {
        paths: Vec<String>,
    },
    FsharpMove {
        path: String,
        up: bool,
    },
    View(ViewInput),
    CopyPath {
        path: String,
        relative: bool,
    },
    EditProjectFile {
        project: String,
    },
}

impl FilesRequest {
    /// The command id of this request.
    pub fn command(&self) -> &'static str {
        match self {
            FilesRequest::Rename { .. } => RENAME,
            FilesRequest::Move { .. } => MOVE,
            FilesRequest::Copy { .. } => COPY,
            FilesRequest::Delete { .. } => DELETE,
            FilesRequest::NewFolder { .. } => NEW_FOLDER,
            FilesRequest::Include { .. } => INCLUDE,
            FilesRequest::Exclude { .. } => EXCLUDE,
            FilesRequest::FsharpMove { .. } => FSHARP_MOVE,
            FilesRequest::View(_) => VIEW,
            FilesRequest::CopyPath { .. } => COPY_PATH,
            FilesRequest::EditProjectFile { .. } => EDIT_PROJECT_FILE,
        }
    }
}

/// The state of a file operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FilesState {
    Applying,
    Applied,
    Failed,
}

/// One path a file operation changed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PathChange {
    pub path: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub to: Option<String>,
}

/// `workspace-files.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct FilesOutput {
    pub state: FilesState,
    pub paths: Vec<PathChange>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub project_edits: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub open_documents: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

impl FilesOutput {
    pub fn applying() -> Self {
        Self {
            state: FilesState::Applying,
            paths: Vec::new(),
            project_edits: Vec::new(),
            open_documents: Vec::new(),
            message: None,
        }
    }

    pub fn failed(message: impl Into<String>) -> Self {
        Self {
            state: FilesState::Failed,
            message: Some(message.into()),
            ..Self::applying()
        }
    }
}

/// `workspace-files-view.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub struct ViewOutput {
    pub show_all_files: bool,
    pub file_nesting: bool,
    pub track_active_item: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub selected: Option<String>,
}

/// What a command answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilesAnswer {
    Files(FilesOutput),
    View(ViewOutput),
    /// The text put on the clipboard.
    CopyPath(String),
    /// The project file opened.
    EditProjectFile(String),
}

impl FilesAnswer {
    pub fn to_json(&self) -> Value {
        match self {
            FilesAnswer::Files(o) => serde_json::to_value(o),
            FilesAnswer::View(o) => serde_json::to_value(o),
            FilesAnswer::CopyPath(text) => Ok(serde_json::json!({ "text": text })),
            FilesAnswer::EditProjectFile(path) => Ok(serde_json::json!({ "path": path })),
        }
        .expect("the outputs serialize")
    }
}

/// What the shell implements. Called on the invoking thread: the UI thread for the person's calls (which answer at
/// once, `applying`), a server thread for agents' (which wait for the operation).
pub trait WorkspaceFilesTarget: Send + Sync {
    fn apply(&self, request: FilesRequest) -> Result<FilesAnswer, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct RenameIn {
    path: String,
    name: String,
    #[serde(default)]
    rename_symbol: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsToIn {
    paths: Vec<String>,
    to: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DeleteIn {
    paths: Vec<String>,
    #[serde(default)]
    permanent: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct NewFolderIn {
    parent: String,
    name: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathsIn {
    paths: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FsharpMoveIn {
    path: String,
    direction: Direction,
}

#[derive(Deserialize)]
#[serde(rename_all = "lowercase")]
enum Direction {
    Up,
    Down,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CopyPathIn {
    path: String,
    #[serde(default)]
    relative: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct EditProjectFileIn {
    project: String,
}

fn invalid(message: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(message.into())
}

fn decode<T: serde::de::DeserializeOwned>(value: Value) -> Result<T, CommandError> {
    let value = if value.is_null() {
        Value::Object(Default::default())
    } else {
        value
    };
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}

fn non_empty(field: &str, s: &str) -> Result<(), CommandError> {
    if s.trim().is_empty() {
        return Err(invalid(format!("`{field}` must not be empty")));
    }
    Ok(())
}

fn check_paths(paths: &[String]) -> Result<(), CommandError> {
    if paths.is_empty() || paths.len() > MAX_PATHS {
        return Err(invalid(format!("`paths` holds 1 to {MAX_PATHS} paths")));
    }
    paths.iter().try_for_each(|p| non_empty("paths", p))
}

/// A file or folder name: not empty, no separators, not `.` or `..`, and none of the characters Windows refuses.
pub fn check_name(name: &str) -> Result<(), CommandError> {
    let bad = name.trim().is_empty()
        || name == "."
        || name == ".."
        || name.len() > 255
        || name
            .chars()
            .any(|c| matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') || c < ' ');
    if bad {
        return Err(invalid(format!(
            "`{name}` is not a valid file or folder name"
        )));
    }
    Ok(())
}

/// Parse and validate the input of command `id`.
pub fn parse(id: &str, value: Value) -> Result<FilesRequest, CommandError> {
    Ok(match id {
        RENAME => {
            let i: RenameIn = decode(value)?;
            non_empty("path", &i.path)?;
            check_name(&i.name)?;
            FilesRequest::Rename {
                path: i.path,
                name: i.name,
                rename_symbol: i.rename_symbol,
            }
        }
        MOVE | COPY => {
            let i: PathsToIn = decode(value)?;
            check_paths(&i.paths)?;
            non_empty("to", &i.to)?;
            if id == MOVE {
                FilesRequest::Move {
                    paths: i.paths,
                    to: i.to,
                }
            } else {
                FilesRequest::Copy {
                    paths: i.paths,
                    to: i.to,
                }
            }
        }
        DELETE => {
            let i: DeleteIn = decode(value)?;
            check_paths(&i.paths)?;
            FilesRequest::Delete {
                paths: i.paths,
                permanent: i.permanent,
            }
        }
        NEW_FOLDER => {
            let i: NewFolderIn = decode(value)?;
            non_empty("parent", &i.parent)?;
            check_name(&i.name)?;
            FilesRequest::NewFolder {
                parent: i.parent,
                name: i.name,
            }
        }
        INCLUDE | EXCLUDE => {
            let i: PathsIn = decode(value)?;
            check_paths(&i.paths)?;
            if id == INCLUDE {
                FilesRequest::Include { paths: i.paths }
            } else {
                FilesRequest::Exclude { paths: i.paths }
            }
        }
        FSHARP_MOVE => {
            let i: FsharpMoveIn = decode(value)?;
            non_empty("path", &i.path)?;
            FilesRequest::FsharpMove {
                path: i.path,
                up: matches!(i.direction, Direction::Up),
            }
        }
        VIEW => FilesRequest::View(decode(value)?),
        COPY_PATH => {
            let i: CopyPathIn = decode(value)?;
            non_empty("path", &i.path)?;
            FilesRequest::CopyPath {
                path: i.path,
                relative: i.relative,
            }
        }
        EDIT_PROJECT_FILE => {
            let i: EditProjectFileIn = decode(value)?;
            non_empty("project", &i.project)?;
            FilesRequest::EditProjectFile { project: i.project }
        }
        other => return Err(invalid(format!("unknown command {other}"))),
    })
}

/// `delete` with `permanent` is dangerous: nothing brings the files back.
fn escalation(id: &str) -> Option<EscalationHook> {
    (id == DELETE).then(|| -> EscalationHook {
        Arc::new(|input: &Value, _| {
            (input.get("permanent") == Some(&Value::Bool(true))).then(|| {
                Escalation::raise(
                    PermissionClass::Dangerous,
                    "it deletes for good, not to the trash",
                )
            })
        })
    })
}

/// Register every command, applied by `target`; the writes keep their arguments in the audit log.
pub fn register(registry: &CommandRegistry, target: Arc<dyn WorkspaceFilesTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace_with_escalation(spec(id), escalation(id), move |input| {
            let request = parse(id, input)?;
            target.apply(request).map(|out| out.to_json())
        });
    }
    for id in WRITES {
        registry.always_audit_arguments(id);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Every member of `value` is a property of `schema`, and every required one is there.
    fn fits(value: &Value, schema: &str) {
        let schema: Value = serde_json::from_str(schema).unwrap();
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
            parse(RENAME, json!({"path": "/w/A.cs", "name": "B.cs"})).unwrap(),
            FilesRequest::Rename {
                path: "/w/A.cs".into(),
                name: "B.cs".into(),
                rename_symbol: false
            }
        );
        for bad in ["", " ", "a/b", "a\\b", "..", "a:b", "a?"] {
            assert!(
                parse(RENAME, json!({"path": "/w/A.cs", "name": bad})).is_err(),
                "{bad:?}"
            );
        }
        assert!(parse(RENAME, json!({"path": "/w/A.cs", "name": "B", "x": 1})).is_err());
        assert_eq!(
            parse(MOVE, json!({"paths": ["/w/a"], "to": "/w/b"})).unwrap(),
            FilesRequest::Move {
                paths: vec!["/w/a".into()],
                to: "/w/b".into()
            }
        );
        assert!(parse(MOVE, json!({"paths": [], "to": "/w/b"})).is_err());
        assert!(matches!(
            parse(COPY, json!({"paths": ["/w/a"], "to": "/w/b"})).unwrap(),
            FilesRequest::Copy { .. }
        ));
        assert_eq!(
            parse(DELETE, json!({"paths": ["/w/a"]})).unwrap(),
            FilesRequest::Delete {
                paths: vec!["/w/a".into()],
                permanent: false
            }
        );
        let many: Vec<String> = (0..=MAX_PATHS).map(|i| format!("/w/{i}")).collect();
        assert!(parse(DELETE, json!({ "paths": many })).is_err());
        assert!(matches!(
            parse(NEW_FOLDER, json!({"parent": "/w", "name": "NewFolder"})).unwrap(),
            FilesRequest::NewFolder { .. }
        ));
        assert!(matches!(
            parse(INCLUDE, json!({"paths": ["/w/a"]})).unwrap(),
            FilesRequest::Include { .. }
        ));
        assert!(matches!(
            parse(EXCLUDE, json!({"paths": ["/w/a"]})).unwrap(),
            FilesRequest::Exclude { .. }
        ));
        assert_eq!(
            parse(FSHARP_MOVE, json!({"path": "/w/A.fs", "direction": "up"})).unwrap(),
            FilesRequest::FsharpMove {
                path: "/w/A.fs".into(),
                up: true
            }
        );
        assert!(parse(FSHARP_MOVE, json!({"path": "/w/A.fs", "direction": "left"})).is_err());
        assert_eq!(
            parse(VIEW, Value::Null).unwrap(),
            FilesRequest::View(ViewInput::default())
        );
        assert_eq!(
            parse(VIEW, json!({"showAllFiles": true, "collapseAll": true})).unwrap(),
            FilesRequest::View(ViewInput {
                show_all_files: Some(true),
                collapse_all: true,
                ..ViewInput::default()
            })
        );
        assert_eq!(
            parse(COPY_PATH, json!({"path": "/w/a", "relative": true})).unwrap(),
            FilesRequest::CopyPath {
                path: "/w/a".into(),
                relative: true
            }
        );
        assert!(matches!(
            parse(EDIT_PROJECT_FILE, json!({"project": "App"})).unwrap(),
            FilesRequest::EditProjectFile { .. }
        ));
        for id in ALL {
            assert_eq!(parse(id, json!({})).is_ok(), id == VIEW, "{id}");
            assert_eq!(parse(id, json!({"paths": ["/a"], "path": "/a"})).is_ok(), false);
        }
    }

    #[test]
    fn outputs_fit_their_schemas_and_specs_declare_their_classes() {
        let out = FilesAnswer::Files(FilesOutput {
            state: FilesState::Applied,
            paths: vec![PathChange {
                path: "/w/A.cs".into(),
                to: Some("/w/B.cs".into()),
            }],
            project_edits: vec!["/w/App.csproj".into()],
            open_documents: vec!["/w/A.cs".into()],
            message: None,
        })
        .to_json();
        fits(&out, schema!("workspace-files.output.json"));
        assert_eq!(out["state"], "applied");
        fits(
            &FilesAnswer::Files(FilesOutput::failed("no")).to_json(),
            schema!("workspace-files.output.json"),
        );
        fits(
            &FilesAnswer::View(ViewOutput {
                show_all_files: true,
                file_nesting: false,
                track_active_item: true,
                selected: Some("/w/a".into()),
            })
            .to_json(),
            schema!("workspace-files-view.output.json"),
        );
        fits(
            &FilesAnswer::CopyPath("a".into()).to_json(),
            schema!("workspace-files-copy-path.output.json"),
        );
        fits(
            &FilesAnswer::EditProjectFile("a".into()).to_json(),
            schema!("workspace-files-edit-project-file.output.json"),
        );
        for id in ALL {
            let s = spec(id);
            assert!(s.agent_visible, "{id}");
            let expected = if WRITES.contains(&id) {
                PermissionClass::Execute
            } else {
                PermissionClass::Read
            };
            assert_eq!(s.permission, expected, "{id}");
            assert_eq!(s.input_schema["additionalProperties"], json!(false), "{id}");
        }
    }

    #[test]
    fn a_permanent_delete_is_dangerous_and_the_writes_are_audited() {
        struct Target;
        impl WorkspaceFilesTarget for Target {
            fn apply(&self, request: FilesRequest) -> Result<FilesAnswer, CommandError> {
                Ok(match request {
                    FilesRequest::View(_) => FilesAnswer::View(ViewOutput {
                        show_all_files: false,
                        file_nesting: true,
                        track_active_item: true,
                        selected: None,
                    }),
                    _ => FilesAnswer::Files(FilesOutput::applying()),
                })
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Target));
        let hook = escalation(DELETE).unwrap();
        let view = crate::policy::PolicyView::default();
        assert!(hook(&json!({"paths": ["/a"]}), &view).is_none());
        assert!(matches!(
            hook(&json!({"paths": ["/a"], "permanent": true}), &view),
            Some(Escalation::Raise {
                class: PermissionClass::Dangerous,
                ..
            })
        ));
        assert!(escalation(RENAME).is_none());
        let out = registry
            .invoke(VIEW, json!({"fileNesting": false}))
            .unwrap();
        assert_eq!(out["fileNesting"], true);
        let out = registry.invoke(MOVE, json!({"paths": ["/a"], "to": "/b"})).unwrap();
        assert_eq!(out["state"], "applying");
    }
}
