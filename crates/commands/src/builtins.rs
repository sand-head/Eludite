//! Built-in commands registered at startup.

use serde_json::{Value, json};

use crate::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};

pub const ABOUT: &str = "eludite.help.about";
pub const TOGGLE_TOOL_WINDOW: &str = "eludite.view.toggle_tool_window";
pub const FILE_OPEN: &str = "eludite.file.open";

/// The Eludite version reported by `eludite.help.about`.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

fn id(s: &str) -> CommandId {
    CommandId::new(s).expect("built-in command ids are valid")
}

fn required_string(input: &Value, field: &str) -> Result<String, CommandError> {
    input
        .get(field)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| CommandError::InvalidInput(format!("`{field}` must be a string")))
}

/// Register the built-in commands. UI-bound behaviour (actually toggling a tool
/// window, opening an editor) is stubbed until the shell wires real handlers.
pub fn register_builtins(registry: &mut CommandRegistry) -> Result<(), CommandError> {
    registry.register(
        CommandSpec {
            id: id(ABOUT),
            title: "Help: About Eludite".into(),
            input_schema: json!({"type": "object", "properties": {}, "additionalProperties": false}),
            output_schema: json!({
                "type": "object",
                "properties": {"name": {"type": "string"}, "version": {"type": "string"}},
                "required": ["name", "version"]
            }),
            permission: PermissionClass::Read,
        },
        |_| Ok(json!({"name": "Eludite", "version": VERSION})),
    )?;

    registry.register(
        CommandSpec {
            id: id(TOGGLE_TOOL_WINDOW),
            title: "View: Toggle Tool Window".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string", "description": "Tool window id, e.g. solution_explorer"}},
                "required": ["id"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {"id": {"type": "string"}, "toggled": {"type": "boolean"}},
                "required": ["id", "toggled"]
            }),
            permission: PermissionClass::Read,
        },
        |input| {
            let id = required_string(&input, "id")?;
            Ok(json!({"id": id, "toggled": true}))
        },
    )?;

    registry.register(
        CommandSpec {
            id: id(FILE_OPEN),
            title: "File: Open".into(),
            input_schema: json!({
                "type": "object",
                "properties": {"path": {"type": "string", "description": "Absolute or workspace-relative path"}},
                "required": ["path"],
                "additionalProperties": false
            }),
            output_schema: json!({
                "type": "object",
                "properties": {"opened": {"type": "boolean"}},
                "required": ["opened"]
            }),
            permission: PermissionClass::Read,
        },
        |input| {
            required_string(&input, "path")?;
            Ok(json!({"opened": true}))
        },
    )?;

    Ok(())
}

/// A registry with the built-ins already registered.
pub fn default_registry() -> CommandRegistry {
    let mut registry = CommandRegistry::new();
    register_builtins(&mut registry).expect("built-ins register into an empty registry");
    registry
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Checks the minimal shape we require of every input schema: an object
    /// schema whose `properties` is an object and whose `required` names only
    /// declared properties.
    fn assert_object_schema(id: &str, schema: &Value) {
        let obj = schema
            .as_object()
            .unwrap_or_else(|| panic!("{id}: schema is not a JSON object"));
        assert_eq!(
            obj.get("type"),
            Some(&json!("object")),
            "{id}: type must be object"
        );
        let props = obj
            .get("properties")
            .and_then(Value::as_object)
            .unwrap_or_else(|| panic!("{id}: properties must be an object"));
        for (name, prop) in props {
            assert!(
                prop.is_object(),
                "{id}: property {name} must be a schema object"
            );
        }
        if let Some(required) = obj.get("required") {
            let required = required
                .as_array()
                .unwrap_or_else(|| panic!("{id}: required must be an array"));
            for r in required {
                let r = r.as_str().expect("required entries are strings");
                assert!(
                    props.contains_key(r),
                    "{id}: required `{r}` not in properties"
                );
            }
        }
    }

    #[test]
    fn builtins_registered() {
        let r = default_registry();
        let ids: Vec<_> = r.list().map(|s| s.id.as_str().to_owned()).collect();
        assert_eq!(ids, [FILE_OPEN, ABOUT, TOGGLE_TOOL_WINDOW]);
    }

    #[test]
    fn every_input_schema_is_object_schema() {
        let r = default_registry();
        for spec in r.list() {
            assert_object_schema(spec.id.as_str(), &spec.input_schema);
        }
    }

    #[test]
    fn about_returns_version() {
        let r = default_registry();
        let out = r.invoke(ABOUT, json!({})).unwrap();
        assert_eq!(out["version"], json!(VERSION));
    }

    #[test]
    fn toggle_and_open_validate_input() {
        let r = default_registry();
        assert_eq!(
            r.invoke(TOGGLE_TOOL_WINDOW, json!({"id": "output"}))
                .unwrap()["toggled"],
            json!(true)
        );
        assert!(matches!(
            r.invoke(TOGGLE_TOOL_WINDOW, json!({})),
            Err(CommandError::InvalidInput(_))
        ));
        assert_eq!(
            r.invoke(FILE_OPEN, json!({"path": "/tmp/a.cs"})).unwrap(),
            json!({"opened": true})
        );
        assert!(r.invoke(FILE_OPEN, json!({"path": 3})).is_err());
        assert_eq!(r.audit_log().len(), 4);
    }
}
