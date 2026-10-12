//! The editor's editing operations as commands (brief 0068): Edit > Advanced's indent, comment, line and case
//! commands, Expand and Contract Selection, the Clipboard Ring, selections (`eludite.editor.select`, so an agent can
//! set up the others), Go To Line, Quick Replace, and the Edit menu's Cut, Copy, Paste and Select All. Each acts on an
//! open document (default the active one) and returns its selections afterwards.
//!
//! The schemas are `protocol/schemas/editor-edit.*`, `editor-move-lines.input.json`, `editor-select.input.json`,
//! `editor-go-to-line.input.json` and `editor-replace.*`. The shell implements [`EditingCommands`] on the UI thread;
//! the editor's keys run the same `Editor` operations directly.

use std::sync::Arc;

use serde::Deserialize;
use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const INDENT: &str = "eludite.editor.indent";
pub const OUTDENT: &str = "eludite.editor.outdent";
pub const COMMENT_SELECTION: &str = "eludite.editor.comment_selection";
pub const UNCOMMENT_SELECTION: &str = "eludite.editor.uncomment_selection";
pub const TOGGLE_LINE_COMMENT: &str = "eludite.editor.toggle_line_comment";
pub const TOGGLE_BLOCK_COMMENT: &str = "eludite.editor.toggle_block_comment";
pub const DUPLICATE: &str = "eludite.editor.duplicate";
pub const DELETE_LINE: &str = "eludite.editor.delete_line";
pub const LINE_CUT: &str = "eludite.editor.line_cut";
pub const OPEN_LINE_ABOVE: &str = "eludite.editor.open_line_above";
pub const OPEN_LINE_BELOW: &str = "eludite.editor.open_line_below";
pub const MAKE_UPPERCASE: &str = "eludite.editor.make_uppercase";
pub const MAKE_LOWERCASE: &str = "eludite.editor.make_lowercase";
pub const TRANSPOSE_LINE: &str = "eludite.editor.transpose_line";
pub const DELETE_HORIZONTAL_WHITE_SPACE: &str = "eludite.editor.delete_horizontal_white_space";
pub const PASTE_RING: &str = "eludite.editor.paste_ring";
pub const EXPAND_SELECTION: &str = "eludite.editor.expand_selection";
pub const CONTRACT_SELECTION: &str = "eludite.editor.contract_selection";
pub const MOVE_LINES: &str = "eludite.editor.move_lines";
pub const SELECT: &str = "eludite.editor.select";
pub const GO_TO_LINE: &str = "eludite.editor.go_to_line";
pub const REPLACE: &str = "eludite.editor.replace";
/// The Edit menu's clipboard items.
pub const CUT: &str = "eludite.edit.cut";
pub const COPY: &str = "eludite.edit.copy";
pub const PASTE: &str = "eludite.edit.paste";
pub const SELECT_ALL: &str = "eludite.edit.select_all";

