//! The MCP server proper: `initialize`, `ping`, `tools/list`, `tools/call`, and the guides as resources
//! (`resources/list`, `resources/read`, `resources/templates/list`; brief 0027, [`crate::resources`]), and the
//! repository's status as the resource `eludite://git/status` (brief 0040); the terminal's guide is
//! `eludite://guides/terminal` (brief 0041). The instructions point agents at `eludite.search.*` for searching files
//! (brief 0042).
//!
//! Transport-agnostic: [`McpServer::handle`] maps one JSON-RPC message to at most one reply. See `transport` for
//! stdio and the local TCP endpoint.
//!
//! The tool list is the command bus's agent-visible commands ([`CommandRegistry::agent_visible`]), computed afresh on
//! every `tools/list`, so a command registered at runtime appears on the next list. Every call is a bus invocation
//! made as the agent ([`eludite_commands::with_caller`]), so the audit log records it with its arguments, and the
//! permission class is applied here, at the boundary (PLAN.md 5.3): class read runs; every other class asks the
//! [`PermissionGate`], which may block this connection's thread while the user answers (never the UI thread).
//!
//! The class is the call's effective one (ADR-0009): [`CommandRegistry::classify`] runs the command's escalation
//! hook once, before the gate, and the call is invoked with that class ([`CommandRegistry::invoke_as`]), so the
//! gate, the prompt and the audit entry agree. A call the policy refuses outright never reaches the gate.

use std::sync::Arc;
use std::time::{Duration, Instant};

use eludite_commands::{
    CallClass, Caller, CommandError, CommandId, CommandRegistry, CommandSpec, Outcome,
    PermissionClass, next_call_id, with_caller,
};
use serde_json::{Value, json};

use crate::{
    ErrorObject, Id, Message, Request, Response, command_id_from_tool_name, mcp_structured_output,
    take_image_content, tool_from_command,
};

/// MCP revisions this server speaks, newest first. `outputSchema`, `structuredContent` and tool `title` exist from
/// 2025-06-18 on.
pub const SUPPORTED_PROTOCOL_VERSIONS: &[&str] =
    &["2025-11-25", "2025-06-18", "2025-03-26", "2024-11-05"];

/// One `tools/call` as the gate and the invoker see it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CallContext {
    /// The agent's name ([`McpServer::with_agent`]).
    pub agent: String,
    /// This call's id ([`eludite_commands::next_call_id`]), also the audit entry's caller call.
    pub call: u64,
    /// The agent's own id for the tool call, when the client sends it in the request's `_meta` (a key ending in
    /// `toolUseId` or `toolCallId`).
    pub tool_call: Option<String>,
    /// The call's effective class: the spec's, or what its escalation hook raised it to (ADR-0009). The gate decides
    /// on this, not on the spec's class.
    pub class: CallClass,
}

impl CallContext {
    /// The caller the bus records for this call.
    pub fn caller(&self) -> Caller {
        Caller::Agent {
            agent: self.agent.clone(),
            call: self.call,
            tool_call: self.tool_call.clone(),
        }
    }
}

/// The gate's answer for a call that is not class read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GateDecision {
    Allow,
    /// Denied, with the reason the agent is told.
    Deny(String),
}

/// Decides whether a tool call that is not class read may run, on its effective class (`CallContext::class`). Read
/// is always allowed (PLAN.md 5.3) and never reaches the gate. It runs on the connection's thread and may block
/// while the user answers a prompt.
pub type PermissionGate =
    Arc<dyn Fn(&CommandSpec, &Value, &CallContext) -> GateDecision + Send + Sync>;

/// Runs an allowed call. The default invokes the bus as the agent; the shell's replaces it to hold the agent's
/// edits as pending changes and answer once they are reviewed.
pub type Invoker =
    Arc<dyn Fn(&CommandSpec, Value, &CallContext) -> Result<Value, CommandError> + Send + Sync>;

/// What happened on one `tools/call`, for the audit line and the UI.
#[derive(Debug, Clone)]
pub struct ToolCallRecord {
    pub tool: String,
    pub command: Option<CommandId>,
    /// The call's effective class.
    pub permission: Option<PermissionClass>,
    pub arguments: Value,
    /// `Ok(output)` or `Err(message)`.
    pub outcome: Result<Value, String>,
    pub elapsed: Duration,
    pub call: u64,
    pub tool_call: Option<String>,
    /// The name of the thread that served the call (proof that it was not the UI thread).
    pub thread: String,
}

