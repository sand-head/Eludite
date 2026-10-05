//! The MCP client: the client side of Eludite's MCP endpoint (`crates/mcp`), over a stdio server (the IDE's relay,
//! `eludite --mcp-relay ADDR` with `ELUDITE_MCP_TOKEN`, from `session/new`'s `mcpServers`) or a streamable HTTP
//! server. Methods: `initialize` (then `notifications/initialized`), `tools/list` (paged), `tools/call` and
//! `resources/read`; `notifications/tools/list_changed` marks the list stale ([`McpClient::take_list_changed`]).
//!
//! Requests are answered through [`oneshot`] receivers, so the ACP side awaits them without blocking its executor.
//! Over stdio one reader thread routes replies by id (the IDE answers tool calls out of order, each on its own
//! thread); over HTTP each request runs on its own thread.
//!
//! [`ToolSet`] maps the listed tools to Chat Completions `function` tools: all of them (`--tools all`), or the core
//! set plus the meta tool `eludite-tools` (`--tools core`, the default), which lists the others and enables the ones
//! the model names from the next request on.

use std::collections::{BTreeSet, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::log;
use crate::provider::SseParser;

/// The MCP revision asked for (Eludite's server speaks it; it falls back to its newest otherwise).
pub const PROTOCOL_VERSION: &str = "2025-06-18";
/// The meta tool's name.
pub const META_TOOL: &str = "eludite-tools";
/// How long `initialize` and `tools/list` may take.
pub const SETUP_TIMEOUT: Duration = Duration::from_secs(30);

type Reply = Result<Value, String>;
type Pending = Arc<Mutex<HashMap<u64, oneshot::Sender<Reply>>>>;

/// One MCP server from `session/new` (ACP's `McpServer`, read from its JSON).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ServerSpec {
    Stdio {
        name: String,
        command: String,
        args: Vec<String>,
        env: Vec<(String, String)>,
    },
    Http {
        name: String,
        url: String,
        headers: Vec<(String, String)>,
    },
}

impl ServerSpec {
    /// From ACP's `McpServer` JSON: `{type: "http", name, url, headers}`, `{name, command, args, env}` (stdio).
    /// `sse` servers are not supported (`None`).
    pub fn from_acp(v: &Value) -> Option<Self> {
        let name = v.get("name")?.as_str()?.to_owned();
        let pairs = |key: &str| -> Vec<(String, String)> {
            v.get(key)
                .and_then(Value::as_array)
                .map(|a| {
                    a.iter()
                        .filter_map(|e| {
                            Some((
                                e.get("name")?.as_str()?.to_owned(),
                                e.get("value")?.as_str()?.to_owned(),
                            ))
                        })
                        .collect()
                })
                .unwrap_or_default()
        };
        match v.get("type").and_then(Value::as_str) {
            Some("http") => Some(ServerSpec::Http {
                name,
                url: v.get("url")?.as_str()?.to_owned(),
                headers: pairs("headers"),
            }),
            Some("sse") => None,
            _ => Some(ServerSpec::Stdio {
                name,
                command: v.get("command")?.as_str()?.to_owned(),
                args: v
                    .get("args")
                    .and_then(Value::as_array)
                    .map(|a| {
                        a.iter()
                            .filter_map(|s| s.as_str().map(str::to_owned))
                            .collect()
                    })
                    .unwrap_or_default(),
                env: pairs("env"),
            }),
        }
    }

    pub fn name(&self) -> &str {
        match self {
            ServerSpec::Stdio { name, .. } | ServerSpec::Http { name, .. } => name,
        }
    }
}

/// A tool as `tools/list` gives it.
#[derive(Debug, Clone, PartialEq)]
pub struct McpTool {
    pub name: String,
    pub description: String,
    pub input_schema: Value,
    /// `_meta["eludite/command"]`: the command id.
    pub command: Option<String>,
    /// `_meta["eludite/permission"]`: `read`, `edit_buffer`, `execute` or `dangerous`.
    pub permission: Option<String>,
}