/// The commands that take only `path` (`editor-edit.input.json`), with their titles and classes.
pub const OPERATIONS: [(&str, &str, PermissionClass); 22] = [
    (
        INDENT,
        "Edit: Increase Line Indent",
        PermissionClass::EditBuffer,
    ),
    (
        OUTDENT,
        "Edit: Decrease Line Indent",
        PermissionClass::EditBuffer,
    ),
    (
        COMMENT_SELECTION,
        "Edit: Comment Selection",
        PermissionClass::EditBuffer,
    ),
    (
        UNCOMMENT_SELECTION,
        "Edit: Uncomment Selection",
        PermissionClass::EditBuffer,
    ),
    (
        TOGGLE_LINE_COMMENT,
        "Edit: Toggle Line Comment",
        PermissionClass::EditBuffer,
    ),
    (
        TOGGLE_BLOCK_COMMENT,
        "Edit: Toggle Block Comment",
        PermissionClass::EditBuffer,
    ),
    (DUPLICATE, "Edit: Duplicate", PermissionClass::EditBuffer),
    (
        DELETE_LINE,
        "Edit: Delete Line",
        PermissionClass::EditBuffer,
    ),
    (LINE_CUT, "Edit: Line Cut", PermissionClass::EditBuffer),
    (
        OPEN_LINE_ABOVE,
        "Edit: Open Line Above",
        PermissionClass::EditBuffer,
    ),
    (
        OPEN_LINE_BELOW,
        "Edit: Open Line Below",
        PermissionClass::EditBuffer,
    ),
    (
        MAKE_UPPERCASE,
        "Edit: Make Uppercase",
        PermissionClass::EditBuffer,
    ),
    (
        MAKE_LOWERCASE,
        "Edit: Make Lowercase",
        PermissionClass::EditBuffer,
    ),
    (
        TRANSPOSE_LINE,
        "Edit: Transpose Line",
        PermissionClass::EditBuffer,
    ),
    (
        DELETE_HORIZONTAL_WHITE_SPACE,
        "Edit: Delete Horizontal White Space",
        PermissionClass::EditBuffer,
    ),
    (
        PASTE_RING,
        "Edit: Cycle Clipboard Ring",
        PermissionClass::EditBuffer,
    ),
    (
        EXPAND_SELECTION,
        "Edit: Expand Selection",
        PermissionClass::Read,
    ),
    (
        CONTRACT_SELECTION,
        "Edit: Contract Selection",
        PermissionClass::Read,
    ),
    (CUT, "Edit: Cut", PermissionClass::EditBuffer),
    (COPY, "Edit: Copy", PermissionClass::Read),
    (PASTE, "Edit: Paste", PermissionClass::EditBuffer),
    (SELECT_ALL, "Edit: Select All", PermissionClass::Read),
];

/// Every command this module registers.
pub fn all() -> impl Iterator<Item = &'static str> {
    OPERATIONS
        .iter()
        .map(|(id, _, _)| *id)
        .chain([MOVE_LINES, SELECT, GO_TO_LINE, REPLACE])
}

/// One selection to set, 1-based.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangeSpec {
    pub line: u32,
    pub column: u32,
    pub end_line: Option<u32>,
    pub end_column: Option<u32>,
}

/// A box corner: 1-based line and visual column.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxPoint {
    pub line: u32,
    pub column: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BoxSpec {
    pub anchor: BoxPoint,
    pub head: BoxPoint,
}

/// What `eludite.editor.select` sets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SelectionSpec {
    Ranges(Vec<RangeSpec>),
    Box(BoxSpec),
}

/// Quick Replace's input.
#[derive(Debug, Clone, Default, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReplaceArgs {
    pub path: Option<String>,
    pub query: Option<String>,
    #[serde(default)]
    pub replacement: String,
    #[serde(default)]
    pub match_case: bool,
    #[serde(default)]
    pub whole_word: bool,
    #[serde(default)]
    pub regex: bool,
    #[serde(default)]
    pub in_selection: bool,
    #[serde(default)]
    pub all: bool,
}

/// A parsed editing command.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditingRequest {
    /// One of [`OPERATIONS`].
    Operation {
        command: &'static str,
        path: Option<String>,
    },
    MoveLines {
        path: Option<String>,
        up: bool,
    },
    Select {
        path: Option<String>,
        selection: SelectionSpec,
    },
    GoToLine {
        path: Option<String>,
        line: Option<u32>,
    },
    Replace(ReplaceArgs),
}

impl EditingRequest {
    pub fn command(&self) -> &'static str {
        match self {
            EditingRequest::Operation { command, .. } => command,
            EditingRequest::MoveLines { .. } => MOVE_LINES,
            EditingRequest::Select { .. } => SELECT,
            EditingRequest::GoToLine { .. } => GO_TO_LINE,
            EditingRequest::Replace(_) => REPLACE,
        }
    }

    pub fn path(&self) -> Option<&str> {
        match self {
            EditingRequest::Operation { path, .. }
            | EditingRequest::MoveLines { path, .. }
            | EditingRequest::Select { path, .. }
            | EditingRequest::GoToLine { path, .. } => path.as_deref(),
            EditingRequest::Replace(a) => a.path.as_deref(),
        }
    }
}

/// The shell's side.
pub trait EditingCommands: Send + Sync {
    fn apply(&self, request: EditingRequest) -> Result<Value, CommandError>;
}

