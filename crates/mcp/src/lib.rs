//! MCP server (PLAN.md D3, 5.1, 5.2).
//!
//! Exposes the command bus to hosted agents: every agent-visible `CommandSpec`
//! becomes an MCP tool with the same input/output schemas
//! (`protocol/schemas/mcp-tool.json`), so there is no feature the human can use
//! that an agent cannot. Transport is JSON-RPC 2.0 (`eludite-protocol`),
//! newline-delimited as in MCP's stdio transport.
//!
//! Public API: the tool mapping in this module ([`tool_from_command`],
//! [`tool_name`], [`mcp_output_schema`]), [`McpServer`] (the protocol, with the
//! permission gate and the invoker hook the shell supplies), and [`transport`]
//! (stdio serving, the IDE's local TCP endpoint and the stdio relay an agent
//! launches). Hand-written rather than built on `rmcp`: four methods over the
//! existing `eludite-protocol` types, no async runtime, and the schemas come
//! from `protocol/` rather than being derived from Rust types.

use eludite_commands::{CommandId, CommandSpec, PermissionClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use eludite_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};

mod server;
pub mod transport;

pub use server::{
    AgentName, CallContext, CallObserver, GateDecision, Invoker, McpServer, NO_GATE,
    PermissionGate, SUPPORTED_PROTOCOL_VERSIONS, ToolCallRecord,
};

/// MCP `Tool` as returned by `tools/list`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct McpToolDescriptor {
    pub name: String,
    pub title: String,
    pub description: String,
    pub input_schema: Value,
    pub output_schema: Value,
    pub annotations: ToolAnnotations,
    #[serde(rename = "_meta")]
    pub meta: ToolMeta,
}

/// The descriptor's `_meta`: which command a tool is and its permission class.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMeta {
    #[serde(rename = "eludite/command")]
    pub command: String,
    #[serde(rename = "eludite/permission")]
    pub permission: PermissionClass,
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
    // The input schema's root `description` (from `protocol/schemas/`) is what
    // the model reads to decide when to call the tool.
    let description = spec
        .input_schema
        .get("description")
        .and_then(Value::as_str)
        .map_or_else(|| spec.title.clone(), str::to_owned);
    McpToolDescriptor {
        name: tool_name(&spec.id),
        title: spec.title.clone(),
        description,
        input_schema: strip_meta_keywords(spec.input_schema.clone()),
        output_schema: mcp_output_schema(spec),
        annotations: ToolAnnotations {
            read_only_hint: spec.permission == PermissionClass::Read,
            destructive_hint: spec.permission == PermissionClass::Dangerous,
        },
        meta: ToolMeta {
            command: spec.id.to_string(),
            permission: spec.permission,
        },
    }
}

/// Drop `$schema` and `$id` from a schema root. MCP defaults to JSON Schema
/// 2020-12, and some clients' validators reject an explicit 2020-12 `$schema`
/// or a duplicate `$id`.
fn strip_meta_keywords(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
        obj.remove("$id");
    }
    schema
}

fn is_object_schema(schema: &Value) -> bool {
    schema.get("type").and_then(Value::as_str) == Some("object")
}

/// MCP requires `outputSchema` to describe an object (`structuredContent` is
/// always an object). Commands whose output is not an object, such as the
/// array returned by `diagnostics.list`, are wrapped as `{"result": <output>}`
/// on the MCP side only; the command bus keeps the schema from `protocol/`.
pub fn mcp_output_schema(spec: &CommandSpec) -> Value {
    let schema = strip_meta_keywords(spec.output_schema.clone());
    if is_object_schema(&schema) {
        schema
    } else {
        serde_json::json!({
            "type": "object",
            "properties": {"result": schema},
            "required": ["result"],
            "additionalProperties": false
        })
    }
}

/// The `structuredContent` for a command output, wrapped like [`mcp_output_schema`].
pub fn mcp_structured_output(spec: &CommandSpec, output: Value) -> Value {
    if is_object_schema(&spec.output_schema) {
        output
    } else {
        serde_json::json!({"result": output})
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod mapping_tests {
    use super::*;
    use eludite_commands::builtins;
    use serde_json::json;

    #[test]
    fn tool_from_builtin_command() {
        let registry = builtins::default_registry();
        let spec = registry.lookup(builtins::FILE_OPEN).unwrap();
        let tool = tool_from_command(&spec);
        assert_eq!(tool.name, "eludite-file-open");
        assert_eq!(tool.title, "File: Open");
        assert_eq!(tool.input_schema, spec.input_schema);
        assert!(tool.annotations.read_only_hint);
        assert!(!tool.annotations.destructive_hint);
        assert_eq!(command_id_from_tool_name(&tool.name).unwrap(), spec.id);

        let v = serde_json::to_value(&tool).unwrap();
        assert_eq!(v["inputSchema"]["required"], json!(["path"]));
        assert_eq!(v["annotations"]["readOnlyHint"], json!(true));
        assert_eq!(v["_meta"]["eludite/command"], json!("eludite.file.open"));
        assert_eq!(v["_meta"]["eludite/permission"], json!("read"));
    }

    #[test]
    fn dangerous_is_destructive() {
        let spec = CommandSpec {
            id: CommandId::new("eludite.git.push").unwrap(),
            title: "Git: Push".into(),
            input_schema: json!({"type": "object", "properties": {}}),
            output_schema: json!({}),
            permission: PermissionClass::Dangerous,
            agent_visible: true,
        };
        let tool = tool_from_command(&spec);
        assert!(tool.annotations.destructive_hint);
        assert!(!tool.annotations.read_only_hint);
    }
}