impl McpTool {
    fn from_json(v: &Value) -> Option<Self> {
        let meta = v.get("_meta");
        let m = |k: &str| {
            meta.and_then(|m| m.get(k))
                .and_then(Value::as_str)
                .map(str::to_owned)
        };
        Some(Self {
            name: v.get("name")?.as_str()?.to_owned(),
            description: v
                .get("description")
                .and_then(Value::as_str)
                .or_else(|| v.get("title").and_then(Value::as_str))
                .unwrap_or("")
                .to_owned(),
            input_schema: v
                .get("inputSchema")
                .cloned()
                .unwrap_or_else(|| json!({"type": "object", "properties": {}})),
            command: m("eludite/command"),
            permission: m("eludite/permission"),
        })
    }

    /// The command id without the `eludite.` prefix (`file.read`, `diagnostics.list`), from `_meta` or the name.
    pub fn short_id(&self) -> String {
        let id = self
            .command
            .clone()
            .unwrap_or_else(|| self.name.replace('-', "."));
        id.strip_prefix("eludite.").unwrap_or(&id).to_owned()
    }
}

/// A `tools/call` result as text for the model, plus any images.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolResult {
    pub text: String,
    /// `(mime type, base64 data)`.
    pub images: Vec<(String, String)>,
    pub is_error: bool,
}

impl ToolResult {
    /// From MCP's `CallToolResult`.
    pub fn from_json(v: &Value) -> Self {
        let mut texts = Vec::new();
        let mut images = Vec::new();
        for c in v
            .get("content")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            match c.get("type").and_then(Value::as_str) {
                Some("text") => {
                    if let Some(t) = c.get("text").and_then(Value::as_str) {
                        texts.push(t.to_owned());
                    }
                }
                Some("image") => {
                    if let Some(d) = c.get("data").and_then(Value::as_str) {
                        let mime = c
                            .get("mimeType")
                            .and_then(Value::as_str)
                            .unwrap_or("image/png");
                        images.push((mime.to_owned(), d.to_owned()));
                    }
                }
                Some("resource") => {
                    if let Some(t) = c.pointer("/resource/text").and_then(Value::as_str) {
                        texts.push(t.to_owned());
                    }
                }
                _ => {}
            }
        }
        if texts.is_empty()
            && let Some(s) = v.get("structuredContent")
        {
            texts.push(s.to_string());
        }
        Self {
            text: texts.join("\n"),
            images,
            is_error: v.get("isError").and_then(Value::as_bool).unwrap_or(false),
        }
    }
}

enum Transport {
    Lines {
        writer: Mutex<Box<dyn Write + Send>>,
        child: Option<Mutex<Child>>,
    },
    Http {
        url: String,
        headers: Vec<(String, String)>,
        agent: ureq::Agent,
        session: Arc<Mutex<Option<String>>>,
    },
}

/// A connection to one MCP server.
pub struct McpClient {
    name: String,
    transport: Transport,
    next_id: AtomicU64,
    pending: Pending,
    closed: Arc<AtomicBool>,
    list_changed: Arc<AtomicBool>,
}

