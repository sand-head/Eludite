//! MCP server (PLAN.md D3, 5.1, 5.2).
//!
//! Exposes the command bus to hosted agents: every `CommandSpec` becomes an MCP
//! tool with the same input/output schemas, so there is no feature the human
//! can use that an agent cannot. Transport is JSON-RPC 2.0 (`niello-protocol`).

use niello_commands::{CommandId, CommandSpec, PermissionClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use niello_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};

/// MCP `Tool` as returned by `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: String,
    pub title: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub annotations: ToolAnnotations,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ToolAnnotations {
    pub read_only_hint: bool,
    pub destructive_hint: bool,
}

/// MCP tool names: command ids with `.` replaced by `-`, which many clients
/// require (`^[a-zA-Z0-9_-]+$`). Reversible because ids never contain `-`.
pub fn tool_name(id: &CommandId) -> String {
    id.as_str().replace('.', "-")
}

/// Inverse of [`tool_name`].
pub fn command_id_from_tool_name(name: &str) -> Option<CommandId> {
    CommandId::new(name.replace('-', ".")).ok()
}

pub fn tool_from_command(spec: &CommandSpec) -> McpToolDescriptor {
    McpToolDescriptor {
        name: tool_name(&spec.id),
        title: spec.title.clone(),
        input_schema: spec.input_schema.clone(),
        output_schema: spec.output_schema.clone(),
        annotations: ToolAnnotations {
            read_only_hint: spec.permission == PermissionClass::Read,
            destructive_hint: spec.permission == PermissionClass::Dangerous,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use niello_commands::builtins;
    use serde_json::json;

    #[test]
    fn tool_from_builtin_command() {
        let registry = builtins::default_registry();
        let spec = registry.lookup(builtins::FILE_OPEN).unwrap();
        let tool = tool_from_command(spec);
        assert_eq!(tool.name, "niello-file-open");
        assert_eq!(tool.title, "File: Open");
        assert_eq!(tool.input_schema, spec.input_schema);
        assert!(tool.annotations.read_only_hint);
        assert!(!tool.annotations.destructive_hint);
        assert_eq!(command_id_from_tool_name(&tool.name).unwrap(), spec.id);

        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["inputSchema"]["required"], json!(["path"]));
        assert_eq!(v["annotations"]["readOnlyHint"], json!(true));
    }

    #[test]
    fn dangerous_is_destructive() {
        let spec = CommandSpec {
            id: CommandId::new("niello.git.push").unwrap(),
            title: "Git: Push".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({}),
            permission: PermissionClass::Dangerous,
        };
        let tool = tool_from_command(&spec);
        assert!(tool.annotations.destructive_hint);
        assert!(!tool.annotations.read_only_hint);
    }
}
