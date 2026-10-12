//! The debugger's inspection commands of brief 0077, beside [`crate::debug`]'s: Show Next Statement, the Modules
//! window's rows, QuickWatch, Enable/Disable (All) Breakpoints, and the Watch 2 to 4 windows' edits through
//! `eludite.debug.watch`. They travel as one [`crate::debug::DebugRequest::Inspect`] so the shell dispatches them in one
//! place; their schemas are `protocol/schemas/debug-{show-next-statement,modules,quickwatch,enable-breakpoints}.*`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{CommandError, CommandId, CommandSpec, PermissionClass};

pub const SHOW_NEXT_STATEMENT: &str = "eludite.debug.show_next_statement";
pub const MODULES: &str = "eludite.debug.modules";
pub const QUICKWATCH: &str = "eludite.debug.quickwatch";
pub const ENABLE_BREAKPOINTS: &str = "eludite.debug.enable_breakpoints";

pub const ALL: [&str; 4] = [SHOW_NEXT_STATEMENT, MODULES, QUICKWATCH, ENABLE_BREAKPOINTS];

/// The Watch windows (Watch 1 to 4).
pub const WATCH_WINDOWS: usize = 4;
/// Modules listed at most.
pub const MAX_MODULES: usize = 2_000;

/// A Watch window edit beyond Watch 1's add and remove.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WatchOp {
    Add(String),
    Remove(usize),
    Edit { index: usize, expression: String },
    Move { from: usize, to: usize },
}

/// One of brief 0077's commands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum InspectRequest {
    ShowNextStatement,
    Modules,
    QuickWatch {
        expression: Option<String>,
    },
    EnableBreakpoints {
        all: bool,
        enabled: Option<bool>,
        path: Option<String>,
        line: Option<u32>,
    },
    /// `eludite.debug.watch` with `window` (1 to 4), `edit` or `move`.
    Watch {
        window: usize,
        op: WatchOp,
    },
}

impl InspectRequest {
    pub fn command(&self) -> &'static str {
        match self {
            InspectRequest::ShowNextStatement => SHOW_NEXT_STATEMENT,
            InspectRequest::Modules => MODULES,
            InspectRequest::QuickWatch { .. } => QUICKWATCH,
            InspectRequest::EnableBreakpoints { .. } => ENABLE_BREAKPOINTS,
            InspectRequest::Watch { .. } => crate::debug::WATCH,
        }
    }
}

/// A row of the Modules window.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModuleRow {
    pub id: String,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub optimized: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_code: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub symbol_status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ModulesOutput {
    pub modules: Vec<ModuleRow>,
    pub total: usize,
}

fn invalid(m: impl Into<String>) -> CommandError {
    CommandError::InvalidInput(m.into())
}