impl std::fmt::Debug for McpClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("McpClient")
            .field("name", &self.name)
            .finish()
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Transport::Lines {
            child: Some(child), ..
        } = &self.transport
            && let Ok(mut c) = child.lock()
        {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn is_secret_name(name: &str) -> bool {
    let n = name.to_ascii_uppercase();
    ["TOKEN", "KEY", "SECRET", "PASSWORD"]
        .iter()
        .any(|s| n.contains(s))
}

impl McpClient {
    /// Start `spec` (a stdio child, or the HTTP endpoint) without talking to it yet.
    pub fn connect(spec: &ServerSpec, cwd: &std::path::Path) -> Result<Arc<Self>, String> {
        match spec {
            ServerSpec::Stdio {
                name,
                command,
                args,
                env,
            } => {
                let mut cmd = Command::new(command);
                cmd.args(args)
                    .current_dir(cwd)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::piped());
                for (k, v) in env {
                    if is_secret_name(k) {
                        log::add_secret(v);
                    }
                    cmd.env(k, v);
                }
                let mut child = cmd.spawn().map_err(|e| {
                    format!("could not start the MCP server {name} ({command}): {e}")
                })?;
                let stdin = child.stdin.take().ok_or("no stdin")?;
                let stdout = child.stdout.take().ok_or("no stdout")?;
                if let Some(stderr) = child.stderr.take() {
                    let label = name.clone();
                    let _ =
                        std::thread::Builder::new()
                            .name("mcp-stderr".into())
                            .spawn(move || {
                                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                                    log::info(format_args!("mcp {label}: {line}"));
                                }
                            });
                }
                Ok(Self::over_lines(name, stdout, stdin, Some(child)))
            }
            ServerSpec::Http { name, url, headers } => {
                for (k, v) in headers {
                    if is_secret_name(k) || k.eq_ignore_ascii_case("authorization") {
                        log::add_secret(v);
                    }
                }
                let agent: ureq::Agent = ureq::Agent::config_builder()
                    .http_status_as_error(false)
                    .timeout_connect(Some(Duration::from_secs(10)))
                    .proxy(if crate::provider::is_loopback(url) {
                        None
                    } else {
                        ureq::Proxy::try_from_env()
                    })
                    .build()
                    .into();
                Ok(Arc::new(Self {
                    name: name.clone(),
                    transport: Transport::Http {
                        url: url.clone(),
                        headers: headers.clone(),
                        agent,
                        session: Arc::default(),
                    },
                    next_id: AtomicU64::new(1),
                    pending: Pending::default(),
                    closed: Arc::default(),
                    list_changed: Arc::default(),
                }))
            }
        }
    }

    /// A client over newline-delimited JSON-RPC on `reader` and `writer` (a stdio child's pipes, or a socket in
    /// tests), with one reader thread.
    pub fn over_lines(
        name: &str,
        reader: impl Read + Send + 'static,
        writer: impl Write + Send + 'static,
        child: Option<Child>,
    ) -> Arc<Self> {
        let client = Arc::new(Self {
            name: name.to_owned(),
            transport: Transport::Lines {
                writer: Mutex::new(Box::new(writer)),
                child: child.map(Mutex::new),
            },
            next_id: AtomicU64::new(1),
            pending: Pending::default(),
            closed: Arc::default(),
            list_changed: Arc::default(),
        });
        let (pending, closed, changed) = (
            client.pending.clone(),
            client.closed.clone(),
            client.list_changed.clone(),
        );
        // The reader answers server requests (`ping`) through a weak handle, so it never keeps the client alive.
        let weak = Arc::downgrade(&client);
        let label = name.to_owned();
        let _ = std::thread::Builder::new()
            .name("mcp-reader".into())
            .spawn(move || {
                for line in BufReader::new(reader).lines() {
                    let Ok(line) = line else { break };
                    if line.trim().is_empty() {
                        continue;
                    }
                    let Ok(msg) = serde_json::from_str::<Value>(&line) else {
                        log::info(format_args!("mcp {label}: a line that is not JSON"));
                        continue;
                    };
                    dispatch(&msg, &pending, &changed, |reply| {
                        if let Some(c) = weak.upgrade() {
                            let _ = c.write_line(&reply);
                        }
                    });
                }
                closed.store(true, Ordering::Release);
                let waiting: Vec<_> = pending
                    .lock()
                    .map(|mut p| p.drain().collect())
                    .unwrap_or_default();
                for (_, tx) in waiting {
                    let _ = tx.send(Err("the MCP server closed the connection".into()));
                }
            });
        client
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    /// True once after the server said its tool list changed.
    pub fn take_list_changed(&self) -> bool {
        self.list_changed.swap(false, Ordering::AcqRel)
    }

    fn write_line(&self, msg: &Value) -> Result<(), String> {
        let Transport::Lines { writer, .. } = &self.transport else {
            return Err("not a stream".into());
        };
        let mut line = msg.to_string();
        line.push('\n');
        let mut w = writer.lock().unwrap_or_else(|e| e.into_inner());
        w.write_all(line.as_bytes())
            .and_then(|()| w.flush())
            .map_err(|e| format!("could not write to the MCP server: {e}"))
    }

    /// Send a request; the receiver gets the result (or the error's message).
    pub fn request(&self, method: &str, params: Value) -> oneshot::Receiver<Reply> {
        self.start(method, params).1
    }

    fn start(&self, method: &str, params: Value) -> (u64, oneshot::Receiver<Reply>) {
        let (tx, rx) = oneshot::channel();
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let msg = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        match &self.transport {
            Transport::Lines { .. } => {
                if self.closed.load(Ordering::Acquire) {
                    let _ = tx.send(Err("the MCP server closed the connection".into()));
                    return (id, rx);
                }
                if let Ok(mut p) = self.pending.lock() {
                    p.insert(id, tx);
                }
                if let Err(e) = self.write_line(&msg)
                    && let Some(tx) = self.pending.lock().ok().and_then(|mut p| p.remove(&id))
                {
                    let _ = tx.send(Err(e));
                }
            }
            Transport::Http {
                url,
                headers,
                agent,
                session,
            } => {
                let (url, headers, agent, session) =
                    (url.clone(), headers.clone(), agent.clone(), session.clone());
                let changed = self.list_changed.clone();
                let _ = std::thread::Builder::new()
                    .name("mcp-http".into())
                    .spawn(move || {
                        let r =
                            http_post(&agent, &url, &headers, &session, &msg, Some(id), &changed);
                        let _ = tx.send(r);
                    });
            }
        }
        (id, rx)
    }

    /// Fail the request `id` if it is still waiting after `after` (for setup requests over a stream).
    /// The thread ends as soon as the returned guard is dropped (the request was answered).
    fn watchdog(&self, id: u64, after: Duration) -> std::sync::mpsc::Sender<()> {
        let pending = self.pending.clone();
        let (guard, done) = std::sync::mpsc::channel::<()>();
        let _ = std::thread::Builder::new()
            .name("mcp-watchdog".into())
            .spawn(move || {
                if done.recv_timeout(after) == Err(std::sync::mpsc::RecvTimeoutError::Timeout)
                    && let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(&id))
                {
                    let _ = tx.send(Err("the MCP server did not answer in time".into()));
                }
            });
        guard
    }

    fn notify(&self, method: &str) {
        let msg = json!({"jsonrpc": "2.0", "method": method});
        match &self.transport {
            Transport::Lines { .. } => {
                let _ = self.write_line(&msg);
            }
            Transport::Http {
                url,
                headers,
                agent,
                session,
            } => {
                let _ = http_post(agent, url, headers, session, &msg, None, &self.list_changed);
            }
        }
    }

    async fn call(&self, method: &str, params: Value, timeout: Option<Duration>) -> Reply {
        let (id, rx) = self.start(method, params);
        let _guard = match timeout {
            Some(t) if matches!(self.transport, Transport::Lines { .. }) => {
                Some(self.watchdog(id, t))
            }
            _ => None,
        };
        rx.await
            .unwrap_or_else(|_| Err("the MCP request was dropped".into()))
    }

    /// `initialize`, then `notifications/initialized`.
    pub async fn initialize(&self) -> Result<Value, String> {
        let r = self
            .call(
                "initialize",
                json!({
                    "protocolVersion": PROTOCOL_VERSION,
                    "capabilities": {},
                    "clientInfo": {"name": "eludite-openai-acp", "version": env!("CARGO_PKG_VERSION")},
                }),
                Some(SETUP_TIMEOUT),
            )
            .await?;
        self.notify("notifications/initialized");
        Ok(r)
    }

    /// Every tool, following `nextCursor`.
    pub async fn list_tools(&self) -> Result<Vec<McpTool>, String> {
        let mut out = Vec::new();
        let mut cursor: Option<String> = None;
        for _ in 0..100 {
            let params = match &cursor {
                Some(c) => json!({"cursor": c}),
                None => json!({}),
            };
            let r = self.call("tools/list", params, Some(SETUP_TIMEOUT)).await?;
            out.extend(
                r.get("tools")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .filter_map(McpTool::from_json),
            );
            cursor = r
                .get("nextCursor")
                .and_then(Value::as_str)
                .map(str::to_owned);
            if cursor.is_none() {
                break;
            }
        }
        Ok(out)
    }

    /// `tools/call` with the ACP tool call's id in `_meta` (so the IDE ties its permission prompt and pending
    /// changes to it). A JSON-RPC error is a failed result.
    pub async fn call_tool(&self, name: &str, arguments: &Value, tool_call_id: &str) -> ToolResult {
        let params = json!({
            "name": name,
            "arguments": arguments,
            "_meta": {"eludite/toolCallId": tool_call_id},
        });
        match self.call("tools/call", params, None).await {
            Ok(v) => ToolResult::from_json(&v),
            Err(e) => ToolResult {
                text: e,
                images: Vec::new(),
                is_error: true,
            },
        }
    }

    /// `resources/read`: the first text content of `uri`.
    pub async fn read_resource(&self, uri: &str) -> Result<String, String> {
        let r = self
            .call("resources/read", json!({"uri": uri}), Some(SETUP_TIMEOUT))
            .await?;
        r.get("contents")
            .and_then(Value::as_array)
            .and_then(|c| c.iter().find_map(|x| x.get("text").and_then(Value::as_str)))
            .map(str::to_owned)
            .ok_or_else(|| format!("{uri} has no text"))
    }
}

