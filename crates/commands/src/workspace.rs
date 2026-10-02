//! Solution, file and editor commands (PLAN.md 4.1, 4.2, 5.1; brief 0012): `eludite.solution.open` and `close`,
//! `eludite.file.open` and `close`, and the editor actions that must be commands, `eludite.editor.save`, `undo`,
//! `redo` and `find`.
//!
//! The schemas are the files in `protocol/schemas/` (checked in first, CLAUDE.md invariant 4), embedded at compile
//! time. This module parses and validates input into a typed [`WorkspaceRequest`] and serializes the typed
//! [`WorkspaceOutput`]; the shell implements [`WorkspaceTarget`]. File > Open Project/Solution, Solution Explorer,
//! the document tabs, the Error List, Ctrl+S and agents all reach the workspace through these commands.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const SOLUTION_OPEN: &str = "eludite.solution.open";
pub const SOLUTION_CLOSE: &str = "eludite.solution.close";
pub const FILE_OPEN: &str = "eludite.file.open";
pub const FILE_CLOSE: &str = "eludite.file.close";
pub const EDITOR_SAVE: &str = "eludite.editor.save";
pub const EDITOR_UNDO: &str = "eludite.editor.undo";
pub const EDITOR_REDO: &str = "eludite.editor.redo";
pub const EDITOR_FIND: &str = "eludite.editor.find";

/// Every command this module registers.
pub const ALL: [&str; 8] = [
    SOLUTION_OPEN,
    SOLUTION_CLOSE,
    FILE_OPEN,
    FILE_CLOSE,
    EDITOR_SAVE,
    EDITOR_UNDO,
    EDITOR_REDO,
    EDITOR_FIND,
];

const HISTORY_OUTPUT: &str = include_str!("../../../protocol/schemas/editor-history.output.json");

/// (title, input schema, output schema, permission)
fn schemas(id: &str) -> (&'static str, &'static str, &'static str, PermissionClass) {
    use PermissionClass::*;
    match id {
        // Loading runs MSBuild evaluations and design-time builds (PLAN.md 5.3, execute).
        SOLUTION_OPEN => (
            "File: Open Project/Solution",
            include_str!("../../../protocol/schemas/solution-open.input.json"),
            include_str!("../../../protocol/schemas/solution-open.output.json"),
            Execute,
        ),
        SOLUTION_CLOSE => (
            "File: Close Solution",
            include_str!("../../../protocol/schemas/solution-close.input.json"),
            include_str!("../../../protocol/schemas/solution-close.output.json"),
            Read,
        ),
        FILE_OPEN => (
            "File: Open",
            include_str!("../../../protocol/schemas/file-open.input.json"),
            include_str!("../../../protocol/schemas/file-open.output.json"),
            Read,
        ),
        // Closing can save or drop unsaved changes.
        FILE_CLOSE => (
            "File: Close",
            include_str!("../../../protocol/schemas/file-close.input.json"),
            include_str!("../../../protocol/schemas/file-close.output.json"),
            EditBuffer,
        ),
        EDITOR_SAVE => (
            "File: Save",
            include_str!("../../../protocol/schemas/editor-save.input.json"),
            include_str!("../../../protocol/schemas/editor-save.output.json"),
            EditBuffer,
        ),
        EDITOR_UNDO => (
            "Edit: Undo",
            include_str!("../../../protocol/schemas/editor-undo.input.json"),
            HISTORY_OUTPUT,
            EditBuffer,
        ),
        EDITOR_REDO => (
            "Edit: Redo",
            include_str!("../../../protocol/schemas/editor-redo.input.json"),
            HISTORY_OUTPUT,
            EditBuffer,
        ),
        EDITOR_FIND => (
            "Edit: Find",
            include_str!("../../../protocol/schemas/editor-find.input.json"),
            include_str!("../../../protocol/schemas/editor-find.output.json"),
            Read,
        ),
        other => unreachable!("not a workspace command: {other}"),
    }
}

/// What `eludite.file.close` does with unsaved changes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum CloseSave {
    Save,
    Discard,
}

/// A parsed, validated workspace command. `path: None` on an editor command means the active document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceRequest {
    SolutionOpen {
        path: String,
    },
    SolutionClose,
    FileOpen {
        path: String,
        /// 1-based.
        line: Option<u32>,
        /// 1-based.
        column: Option<u32>,
    },
    FileClose {
        path: String,
        save: Option<CloseSave>,
    },
    Save {
        path: Option<String>,
    },
    Undo {
        path: Option<String>,
    },
    Redo {
        path: Option<String>,
    },
    Find {
        path: Option<String>,
        query: Option<String>,
        case_sensitive: bool,
    },
}