fn input<T: for<'de> Deserialize<'de> + Default>(value: Value) -> Result<T, CommandError> {
    if value.is_null() {
        return Ok(T::default());
    }
    serde_json::from_value(value).map_err(|e| invalid(e.to_string()))
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct Empty {}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct QuickWatchIn {
    expression: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct EnableIn {
    #[serde(default)]
    all: bool,
    enabled: Option<bool>,
    path: Option<String>,
    line: Option<u32>,
}

/// Parse one of [`ALL`]'s inputs.
pub fn parse(id: &str, value: Value) -> Result<InspectRequest, CommandError> {
    Ok(match id {
        SHOW_NEXT_STATEMENT => {
            let _: Empty = input(value)?;
            InspectRequest::ShowNextStatement
        }
        MODULES => {
            let _: Empty = input(value)?;
            InspectRequest::Modules
        }
        QUICKWATCH => {
            let i: QuickWatchIn = input(value)?;
            if i.expression.as_ref().is_some_and(|e| e.trim().is_empty()) {
                return Err(invalid("`expression` must not be empty"));
            }
            InspectRequest::QuickWatch {
                expression: i.expression.map(|e| e.trim().to_owned()),
            }
        }
        ENABLE_BREAKPOINTS => {
            let i: EnableIn = input(value)?;
            if i.all && i.enabled.is_none() {
                return Err(invalid("`all` goes with `enabled` (true or false)"));
            }
            if i.all && (i.path.is_some() || i.line.is_some()) {
                return Err(invalid("`all` takes no `path` or `line`"));
            }
            if i.line == Some(0) {
                return Err(invalid("`line` starts at 1"));
            }
            if i.path.as_ref().is_some_and(|p| p.trim().is_empty()) {
                return Err(invalid("`path` must not be empty"));
            }
            InspectRequest::EnableBreakpoints {
                all: i.all,
                enabled: i.enabled,
                path: i.path,
                line: i.line,
            }
        }
        other => return Err(invalid(format!("not an inspection command: {other}"))),
    })
}

/// `eludite.debug.watch`'s input: `None` when it is Watch 1's plain `add` or `remove` (the existing requests).
pub fn parse_watch(
    window: Option<usize>,
    add: Option<String>,
    remove: Option<usize>,
    edit: Option<Value>,
    mv: Option<Value>,
) -> Result<Option<InspectRequest>, CommandError> {
    let window = window.unwrap_or(1);
    if !(1..=WATCH_WINDOWS).contains(&window) {
        return Err(invalid(format!(
            "`window` is 1 to {WATCH_WINDOWS}, not {window}"
        )));
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct EditIn {
        index: usize,
        expression: String,
    }
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct MoveIn {
        from: usize,
        to: usize,
    }
    let given = [
        add.is_some(),
        remove.is_some(),
        edit.is_some(),
        mv.is_some(),
    ]
    .iter()
    .filter(|b| **b)
    .count();
    if given != 1 {
        return Err(invalid(
            "give exactly one of `add` (an expression), `remove`, `edit` and `move`",
        ));
    }
    let op = if let Some(e) = edit {
        let e: EditIn = serde_json::from_value(e).map_err(|e| invalid(e.to_string()))?;
        if e.expression.trim().is_empty() {
            return Err(invalid("`edit.expression` must not be empty"));
        }
        WatchOp::Edit {
            index: e.index,
            expression: e.expression.trim().to_owned(),
        }
    } else if let Some(m) = mv {
        let m: MoveIn = serde_json::from_value(m).map_err(|e| invalid(e.to_string()))?;
        WatchOp::Move {
            from: m.from,
            to: m.to,
        }
    } else if let Some(a) = add {
        if a.trim().is_empty() {
            return Err(invalid("`add` must not be empty"));
        }
        if window == 1 {
            return Ok(None);
        }
        WatchOp::Add(a.trim().to_owned())
    } else {
        if window == 1 {
            return Ok(None);
        }
        WatchOp::Remove(remove.unwrap_or_default())
    };
    Ok(Some(InspectRequest::Watch { window, op }))
}

/// The public description of one of [`ALL`].
pub fn spec(id: &str) -> CommandSpec {
    use PermissionClass::*;
    macro_rules! schema {
        ($f:literal) => {
            include_str!(concat!("../../../protocol/schemas/", $f))
        };
    }
    let state = schema!("debug-state.output.json");
    let (title, input, output, permission) = match id {
        // Moves what the windows show, not the debuggee.
        SHOW_NEXT_STATEMENT => (
            "Debug: Show Next Statement",
            schema!("debug-show-next-statement.input.json"),
            state,
            Read,
        ),
        MODULES => (
            "Debug: Modules",
            schema!("debug-modules.input.json"),
            schema!("debug-modules.output.json"),
            Read,
        ),
        // Evaluating can call methods in the debuggee, as eludite.debug.evaluate.
        QUICKWATCH => (
            "Debug: QuickWatch",
            schema!("debug-quickwatch.input.json"),
            schema!("debug-evaluate.output.json"),
            Execute,
        ),
        ENABLE_BREAKPOINTS => (
            "Debug: Enable Breakpoints",
            schema!("debug-enable-breakpoints.input.json"),
            state,
            Read,
        ),
        other => unreachable!("not an inspection command: {other}"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn inputs_parse_and_specs_load() {
        for id in ALL {
            let s = spec(id);
            assert_eq!(s.id.as_str(), id);
        }
        assert_eq!(
            parse(SHOW_NEXT_STATEMENT, json!({})).unwrap(),
            InspectRequest::ShowNextStatement
        );
        assert!(parse(SHOW_NEXT_STATEMENT, json!({"x": 1})).is_err());
        assert_eq!(
            parse(QUICKWATCH, json!({"expression": " y "})).unwrap(),
            InspectRequest::QuickWatch {
                expression: Some("y".into())
            }
        );
        assert!(parse(QUICKWATCH, json!({"expression": " "})).is_err());
        assert!(parse(ENABLE_BREAKPOINTS, json!({"all": true})).is_err());
        assert_eq!(
            parse(ENABLE_BREAKPOINTS, json!({"all": true, "enabled": false})).unwrap(),
            InspectRequest::EnableBreakpoints {
                all: true,
                enabled: Some(false),
                path: None,
                line: None
            }
        );
        assert!(parse(ENABLE_BREAKPOINTS, json!({"line": 0})).is_err());
    }

    #[test]
    fn watch_edits_beyond_watch_1_are_inspection_requests() {
        assert_eq!(
            parse_watch(None, Some("x".into()), None, None, None).unwrap(),
            None
        );
        assert_eq!(
            parse_watch(Some(2), Some("x".into()), None, None, None).unwrap(),
            Some(InspectRequest::Watch {
                window: 2,
                op: WatchOp::Add("x".into())
            })
        );
        assert_eq!(
            parse_watch(
                None,
                None,
                None,
                Some(json!({"index": 0, "expression": "y"})),
                None
            )
            .unwrap(),
            Some(InspectRequest::Watch {
                window: 1,
                op: WatchOp::Edit {
                    index: 0,
                    expression: "y".into()
                }
            })
        );
        assert!(parse_watch(Some(5), Some("x".into()), None, None, None).is_err());
        assert!(parse_watch(None, Some("x".into()), Some(0), None, None).is_err());
        assert!(parse_watch(None, None, None, None, None).is_err());
    }
}
