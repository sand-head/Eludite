//! MCP server (PLAN.md D3, 5.1, 5.2).
//!
//! Exposes the command bus to hosted agents: every agent-visible `CommandSpec`
//! becomes an MCP tool with the same input/output schemas
//! (`protocol/schemas/mcp-tool.json`), so there is no feature the human can use
//! that an agent cannot. Transport is JSON-RPC 2.0 (`eludite-protocol`),
//! newline-delimited as in MCP's stdio transport.
//!
//! Public API: the tool mapping in this module ([`tool_from_command`],
//! [`tool_name`], [`mcp_output_schema`], [`take_image_content`] for outputs
//! that carry an image, brief 0023), [`McpServer`] (the protocol, with the
//! permission gate and the invoker hook the shell supplies; the gate sees each
//! call's effective class, ADR-0009), and [`transport`]
//! (stdio serving, the IDE's local TCP endpoint and the stdio relay an agent
//! launches), and [`resources`]: the guides for agents served as MCP resources (`eludite://guides/debugging`,
//! brief 0027). Hand-written rather than built on `rmcp`: a few methods over the
//! existing `eludite-protocol` types, no async runtime, and the schemas come
//! from `protocol/` rather than being derived from Rust types.

use eludite_commands::{CommandId, CommandSpec, PermissionClass};
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub use eludite_protocol::jsonrpc::{
    self, ErrorObject, Id, Message, Notification, Request, Response,
};

pub mod resources;
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

/// The descriptor's `_meta`: which command a tool is, its declared permission class, and when a call is raised
/// above it (ADR-0009).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolMeta {
    #[serde(rename = "eludite/command")]
    pub command: String,
    #[serde(rename = "eludite/permission")]
    pub permission: PermissionClass,
    #[serde(
        rename = "eludite/escalates",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub escalates: Option<String>,
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
            // A read command whose calls may escalate (`storage` with `clear`) is not read-only.
            read_only_hint: spec.permission == PermissionClass::Read && spec.escalates().is_none(),
            destructive_hint: spec.permission == PermissionClass::Dangerous,
        },
        meta: ToolMeta {
            command: spec.id.to_string(),
            permission: spec.permission,
            escalates: spec.escalates().map(str::to_owned),
        },
    }
}

/// Drop `$schema` and `$id` from a schema root. MCP defaults to JSON Schema
/// 2020-12, and some clients' validators reject an explicit 2020-12 `$schema`
/// or a duplicate `$id`. `x-eludite-escalates` goes to `_meta` instead.
fn strip_meta_keywords(mut schema: Value) -> Value {
    if let Some(obj) = schema.as_object_mut() {
        obj.remove("$schema");
        obj.remove("$id");
        obj.remove(eludite_commands::ESCALATES_KEY);
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

/// The output schema's marker for a top-level base64 string sent as MCP image content (brief 0023).
pub const IMAGE_CONTENT_MARKER: &str = "x-eludite-mcp-content";
/// What the text part and `structuredContent` carry in place of an image sent as image content.
pub const IMAGE_PLACEHOLDER: &str = "(image content)";

/// Move every top-level string property the output schema marks with
/// `"x-eludite-mcp-content": "image"` out of `output` into MCP image content
/// (`{"type": "image", "data", "mimeType"}`), leaving [`IMAGE_PLACEHOLDER`] in
/// its place, so the base64 is sent once. The media type is the property's
/// `contentMediaType`; `image/*` (or none) is read from the data's first bytes.
pub fn take_image_content(spec: &CommandSpec, output: &mut Value) -> Vec<Value> {
    let Some(props) = spec
        .output_schema
        .get("properties")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let Some(out) = output.as_object_mut() else {
        return Vec::new();
    };
    let mut images = Vec::new();
    for (name, schema) in props {
        if schema.get(IMAGE_CONTENT_MARKER).and_then(Value::as_str) != Some("image") {
            continue;
        }
        let Some(Value::String(data)) = out.get(name) else {
            continue;
        };
        let declared = schema.get("contentMediaType").and_then(Value::as_str);
        let mime = match declared {
            Some(m) if m != "image/*" => m.to_owned(),
            _ => sniff_image_type(data)
                .unwrap_or("application/octet-stream")
                .to_owned(),
        };
        let data = std::mem::replace(
            out.get_mut(name).expect("present"),
            Value::String(IMAGE_PLACEHOLDER.to_owned()),
        );
        images.push(serde_json::json!({"type": "image", "data": data, "mimeType": mime}));
    }
    images
}

/// The media type of base64 image data from its first bytes: PNG, JPEG, GIF or WebP.
pub fn sniff_image_type(base64: &str) -> Option<&'static str> {
    let head = decode_base64_prefix(base64, 12);
    if head.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if head.starts_with(&[0xFF, 0xD8, 0xFF]) {
        Some("image/jpeg")
    } else if head.starts_with(b"GIF8") {
        Some("image/gif")
    } else if head.starts_with(b"RIFF") && head.get(8..12) == Some(b"WEBP") {
        Some("image/webp")
    } else {
        None
    }
}

/// The first `n` bytes of standard base64 (whitespace and padding ignored).
fn decode_base64_prefix(s: &str, n: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(n);
    let (mut acc, mut bits) = (0u32, 0u32);
    for c in s.bytes() {
        let v = match c {
            b'A'..=b'Z' => c - b'A',
            b'a'..=b'z' => c - b'a' + 26,
            b'0'..=b'9' => c - b'0' + 52,
            b'+' => 62,
            b'/' => 63,
            _ => continue,
        };
        acc = (acc << 6) | u32::from(v);
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push((acc >> bits) as u8);
            acc &= (1 << bits) - 1;
            if out.len() == n {
                break;
            }
        }
    }
    out
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
