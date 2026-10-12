//! IntelliSense editing commands (brief 0069): `eludite.editor.insert_snippet` (Ctrl+K, Ctrl+X),
//! `eludite.editor.surround_with` (Ctrl+K, Ctrl+S), `eludite.editor.snippets`, `eludite.editor.format_selection`
//! (Ctrl+K, Ctrl+F), `eludite.editor.remove_and_sort_usings` (Ctrl+R, Ctrl+G) and `eludite.editor.code_cleanup`
//! (Ctrl+K, Ctrl+E), registered by the shell through an [`IntellisenseTarget`]. The schemas are
//! `protocol/schemas/editor-{insert-snippet,surround-with,snippets,format-selection,remove-and-sort-usings,code-cleanup}`;
//! this module parses and validates input into an [`IntellisenseRequest`]; outputs are the schemas' JSON.

use std::sync::Arc;

use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const INSERT_SNIPPET: &str = "eludite.editor.insert_snippet";
pub const SURROUND_WITH: &str = "eludite.editor.surround_with";
pub const SNIPPETS: &str = "eludite.editor.snippets";
pub const FORMAT_SELECTION: &str = "eludite.editor.format_selection";
pub const REMOVE_AND_SORT_USINGS: &str = "eludite.editor.remove_and_sort_usings";
pub const CODE_CLEANUP: &str = "eludite.editor.code_cleanup";

pub const ALL: [&str; 6] = [
    INSERT_SNIPPET,
    SURROUND_WITH,
    SNIPPETS,
    FORMAT_SELECTION,
    REMOVE_AND_SORT_USINGS,
    CODE_CLEANUP,
];

/// A parsed, validated command. `path: None` means the active document.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IntellisenseRequest {
    InsertSnippet {
        path: Option<String>,
        shortcut: Option<String>,
    },
    SurroundWith {
        path: Option<String>,
        shortcut: Option<String>,
    },
    Snippets {
        language: Option<String>,
    },
    FormatSelection {
        path: Option<String>,
    },
    RemoveAndSortUsings {
        path: Option<String>,
    },
    CodeCleanup {
        path: Option<String>,
        profile: u8,
    },
}

impl IntellisenseRequest {
    pub fn command(&self) -> &'static str {
        match self {
            IntellisenseRequest::InsertSnippet { .. } => INSERT_SNIPPET,
            IntellisenseRequest::SurroundWith { .. } => SURROUND_WITH,
            IntellisenseRequest::Snippets { .. } => SNIPPETS,
            IntellisenseRequest::FormatSelection { .. } => FORMAT_SELECTION,
            IntellisenseRequest::RemoveAndSortUsings { .. } => REMOVE_AND_SORT_USINGS,
            IntellisenseRequest::CodeCleanup { .. } => CODE_CLEANUP,
        }
    }

    /// The document the command names (`None`: the active one, or none for `snippets`).
    pub fn path(&self) -> Option<&str> {
        match self {
            IntellisenseRequest::InsertSnippet { path, .. }
            | IntellisenseRequest::SurroundWith { path, .. }
            | IntellisenseRequest::FormatSelection { path }
            | IntellisenseRequest::RemoveAndSortUsings { path }
            | IntellisenseRequest::CodeCleanup { path, .. } => path.as_deref(),
            IntellisenseRequest::Snippets { .. } => None,
        }
    }
}

/// What applies the commands (the shell).
pub trait IntellisenseTarget: Send + Sync {
    fn apply(&self, request: IntellisenseRequest) -> Result<Value, CommandError>;
}

fn no_extra(value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let Some(obj) = value.as_object() else {
        return Err(CommandError::InvalidInput("input must be an object".into()));
    };
    if let Some(k) = obj.keys().find(|k| !allowed.contains(&k.as_str())) {
        return Err(CommandError::InvalidInput(format!("unknown field `{k}`")));
    }
    Ok(())
}

fn string(value: &Value, name: &str) -> Result<Option<String>, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(s)) if !s.is_empty() => Ok(Some(s.clone())),
        Some(_) => Err(CommandError::InvalidInput(format!(
            "`{name}` must be a non-empty string"
        ))),
    }
}