fn invalid(e: impl std::fmt::Display) -> CommandError {
    CommandError::InvalidInput(e.to_string())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PathOnly {
    path: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MoveArgs {
    path: Option<String>,
    direction: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SelectArgs {
    path: Option<String>,
    ranges: Option<Vec<RangeSpec>>,
    #[serde(rename = "box")]
    boxed: Option<BoxSpec>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GoToArgs {
    path: Option<String>,
    line: Option<u32>,
}

fn check_path(path: &Option<String>) -> Result<(), CommandError> {
    if path.as_deref() == Some("") {
        return Err(invalid("`path` must not be empty"));
    }
    Ok(())
}

fn positive(n: u32, name: &str) -> Result<(), CommandError> {
    if n == 0 {
        return Err(invalid(format!("`{name}` is 1-based")));
    }
    Ok(())
}

/// Parse and check the input of command `id`.
pub fn parse(id: &str, value: Value) -> Result<EditingRequest, CommandError> {
    if let Some((command, _, _)) = OPERATIONS.iter().find(|(c, _, _)| *c == id) {
        let a: PathOnly = serde_json::from_value(value).map_err(invalid)?;
        check_path(&a.path)?;
        return Ok(EditingRequest::Operation {
            command,
            path: a.path,
        });
    }
    match id {
        MOVE_LINES => {
            let a: MoveArgs = serde_json::from_value(value).map_err(invalid)?;
            check_path(&a.path)?;
            let up = match a.direction.as_str() {
                "up" => true,
                "down" => false,
                _ => return Err(invalid("`direction` is `up` or `down`")),
            };
            Ok(EditingRequest::MoveLines { path: a.path, up })
        }
        SELECT => {
            let a: SelectArgs = serde_json::from_value(value).map_err(invalid)?;
            check_path(&a.path)?;
            let selection = match (a.ranges, a.boxed) {
                (Some(r), None) if !r.is_empty() => {
                    for s in &r {
                        positive(s.line, "line")?;
                        positive(s.column, "column")?;
                        positive(s.end_line.unwrap_or(1), "end_line")?;
                        positive(s.end_column.unwrap_or(1), "end_column")?;
                    }
                    SelectionSpec::Ranges(r)
                }
                (None, Some(b)) => {
                    for p in [b.anchor, b.head] {
                        positive(p.line, "line")?;
                        positive(p.column, "column")?;
                    }
                    SelectionSpec::Box(b)
                }
                _ => {
                    return Err(invalid(
                        "give exactly one of `ranges` (not empty) and `box`",
                    ));
                }
            };
            Ok(EditingRequest::Select {
                path: a.path,
                selection,
            })
        }
        GO_TO_LINE => {
            let a: GoToArgs = serde_json::from_value(value).map_err(invalid)?;
            check_path(&a.path)?;
            if let Some(l) = a.line {
                positive(l, "line")?;
            }
            Ok(EditingRequest::GoToLine {
                path: a.path,
                line: a.line,
            })
        }
        REPLACE => {
            let a: ReplaceArgs = serde_json::from_value(value).map_err(invalid)?;
            check_path(&a.path)?;
            if a.query.as_deref() == Some("") {
                return Err(invalid("`query` must not be empty"));
            }
            Ok(EditingRequest::Replace(a))
        }
        other => Err(invalid(format!("not an editing command: {other}"))),
    }
}

const EDIT_INPUT: &str = include_str!("../../../protocol/schemas/editor-edit.input.json");
const EDIT_OUTPUT: &str = include_str!("../../../protocol/schemas/editor-edit.output.json");

/// The spec of command `id`.
pub fn spec(id: &str) -> CommandSpec {
    let (title, input, output, permission) =
        if let Some((_, title, class)) = OPERATIONS.iter().find(|(c, _, _)| *c == id) {
            (*title, EDIT_INPUT, EDIT_OUTPUT, *class)
        } else {
            match id {
                MOVE_LINES => (
                    "Edit: Move Selected Lines",
                    include_str!("../../../protocol/schemas/editor-move-lines.input.json"),
                    EDIT_OUTPUT,
                    PermissionClass::EditBuffer,
                ),
                SELECT => (
                    "Edit: Select",
                    include_str!("../../../protocol/schemas/editor-select.input.json"),
                    EDIT_OUTPUT,
                    PermissionClass::Read,
                ),
                GO_TO_LINE => (
                    "Edit: Go To Line",
                    include_str!("../../../protocol/schemas/editor-go-to-line.input.json"),
                    EDIT_OUTPUT,
                    PermissionClass::Read,
                ),
                REPLACE => (
                    "Edit: Quick Replace",
                    include_str!("../../../protocol/schemas/editor-replace.input.json"),
                    include_str!("../../../protocol/schemas/editor-replace.output.json"),
                    PermissionClass::EditBuffer,
                ),
                other => unreachable!("not an editing command: {other}"),
            }
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

/// Register every editing command, routed to `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn EditingCommands>) {
    for id in all() {
        let target = target.clone();
        registry.replace(spec(id), move |value| target.apply(parse(id, value)?));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn every_schema_parses() {
        for id in all() {
            let s = spec(id);
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(s.output_schema["additionalProperties"], false, "{id}");
            assert!(s.agent_visible);
        }
        assert_eq!(all().count(), 26);
        assert_eq!(spec(EXPAND_SELECTION).permission, PermissionClass::Read);
        assert_eq!(spec(REPLACE).permission, PermissionClass::EditBuffer);
        assert_eq!(
            spec(MOVE_LINES).input_schema["title"],
            "eludite.editor.move_lines input"
        );
    }

    #[test]
    fn input_is_checked() {
        assert_eq!(
            parse(DUPLICATE, json!({"path": "/a.cs"})).unwrap(),
            EditingRequest::Operation {
                command: DUPLICATE,
                path: Some("/a.cs".into())
            }
        );
        assert!(parse(DUPLICATE, json!({"x": 1})).is_err());
        assert!(parse(DUPLICATE, json!({"path": ""})).is_err());
        assert_eq!(
            parse(MOVE_LINES, json!({"direction": "up"})).unwrap(),
            EditingRequest::MoveLines {
                path: None,
                up: true
            }
        );
        assert!(parse(MOVE_LINES, json!({"direction": "left"})).is_err());
        assert!(parse(MOVE_LINES, json!({})).is_err());
        assert!(matches!(
            parse(SELECT, json!({"ranges": [{"line": 1, "column": 1}]})).unwrap(),
            EditingRequest::Select {
                selection: SelectionSpec::Ranges(_),
                ..
            }
        ));
        assert!(matches!(
            parse(SELECT, json!({"box": {"anchor": {"line": 1, "column": 2}, "head": {"line": 3, "column": 9}}}))
                .unwrap(),
            EditingRequest::Select { selection: SelectionSpec::Box(_), .. }
        ));
        assert!(parse(SELECT, json!({})).is_err());
        assert!(parse(SELECT, json!({"ranges": [{"line": 0, "column": 1}]})).is_err());
        assert_eq!(
            parse(GO_TO_LINE, json!({"line": 7})).unwrap(),
            EditingRequest::GoToLine {
                path: None,
                line: Some(7)
            }
        );
        assert!(parse(GO_TO_LINE, json!({"line": 0})).is_err());
        let r = parse(
            REPLACE,
            json!({"query": "a", "replacement": "b", "all": true, "regex": true}),
        )
        .unwrap();
        assert_eq!(r.command(), REPLACE);
        assert!(parse(REPLACE, json!({"query": ""})).is_err());
        assert!(parse("eludite.editor.nope", json!({})).is_err());
    }

    #[test]
    fn registered_commands_reach_the_target() {
        struct Echo;
        impl EditingCommands for Echo {
            fn apply(&self, request: EditingRequest) -> Result<Value, CommandError> {
                Ok(json!({"command": request.command()}))
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        assert_eq!(
            registry.invoke(COMMENT_SELECTION, json!({})).unwrap()["command"],
            COMMENT_SELECTION
        );
        assert_eq!(registry.invoke(CUT, json!({})).unwrap()["command"], CUT);
        assert!(registry.invoke(GO_TO_LINE, json!({"line": "x"})).is_err());
    }
}