pub type CallObserver = Arc<dyn Fn(&ToolCallRecord) + Send + Sync>;

/// The name of the agent calling, asked at each call (one endpoint serves whichever agent the window runs).
pub type AgentName = Arc<dyn Fn() -> String + Send + Sync>;

/// Exposes the agent-visible commands of a [`CommandRegistry`] as MCP tools.
#[derive(Clone)]
pub struct McpServer {
    registry: Arc<CommandRegistry>,
    /// An allow-list on top of `agent_visible` (tests, the fixture example).
    only: Option<Vec<CommandId>>,
    gate: PermissionGate,
    invoker: Option<Invoker>,
    observer: Option<CallObserver>,
    agent: AgentName,
    name: String,
    version: String,
}

impl std::fmt::Debug for McpServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpServer")
            .field("agent", &(self.agent)())
            .field("only", &self.only)
            .finish_non_exhaustive()
    }
}

/// The reason a call is denied when no gate is set.
pub const NO_GATE: &str = "no one is there to allow it";

impl McpServer {
    /// Serve every agent-visible command of `registry`. Calls to anything but class read are denied until a gate is
    /// set.
    pub fn new(registry: Arc<CommandRegistry>) -> Self {
        Self {
            registry,
            only: None,
            gate: Arc::new(|_, _, _| GateDecision::Deny(NO_GATE.into())),
            invoker: None,
            observer: None,
            agent: Arc::new(|| "agent".to_owned()),
            name: "eludite".into(),
            version: env!("CARGO_PKG_VERSION").into(),
        }
    }

    /// Serve only these of the agent-visible commands.
    pub fn with_only(mut self, ids: impl IntoIterator<Item = CommandId>) -> Self {
        self.only = Some(ids.into_iter().collect());
        self
    }

    /// The agent's name, recorded as the caller of its calls.
    pub fn with_agent(mut self, agent: impl Into<String>) -> Self {
        let agent = agent.into();
        self.agent = Arc::new(move || agent.clone());
        self
    }

    /// The agent's name, asked at each call.
    pub fn with_agent_name(mut self, name: AgentName) -> Self {
        self.agent = name;
        self
    }

    pub fn with_permission_gate(mut self, gate: PermissionGate) -> Self {
        self.gate = gate;
        self
    }

    pub fn with_invoker(mut self, invoker: Invoker) -> Self {
        self.invoker = Some(invoker);
        self
    }

    pub fn with_observer(mut self, observer: CallObserver) -> Self {
        self.observer = Some(observer);
        self
    }

    pub fn registry(&self) -> &Arc<CommandRegistry> {
        &self.registry
    }

    /// The commands advertised right now, sorted by id.
    pub fn specs(&self) -> Vec<CommandSpec> {
        self.registry
            .agent_visible()
            .into_iter()
            .filter(|s| self.only.as_ref().is_none_or(|o| o.contains(&s.id)))
            .collect()
    }

    fn spec_for_tool(&self, name: &str) -> Option<CommandSpec> {
        let id = command_id_from_tool_name(name)?;
        let spec = self.registry.lookup(id.as_str())?;
        (spec.agent_visible && self.only.as_ref().is_none_or(|o| o.contains(&id))).then_some(spec)
    }

