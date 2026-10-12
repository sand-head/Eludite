//! The editor view's commands (brief 0067): Edit > Outlining (`eludite.editor.toggle_outlining_expansion`,
//! `toggle_all_outlining`, `stop_outlining`, `start_automatic_outlining`, `collapse_to_definitions`,
//! `expand_all_outlining`, `collapse_all_outlining`, `collapse_current_region`, `expand_current_region`,
//! `hide_selection`, `stop_hiding_current`), Edit > Advanced's Word Wrap and View White Space
//! (`toggle_word_wrap`, `toggle_white_space`), Go To Brace and Extend to Brace (`go_to_brace`, `extend_to_brace`),
//! View > Zoom In and Zoom Out with the zoom box (`zoom_in`, `zoom_out`, `zoom`), and the read-only
//! `eludite.editor.view_state`. Registered by the shell through an [`OutliningTarget`]; every one answers with the
//! document's display state (`editor-view-state.output.json`).

use std::sync::Arc;

use serde_json::Value;

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const TOGGLE_OUTLINING_EXPANSION: &str = "eludite.editor.toggle_outlining_expansion";
pub const TOGGLE_ALL_OUTLINING: &str = "eludite.editor.toggle_all_outlining";
pub const STOP_OUTLINING: &str = "eludite.editor.stop_outlining";
pub const START_AUTOMATIC_OUTLINING: &str = "eludite.editor.start_automatic_outlining";
pub const COLLAPSE_TO_DEFINITIONS: &str = "eludite.editor.collapse_to_definitions";
pub const EXPAND_ALL_OUTLINING: &str = "eludite.editor.expand_all_outlining";
pub const COLLAPSE_ALL_OUTLINING: &str = "eludite.editor.collapse_all_outlining";
pub const COLLAPSE_CURRENT_REGION: &str = "eludite.editor.collapse_current_region";
pub const EXPAND_CURRENT_REGION: &str = "eludite.editor.expand_current_region";
pub const HIDE_SELECTION: &str = "eludite.editor.hide_selection";
pub const STOP_HIDING_CURRENT: &str = "eludite.editor.stop_hiding_current";
pub const TOGGLE_WORD_WRAP: &str = "eludite.editor.toggle_word_wrap";
pub const TOGGLE_WHITE_SPACE: &str = "eludite.editor.toggle_white_space";
pub const GO_TO_BRACE: &str = "eludite.editor.go_to_brace";
pub const EXTEND_TO_BRACE: &str = "eludite.editor.extend_to_brace";
pub const ZOOM_IN: &str = "eludite.editor.zoom_in";
pub const ZOOM_OUT: &str = "eludite.editor.zoom_out";
pub const ZOOM: &str = "eludite.editor.zoom";
pub const VIEW_STATE: &str = "eludite.editor.view_state";

pub const ALL: [&str; 19] = [
    TOGGLE_OUTLINING_EXPANSION,
    TOGGLE_ALL_OUTLINING,
    STOP_OUTLINING,
    START_AUTOMATIC_OUTLINING,
    COLLAPSE_TO_DEFINITIONS,
    EXPAND_ALL_OUTLINING,
    COLLAPSE_ALL_OUTLINING,
    COLLAPSE_CURRENT_REGION,
    EXPAND_CURRENT_REGION,
    HIDE_SELECTION,
    STOP_HIDING_CURRENT,
    TOGGLE_WORD_WRAP,
    TOGGLE_WHITE_SPACE,
    GO_TO_BRACE,
    EXTEND_TO_BRACE,
    ZOOM_IN,
    ZOOM_OUT,
    ZOOM,
    VIEW_STATE,
];

const OUTPUT: &str = include_str!("../../../protocol/schemas/editor-view-state.output.json");

/// What a command does to the document's view.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OutliningAction {
    /// The innermost region at the caret, or at a 1-based line.
    ToggleExpansion {
        line: Option<u32>,
    },
    ToggleAll,
    Stop,
    Start,
    CollapseToDefinitions,
    ExpandAll,
    CollapseAll,
    CollapseCurrent,
    ExpandCurrent,
    HideSelection,
    StopHidingCurrent,
    /// `None` toggles.
    WordWrap(Option<bool>),
    WhiteSpace(Option<bool>),
    GoToBrace,
    ExtendToBrace,
    ZoomIn,
    ZoomOut,
    Zoom(u32),
    State,
}