impl WorkspaceRequest {
    /// The command id this request came from.
    pub fn command(&self) -> &'static str {
        match self {
            WorkspaceRequest::SolutionOpen { .. } => SOLUTION_OPEN,
            WorkspaceRequest::SolutionClose => SOLUTION_CLOSE,
            WorkspaceRequest::FileOpen { .. } => FILE_OPEN,
            WorkspaceRequest::FileClose { .. } => FILE_CLOSE,
            WorkspaceRequest::Save { .. } => EDITOR_SAVE,
            WorkspaceRequest::Undo { .. } => EDITOR_UNDO,
            WorkspaceRequest::Redo { .. } => EDITOR_REDO,
            WorkspaceRequest::Find { .. } => EDITOR_FIND,
        }
    }
}

/// `solution-open.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionOpenOutput {
    pub path: String,
    /// Always `"loading"`.
    pub state: String,
}

/// `solution-close.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SolutionCloseOutput {
    pub closed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
}

/// `file-open.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileOpenOutput {
    pub path: String,
    pub already_open: bool,
}

/// `file-close.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FileCloseOutput {
    pub path: String,
    pub closed: bool,
    pub saved: bool,
}

/// `editor-save.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SaveOutput {
    pub path: String,
    pub bytes: u64,
}

/// `editor-history.output.json` (undo and redo).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct HistoryOutput {
    pub path: String,
    pub applied: bool,
    pub dirty: bool,
}

/// `editor-find.output.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FindOutput {
    pub path: String,
    pub found: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub line: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub column: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub find_bar_open: Option<bool>,
}

/// The typed result of a workspace command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorkspaceOutput {
    SolutionOpen(SolutionOpenOutput),
    SolutionClose(SolutionCloseOutput),
    FileOpen(FileOpenOutput),
    FileClose(FileCloseOutput),
    Save(SaveOutput),
    History(HistoryOutput),
    Find(FindOutput),
}

impl WorkspaceOutput {
    pub fn to_json(&self) -> Value {
        match self {
            WorkspaceOutput::SolutionOpen(o) => serde_json::to_value(o),
            WorkspaceOutput::SolutionClose(o) => serde_json::to_value(o),
            WorkspaceOutput::FileOpen(o) => serde_json::to_value(o),
            WorkspaceOutput::FileClose(o) => serde_json::to_value(o),
            WorkspaceOutput::Save(o) => serde_json::to_value(o),
            WorkspaceOutput::History(o) => serde_json::to_value(o),
            WorkspaceOutput::Find(o) => serde_json::to_value(o),
        }
        .expect("workspace outputs serialize")
    }
}