    /// Handle one incoming message. Returns the reply for requests, `None` for notifications and stray responses.
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
            "resources/list" => Ok(crate::resources::list(&self.registry)),
            "resources/templates/list" => Ok(json!({ "resourceTemplates": [] })),
            "resources/read" => {
                let uri = params.get("uri").and_then(Value::as_str).ok_or_else(|| {
                    ErrorObject::new(ErrorObject::INVALID_PARAMS, "`uri` must be a string")
                });
                let caller = Caller::Agent {
                    agent: (self.agent)(),
                    call: next_call_id(),
                    tool_call: None,
                };
                uri.and_then(
                    |uri| match crate::resources::read(uri, &self.registry, caller) {
                        Some(Ok(v)) => Ok(v),
                        Some(Err(e)) => Err(ErrorObject::new(ErrorObject::INTERNAL_ERROR, e)),
                        None => Err(ErrorObject::new(
                            crate::resources::RESOURCE_NOT_FOUND,
                            format!("resource not found: {uri}"),
                        )),
                    },
                )
            }
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
            "capabilities": {"tools": {"listChanged": true}, "resources": {}},
            "serverInfo": {"name": self.name, "title": "Eludite", "version": self.version},
            "instructions": "Eludite IDE tools. Each tool is an Eludite command with the same id, schemas and permission class as in the IDE. Read tools run at once; edits are shown to the user as pending changes and the tool answers once they are accepted or rejected; build, run and other commands may ask the user first. Before driving the debugger (eludite.debug.*), read the resource eludite://guides/debugging; before using git (eludite.git.*), eludite://guides/git; before running commands in the terminal (eludite.terminal.*), eludite://guides/terminal. The resource eludite://git/status is the repository's status. To search the workspace's files use eludite.search.find (read; it honors .gitignore and reads open documents' unsaved text) rather than reading files one by one; eludite.search.replace holds its replacements as pending changes for review unless `preview` is false."
        })
    }

    fn tools_list(&self, _params: &Value) -> Value {
        // One page: the list is small, so `cursor` is ignored and no `nextCursor` is sent.
        let tools: Vec<Value> = self
            .specs()
            .iter()
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
        let spec = self.spec_for_tool(name).ok_or_else(|| {
            ErrorObject::new(ErrorObject::INVALID_PARAMS, format!("unknown tool: {name}"))
        })?;
        // The call's effective class, once (ADR-0009).
        let class = self
            .registry
            .classify(spec.id.as_str(), &arguments)
            .unwrap_or_else(|| CallClass::declared(spec.permission));
        let ctx = CallContext {
            agent: (self.agent)(),
            call: next_call_id(),
            tool_call: tool_call_id(params),
            class,
        };

        let decision = if let Some(why) = &ctx.class.refused {
            GateDecision::Deny(why.clone())
        } else if ctx.class.class == PermissionClass::Read {
            GateDecision::Allow
        } else {
            (self.gate)(&spec, &arguments, &ctx)
        };
        let outcome = match decision {
            GateDecision::Allow => match &self.invoker {
                Some(invoke) => invoke(&spec, arguments.clone(), &ctx),
                None => with_caller(ctx.caller(), || {
                    self.registry
                        .invoke_as(spec.id.as_str(), arguments.clone(), &ctx.class)
                        .1
                }),
            },
            GateDecision::Deny(reason) => {
                let err = CommandError::Failed(format!(
                    "permission denied: `{}` is class {}{} and {reason}",
                    spec.id,
                    ctx.class.class.as_str(),
                    ctx.class
                        .reason
                        .as_deref()
                        .map(|r| format!(" ({r})"))
                        .unwrap_or_default()
                ));
                // Denied calls never reach the bus; record them so every tool call is audited.
                self.registry.audit_log().record_call_class(
                    spec.id.as_str(),
                    &ctx.class,
                    Outcome::Err(err.to_string()),
                    ctx.caller(),
                    Some(arguments.clone()),
                );
                Err(err)
            }
        };
        let record = ToolCallRecord {
            tool: name.to_owned(),
            command: Some(spec.id.clone()),
            permission: Some(ctx.class.class),
            arguments,
            outcome: outcome.clone().map_err(|e| e.to_string()),
            elapsed: started.elapsed(),
            call: ctx.call,
            tool_call: ctx.tool_call.clone(),
            thread: std::thread::current().name().unwrap_or("?").to_owned(),
        };
        if let Some(obs) = &self.observer {
            obs(&record);
        }
        // Tool failures are results with `isError`, so the model can see them; only unknown tools and malformed
        // requests are JSON-RPC errors.
        Ok(match outcome {
            Ok(mut output) => {
                // An image the output carries goes once, as image content after the text (brief 0023).
                let images = take_image_content(&spec, &mut output);
                let mut content = vec![
                    json!({"type": "text", "text": serde_json::to_string(&output).expect("serializes")}),
                ];
                content.extend(images);
                json!({
                    "content": content,
                    "structuredContent": mcp_structured_output(&spec, output),
                    "isError": false
                })
            }
            Err(e) => json!({
                "content": [{"type": "text", "text": e.to_string()}],
                "isError": true
            }),
        })
    }
}

/// The client's id for the tool call, from the request's `_meta` (Claude Code sends `claudecode/toolUseId`).
fn tool_call_id(params: &Value) -> Option<String> {
    params
        .get("_meta")?
        .as_object()?
        .iter()
        .find(|(k, _)| k.ends_with("toolUseId") || k.ends_with("toolCallId"))
        .and_then(|(_, v)| v.as_str())
        .map(str::to_owned)
}