/// A parsed, validated command: the document (`None`: the active one) and the action.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutliningRequest {
    pub path: Option<String>,
    pub action: OutliningAction,
}

impl OutliningRequest {
    pub fn command(&self) -> &'static str {
        use OutliningAction::*;
        match self.action {
            ToggleExpansion { .. } => TOGGLE_OUTLINING_EXPANSION,
            ToggleAll => TOGGLE_ALL_OUTLINING,
            Stop => STOP_OUTLINING,
            Start => START_AUTOMATIC_OUTLINING,
            CollapseToDefinitions => COLLAPSE_TO_DEFINITIONS,
            ExpandAll => EXPAND_ALL_OUTLINING,
            CollapseAll => COLLAPSE_ALL_OUTLINING,
            CollapseCurrent => COLLAPSE_CURRENT_REGION,
            ExpandCurrent => EXPAND_CURRENT_REGION,
            HideSelection => HIDE_SELECTION,
            StopHidingCurrent => STOP_HIDING_CURRENT,
            WordWrap(_) => TOGGLE_WORD_WRAP,
            WhiteSpace(_) => TOGGLE_WHITE_SPACE,
            GoToBrace => GO_TO_BRACE,
            ExtendToBrace => EXTEND_TO_BRACE,
            ZoomIn => ZOOM_IN,
            ZoomOut => ZOOM_OUT,
            Zoom(_) => ZOOM,
            State => VIEW_STATE,
        }
    }
}

/// What applies the commands (the shell).
pub trait OutliningTarget: Send + Sync {
    fn apply(&self, request: OutliningRequest) -> Result<Value, CommandError>;
}

fn no_extra(value: &Value, allowed: &[&str]) -> Result<(), CommandError> {
    let Some(obj) = value.as_object() else {
        return Err(CommandError::InvalidInput("input must be an object".into()));
    };
    if let Some(k) = obj
        .keys()
        .find(|k| k.as_str() != "path" && !allowed.contains(&k.as_str()))
    {
        return Err(CommandError::InvalidInput(format!("unknown field `{k}`")));
    }
    Ok(())
}

fn flag(value: &Value, name: &str) -> Result<Option<bool>, CommandError> {
    match value.get(name) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::Bool(b)) => Ok(Some(*b)),
        Some(_) => Err(CommandError::InvalidInput(format!(
            "`{name}` must be a boolean"
        ))),
    }
}

/// Parse and validate a command's input.
pub fn parse(id: &str, value: Value) -> Result<OutliningRequest, CommandError> {
    use OutliningAction::*;
    let path = match value.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => {
            return Err(CommandError::InvalidInput(
                "`path` must be a non-empty string".into(),
            ));
        }
    };
    let action = match id {
        TOGGLE_OUTLINING_EXPANSION => {
            no_extra(&value, &["line"])?;
            let line = match value.get("line") {
                None | Some(Value::Null) => None,
                Some(v) => match v.as_u64() {
                    Some(n) if (1..=u32::MAX as u64).contains(&n) => Some(n as u32),
                    _ => {
                        return Err(CommandError::InvalidInput(
                            "`line` must be an integer of at least 1".into(),
                        ));
                    }
                },
            };
            ToggleExpansion { line }
        }
        TOGGLE_WORD_WRAP => {
            no_extra(&value, &["enabled"])?;
            WordWrap(flag(&value, "enabled")?)
        }
        TOGGLE_WHITE_SPACE => {
            no_extra(&value, &["enabled"])?;
            WhiteSpace(flag(&value, "enabled")?)
        }
        ZOOM => {
            no_extra(&value, &["level"])?;
            match value.get("level").and_then(Value::as_u64) {
                Some(n) if (20..=400).contains(&n) => Zoom(n as u32),
                _ => {
                    return Err(CommandError::InvalidInput(
                        "`level` must be an integer from 20 to 400".into(),
                    ));
                }
            }
        }
        other => {
            no_extra(&value, &[])?;
            match other {
                TOGGLE_ALL_OUTLINING => ToggleAll,
                STOP_OUTLINING => Stop,
                START_AUTOMATIC_OUTLINING => Start,
                COLLAPSE_TO_DEFINITIONS => CollapseToDefinitions,
                EXPAND_ALL_OUTLINING => ExpandAll,
                COLLAPSE_ALL_OUTLINING => CollapseAll,
                COLLAPSE_CURRENT_REGION => CollapseCurrent,
                EXPAND_CURRENT_REGION => ExpandCurrent,
                HIDE_SELECTION => HideSelection,
                STOP_HIDING_CURRENT => StopHidingCurrent,
                GO_TO_BRACE => GoToBrace,
                EXTEND_TO_BRACE => ExtendToBrace,
                ZOOM_IN => ZoomIn,
                ZOOM_OUT => ZoomOut,
                VIEW_STATE => State,
                other => {
                    return Err(CommandError::InvalidInput(format!(
                        "not an editor view command: {other}"
                    )));
                }
            }
        }
    };
    Ok(OutliningRequest { path, action })
}