/// Route one incoming message: a reply to its waiter, a server request answered through `reply`, a notification.
fn dispatch(msg: &Value, pending: &Pending, changed: &AtomicBool, reply: impl Fn(Value)) {
    let method = msg.get("method").and_then(Value::as_str);
    match (method, msg.get("id")) {
        (None, Some(id)) => {
            let Some(id) = id.as_u64() else { return };
            let Some(tx) = pending.lock().ok().and_then(|mut p| p.remove(&id)) else {
                return;
            };
            let r = match msg.get("error") {
                Some(e) => Err(e
                    .get("message")
                    .and_then(Value::as_str)
                    .unwrap_or("the MCP server answered an error")
                    .to_owned()),
                None => Ok(msg.get("result").cloned().unwrap_or(Value::Null)),
            };
            let _ = tx.send(r);
        }
        (Some(m), Some(id)) => {
            let answer = if m == "ping" {
                json!({"jsonrpc": "2.0", "id": id, "result": {}})
            } else {
                json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("method not found: {m}")}})
            };
            reply(answer);
        }
        (Some("notifications/tools/list_changed"), None) => {
            changed.store(true, Ordering::Release);
        }
        _ => {}
    }
}

/// One streamable-HTTP exchange: a JSON body, or an event stream read until the reply to `id`.
fn http_post(
    agent: &ureq::Agent,
    url: &str,
    headers: &[(String, String)],
    session: &Mutex<Option<String>>,
    msg: &Value,
    id: Option<u64>,
    changed: &AtomicBool,
) -> Reply {
    let mut req = agent
        .post(url)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("MCP-Protocol-Version", PROTOCOL_VERSION);
    for (k, v) in headers {
        req = req.header(k.as_str(), v.as_str());
    }
    if let Some(s) = session.lock().ok().and_then(|s| s.clone()) {
        req = req.header("Mcp-Session-Id", s.as_str());
    }
    let resp = req
        .send(msg.to_string().as_bytes())
        .map_err(|e| format!("could not reach the MCP server: {e}"))?;
    if let Some(s) = resp
        .headers()
        .get("mcp-session-id")
        .and_then(|v| v.to_str().ok())
        && let Ok(mut slot) = session.lock()
    {
        *slot = Some(s.to_owned());
    }
    let status = resp.status().as_u16();
    let ctype = resp
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_ascii_lowercase();
    let Some(id) = id else {
        return Ok(Value::Null);
    };
    if !(200..300).contains(&status) {
        return Err(format!("the MCP server answered {status}"));
    }
    let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
    let pending = Pending::default();
    let (tx, mut rx) = oneshot::channel();
    if let Ok(mut p) = pending.lock() {
        p.insert(id, tx);
    }
    if ctype.starts_with("text/event-stream") {
        let mut sse = SseParser::default();
        for line in BufReader::new(reader).lines() {
            let Ok(line) = line else { break };
            if let Some(crate::provider::SseItem::Data(d)) = sse.line(&line)
                && let Ok(v) = serde_json::from_str::<Value>(&d)
            {
                dispatch(&v, &pending, changed, |_| {});
                if let Ok(Some(r)) = rx.try_recv() {
                    return r;
                }
            }
        }
        Err("the MCP server's stream ended without a reply".into())
    } else {
        let mut body = String::new();
        BufReader::new(reader)
            .read_to_string(&mut body)
            .map_err(|e| e.to_string())?;
        let v: Value = serde_json::from_str(&body).map_err(|e| e.to_string())?;
        dispatch(&v, &pending, changed, |_| {});
        rx.try_recv()
            .ok()
            .flatten()
            .unwrap_or_else(|| Err("the MCP server's answer is not the reply".into()))
    }
}