/// Whatever owns the solution and the editors (the shell). Called on the invoking thread: the UI thread for menus,
/// keys and clicks, a server thread for agents.
pub trait WorkspaceTarget: Send + Sync {
    fn apply(&self, request: WorkspaceRequest) -> Result<WorkspaceOutput, CommandError>;
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathIn {
    path: String,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileOpenIn {
    path: String,
    line: Option<u32>,
    column: Option<u32>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct FileCloseIn {
    path: String,
    save: Option<CloseSave>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct DocIn {
    path: Option<String>,
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct FindIn {
    path: Option<String>,
    query: Option<String>,
    case_sensitive: Option<bool>,
}

fn input<T: serde::de::DeserializeOwned + Default>(input: Value) -> Result<T, CommandError> {
    if input.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn required<T: serde::de::DeserializeOwned>(input: Value) -> Result<T, CommandError> {
    serde_json::from_value(input).map_err(|e| CommandError::InvalidInput(e.to_string()))
}

fn non_empty(field: &str, s: &str) -> Result<(), CommandError> {
    if s.is_empty() {
        Err(CommandError::InvalidInput(format!(
            "`{field}` must not be empty"
        )))
    } else {
        Ok(())
    }
}

fn optional_path(path: Option<String>) -> Result<Option<String>, CommandError> {
    if let Some(p) = &path {
        non_empty("path", p)?;
    }
    Ok(path)
}

/// Parse and validate the input of workspace command `id`.
pub fn parse(id: &str, value: Value) -> Result<WorkspaceRequest, CommandError> {
    Ok(match id {
        SOLUTION_OPEN => {
            let i: PathIn = required(value)?;
            non_empty("path", &i.path)?;
            WorkspaceRequest::SolutionOpen { path: i.path }
        }
        SOLUTION_CLOSE => {
            let _: Empty = input(value)?;
            WorkspaceRequest::SolutionClose
        }
        FILE_OPEN => {
            let i: FileOpenIn = required(value)?;
            non_empty("path", &i.path)?;
            if i.line == Some(0) || i.column == Some(0) {
                return Err(CommandError::InvalidInput(
                    "`line` and `column` are 1-based".into(),
                ));
            }
            WorkspaceRequest::FileOpen {
                path: i.path,
                line: i.line,
                column: i.column,
            }
        }
        FILE_CLOSE => {
            let i: FileCloseIn = required(value)?;
            non_empty("path", &i.path)?;
            WorkspaceRequest::FileClose {
                path: i.path,
                save: i.save,
            }
        }
        EDITOR_SAVE | EDITOR_UNDO | EDITOR_REDO => {
            let i: DocIn = input(value)?;
            let path = optional_path(i.path)?;
            match id {
                EDITOR_SAVE => WorkspaceRequest::Save { path },
                EDITOR_UNDO => WorkspaceRequest::Undo { path },
                _ => WorkspaceRequest::Redo { path },
            }
        }
        EDITOR_FIND => {
            let i: FindIn = input(value)?;
            if let Some(q) = &i.query {
                non_empty("query", q)?;
            }
            WorkspaceRequest::Find {
                path: optional_path(i.path)?,
                query: i.query,
                case_sensitive: i.case_sensitive.unwrap_or(false),
            }
        }
        other => return Err(CommandError::UnknownCommand(other.to_owned())),
    })
}

fn parse_schema(text: &str) -> Value {
    serde_json::from_str(text).expect("protocol schemas are valid JSON")
}

/// The public description of workspace command `id` (one of [`ALL`]).
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) = schemas(id);
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: parse_schema(input),
        output_schema: parse_schema(output),
        permission,
    }
}

/// Register every workspace command, applying them to `target`. Replaces an earlier registration of the same id
/// (the built-in `eludite.file.open` placeholder).
pub fn register(registry: &mut CommandRegistry, target: Arc<dyn WorkspaceTarget>) {
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
    use std::sync::Mutex;

    use super::*;
    use serde_json::json;

    /// Checks `value` against the subset of JSON Schema these files use.
    fn conforms(schema: &Value, value: &Value) {
        let obj = value.as_object().expect("outputs are objects");
        for r in schema["required"].as_array().unwrap() {
            assert!(obj.contains_key(r.as_str().unwrap()), "missing {r}");
        }
        let props = schema["properties"].as_object().unwrap();
        for (k, v) in obj {
            let p = props
                .get(k)
                .unwrap_or_else(|| panic!("unexpected property {k}"));
            match p.get("type").and_then(Value::as_str) {
                Some("string") => assert!(v.is_string(), "{k}"),
                Some("boolean") => assert!(v.is_boolean(), "{k}"),
                Some("integer") => assert!(v.is_u64(), "{k}"),
                _ => {}
            }
            if let Some(c) = p.get("const") {
                assert_eq!(c, v, "{k}");
            }
        }
    }

    #[derive(Default)]
    struct Recorder(Mutex<Vec<WorkspaceRequest>>);

    impl WorkspaceTarget for Recorder {
        fn apply(&self, request: WorkspaceRequest) -> Result<WorkspaceOutput, CommandError> {
            self.0.lock().unwrap().push(request.clone());
            Ok(match request {
                WorkspaceRequest::SolutionOpen { path } => {
                    WorkspaceOutput::SolutionOpen(SolutionOpenOutput {
                        path,
                        state: "loading".into(),
                    })
                }
                _ => WorkspaceOutput::SolutionClose(SolutionCloseOutput {
                    closed: false,
                    path: None,
                }),
            })
        }
    }

    #[test]
    fn specs_use_the_protocol_schemas() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["type"], "object", "{id}");
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["type"], "object", "{id}");
            assert!(
                s.input_schema["$id"]
                    .as_str()
                    .unwrap()
                    .ends_with(".input.json")
            );
        }
        assert_eq!(spec(SOLUTION_OPEN).permission, PermissionClass::Execute);
        assert_eq!(spec(EDITOR_SAVE).permission, PermissionClass::EditBuffer);
        assert_eq!(spec(EDITOR_FIND).permission, PermissionClass::Read);
        assert_eq!(spec(FILE_OPEN).input_schema["required"], json!(["path"]));
    }

    #[test]
    fn parses_every_command() {
        let p = |id, v| parse(id, v).unwrap();
        assert_eq!(
            p(SOLUTION_OPEN, json!({"path": "/s/App.slnx"})),
            WorkspaceRequest::SolutionOpen {
                path: "/s/App.slnx".into()
            }
        );
        assert_eq!(
            p(SOLUTION_CLOSE, json!({})),
            WorkspaceRequest::SolutionClose
        );
        assert_eq!(
            p(SOLUTION_CLOSE, Value::Null),
            WorkspaceRequest::SolutionClose
        );
        assert_eq!(
            p(FILE_OPEN, json!({"path": "/a.cs", "line": 3, "column": 7})),
            WorkspaceRequest::FileOpen {
                path: "/a.cs".into(),
                line: Some(3),
                column: Some(7)
            }
        );
        assert_eq!(
            p(FILE_CLOSE, json!({"path": "/a.cs", "save": "discard"})),
            WorkspaceRequest::FileClose {
                path: "/a.cs".into(),
                save: Some(CloseSave::Discard)
            }
        );
        assert_eq!(
            p(EDITOR_SAVE, json!({})),
            WorkspaceRequest::Save { path: None }
        );
        assert_eq!(
            p(EDITOR_UNDO, json!({"path": "/a.cs"})),
            WorkspaceRequest::Undo {
                path: Some("/a.cs".into())
            }
        );
        assert_eq!(
            p(EDITOR_REDO, Value::Null),
            WorkspaceRequest::Redo { path: None }
        );
        assert_eq!(
            p(
                EDITOR_FIND,
                json!({"query": "Widget", "case_sensitive": true})
            ),
            WorkspaceRequest::Find {
                path: None,
                query: Some("Widget".into()),
                case_sensitive: true
            }
        );
        for id in ALL {
            assert!(parse(id, json!({"bogus": 1})).is_err(), "{id}");
        }
    }

    #[test]
    fn rejects_bad_input() {
        for (id, bad) in [
            (SOLUTION_OPEN, json!({})),
            (SOLUTION_OPEN, json!({"path": ""})),
            (SOLUTION_CLOSE, json!({"path": "/a"})),
            (FILE_OPEN, json!({"path": "/a", "line": 0})),
            (FILE_OPEN, json!({"path": 3})),
            (FILE_CLOSE, json!({"path": "/a", "save": "maybe"})),
            (EDITOR_SAVE, json!({"path": ""})),
            (EDITOR_FIND, json!({"query": ""})),
            (EDITOR_UNDO, json!("x")),
        ] {
            assert!(
                matches!(parse(id, bad.clone()), Err(CommandError::InvalidInput(_))),
                "{id} {bad}"
            );
        }
    }

    #[test]
    fn outputs_match_their_schemas() {
        let cases = [
            (
                SOLUTION_OPEN,
                WorkspaceOutput::SolutionOpen(SolutionOpenOutput {
                    path: "/s/App.slnx".into(),
                    state: "loading".into(),
                }),
            ),
            (
                SOLUTION_CLOSE,
                WorkspaceOutput::SolutionClose(SolutionCloseOutput {
                    closed: true,
                    path: Some("/s/App.slnx".into()),
                }),
            ),
            (
                FILE_OPEN,
                WorkspaceOutput::FileOpen(FileOpenOutput {
                    path: "/a.cs".into(),
                    already_open: false,
                }),
            ),
            (
                FILE_CLOSE,
                WorkspaceOutput::FileClose(FileCloseOutput {
                    path: "/a.cs".into(),
                    closed: true,
                    saved: true,
                }),
            ),
            (
                EDITOR_SAVE,
                WorkspaceOutput::Save(SaveOutput {
                    path: "/a.cs".into(),
                    bytes: 12,
                }),
            ),
            (
                EDITOR_UNDO,
                WorkspaceOutput::History(HistoryOutput {
                    path: "/a.cs".into(),
                    applied: true,
                    dirty: false,
                }),
            ),
            (
                EDITOR_FIND,
                WorkspaceOutput::Find(FindOutput {
                    path: "/a.cs".into(),
                    found: true,
                    line: Some(4),
                    column: Some(9),
                    find_bar_open: None,
                }),
            ),
        ];
        for (id, out) in cases {
            conforms(&spec(id).output_schema, &out.to_json());
        }
    }

    #[test]
    fn register_replaces_the_placeholder_and_routes_to_the_target() {
        let mut r = crate::builtins::default_registry();
        let target = Arc::new(Recorder::default());
        register(&mut r, target.clone());
        let out = r
            .invoke(SOLUTION_OPEN, json!({"path": "/s/App.slnx"}))
            .unwrap();
        assert_eq!(out, json!({"path": "/s/App.slnx", "state": "loading"}));
        r.invoke(FILE_OPEN, json!({"path": "/a.cs"})).unwrap();
        assert!(r.invoke(FILE_OPEN, json!({})).is_err());
        assert_eq!(target.0.lock().unwrap().len(), 2);
        assert_eq!(r.lookup(FILE_OPEN).unwrap().title, "File: Open");
        assert_eq!(r.list().filter(|s| s.id.as_str() == FILE_OPEN).count(), 1);
    }
}