fn file_name(id: &str) -> String {
    format!(
        "editor-{}.input.json",
        id.trim_start_matches("eludite.editor.").replace('_', "-")
    )
}

fn input_schema(id: &str) -> &'static str {
    match id {
        TOGGLE_OUTLINING_EXPANSION => {
            include_str!("../../../protocol/schemas/editor-toggle-outlining-expansion.input.json")
        }
        TOGGLE_ALL_OUTLINING => {
            include_str!("../../../protocol/schemas/editor-toggle-all-outlining.input.json")
        }
        STOP_OUTLINING => {
            include_str!("../../../protocol/schemas/editor-stop-outlining.input.json")
        }
        START_AUTOMATIC_OUTLINING => {
            include_str!("../../../protocol/schemas/editor-start-automatic-outlining.input.json")
        }
        COLLAPSE_TO_DEFINITIONS => {
            include_str!("../../../protocol/schemas/editor-collapse-to-definitions.input.json")
        }
        EXPAND_ALL_OUTLINING => {
            include_str!("../../../protocol/schemas/editor-expand-all-outlining.input.json")
        }
        COLLAPSE_ALL_OUTLINING => {
            include_str!("../../../protocol/schemas/editor-collapse-all-outlining.input.json")
        }
        COLLAPSE_CURRENT_REGION => {
            include_str!("../../../protocol/schemas/editor-collapse-current-region.input.json")
        }
        EXPAND_CURRENT_REGION => {
            include_str!("../../../protocol/schemas/editor-expand-current-region.input.json")
        }
        HIDE_SELECTION => {
            include_str!("../../../protocol/schemas/editor-hide-selection.input.json")
        }
        STOP_HIDING_CURRENT => {
            include_str!("../../../protocol/schemas/editor-stop-hiding-current.input.json")
        }
        TOGGLE_WORD_WRAP => {
            include_str!("../../../protocol/schemas/editor-toggle-word-wrap.input.json")
        }
        TOGGLE_WHITE_SPACE => {
            include_str!("../../../protocol/schemas/editor-toggle-white-space.input.json")
        }
        GO_TO_BRACE => include_str!("../../../protocol/schemas/editor-go-to-brace.input.json"),
        EXTEND_TO_BRACE => {
            include_str!("../../../protocol/schemas/editor-extend-to-brace.input.json")
        }
        ZOOM_IN => include_str!("../../../protocol/schemas/editor-zoom-in.input.json"),
        ZOOM_OUT => include_str!("../../../protocol/schemas/editor-zoom-out.input.json"),
        ZOOM => include_str!("../../../protocol/schemas/editor-zoom.input.json"),
        VIEW_STATE => include_str!("../../../protocol/schemas/editor-view-state.input.json"),
        other => unreachable!("not an editor view command: {other}"),
    }
}