/// `--tools`: which tools the model is sent.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ToolsMode {
    /// The core set plus `eludite-tools` (the default; small windows).
    #[default]
    Core,
    /// Every listed tool.
    All,
}

impl std::str::FromStr for ToolsMode {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        match s {
            "core" => Ok(ToolsMode::Core),
            "all" => Ok(ToolsMode::All),
            other => Err(format!("--tools is core or all, not {other}")),
        }
    }
}

/// The core set, by command id without `eludite.` (brief 0059). `build.solution` stands in for `build.workspace`
/// where the IDE has not renamed it yet.
pub const CORE_TOOLS: &[&str] = &[
    "file.read",
    "file.edit",
    "file.open",
    "workspace.tree",
    "workspace.apply_edit",
    "search.find",
    "search.replace",
    "diagnostics.list",
    "build.workspace",
    "build.project",
    "output.show",
    "test.discover",
    "test.run",
    "test.results",
    "terminal.open",
    "terminal.send",
    "terminal.read",
    "terminal.wait",
    "git.status",
    "git.diff",
    "git.stage",
    "git.commit",
    "git.log",
    "editor.go_to_definition",
    "editor.find_references",
    "editor.hover",
    "debug.start",
    "debug.snapshot",
    "debug.toggle_breakpoint",
    "debug.wait",
];