/// Parse and validate a command's input.
pub fn parse(id: &str, value: Value) -> Result<IntellisenseRequest, CommandError> {
    match id {
        INSERT_SNIPPET | SURROUND_WITH => {
            no_extra(&value, &["path", "shortcut"])?;
            let (path, shortcut) = (string(&value, "path")?, string(&value, "shortcut")?);
            Ok(if id == INSERT_SNIPPET {
                IntellisenseRequest::InsertSnippet { path, shortcut }
            } else {
                IntellisenseRequest::SurroundWith { path, shortcut }
            })
        }
        SNIPPETS => {
            no_extra(&value, &["language"])?;
            let language = string(&value, "language")?;
            if let Some(l) = &language
                && l != "csharp"
                && l != "vb"
            {
                return Err(CommandError::InvalidInput(
                    "`language` must be `csharp` or `vb`".into(),
                ));
            }
            Ok(IntellisenseRequest::Snippets { language })
        }
        FORMAT_SELECTION | REMOVE_AND_SORT_USINGS => {
            no_extra(&value, &["path"])?;
            let path = string(&value, "path")?;
            Ok(if id == FORMAT_SELECTION {
                IntellisenseRequest::FormatSelection { path }
            } else {
                IntellisenseRequest::RemoveAndSortUsings { path }
            })
        }
        CODE_CLEANUP => {
            no_extra(&value, &["path", "profile"])?;
            let profile = match value.get("profile") {
                None | Some(Value::Null) => 1,
                Some(v) => match v.as_u64() {
                    Some(n @ 1..=2) => n as u8,
                    _ => {
                        return Err(CommandError::InvalidInput(
                            "`profile` must be 1 or 2".into(),
                        ));
                    }
                },
            };
            Ok(IntellisenseRequest::CodeCleanup {
                path: string(&value, "path")?,
                profile,
            })
        }
        other => Err(CommandError::InvalidInput(format!(
            "not an IntelliSense command: {other}"
        ))),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    use PermissionClass::*;
    let (title, input, output, permission) = match id {
        INSERT_SNIPPET => (
            "Edit: IntelliSense: Insert Snippet",
            include_str!("../../../protocol/schemas/editor-insert-snippet.input.json"),
            include_str!("../../../protocol/schemas/editor-insert-snippet.output.json"),
            EditBuffer,
        ),
        SURROUND_WITH => (
            "Edit: IntelliSense: Surround With",
            include_str!("../../../protocol/schemas/editor-surround-with.input.json"),
            include_str!("../../../protocol/schemas/editor-surround-with.output.json"),
            EditBuffer,
        ),
        SNIPPETS => (
            "Edit: IntelliSense: Code Snippets",
            include_str!("../../../protocol/schemas/editor-snippets.input.json"),
            include_str!("../../../protocol/schemas/editor-snippets.output.json"),
            Read,
        ),
        FORMAT_SELECTION => (
            "Edit: Advanced: Format Selection",
            include_str!("../../../protocol/schemas/editor-format-selection.input.json"),
            include_str!("../../../protocol/schemas/editor-format-selection.output.json"),
            EditBuffer,
        ),
        REMOVE_AND_SORT_USINGS => (
            "Edit: IntelliSense: Remove and Sort Usings",
            include_str!("../../../protocol/schemas/editor-remove-and-sort-usings.input.json"),
            include_str!("../../../protocol/schemas/editor-remove-and-sort-usings.output.json"),
            EditBuffer,
        ),
        CODE_CLEANUP => (
            "Analyze: Code Cleanup",
            include_str!("../../../protocol/schemas/editor-code-cleanup.input.json"),
            include_str!("../../../protocol/schemas/editor-code-cleanup.output.json"),
            EditBuffer,
        ),
        other => unreachable!("not an IntelliSense command: {other}"),
    };
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title.into(),
        input_schema: serde_json::from_str(input).expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(output).expect("protocol schemas are valid JSON"),
        permission,
        agent_visible: true,
    }
}

/// Register the commands on `registry`, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn IntellisenseTarget>) {
    for id in ALL {
        let target = target.clone();
        registry.replace(spec(id), move |value| target.apply(parse(id, value)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_schema_parses_and_names_its_command() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.input_schema["title"], format!("{id} input"));
            assert_eq!(s.output_schema["title"], format!("{id} output"));
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
        }
        assert_eq!(spec(SNIPPETS).permission, PermissionClass::Read);
        assert_eq!(spec(CODE_CLEANUP).permission, PermissionClass::EditBuffer);
    }

    #[test]
    fn input_is_checked() {
        assert_eq!(
            parse(INSERT_SNIPPET, json!({"shortcut": "prop"})).unwrap(),
            IntellisenseRequest::InsertSnippet {
                path: None,
                shortcut: Some("prop".into())
            }
        );
        assert!(parse(INSERT_SNIPPET, json!({"shortcut": ""})).is_err());
        assert!(parse(SURROUND_WITH, json!({"x": 1})).is_err());
        assert_eq!(
            parse(SNIPPETS, json!({"language": "vb"})).unwrap(),
            IntellisenseRequest::Snippets {
                language: Some("vb".into())
            }
        );
        assert!(parse(SNIPPETS, json!({"language": "rust"})).is_err());
        assert_eq!(
            parse(CODE_CLEANUP, json!({})).unwrap(),
            IntellisenseRequest::CodeCleanup {
                path: None,
                profile: 1
            }
        );
        assert!(parse(CODE_CLEANUP, json!({"profile": 3})).is_err());
        assert_eq!(
            parse(FORMAT_SELECTION, json!({"path": "/a.cs"}))
                .unwrap()
                .path(),
            Some("/a.cs")
        );
        assert_eq!(
            parse(REMOVE_AND_SORT_USINGS, json!({})).unwrap().command(),
            REMOVE_AND_SORT_USINGS
        );
    }

    #[test]
    fn registered_commands_reach_the_target() {
        struct Echo;
        impl IntellisenseTarget for Echo {
            fn apply(&self, request: IntellisenseRequest) -> Result<Value, CommandError> {
                Ok(json!({"command": request.command()}))
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        assert_eq!(
            registry.invoke(SNIPPETS, json!({})).unwrap()["command"],
            SNIPPETS
        );
    }
}