fn title(id: &str) -> &'static str {
    match id {
        TOGGLE_OUTLINING_EXPANSION => "Edit: Outlining: Toggle Outlining Expansion",
        TOGGLE_ALL_OUTLINING => "Edit: Outlining: Toggle All Outlining",
        STOP_OUTLINING => "Edit: Outlining: Stop Outlining",
        START_AUTOMATIC_OUTLINING => "Edit: Outlining: Start Automatic Outlining",
        COLLAPSE_TO_DEFINITIONS => "Edit: Outlining: Collapse to Definitions",
        EXPAND_ALL_OUTLINING => "Edit: Outlining: Expand All Outlining",
        COLLAPSE_ALL_OUTLINING => "Edit: Outlining: Collapse All Outlining",
        COLLAPSE_CURRENT_REGION => "Edit: Outlining: Collapse Current Region",
        EXPAND_CURRENT_REGION => "Edit: Outlining: Expand Current Region",
        HIDE_SELECTION => "Edit: Outlining: Hide Selection",
        STOP_HIDING_CURRENT => "Edit: Outlining: Stop Hiding Current",
        TOGGLE_WORD_WRAP => "Edit: Advanced: Word Wrap",
        TOGGLE_WHITE_SPACE => "Edit: Advanced: View White Space",
        GO_TO_BRACE => "Edit: Go To Brace",
        EXTEND_TO_BRACE => "Edit: Extend to Brace",
        ZOOM_IN => "View: Zoom In",
        ZOOM_OUT => "View: Zoom Out",
        ZOOM => "View: Zoom",
        VIEW_STATE => "Editor: View State",
        other => unreachable!("not an editor view command: {other}"),
    }
}

pub fn spec(id: &str) -> CommandSpec {
    CommandSpec {
        id: CommandId::new(id).expect("valid id"),
        title: title(id).into(),
        input_schema: serde_json::from_str(input_schema(id))
            .expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(OUTPUT).expect("protocol schemas are valid JSON"),
        // The view changes, the text never does (Go To Brace moves the caret, as reading a document does).
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

/// Register the commands on `registry`, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn OutliningTarget>) {
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
            assert_eq!(s.input_schema["additionalProperties"], false, "{id}");
            assert_eq!(
                s.input_schema["$id"],
                format!(
                    "https://github.com/sand-head/Eludite/protocol/schemas/{}",
                    file_name(id)
                )
            );
            assert_eq!(s.output_schema["additionalProperties"], false);
            assert_eq!(s.permission, PermissionClass::Read);
            assert!(s.agent_visible);
        }
    }

    #[test]
    fn input_is_checked() {
        use OutliningAction::*;
        assert_eq!(
            parse(
                TOGGLE_OUTLINING_EXPANSION,
                json!({"path": "/a.cs", "line": 3})
            )
            .unwrap(),
            OutliningRequest {
                path: Some("/a.cs".into()),
                action: ToggleExpansion { line: Some(3) }
            }
        );
        assert!(parse(TOGGLE_OUTLINING_EXPANSION, json!({"line": 0})).is_err());
        assert!(parse(TOGGLE_ALL_OUTLINING, json!({"line": 3})).is_err());
        assert_eq!(
            parse(TOGGLE_WORD_WRAP, json!({"enabled": true}))
                .unwrap()
                .action,
            WordWrap(Some(true))
        );
        assert_eq!(
            parse(TOGGLE_WORD_WRAP, json!({})).unwrap().action,
            WordWrap(None)
        );
        assert!(parse(TOGGLE_WHITE_SPACE, json!({"enabled": 1})).is_err());
        assert_eq!(
            parse(ZOOM, json!({"level": 150})).unwrap().action,
            Zoom(150)
        );
        assert!(parse(ZOOM, json!({"level": 401})).is_err());
        assert!(parse(ZOOM, json!({})).is_err());
        assert!(parse(VIEW_STATE, json!({"path": ""})).is_err());
        assert!(parse(VIEW_STATE, json!("x")).is_err());
        for id in ALL {
            if id != ZOOM {
                assert_eq!(parse(id, json!({})).unwrap().command(), id);
            }
        }
        assert!(parse("eludite.editor.other", json!({})).is_err());
    }

    #[test]
    fn registered_commands_reach_the_target() {
        struct Echo;
        impl OutliningTarget for Echo {
            fn apply(&self, request: OutliningRequest) -> Result<Value, CommandError> {
                Ok(json!({"command": request.command()}))
            }
        }
        let registry = CommandRegistry::new();
        register(&registry, Arc::new(Echo));
        assert_eq!(
            registry.invoke(COLLAPSE_TO_DEFINITIONS, json!({})).unwrap()["command"],
            COLLAPSE_TO_DEFINITIONS
        );
        assert!(registry.invoke(ZOOM, json!({"level": 5})).is_err());
    }
}