/// A tool the model may call: which server has it, the name the model uses, and the MCP tool.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolEntry {
    pub server: usize,
    /// The function name the model sees: the MCP name, prefixed with `<server>__` only when two servers share it.
    pub name: String,
    pub tool: McpTool,
}

/// The session's tools and which of them the model is sent.
#[derive(Debug, Clone, Default)]
pub struct ToolSet {
    pub mode: ToolsMode,
    entries: Vec<ToolEntry>,
    enabled: BTreeSet<String>,
}

fn sanitize(name: &str) -> String {
    name.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect()
}

impl ToolSet {
    pub fn new(mode: ToolsMode) -> Self {
        Self {
            mode,
            ..Self::default()
        }
    }

    /// Replace the tools with `lists[i]` from server `i` (named `names[i]`), keeping what the model enabled.
    pub fn set(&mut self, names: &[String], lists: Vec<Vec<McpTool>>) {
        let mut count: HashMap<String, usize> = HashMap::new();
        for t in lists.iter().flatten() {
            *count.entry(t.name.clone()).or_default() += 1;
        }
        self.entries = lists
            .into_iter()
            .enumerate()
            .flat_map(|(server, tools)| {
                let count = &count;
                tools.into_iter().map(move |tool| {
                    let name = if count[&tool.name] > 1 {
                        sanitize(&format!(
                            "{}__{}",
                            names.get(server).map(String::as_str).unwrap_or("mcp"),
                            tool.name
                        ))
                    } else {
                        sanitize(&tool.name)
                    };
                    ToolEntry { server, name, tool }
                })
            })
            .collect();
        let has_workspace_build = self
            .entries
            .iter()
            .any(|e| e.tool.short_id() == "build.workspace");
        for e in &self.entries {
            let id = e.tool.short_id();
            if CORE_TOOLS.contains(&id.as_str()) || (id == "build.solution" && !has_workspace_build)
            {
                self.enabled.insert(e.name.clone());
            }
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn find(&self, name: &str) -> Option<&ToolEntry> {
        self.entries.iter().find(|e| e.name == name)
    }

    fn sent(&self, e: &ToolEntry) -> bool {
        self.mode == ToolsMode::All || self.enabled.contains(&e.name)
    }

    /// The Chat Completions `tools` array: the sent tools, then `eludite-tools` in core mode.
    pub fn function_tools(&self) -> Vec<Value> {
        let mut out: Vec<Value> = self
            .entries
            .iter()
            .filter(|e| self.sent(e))
            .map(|e| function_tool(&e.name, &e.tool.description, &e.tool.input_schema))
            .collect();
        if self.mode == ToolsMode::Core && !self.entries.is_empty() {
            out.push(meta_tool());
        }
        out
    }

    /// The names of the tools sent to the model.
    pub fn sent_names(&self) -> Vec<String> {
        self.entries
            .iter()
            .filter(|e| self.sent(e))
            .map(|e| e.name.clone())
            .collect()
    }

    /// `eludite-tools {prefix?, enable?}`: enable the named tools from the next request, then list the tools not
    /// sent (filtered by `prefix`) with one line each.
    pub fn meta_call(&mut self, args: &Value) -> String {
        let mut out = String::new();
        if let Some(names) = args.get("enable").and_then(Value::as_array) {
            let mut added = Vec::new();
            let mut unknown = Vec::new();
            for n in names.iter().filter_map(Value::as_str) {
                if self.find(n).is_some() {
                    if self.enabled.insert(n.to_owned()) {
                        added.push(n.to_owned());
                    }
                } else {
                    unknown.push(n.to_owned());
                }
            }
            if !added.is_empty() {
                out.push_str(&format!(
                    "Enabled from the next step: {}.\n",
                    added.join(", ")
                ));
            }
            if !unknown.is_empty() {
                out.push_str(&format!("No such tools: {}.\n", unknown.join(", ")));
            }
            if args.get("prefix").is_none() {
                return if out.is_empty() {
                    "Those tools are already enabled.".into()
                } else {
                    out
                };
            }
        }
        let prefix = args.get("prefix").and_then(Value::as_str).unwrap_or("");
        let rows: Vec<String> = self
            .entries
            .iter()
            .filter(|e| !self.sent(e) && e.name.starts_with(prefix))
            .map(|e| format!("{}: {}", e.name, first_line(&e.tool.description)))
            .collect();
        if rows.is_empty() {
            out.push_str("No other tools match.");
        } else {
            out.push_str(&format!(
                "{} more tools (call eludite-tools with enable: [names] to use them):\n{}",
                rows.len(),
                rows.join("\n")
            ));
        }
        out
    }
}

fn first_line(s: &str) -> String {
    let line = s.lines().next().unwrap_or("").trim();
    let end = line
        .find(". ")
        .map(|i| i + 1)
        .unwrap_or(line.len())
        .min(160);
    line.chars().take(end).collect()
}

/// A Chat Completions function tool.
pub fn function_tool(name: &str, description: &str, schema: &Value) -> Value {
    let mut parameters = schema.clone();
    if let Some(o) = parameters.as_object_mut() {
        // JSON Schema's identity keys mean nothing to the model and some servers reject them.
        o.remove("$schema");
        o.remove("$id");
        o.remove("title");
        if !o.contains_key("type") {
            o.insert("type".into(), json!("object"));
        }
    }
    json!({
        "type": "function",
        "function": {"name": name, "description": description, "parameters": parameters},
    })
}

/// `eludite-tools`' own definition.
pub fn meta_tool() -> Value {
    function_tool(
        META_TOOL,
        "Lists the IDE's other tools with one line each (filtered by `prefix`, such as `eludite-debug-`), and with \
         `enable: [names]` adds those tools to your tool list from the next step.",
        &json!({
            "type": "object",
            "properties": {
                "prefix": {"type": "string", "description": "Only tools whose names start with this."},
                "enable": {"type": "array", "items": {"type": "string"}, "description": "Tool names to add."}
            },
            "additionalProperties": false
        }),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(name: &str, command: Option<&str>) -> McpTool {
        McpTool {
            name: name.into(),
            description: format!("Does {name}. More text."),
            input_schema: json!({"$schema": "x", "title": "t", "type": "object"}),
            command: command.map(str::to_owned),
            permission: Some("read".into()),
        }
    }

    #[test]
    fn core_set_meta_tool_and_enable() {
        let mut set = ToolSet::new(ToolsMode::Core);
        set.set(
            &["eludite".into()],
            vec![vec![
                tool("eludite-file-read", Some("eludite.file.read")),
                tool("diagnostics-list", Some("diagnostics.list")),
                tool("eludite-build-solution", Some("eludite.build.solution")),
                tool("eludite-debug-evaluate", Some("eludite.debug.evaluate")),
                tool("eludite-git-push", None),
            ]],
        );
        let names: Vec<String> = set
            .function_tools()
            .iter()
            .map(|t| t["function"]["name"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(
            names,
            [
                "eludite-file-read",
                "diagnostics-list",
                "eludite-build-solution",
                META_TOOL
            ]
        );
        let f = &set.function_tools()[0]["function"]["parameters"];
        assert!(f.get("$schema").is_none() && f.get("title").is_none());
        let listing = set.meta_call(&json!({"prefix": "eludite-debug"}));
        assert!(
            listing.contains("eludite-debug-evaluate: Does eludite-debug-evaluate."),
            "{listing}"
        );
        assert!(!listing.contains("git-push"));
        let r = set.meta_call(&json!({"enable": ["eludite-debug-evaluate", "nope"]}));
        assert!(
            r.contains("Enabled from the next step: eludite-debug-evaluate"),
            "{r}"
        );
        assert!(r.contains("No such tools: nope"));
        assert!(
            set.sent_names()
                .contains(&"eludite-debug-evaluate".to_owned())
        );
        // A new list keeps what was enabled; all mode sends everything and no meta tool.
        set.set(
            &["eludite".into()],
            vec![vec![tool("eludite-debug-evaluate", None)]],
        );
        assert_eq!(set.sent_names(), ["eludite-debug-evaluate"]);
        let mut all = ToolSet::new(ToolsMode::All);
        all.set(&["e".into()], vec![vec![tool("a", None), tool("b", None)]]);
        assert_eq!(all.function_tools().len(), 2);
    }

    #[test]
    fn colliding_names_are_prefixed_and_specs_parse() {
        let mut set = ToolSet::new(ToolsMode::All);
        set.set(
            &["one".into(), "two".into()],
            vec![
                vec![tool("x", None)],
                vec![tool("x", None), tool("y", None)],
            ],
        );
        assert_eq!(set.sent_names(), ["one__x", "two__x", "y"]);
        assert_eq!(set.find("two__x").unwrap().server, 1);
        let s = ServerSpec::from_acp(
            &json!({"name": "eludite", "command": "eludite", "args": ["--mcp-relay", "1.2.3.4:5"],
            "env": [{"name": "ELUDITE_MCP_TOKEN", "value": "t"}]}),
        )
        .unwrap();
        assert!(matches!(&s, ServerSpec::Stdio { env, .. } if env[0].0 == "ELUDITE_MCP_TOKEN"));
        let h = ServerSpec::from_acp(
            &json!({"type": "http", "name": "h", "url": "http://x/mcp", "headers": []}),
        )
        .unwrap();
        assert_eq!(h.name(), "h");
        assert!(
            ServerSpec::from_acp(&json!({"type": "sse", "name": "s", "url": "u", "headers": []}))
                .is_none()
        );
    }

    #[test]
    fn tool_results_and_dispatch() {
        let r = ToolResult::from_json(&json!({"content": [{"type": "text", "text": "a"},
            {"type": "image", "data": "AAA", "mimeType": "image/png"}], "isError": false}));
        assert_eq!(r.text, "a");
        assert_eq!(r.images, [("image/png".to_owned(), "AAA".to_owned())]);
        let e = ToolResult::from_json(
            &json!({"content": [{"type": "text", "text": "permission denied"}], "isError": true}),
        );
        assert!(e.is_error);
        let pending = Pending::default();
        let changed = AtomicBool::new(false);
        let (tx, mut rx) = oneshot::channel();
        pending.lock().unwrap().insert(7, tx);
        dispatch(
            &json!({"jsonrpc": "2.0", "method": "notifications/tools/list_changed"}),
            &pending,
            &changed,
            |_| {},
        );
        assert!(changed.load(Ordering::Acquire));
        let replies = Mutex::new(Vec::new());
        dispatch(
            &json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}),
            &pending,
            &changed,
            |r| replies.lock().unwrap().push(r),
        );
        assert_eq!(replies.lock().unwrap()[0]["result"], json!({}));
        dispatch(
            &json!({"jsonrpc": "2.0", "id": 7, "result": {"ok": true}}),
            &pending,
            &changed,
            |_| {},
        );
        assert_eq!(rx.try_recv().unwrap().unwrap().unwrap()["ok"], true);
    }
}
