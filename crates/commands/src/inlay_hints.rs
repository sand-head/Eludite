//! `eludite.editor.toggle_inlay_hints` (brief 0066): Edit > IntelliSense > Toggle Inline Hints, Alt+F1. It shows or
//! hides the language servers' inlay hints in every editor for the session (the `editor.inlayHints.*` settings are
//! unchanged). The shell applies it on the UI thread through [`InlayHintsTarget`].

use std::sync::Arc;

use serde_json::{Value, json};

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const TOGGLE: &str = "eludite.editor.toggle_inlay_hints";

/// The command's input: the document to count hints of (the active one when `None`), and the state to set (flip when
/// `None`).
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ToggleRequest {
    pub path: Option<String>,
    pub enabled: Option<bool>,
}

/// The command's output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToggleResult {
    pub path: Option<String>,
    pub enabled: bool,
    pub hints: usize,
}

impl ToggleResult {
    pub fn to_json(&self) -> Value {
        let mut v = json!({"enabled": self.enabled, "hints": self.hints});
        if let Some(p) = &self.path {
            v["path"] = json!(p);
        }
        v
    }
}

/// Applies the toggle (the shell).
pub trait InlayHintsTarget: Send + Sync {
    fn toggle(&self, request: ToggleRequest) -> Result<ToggleResult, CommandError>;
}

pub fn parse(value: Value) -> Result<ToggleRequest, CommandError> {
    let obj = match &value {
        Value::Null => return Ok(ToggleRequest::default()),
        Value::Object(o) => o,
        _ => return Err(CommandError::InvalidInput("input must be an object".into())),
    };
    if let Some(k) = obj.keys().find(|k| *k != "path" && *k != "enabled") {
        return Err(CommandError::InvalidInput(format!("unknown field `{k}`")));
    }
    let path = match obj.get("path") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) if !s.is_empty() => Some(s.clone()),
        Some(_) => {
            return Err(CommandError::InvalidInput(
                "`path` must be a non-empty string".into(),
            ));
        }
    };
    let enabled = match obj.get("enabled") {
        None | Some(Value::Null) => None,
        Some(Value::Bool(b)) => Some(*b),
        Some(_) => {
            return Err(CommandError::InvalidInput(
                "`enabled` must be a boolean".into(),
            ));
        }
    };
    Ok(ToggleRequest { path, enabled })
}

pub fn spec() -> CommandSpec {
    CommandSpec {
        id: CommandId::new(TOGGLE).expect("valid id"),
        title: "Edit: Toggle Inline Hints".into(),
        input_schema: serde_json::from_str(include_str!(
            "../../../protocol/schemas/editor-toggle-inlay-hints.input.json"
        ))
        .expect("protocol schemas are valid JSON"),
        output_schema: serde_json::from_str(include_str!(
            "../../../protocol/schemas/editor-toggle-inlay-hints.output.json"
        ))
        .expect("protocol schemas are valid JSON"),
        permission: PermissionClass::Read,
        agent_visible: true,
    }
}

/// Register the command, applied by `target`.
pub fn register(registry: &CommandRegistry, target: Arc<dyn InlayHintsTarget>) {
    registry.replace(spec(), move |value| {
        target.toggle(parse(value)?).map(|r| r.to_json())
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_names_the_command_and_parses_input() {
        let s = spec();
        assert_eq!(s.input_schema["title"], format!("{TOGGLE} input"));
        assert_eq!(s.output_schema["title"], format!("{TOGGLE} output"));
        assert_eq!(s.input_schema["additionalProperties"], false);
        assert_eq!(s.output_schema["additionalProperties"], false);
        assert!(s.agent_visible);
        assert_eq!(parse(json!({})).unwrap(), ToggleRequest::default());
        assert_eq!(
            parse(json!({"path": "/a.cs", "enabled": false})).unwrap(),
            ToggleRequest {
                path: Some("/a.cs".into()),
                enabled: Some(false)
            }
        );
        assert!(parse(json!({"enabled": "yes"})).is_err());
        assert!(parse(json!({"other": 1})).is_err());
        assert!(parse(json!({"path": ""})).is_err());
        let out = ToggleResult {
            path: None,
            enabled: true,
            hints: 3,
        }
        .to_json();
        assert_eq!(out, json!({"enabled": true, "hints": 3}));
    }

    #[test]
    fn register_routes_to_the_target() {
        struct T;
        impl InlayHintsTarget for T {
            fn toggle(&self, r: ToggleRequest) -> Result<ToggleResult, CommandError> {
                Ok(ToggleResult {
                    path: r.path,
                    enabled: r.enabled.unwrap_or(true),
                    hints: 0,
                })
            }
        }
        let r = crate::builtins::default_registry();
        register(&r, Arc::new(T));
        let out = r
            .invoke(TOGGLE, json!({"path": "/a.cs", "enabled": false}))
            .unwrap();
        assert_eq!(out, json!({"path": "/a.cs", "enabled": false, "hints": 0}));
    }
}
