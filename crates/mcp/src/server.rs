//! The MCP server proper: `initialize`, `ping`, `tools/list`, `tools/call`.
//!
//! Transport-agnostic: [`McpServer::handle`] maps one JSON-RPC message to at
//! most one reply. See `transport` for stdio and the local TCP endpoint.

use std::sync::Arc;
use std::time::{Duration, Instant};

use niello_commands::{CommandError, CommandId, CommandRegistry, CommandSpec, PermissionClass};
use serde_json::{Value, json};

use crate::{
    ErrorObject, Id, Message, Request, Response, command_id_from_tool_name, mcp_structured_output,
    tool_from_command,
};

/// MCP revisions this server speaks, newest first. `outputSchema`,
/// `structuredContent` and tool `title` exist from 2025-06-18 on.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// Decides whether a tool call that is not class read may run. Read is always
/// allowed (PLAN.md 5.3) and never reaches the gate.
pub type PermissionGate = Arc<dyn Fn(&CommandSpec, &Value) -> bool + Send + Sync>;

/// What happened on one `tools/call`, for the audit log line and the UI.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool: String,
    pub command: Option<CommandId>,
    pub permission: Option<PermissionClass>,
    pub arguments: Value,
    /// `Ok(output)` or `Err(message)`.
    pub outcome: Result<Value, String>,
    pub elapsed: Duration,
}

pub type CallObserver = Arc<dyn Fn(&ToolCallRecord) + Send + Sync>;

/// Exposes an allow-listed subset of a [`CommandRegistry`] as MCP tools.
#[derive(Clone)]
pub struct McpServer {
    registry: Arc<CommandRegistry>,
    exposed: Vec<CommandId>,
    gate: PermissionGate,
    observer: Option<CallObserver>,
    name: String,
    version: String,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("exposed", &self.exposed)
            .finish_non_exhaustive()
    }
}

impl McpServer {
    /// Serve the commands in `exposed` (ids that are not registered are ignored).
    /// Calls to anything but class read are denied until a gate is set.
    pub fn new(
        registry: Arc<CommandRegistry>,
        exposed: impl IntoIterator<Item = CommandId>,
    ) -> Self {
        let exposed = exposed
            .into_iter()
            .filter(|id| registry.lookup(id.as_str()).is_some())
            .collect();
        Self {
            registry,
            exposed,
            gate: Arc::new(|_, _| false),
            observer: None,
            name: "niello".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    pub fn with_permission_gate(mut self, gate: PermissionGate) -> Self {
        self.gate = gate;
        self
    }

    pub fn with_observer(mut self, observer: CallObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn registry(&self) -> &Arc<CommandRegistry> {
        &self.registry
    }

    fn specs(&self) -> impl Iterator<Item = &CommandSpec> {
        self.exposed
            .iter()
            .filter_map(|id| self.registry.lookup(id.as_str()))
    }

    /// Handle one incoming message. Returns the reply for requests, `None` for
    /// notifications and stray responses.
    pub fn handle(&self, msg: Message) -> Option<Response> {
        match msg {
            Message::Request(req) => Some(self.handle_request(req)),
            Message::Notification(_) | Message::Response(_) => None,
        }
    }

    /// Parse and handle one line of newline-delimited JSON-RPC.
    pub fn handle_line(&self, line: &str) -> Option<Response> {
        match serde_json::from_str::<Message>(line) {
            Ok(msg) => self.handle(msg),
            Err(e) => {
                // Batches are not part of MCP since 2025-06-18; anything else is garbage.
                let id = serde_json::from_str::<Value>(line)
                    .ok()
                    .and_then(|v| v.get("id").cloned())
                    .and_then(|id| serde_json::from_value::<Id>(id).ok());
                Some(Response::err(
                    id,
                    ErrorObject::new(ErrorObject::PARSE_ERROR, e.to_string()),
                ))
            }
        }
    }

    fn handle_request(&self, req: Request) -> Response {
        let params = req.params.unwrap_or(Value::Null);
        let result = match req.method.as_str() {
            "initialize" => Ok(self.initialize(&params)),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(self.tools_list(&params)),
            "tools/call" => self.tools_call(&params),
            other => Err(ErrorObject::new(
                ErrorObject::METHOD_NOT_FOUND,
                format!("method not found: {other}"),
            )),
        };
        match result {
            Ok(v) => Response::ok(req.id, v),
            Err(e) => Response::err(Some(req.id), e),
        }
    }

    fn initialize(&self, params: &Value) -> Value {
        let requested = params
            .get("protocolVersion")
            .and_then(Value::as_str)
            .unwrap_or_default();
        let version = SUPPORTED_PROTOCOL_VERSIONS
            .iter()
            .find(|v| **v == requested)
            .copied()
            .unwrap_or(SUPPORTED_PROTOCOL_VERSIONS[0]);
        json!({
            "protocolVersion": version,
            "capabilities": {"tools": {"listChanged": false}},
            "serverInfo": {"name": self.name, "title": "Niello", "version": self.version},
            "instructions": "Niello IDE tools. Each tool is a Niello command with the same id, schemas and permission class as in the IDE."
        })
    }

    fn tools_list(&self, _params: &Value) -> Value {
        // One page: the exposed list is small, so `cursor` is ignored and no `nextCursor` is sent.
        let tools: Vec<Value> = self
            .specs()
            .map(|s| serde_json::to_value(tool_from_command(s)).expect("descriptor serializes"))
            .collect();
        json!({"tools": tools})
    }

    fn tools_call(&self, params: &Value) -> Result<Value, ErrorObject> {
        let started = Instant::now();
        let name = params.get("name").and_then(Value::as_str).ok_or_else(|| {
            ErrorObject::new(ErrorObject::INVALID_PARAMS, "`name` must be a string")
        })?;
        let arguments = params
            .get("arguments")
            .cloned()
            .unwrap_or_else(|| json!({}));
        let spec = command_id_from_tool_name(name)
            .filter(|id| self.exposed.contains(id))
            .and_then(|id| self.registry.lookup(id.as_str()))
            .ok_or_else(|| {
                ErrorObject::new(ErrorObject::INVALID_PARAMS, format!("unknown tool: {name}"))
            })?;

        let allowed = spec.permission == PermissionClass::Read || (self.gate)(spec, &arguments);
        let outcome = if allowed {
            self.registry.invoke(spec.id.as_str(), arguments.clone())
        } else {
            Err(CommandError::Failed(format!(
                "permission denied: `{}` is class {:?} and the user did not allow it",
                spec.id, spec.permission
            )))
        };
        let record = ToolCallRecord {
            tool: name.to_owned(),
            command: Some(spec.id.clone()),
            permission: Some(spec.permission),
            arguments,
            outcome: outcome.clone().map_err(|e| e.to_string()),
            elapsed: started.elapsed(),
        };
        if let Some(obs) = &self.observer {
            obs(&record);
        }
        // Tool failures are results with `isError`, so the model can see them;
        // only unknown tools and malformed requests are JSON-RPC errors.
        Ok(match outcome {
            Ok(output) => json!({
                "content": [{"type": "text", "text": serde_json::to_string(&output).expect("serializes")}],
                "structuredContent": mcp_structured_output(spec, output),
                "isError": false
            }),
            Err(e) => json!({
                "content": [{"type": "text", "text": e.to_string()}],
                "isError": true
            }),
        })
    }
}
