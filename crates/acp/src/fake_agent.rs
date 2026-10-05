//! A scripted ACP agent for tests and benchmarks.
//!
//! Its messages replay the shapes recorded from the real Claude adapter
//! (`tests/fixtures/claude-agent-acp-0.85.0-diagnostics.jsonl`): the same
//! `tool_call` / `tool_call_update` sequence, `_meta.claudeCode.toolName`, the
//! permission options, the `-32000` auth error. It reacts to the client: it
//! honours permission answers and, when `session/new` passes a stdio MCP
//! server, really launches it and calls `diagnostics-list` through it.
//!
//! Brief 0016 adds the scenarios the Agents window is tested with: an edit
//! through Eludite's `eludite.workspace.apply_edit` tool (with a thought and a
//! plan first), a write with the agent's own file tool (a permission request
//! carrying a diff, as Claude's `Write` makes), and an agent that exits mid-turn.
//!
//! Brief 0024 adds a scripted `script` scenario (each step an Eludite tool call
//! through one MCP connection, its result's text and image content forwarded as
//! the tool call's content, as Claude's adapter forwards MCP results) and the
//! `browser-form` scenario of proposal 0002 brief A's proof: open the form at
//! `--url`, screenshot it, read it, fill the name and the size, click Submit,
//! wait for the result and read its text. [`McpClient`] is the MCP connection,
//! also usable from tests.
//!
//! Brief 0030 adds the `planned` scenario: a script whose next tool call is
//! decided from what the agent has seen so far ([`Planner`], given the prompt,
//! the tools from `tools/list`, the debugging guide from `resources/read` and
//! every answer with its size), as the agent debugging proving scenario needs:
//! a breakpoint's line comes from a find, a step from the stop it quotes.
//! [`McpClient`] gained `tools/list` and `resources/read`.
//!
//! Brief 0034: the `stream` scenario ends its turn with a `usage_update` in
//! eludite-claude-acp's shape ([`stream_usage`]).
//!
//! Brief 0056: the `stream` scenario sends two slash commands ([`stream_commands`]) in an
//! `available_commands_update` right after `session/new` answers, as eludite-claude-acp does, and answers a prompt
//! that starts with `/` the way Claude Code answers a local command: one line of text naming it, no stream.
//!
//! Brief 0057: `--options` (any scenario) makes `session/new` answer `modes` (`default` "Manual" and `plan`) and
//! two select config options ([`fake_options`]: `model` with `fast` and `smart`, `effort` with `default`, `low`,
//! `high` and `max`); `_meta.claudeCode.options.model` and `.effort` in `session/new` choose their current values, so
//! a test reads back what the client sent. `session/set_mode` and `session/set_config_option` set a listed value,
//! answer and notify `current_mode_update` or `config_option_update`; an unlisted one is `invalid_params`, and so is
//! the listed effort `max` ([`REFUSED_EFFORT`]), so a test sees an agent refuse a choice it offered.
//!
//! Run it with [`run`] over any streams, or as the `eludite-fake-acp-agent`
//! binary (`--scenario NAME`, `--chunks N`, `--rate HZ`, `--edit RELPATH`,
//! `--script JSON`, `--url URL`, `--options`). `planned` needs a [`Planner`] in [`Options`],
//! so it runs in-process only.

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::{Value, json};

use crate::protocol::{AUTH_REQUIRED, McpServer, methods};

/// The tool names the fake agent uses, as Claude's adapter reports them.
pub const DIAGNOSTICS_TOOL: &str = "mcp__eludite__diagnostics-list";
pub const SHELL_TOOL: &str = "Bash";
pub const SHELL_COMMAND: &str = "rm -rf obj/";
/// The Eludite edit tool, as Claude names it.
pub const APPLY_EDIT_TOOL: &str = "mcp__eludite__eludite-workspace-apply_edit";
/// The agent's own file tool.
pub const WRITE_TOOL: &str = "Write";
/// The line the edit scenario inserts at the top of each file.
pub const EDIT_HEADER: &str = "// Edited by the agent\n";
/// The file the write scenario creates, relative to the session's cwd, and its content.
pub const WRITE_FILE: &str = "notes.txt";
pub const WRITE_TEXT: &str = "hello\n";

/// The slash commands the `stream` scenario sends after `session/new` (brief 0056), in ACP's `AvailableCommand` shape.
pub fn stream_commands() -> Value {
    json!([
        {"name": "compact", "description": "Free up context by summarizing the conversation so far",
         "input": {"hint": "<optional custom summarization instructions>"}},
        {"name": "model", "description": "Set the AI model for Claude Code", "input": {"hint": "<model>"}}
    ])
}

/// The text the `stream` scenario answers a slash command `prompt` with (brief 0056).
pub fn slash_reply(prompt: &str) -> String {
    format!("Ran the local command {prompt}")
}

/// The modes `--options` offers (brief 0057), in ACP's `SessionMode` shape.
pub fn fake_modes() -> Value {
    json!([
        {"id": "default", "name": "Manual", "description": "Prompts as the policy says"},
        {"id": "plan", "name": "Plan", "description": "Plans before making changes"}
    ])
}

/// The config options `--options` offers (brief 0057) with these current values, in ACP's `SessionConfigOption` shape.
pub fn fake_options(model: &str, effort: &str) -> Value {
    json!([
        {"id": "model", "name": "Model", "description": "The model the agent uses", "category": "model",
         "type": "select", "currentValue": model, "options": [
            {"value": "fast", "name": "Fast", "description": "Quick answers"},
            {"value": "smart", "name": "Smart", "description": "For complex work"}
        ]},
        {"id": "effort", "name": "Effort", "category": "thought_level", "type": "select", "currentValue": effort,
         "options": [
            {"value": "default", "name": "Default", "description": "The model decides"},
            {"value": "low", "name": "Low"},
            {"value": "high", "name": "High"},
            {"value": "max", "name": "Max", "description": "Refused by the fake agent"}
        ]}
    ])
}

/// The effort `--options` lists but refuses to set, and why.
pub const REFUSED_EFFORT: &str = "max";
pub const REFUSED_WHY: &str = "max effort is not available to this account";

/// Whether `value` is one of the `--options` option `id`'s values.
fn fake_option_has(id: &str, value: &str) -> bool {
    fake_options("", "")
        .as_array()
        .into_iter()
        .flatten()
        .filter(|o| o["id"] == id)
        .flat_map(|o| o["options"].as_array().into_iter().flatten())
        .any(|c| c["value"] == value)
}

/// The `usage_update` the `stream` scenario ends its turn with (brief 0034), in eludite-claude-acp's shape: brief
/// 0030's first OffByOne run's counts.
pub fn stream_usage() -> Value {
    json!({
        "sessionUpdate": "usage_update", "used": 61_204, "size": 1_000_000,
        "cost": {"amount": 0.9512, "currency": "USD"},
        "_meta": {"claudeCode": {"usage": {
            "inputTokens": 162, "cachedReadTokens": 207_671, "cachedWriteTokens": 51_993, "outputTokens": 2_897,
            "totalTokens": 262_723, "model": "claude-fable-5-1"
        }}}
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Scenario {
    /// Ask for `diagnostics-list`, call it, answer which file has most errors.
    Diagnostics,
    /// [`Scenario::Diagnostics`], then ask to run a shell command (class
    /// execute: the client must prompt).
    DiagnosticsThenShell,
    /// Stream `chunks` message chunks at `rate_hz`, each stamped with its send time.
    Stream,
    /// Logged out: `initialize` lists a terminal login method, `session/prompt`
    /// fails with `-32000`, as the real adapter does.
    LoginRequired,
    /// A thought and a plan, then `eludite.workspace.apply_edit` through the MCP
    /// server inserting [`EDIT_HEADER`] at the top of each `--edit` file, then
    /// the outcome the tool reported.
    Edit,
    /// The agent's own `Write` of [`WRITE_FILE`]: a permission request with a
    /// diff; the agent writes the file itself when allowed.
    Write,
    /// One chunk, then the process exits mid-turn.
    Exit,
    /// Each step of `--script` (`[{"tool": "eludite-browser-tabs", "arguments": {...}}, ...]`) as a tool call
    /// through the MCP server, then a summary of the results.
    Script,
    /// Proposal 0002 brief A's proof: fill the form at `--url` through the browser tools and read the result.
    BrowserForm,
    /// Brief 0030: tool calls decided one at a time by [`Options::planner`] from the prompt, the tool list, the
    /// debugging guide and the answers so far; then the planner's answer.
    Planned,
}

/// The MCP resource the planned scenario reads before its first call (brief 0027's guide).
pub const GUIDE_URI: &str = "eludite://guides/debugging";

/// What a [`Planner`] knows when it picks the next step: what a model would have in its context.
#[derive(Debug, Clone, Default)]
pub struct Seen {
    /// The prompt's text.
    pub prompt: String,
    /// The tools `tools/list` returned (name, description, input schema).
    pub tools: Vec<Value>,
    /// The text of [`GUIDE_URI`], empty when the server has none.
    pub guide: String,
    /// The calls made so far, in order.
    pub steps: Vec<Step>,
}

/// One tool call of the planned scenario and its answer.
#[derive(Debug, Clone)]
pub struct Step {
    /// The MCP tool name (`eludite-debug-start`).
    pub tool: String,
    pub arguments: Value,
    /// The result's `structuredContent`, or the error text.
    pub result: Result<Value, String>,
    /// The bytes of the answer's text content: what the model reads.
    pub bytes: usize,
    /// The call's wall time, in milliseconds.
    pub ms: f64,
}

/// The planner's decision.
#[derive(Debug, Clone, PartialEq)]
pub enum Next {
    /// Call `tool` (the MCP name) with `arguments`.
    Call { tool: String, arguments: Value },
    /// End the turn with this message.
    Answer(String),
}

/// Picks the planned scenario's next step from what has been seen.
pub trait Planner: Send {
    fn next(&mut self, seen: &Seen) -> Next;
}

/// A shared [`Planner`] in [`Options`] (the test that made it keeps a handle to read it afterwards).
#[derive(Clone)]
pub struct PlannerHandle(pub Arc<Mutex<dyn Planner>>);

impl std::fmt::Debug for PlannerHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("PlannerHandle")
    }
}

/// The most calls the planned scenario makes before it gives up.
pub const MAX_PLANNED_STEPS: usize = 30;

/// What the `browser-form` scenario fills in, and the result text it expects to read.
pub const FORM_NAME: &str = "Ada Lovelace";
pub const FORM_CHOICE: &str = "Large";
/// The `--url` of `browser-form` when none is given.
const FORM_URL: &str = "http://127.0.0.1/act.html";

impl std::str::FromStr for Scenario {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, String> {
        Ok(match s {
            "diagnostics" => Scenario::Diagnostics,
            "diagnostics-then-shell" => Scenario::DiagnosticsThenShell,
            "stream" => Scenario::Stream,
            "login-required" => Scenario::LoginRequired,
            "edit" => Scenario::Edit,
            "write" => Scenario::Write,
            "exit" => Scenario::Exit,
            "script" => Scenario::Script,
            "browser-form" => Scenario::BrowserForm,
            "planned" => Scenario::Planned,
            other => return Err(format!("unknown scenario {other}")),
        })
    }
}

#[derive(Debug, Clone)]
pub struct Options {
    pub scenario: Scenario,
    pub chunks: usize,
    pub rate_hz: f64,
    /// The files [`Scenario::Edit`] edits, relative to the session's cwd.
    pub edit_files: Vec<String>,
    /// Reach a stdio MCP server whose arguments are `--mcp-relay ADDR` by connecting to ADDR directly with the
    /// token from its environment, instead of launching it (an in-process fake agent in the shell's tests).
    pub mcp_direct: bool,
    /// [`Scenario::Script`]'s steps: `{"tool": NAME, "arguments": {...}}` (the bare MCP tool name).
    pub script: Vec<Value>,
    /// [`Scenario::BrowserForm`]'s page.
    pub url: Option<String>,
    /// [`Scenario::Planned`]'s planner.
    pub planner: Option<PlannerHandle>,
    /// Offer modes and config options (`--options`, brief 0057).
    pub options: bool,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            scenario: Scenario::DiagnosticsThenShell,
            chunks: 1000,
            rate_hz: 200.,
            edit_files: vec![
                "src/App/Program.cs".into(),
                "src/App/Models/Order.cs".into(),
            ],
            mcp_direct: false,
            script: Vec::new(),
            url: None,
            planner: None,
            options: false,
        }
    }
}

impl Options {
    /// Parse `--scenario`, `--chunks`, `--rate` from an argument list.
    pub fn from_args(args: impl IntoIterator<Item = String>) -> Result<Self, String> {
        let mut o = Options::default();
        let mut edits = Vec::new();
        let mut it = args.into_iter();
        while let Some(a) = it.next() {
            let mut val = || it.next().ok_or(format!("{a} needs a value"));
            match a.as_str() {
                "--scenario" => o.scenario = val()?.parse()?,
                "--chunks" => o.chunks = val()?.parse().map_err(|e| format!("{e}"))?,
                "--rate" => o.rate_hz = val()?.parse().map_err(|e| format!("{e}"))?,
                "--edit" => edits.push(val()?),
                "--script" => {
                    o.script =
                        serde_json::from_str(&val()?).map_err(|e| format!("--script: {e}"))?
                }
                "--url" => o.url = Some(val()?),
                "--options" => o.options = true,
                other => return Err(format!("unknown argument {other}")),
            }
        }
        if !edits.is_empty() {
            o.edit_files = edits;
        }
        Ok(o)
    }
}

/// Key in a stream chunk's content `_meta` holding its send time (ns since the Unix epoch).
pub const SENT_AT_META: &str = "eludite/sentAtNs";

pub fn wall_ns() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos())
}

struct Agent<R, W> {
    input: R,
    out: W,
    opts: Options,
    session: String,
    cwd: String,
    mcp: Vec<McpServer>,
    /// Messages read while waiting for a specific response.
    backlog: VecDeque<Value>,
    cancelled: bool,
    next_id: u64,
    /// The text of the prompt being answered.
    prompt: String,
    /// `--options`: the current mode, model and effort.
    mode: String,
    model: String,
    effort: String,
}

/// Serve one client until its stdin closes.
pub fn run(input: impl BufRead, output: impl Write, opts: Options) -> io::Result<()> {
    let mut agent = Agent {
        input,
        out: output,
        opts,
        session: "fake-session-1".into(),
        cwd: String::new(),
        mcp: Vec::new(),
        backlog: VecDeque::new(),
        cancelled: false,
        next_id: 0,
        prompt: String::new(),
        mode: "default".into(),
        model: "smart".into(),
        effort: "default".into(),
    };
    while let Some(msg) = agent.next_message()? {
        agent.dispatch(msg)?;
    }
    Ok(())
}

impl<R: BufRead, W: Write> Agent<R, W> {
    fn next_message(&mut self) -> io::Result<Option<Value>> {
        if let Some(m) = self.backlog.pop_front() {
            return Ok(Some(m));
        }
        self.read_raw()
    }

    fn read_raw(&mut self) -> io::Result<Option<Value>> {
        let mut line = String::new();
        loop {
            line.clear();
            if self.input.read_line(&mut line)? == 0 {
                return Ok(None);
            }
            if !line.trim().is_empty() {
                return serde_json::from_str(&line)
                    .map(Some)
                    .map_err(io::Error::other);
            }
        }
    }

    fn write(&mut self, v: Value) -> io::Result<()> {
        let mut bytes = serde_json::to_vec(&v).map_err(io::Error::other)?;
        bytes.push(b'\n');
        self.out.write_all(&bytes)?;
        self.out.flush()
    }

    fn reply(&mut self, id: &Value, result: Value) -> io::Result<()> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    fn update(&mut self, update: Value) -> io::Result<()> {
        let session = self.session.clone();
        self.write(json!({"jsonrpc": "2.0", "method": methods::SESSION_UPDATE, "params": {"sessionId": session, "update": update}}))
    }

    /// Stream `text` in small chunks, as the real adapter does.
    fn say(&mut self, text: &str) -> io::Result<()> {
        let chars: Vec<char> = text.chars().collect();
        for piece in chars.chunks(12) {
            let s: String = piece.iter().collect();
            self.update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": s}}))?;
        }
        Ok(())
    }

    fn dispatch(&mut self, msg: Value) -> io::Result<()> {
        let id = msg.get("id").cloned();
        let method = msg
            .get("method")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned();
        let params = msg.get("params").cloned().unwrap_or(Value::Null);
        match (method.as_str(), id) {
            (methods::INITIALIZE, Some(id)) => {
                let auth_methods = if self.opts.scenario == Scenario::LoginRequired {
                    json!([{"id": "claude-ai-login", "name": "Claude Subscription", "description": "Use Claude subscription ", "type": "terminal", "args": ["--cli", "auth", "login", "--claudeai"]}])
                } else {
                    json!([])
                };
                self.reply(&id, json!({
                    "protocolVersion": 1,
                    "agentCapabilities": {"loadSession": false, "promptCapabilities": {"image": false, "embeddedContext": false}, "mcpCapabilities": {"http": false, "sse": false}},
                    "agentInfo": {"name": "eludite-fake-acp-agent", "title": "Fake agent", "version": env!("CARGO_PKG_VERSION")},
                    "authMethods": auth_methods
                }))
            }
            (methods::SESSION_NEW, Some(id)) => {
                self.mcp = serde_json::from_value(params["mcpServers"].clone()).unwrap_or_default();
                self.cwd = params["cwd"].as_str().unwrap_or_default().to_owned();
                let session = self.session.clone();
                if self.opts.options {
                    // What the client remembered, when it is a listed value (brief 0057).
                    let meta = |k: &str| {
                        params
                            .pointer(&format!("/_meta/claudeCode/options/{k}"))
                            .and_then(Value::as_str)
                            .filter(|v| fake_option_has(k, v))
                            .map(str::to_owned)
                    };
                    if let Some(m) = meta("model") {
                        self.model = m;
                    }
                    if let Some(e) = meta("effort") {
                        self.effort = e;
                    }
                    let options = fake_options(&self.model, &self.effort);
                    self.reply(&id, json!({"sessionId": session,
                        "modes": {"currentModeId": self.mode, "availableModes": fake_modes()},
                        "configOptions": options}))?;
                } else {
                    self.reply(&id, json!({"sessionId": session}))?;
                }
                let status = if self.opts.scenario == Scenario::LoginRequired {
                    json!({"kind": "none", "label": "Not logged in"})
                } else {
                    json!({"kind": "account", "label": "Fake subscription"})
                };
                self.write(json!({"jsonrpc": "2.0", "method": "_auth/status_update", "params": {"authStatus": status}}))?;
                if self.opts.scenario == Scenario::Stream {
                    self.update(json!({"sessionUpdate": "available_commands_update", "availableCommands": stream_commands()}))?;
                }
                Ok(())
            }
            (methods::SESSION_PROMPT, Some(id)) => {
                self.cancelled = false;
                self.prompt = params["prompt"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|b| b["text"].as_str())
                    .collect::<Vec<_>>()
                    .join("\n");
                let stop = match self.opts.scenario {
                    Scenario::LoginRequired => {
                        return self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": AUTH_REQUIRED, "message": "Authentication required"}}));
                    }
                    Scenario::Stream if self.prompt.starts_with('/') => {
                        let reply = slash_reply(&self.prompt);
                        self.say(&reply)?;
                        "end_turn"
                    }
                    Scenario::Stream => self.stream()?,
                    Scenario::Diagnostics => self.diagnostics(false)?,
                    Scenario::DiagnosticsThenShell => self.diagnostics(true)?,
                    Scenario::Edit => self.edit()?,
                    Scenario::Write => self.write_file()?,
                    Scenario::Script => self.script()?,
                    Scenario::BrowserForm => self.browser_form()?,
                    Scenario::Planned => self.planned()?,
                    Scenario::Exit => {
                        self.say("Starting on it")?;
                        return Err(io::Error::other("the fake agent exits mid-turn"));
                    }
                };
                self.reply(&id, json!({"stopReason": stop}))
            }
            (methods::SESSION_CANCEL, None) => {
                self.cancelled = true;
                Ok(())
            }
            (methods::SESSION_SET_MODE, Some(id)) if self.opts.options => {
                let mode = params["modeId"].as_str().unwrap_or_default().to_owned();
                if !fake_modes()
                    .as_array()
                    .is_some_and(|m| m.iter().any(|m| m["id"] == mode.as_str()))
                {
                    return self.invalid(&id, &format!("unknown mode {mode}"));
                }
                self.mode = mode.clone();
                self.reply(&id, json!({}))?;
                self.update(json!({"sessionUpdate": "current_mode_update", "currentModeId": mode}))
            }
            (methods::SESSION_SET_CONFIG_OPTION, Some(id)) if self.opts.options => {
                let config = params["configId"].as_str().unwrap_or_default().to_owned();
                let value = params["value"].as_str().unwrap_or_default().to_owned();
                if !fake_option_has(&config, &value) {
                    return self.invalid(&id, &format!("unknown value {value} for {config}"));
                }
                if config == "effort" && value == REFUSED_EFFORT {
                    return self.invalid(&id, REFUSED_WHY);
                }
                match config.as_str() {
                    "model" => self.model = value,
                    _ => self.effort = value,
                }
                let options = fake_options(&self.model, &self.effort);
                self.reply(&id, json!({"configOptions": options}))?;
                self.update(json!({"sessionUpdate": "config_option_update", "configOptions": options}))
            }
            (_, Some(id)) if !method.is_empty() => self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("fake agent: no {method}")}})),
            _ => Ok(()),
        }
    }

    fn invalid(&mut self, id: &Value, why: &str) -> io::Result<()> {
        self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": "Invalid params", "data": why}}))
    }

    fn stream(&mut self) -> io::Result<&'static str> {
        let period = Duration::from_secs_f64(1. / self.opts.rate_hz.max(1.));
        let start = Instant::now();
        for i in 0..self.opts.chunks {
            // Absolute deadlines so the rate does not drift with write cost.
            let due = start + period * i as u32;
            if let Some(wait) = due.checked_duration_since(Instant::now()) {
                std::thread::sleep(wait);
            }
            let text = if i % 8 == 7 {
                format!("chunk {i:05} of the stream benchmark.\n")
            } else {
                format!("chunk {i:05} lorem ipsum dolor sit amet, ")
            };
            self.update(json!({"sessionUpdate": "agent_message_chunk", "content": {"type": "text", "text": text, "_meta": {SENT_AT_META: wall_ns().to_string()}}}))?;
        }
        // The turn's usage, as eludite-claude-acp reports Claude Code's (brief 0034).
        self.update(stream_usage())?;
        Ok("end_turn")
    }

    /// Send `session/request_permission` and wait for the answer. Returns true
    /// if an allow option was selected.
    fn ask(&mut self, tool_call: Value) -> io::Result<bool> {
        self.next_id += 1;
        let id = format!("perm-{}", self.next_id);
        let session = self.session.clone();
        self.write(json!({"jsonrpc": "2.0", "id": id, "method": methods::SESSION_REQUEST_PERMISSION, "params": {
            "sessionId": session,
            "toolCall": tool_call,
            "options": [
                {"optionId": "allow-once", "name": "Yes", "kind": "allow_once"},
                {"optionId": "allow-with-updates", "name": "Yes, and don't ask again", "kind": "allow_always"},
                {"optionId": "reject", "name": "No", "kind": "reject_once"}
            ]
        }}))?;
        loop {
            let Some(msg) = self.read_raw()? else {
                return Ok(false);
            };
            if msg.get("id").and_then(Value::as_str) == Some(id.as_str())
                && msg.get("method").is_none()
            {
                let outcome = &msg["result"]["outcome"];
                return Ok(outcome["outcome"] == "selected"
                    && outcome["optionId"]
                        .as_str()
                        .is_some_and(|o| o.starts_with("allow")));
            }
            if msg.get("method").and_then(Value::as_str) == Some(methods::SESSION_CANCEL) {
                self.cancelled = true;
                return Ok(false);
            }
            self.backlog.push_back(msg);
        }
    }

    fn tool_call(id: &str, tool: &str, title: &str, kind: &str, raw_input: Value) -> Value {
        json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": title, "kind": kind, "status": "pending", "rawInput": raw_input, "content": [], "_meta": {"claudeCode": {"toolName": tool}}})
    }

    fn diagnostics(&mut self, then_shell: bool) -> io::Result<&'static str> {
        self.say("I'll pull the IDE's diagnostics list and tally errors per file.")?;
        let tc = "toolu_fake_diagnostics";
        let args = json!({"severity": "error"});
        let mut call = Self::tool_call(tc, DIAGNOSTICS_TOOL, DIAGNOSTICS_TOOL, "other", json!({}));
        self.update(call.clone())?;
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "rawInput": args, "_meta": {"claudeCode": {"toolName": DIAGNOSTICS_TOOL}}}))?;
        call["rawInput"] = args.clone();
        call.as_object_mut()
            .expect("object")
            .remove("sessionUpdate");
        call["_meta"]["claudeCode"]["mcpServer"] = json!({"name": "eludite", "source": "dynamic"});
        if !self.ask(call)? {
            self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "failed", "content": [{"type": "content", "content": {"type": "text", "text": "The user doesn't want to proceed with this tool use. The tool use was rejected."}}]}))?;
            if self.cancelled {
                return Ok("cancelled");
            }
            self.say(
                "Permission to run diagnostics-list was denied, so I can't read the Error List.",
            )?;
            return Ok("end_turn");
        }
        self.update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "in_progress"}),
        )?;
        let (text, rows) = match self.call_mcp_diagnostics(&args) {
            Ok(r) => r,
            Err(e) => {
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "failed", "content": [{"type": "content", "content": {"type": "text", "text": format!("MCP call failed: {e}")}}]}))?;
                self.say(&format!("The diagnostics tool failed: {e}"))?;
                return Ok("end_turn");
            }
        };
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "completed", "rawOutput": text, "content": [{"type": "content", "content": {"type": "text", "text": text}}]}))?;
        self.say(&answer(&rows))?;

        if then_shell {
            let tc = "toolu_fake_shell";
            let input = json!({"command": SHELL_COMMAND, "description": "Remove build output"});
            let title = format!("`{SHELL_COMMAND}`");
            let mut call = Self::tool_call(tc, SHELL_TOOL, &title, "execute", input);
            self.update(call.clone())?;
            call.as_object_mut()
                .expect("object")
                .remove("sessionUpdate");
            if self.ask(call)? {
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "completed", "content": [{"type": "content", "content": {"type": "text", "text": "(fake) removed obj/"}}]}))?;
                self.say("\n\nI also cleaned obj/.")?;
            } else {
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "failed", "content": [{"type": "content", "content": {"type": "text", "text": "The user doesn't want to proceed with this tool use. The tool use was rejected."}}]}))?;
                if self.cancelled {
                    return Ok("cancelled");
                }
                self.say(&format!(
                    "\n\nYou denied permission to run `{SHELL_COMMAND}`, so I did not run it."
                ))?;
            }
        }
        Ok("end_turn")
    }

    /// Call `diagnostics-list` through the first stdio MCP server from
    /// `session/new`, or return a canned result if there is none.
    fn call_mcp_diagnostics(&self, args: &Value) -> Result<(String, Vec<Value>), String> {
        if !self
            .mcp
            .iter()
            .any(|s| matches!(s, McpServer::Stdio { .. }))
        {
            let rows = vec![
                json!({"path": "Program.cs", "line": 1, "column": 1, "severity": "error", "code": "CS0000", "message": "canned: no MCP server was passed"}),
            ];
            let text = json!({"result": rows}).to_string();
            return Ok((text, rows));
        }
        let r = self.call_mcp("diagnostics-list", args, "toolu_fake_diagnostics")?;
        if r["isError"] == true {
            return Err(r["content"][0]["text"]
                .as_str()
                .unwrap_or("tool error")
                .to_owned());
        }
        let rows = r["structuredContent"]["result"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        Ok((r["structuredContent"].to_string(), rows))
    }

    /// Call `tool` through the first stdio MCP server from `session/new`, as
    /// Claude does: `initialize`, then `tools/call` with the tool use id in
    /// `_meta`. Returns the call's `result`.
    fn call_mcp(&self, tool: &str, args: &Value, tool_use_id: &str) -> Result<Value, String> {
        McpClient::connect(&self.mcp, self.opts.mcp_direct)?.call(tool, args, tool_use_id)
    }

    fn mcp_tool_call(id: &str, tool: &str, args: &Value) -> Value {
        let mut call = Self::tool_call(id, tool, tool, "other", args.clone());
        call.as_object_mut()
            .expect("object")
            .remove("sessionUpdate");
        call["_meta"]["claudeCode"]["mcpServer"] = json!({"name": "eludite", "source": "dynamic"});
        call
    }

    fn rejected(&mut self, tc: &str) -> io::Result<()> {
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "failed", "content": [{"type": "content", "content": {"type": "text", "text": "The user doesn't want to proceed with this tool use. The tool use was rejected."}}]}))
    }

    /// [`Scenario::Edit`].
    fn edit(&mut self) -> io::Result<&'static str> {
        self.update(json!({"sessionUpdate": "agent_thought_chunk", "content": {"type": "text", "text": "The user wants a header comment in each file. One workspace edit covers both."}}))?;
        let entries: Vec<Value> = self
            .opts
            .edit_files
            .iter()
            .enumerate()
            .map(|(i, f)| json!({"content": format!("Add the header to {f}"), "priority": "medium", "status": if i == 0 { "in_progress" } else { "pending" }}))
            .collect();
        self.update(json!({"sessionUpdate": "plan", "entries": entries}))?;
        self.say("I'll add a header comment to each file with one workspace edit.")?;
        let tc = "toolu_fake_edit";
        let mut changes = serde_json::Map::new();
        for rel in &self.opts.edit_files {
            let path = std::path::Path::new(&self.cwd).join(rel);
            changes.insert(
                file_uri(&path),
                json!([{"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}}, "newText": EDIT_HEADER}]),
            );
        }
        let args = json!({"edit": {"changes": changes}, "label": "Agent: add header comments"});
        let call = Self::tool_call(tc, APPLY_EDIT_TOOL, APPLY_EDIT_TOOL, "other", json!({}));
        self.update(call)?;
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "rawInput": args, "_meta": {"claudeCode": {"toolName": APPLY_EDIT_TOOL}}}))?;
        if !self.ask(Self::mcp_tool_call(tc, APPLY_EDIT_TOOL, &args))? {
            self.rejected(tc)?;
            return Ok(if self.cancelled {
                "cancelled"
            } else {
                "end_turn"
            });
        }
        self.update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "in_progress"}),
        )?;
        match self.call_mcp("eludite-workspace-apply_edit", &args, tc) {
            Ok(r) => {
                let text = r["content"][0]["text"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let failed = r["isError"] == true;
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": if failed { "failed" } else { "completed" }, "rawOutput": text, "content": [{"type": "content", "content": {"type": "text", "text": text}}]}))?;
                let out = &r["structuredContent"];
                let reply = if failed {
                    format!("The edit failed: {text}")
                } else {
                    format!(
                        "The edit ended `{}`: {} edits in {} files.{}",
                        out["state"].as_str().unwrap_or("?"),
                        out["edits"],
                        out["files"],
                        out["message"]
                            .as_str()
                            .map(|m| format!(" {m}"))
                            .unwrap_or_default()
                    )
                };
                self.say(&reply)?;
            }
            Err(e) => {
                self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "failed", "content": [{"type": "content", "content": {"type": "text", "text": format!("MCP call failed: {e}")}}]}))?;
                self.say(&format!("The edit tool failed: {e}"))?;
            }
        }
        Ok(if self.cancelled {
            "cancelled"
        } else {
            "end_turn"
        })
    }

    /// [`Scenario::Write`].
    fn write_file(&mut self) -> io::Result<&'static str> {
        self.say("I'll create the notes file.")?;
        let tc = "toolu_fake_write";
        let path = std::path::Path::new(&self.cwd).join(WRITE_FILE);
        let path_text = path.to_string_lossy().into_owned();
        let input = json!({"file_path": path_text, "content": WRITE_TEXT});
        let mut call = Self::tool_call(
            tc,
            WRITE_TOOL,
            &format!("Write {WRITE_FILE}"),
            "edit",
            input,
        );
        call["content"] =
            json!([{"type": "diff", "path": path_text, "oldText": null, "newText": WRITE_TEXT}]);
        call["locations"] = json!([{"path": path_text}]);
        self.update(call.clone())?;
        call.as_object_mut()
            .expect("object")
            .remove("sessionUpdate");
        if self.ask(call)? {
            std::fs::write(&path, WRITE_TEXT)?;
            self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "completed", "content": [{"type": "diff", "path": path_text, "oldText": null, "newText": WRITE_TEXT}]}))?;
            self.say("Created notes.txt.")?;
        } else {
            self.rejected(tc)?;
            if self.cancelled {
                return Ok("cancelled");
            }
            self.say("The write to notes.txt was declined, so the file was not created.")?;
        }
        Ok("end_turn")
    }
}

/// One MCP connection, as an agent holds it: the stdio server from `session/new` launched (or, with `direct`, its
/// `--mcp-relay ADDR` reached over TCP with the token from its environment), `initialize`d once, then any number of
/// `tools/call`s.
pub struct McpClient {
    stdin: Box<dyn Write + Send>,
    stdout: Box<dyn BufRead + Send>,
    child: Option<std::process::Child>,
    next_id: u64,
}

impl McpClient {
    /// Connect to the first stdio server of `servers`.
    pub fn connect(servers: &[McpServer], direct: bool) -> Result<Self, String> {
        let Some(McpServer::Stdio {
            command,
            args: argv,
            env,
            ..
        }) = servers
            .iter()
            .find(|s| matches!(s, McpServer::Stdio { .. }))
        else {
            return Err("no MCP server was passed".into());
        };
        if direct {
            let addr = argv
                .iter()
                .skip_while(|a| *a != "--mcp-relay")
                .nth(1)
                .ok_or("no --mcp-relay address")?;
            let token = env
                .iter()
                .find(|e| e.name == "ELUDITE_MCP_TOKEN")
                .map(|e| e.value.clone())
                .ok_or("no token")?;
            return Self::connect_tcp(addr, &token);
        }
        let mut child = Command::new(command)
            .args(argv)
            .envs(env.iter().map(|e| (&e.name, &e.value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {command}: {e}"))?;
        let stdin = child.stdin.take().expect("piped");
        let stdout = BufReader::new(child.stdout.take().expect("piped"));
        Self::start(Box::new(stdin), Box::new(stdout), Some(child))
    }

    /// Connect to Eludite's MCP endpoint at `addr` with its token (what `eludite --mcp-relay` does).
    pub fn connect_tcp(addr: &str, token: &str) -> Result<Self, String> {
        let mut sock =
            std::net::TcpStream::connect(addr).map_err(|e| format!("connect {addr}: {e}"))?;
        writeln!(sock, "{token}").map_err(|e| e.to_string())?;
        let read = sock.try_clone().map_err(|e| e.to_string())?;
        Self::start(Box::new(sock), Box::new(BufReader::new(read)), None)
    }

    fn start(
        stdin: Box<dyn Write + Send>,
        stdout: Box<dyn BufRead + Send>,
        child: Option<std::process::Child>,
    ) -> Result<Self, String> {
        let mut c = Self {
            stdin,
            stdout,
            child,
            next_id: 0,
        };
        c.request(
            "initialize",
            json!({"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "eludite-fake-acp-agent", "version": "0"}}),
        )?;
        c.send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        Ok(c)
    }

    fn send(&mut self, v: Value) -> Result<(), String> {
        writeln!(self.stdin, "{v}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| e.to_string())
    }

    fn request(&mut self, method: &str, params: Value) -> Result<Value, String> {
        self.next_id += 1;
        let id = self.next_id;
        self.send(json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params}))?;
        let mut line = String::new();
        loop {
            line.clear();
            if self
                .stdout
                .read_line(&mut line)
                .map_err(|e| e.to_string())?
                == 0
            {
                return Err("MCP server closed before answering".into());
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if v["id"] == id {
                if let Some(e) = v.get("error") {
                    return Err(e.to_string());
                }
                return Ok(v["result"].clone());
            }
        }
    }

    /// Every tool of `tools/list`, following its cursor.
    pub fn list_tools(&mut self) -> Result<Vec<Value>, String> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let r = self.request("tools/list", params)?;
            tools.extend(r["tools"].as_array().cloned().unwrap_or_default());
            match r["nextCursor"].as_str() {
                Some(c) if !c.is_empty() => cursor = Some(c.to_owned()),
                _ => return Ok(tools),
            }
        }
    }

    /// The text of resource `uri` (`resources/read`).
    pub fn read_resource(&mut self, uri: &str) -> Result<String, String> {
        let r = self.request("resources/read", json!({ "uri": uri }))?;
        Ok(r["contents"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|c| c["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"))
    }

    /// `tools/call` with the agent's tool use id in `_meta`, as Claude sends it. Returns the call's `result`.
    pub fn call(&mut self, tool: &str, args: &Value, tool_use_id: &str) -> Result<Value, String> {
        self.request(
            "tools/call",
            json!({"name": tool, "arguments": args, "_meta": {"claudecode/toolUseId": tool_use_id}}),
        )
    }
}

impl Drop for McpClient {
    fn drop(&mut self) {
        if let Some(c) = self.child.as_mut() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

/// An MCP tool result's content as ACP tool call content: text and images.
fn acp_content(result: &Value) -> Vec<Value> {
    result["content"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|c| matches!(c["type"].as_str(), Some("text" | "image")))
        .map(|c| json!({"type": "content", "content": c}))
        .collect()
}

/// The ref of the first node of `read_page`'s answer with `role` whose name contains `name`.
fn ref_of(page: &Value, role: &str, name: &str) -> Option<String> {
    page["nodes"]
        .as_array()?
        .iter()
        .find(|n| n["role"] == role && n["name"].as_str().is_some_and(|n| n.contains(name)))?["ref"]
        .as_str()
        .map(str::to_owned)
}

impl<R: BufRead, W: Write> Agent<R, W> {
    /// One Eludite tool call as Claude makes it: announced, permission asked (the client allows Eludite's tools and
    /// checks them at its MCP boundary), run through `client`, and ended with the result's content. Returns the
    /// result's `structuredContent`, or the error text.
    fn tool(
        &mut self,
        client: &mut McpClient,
        n: usize,
        tool: &str,
        args: &Value,
    ) -> io::Result<Result<Value, String>> {
        let tc = format!("toolu_fake_step_{n}");
        let full = format!("mcp__eludite__{tool}");
        self.update(Self::tool_call(&tc, &full, &full, "other", json!({})))?;
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "rawInput": args, "_meta": {"claudeCode": {"toolName": full}}}))?;
        if !self.ask(Self::mcp_tool_call(&tc, &full, args))? {
            self.rejected(&tc)?;
            return Ok(Err("the user rejected the tool use".into()));
        }
        self.update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "in_progress"}),
        )?;
        let result = client.call(tool, args, &tc);
        let (status, content, out) = match result {
            Ok(r) if r["isError"] == true => {
                let text = r["content"][0]["text"]
                    .as_str()
                    .unwrap_or("tool error")
                    .to_owned();
                ("failed", acp_content(&r), Err(text))
            }
            Ok(r) => (
                "completed",
                acp_content(&r),
                Ok(r["structuredContent"].clone()),
            ),
            Err(e) => (
                "failed",
                vec![
                    json!({"type": "content", "content": {"type": "text", "text": format!("MCP call failed: {e}")}}),
                ],
                Err(e),
            ),
        };
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": status, "content": content}))?;
        Ok(out)
    }

    fn stop(&self) -> &'static str {
        if self.cancelled {
            "cancelled"
        } else {
            "end_turn"
        }
    }

    /// [`Scenario::Script`].
    fn script(&mut self) -> io::Result<&'static str> {
        let mut client = match McpClient::connect(&self.mcp, self.opts.mcp_direct) {
            Ok(c) => c,
            Err(e) => {
                self.say(&format!("I could not reach Eludite's tools: {e}"))?;
                return Ok("end_turn");
            }
        };
        let steps = self.opts.script.clone();
        let mut lines = Vec::new();
        for (n, step) in steps.iter().enumerate() {
            let tool = step["tool"].as_str().unwrap_or_default();
            let args = step.get("arguments").cloned().unwrap_or_else(|| json!({}));
            let out = self.tool(&mut client, n + 1, tool, &args)?;
            if self.cancelled {
                return Ok("cancelled");
            }
            lines.push(match out {
                Ok(_) => format!("{tool}: ok"),
                Err(e) => format!("{tool}: {e}"),
            });
        }
        self.say(&lines.join("\n"))?;
        Ok(self.stop())
    }

    /// [`Scenario::BrowserForm`]: tab_open, screenshot, read_page, form_input, input (click Submit), wait,
    /// page_text, each through Eludite's MCP tools, then the result text.
    fn browser_form(&mut self) -> io::Result<&'static str> {
        self.say("I'll open the form, fill it in and submit it.")?;
        let mut client = match McpClient::connect(&self.mcp, self.opts.mcp_direct) {
            Ok(c) => c,
            Err(e) => {
                self.say(&format!("I could not reach Eludite's tools: {e}"))?;
                return Ok("end_turn");
            }
        };
        let url = self.opts.url.clone().unwrap_or_else(|| FORM_URL.into());
        let mut n = 0;
        let mut step =
            |agent: &mut Self, tool: &str, args: Value| -> io::Result<Result<Value, String>> {
                n += 1;
                agent.tool(&mut client, n, tool, &args)
            };
        macro_rules! ok {
            ($e:expr) => {
                match $e? {
                    Ok(v) => v,
                    Err(e) => {
                        self.say(&format!("\n\nThat failed: {e}"))?;
                        return Ok(self.stop());
                    }
                }
            };
        }
        ok!(step(
            self,
            "eludite-browser-tab_open",
            json!({ "url": url })
        ));
        ok!(step(
            self,
            "eludite-browser-screenshot",
            json!({"max_width": 640})
        ));
        let page = ok!(step(self, "eludite-browser-read_page", json!({})));
        let (Some(name), Some(choice), Some(submit)) = (
            ref_of(&page, "textbox", "Name"),
            ref_of(&page, "radio", FORM_CHOICE),
            ref_of(&page, "button", "Submit"),
        ) else {
            self.say("\n\nThe page has no Name box, size choice or Submit button.")?;
            return Ok(self.stop());
        };
        ok!(step(
            self,
            "eludite-browser-form_input",
            json!({"fields": [{"ref": name, "value": FORM_NAME}, {"ref": choice, "value": true}]})
        ));
        ok!(step(
            self,
            "eludite-browser-input",
            json!({"action": "click", "ref": submit, "wait_ms": 100})
        ));
        let waited = ok!(step(
            self,
            "eludite-browser-wait",
            json!({"for": "selector", "css": "#result", "wait_ms": 5000})
        ));
        let Some(result) = waited["satisfied"]["ref"].as_str().map(str::to_owned) else {
            self.say("\n\nThe result did not appear.")?;
            return Ok(self.stop());
        };
        let text = ok!(step(
            self,
            "eludite-browser-page_text",
            json!({ "root": result })
        ));
        self.say(&format!(
            "\n\nThe form answered: {}",
            text["text"].as_str().unwrap_or_default()
        ))?;
        Ok(self.stop())
    }
}

impl<R: BufRead, W: Write> Agent<R, W> {
    /// One tool call as [`Agent::tool`] makes it, returning the whole MCP result (or the error text).
    fn tool_raw(
        &mut self,
        client: &mut McpClient,
        n: usize,
        tool: &str,
        args: &Value,
    ) -> io::Result<Result<Value, String>> {
        let tc = format!("toolu_fake_step_{n}");
        let full = format!("mcp__eludite__{tool}");
        self.update(Self::tool_call(&tc, &full, &full, "other", json!({})))?;
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "rawInput": args, "_meta": {"claudeCode": {"toolName": full}}}))?;
        if !self.ask(Self::mcp_tool_call(&tc, &full, args))? {
            self.rejected(&tc)?;
            return Ok(Err("the user rejected the tool use".into()));
        }
        self.update(
            json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": "in_progress"}),
        )?;
        let result = client.call(tool, args, &tc);
        let (status, content) = match &result {
            Ok(r) => (
                if r["isError"] == true {
                    "failed"
                } else {
                    "completed"
                },
                acp_content(r),
            ),
            Err(e) => (
                "failed",
                vec![
                    json!({"type": "content", "content": {"type": "text", "text": format!("MCP call failed: {e}")}}),
                ],
            ),
        };
        self.update(json!({"sessionUpdate": "tool_call_update", "toolCallId": tc, "status": status, "content": content}))?;
        Ok(result)
    }

    /// [`Scenario::Planned`]: read the tools and the guide, then call what the planner picks until it answers.
    fn planned(&mut self) -> io::Result<&'static str> {
        let Some(planner) = self.opts.planner.clone() else {
            self.say("The planned scenario has no planner.")?;
            return Ok("end_turn");
        };
        let mut client = match McpClient::connect(&self.mcp, self.opts.mcp_direct) {
            Ok(c) => c,
            Err(e) => {
                self.say(&format!("I could not reach Eludite's tools: {e}"))?;
                return Ok("end_turn");
            }
        };
        let mut seen = Seen {
            prompt: self.prompt.clone(),
            tools: client.list_tools().unwrap_or_default(),
            guide: client.read_resource(GUIDE_URI).unwrap_or_default(),
            steps: Vec::new(),
        };
        for n in 1..=MAX_PLANNED_STEPS {
            let next = planner
                .0
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .next(&seen);
            let (tool, arguments) = match next {
                Next::Answer(text) => {
                    self.say(&text)?;
                    return Ok(self.stop());
                }
                Next::Call { tool, arguments } => (tool, arguments),
            };
            let started = Instant::now();
            let raw = self.tool_raw(&mut client, n, &tool, &arguments)?;
            let ms = started.elapsed().as_secs_f64() * 1e3;
            if self.cancelled {
                return Ok("cancelled");
            }
            let (result, bytes) = match raw {
                Ok(r) => {
                    let bytes = r["content"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|c| c["text"].as_str())
                        .map(str::len)
                        .sum();
                    let text = r["content"][0]["text"].as_str().unwrap_or("tool error");
                    if r["isError"] == true {
                        (Err(text.to_owned()), bytes)
                    } else {
                        (Ok(r["structuredContent"].clone()), bytes)
                    }
                }
                Err(e) => (Err(e), 0),
            };
            seen.steps.push(Step {
                tool,
                arguments,
                result,
                bytes,
                ms,
            });
        }
        self.say(&format!(
            "I stopped after {MAX_PLANNED_STEPS} tool calls without an answer."
        ))?;
        Ok(self.stop())
    }
}

/// A `file://` URI for an absolute path.
fn file_uri(path: &std::path::Path) -> String {
    let p = path.to_string_lossy().replace('\\', "/");
    if p.starts_with('/') {
        format!("file://{p}")
    } else {
        format!("file:///{p}")
    }
}

/// The answer the fake agent gives: errors per file, the file with the most.
pub fn answer(rows: &[Value]) -> String {
    let mut per_file: BTreeMap<&str, usize> = BTreeMap::new();
    for r in rows.iter().filter(|r| r["severity"] == "error") {
        *per_file
            .entry(r["path"].as_str().unwrap_or("?"))
            .or_default() += 1;
    }
    let total: usize = per_file.values().sum();
    let Some((file, n)) = per_file
        .iter()
        .max_by_key(|(f, n)| (**n, std::cmp::Reverse(**f)))
    else {
        return "The Error List has no errors.".into();
    };
    let mut s = format!(
        "The Error List has {total} errors. The file with the most is `{file}` with {n}.\n"
    );
    for (f, c) in &per_file {
        s.push_str(&format!("\n- {f}: {c}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn script_options_refs_and_content() {
        let o = Options::from_args(
            [
                "--scenario",
                "script",
                "--script",
                r#"[{"tool": "eludite-browser-tabs"}]"#,
                "--url",
                "http://127.0.0.1:1/act.html",
            ]
            .map(String::from),
        )
        .unwrap();
        assert_eq!(o.scenario, Scenario::Script);
        assert_eq!(o.script[0]["tool"], "eludite-browser-tabs");
        assert_eq!(o.url.as_deref(), Some("http://127.0.0.1:1/act.html"));
        assert!(Options::from_args(["--script", "nope"].map(String::from)).is_err());
        assert_eq!(
            "browser-form".parse::<Scenario>().unwrap(),
            Scenario::BrowserForm
        );
        assert_eq!("planned".parse::<Scenario>().unwrap(), Scenario::Planned);
        assert!(o.planner.is_none());
        let page = json!({"nodes": [{"ref": "e1", "role": "textbox", "name": "Notes"}, {"ref": "e2", "role": "textbox", "name": "Name"}, {"ref": "e3", "role": "radio", "name": "Large"}]});
        assert_eq!(ref_of(&page, "textbox", "Name").as_deref(), Some("e2"));
        assert_eq!(ref_of(&page, "radio", "Large").as_deref(), Some("e3"));
        assert_eq!(ref_of(&page, "button", "Submit"), None);
        let content = acp_content(&json!({"content": [
            {"type": "text", "text": "{}"},
            {"type": "image", "data": "iVBORw0KGgo=", "mimeType": "image/png"},
            {"type": "resource", "resource": {}}
        ]}));
        assert_eq!(content.len(), 2);
        assert_eq!(content[1]["content"]["type"], "image");
    }

    /// A planner that echoes once, then answers with what it saw.
    struct Echo;

    impl Planner for Echo {
        fn next(&mut self, seen: &Seen) -> Next {
            match seen.steps.as_slice() {
                [] => Next::Call {
                    tool: "echo".into(),
                    arguments: json!({"text": seen.prompt}),
                },
                [step] => Next::Answer(format!(
                    "tools {}, guide {:?}, echo {} in {} bytes",
                    seen.tools.len(),
                    seen.guide,
                    step.result.as_ref().unwrap()["text"],
                    step.bytes
                )),
                _ => Next::Answer("too many".into()),
            }
        }
    }

    /// A one-connection MCP server on loopback: two pages of tools, the guide, and `echo`.
    fn mcp_server() -> (String, std::thread::JoinHandle<Vec<String>>) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();
        let server = std::thread::spawn(move || {
            let (sock, _) = listener.accept().unwrap();
            let mut methods = Vec::new();
            let mut out = sock.try_clone().unwrap();
            let mut lines = BufReader::new(sock).lines();
            assert_eq!(lines.next().unwrap().unwrap(), "token");
            for line in lines {
                let m: Value = serde_json::from_str(&line.unwrap()).unwrap();
                let method = m["method"].as_str().unwrap_or_default().to_owned();
                methods.push(method.clone());
                let result = match method.as_str() {
                    "initialize" => json!({"capabilities": {}}),
                    "tools/list" if m["params"]["cursor"].is_null() => {
                        json!({"tools": [{"name": "echo"}], "nextCursor": "2"})
                    }
                    "tools/list" => json!({"tools": [{"name": "other"}]}),
                    "resources/read" => {
                        json!({"contents": [{"uri": m["params"]["uri"], "text": "the guide"}]})
                    }
                    "tools/call" => {
                        let text = m["params"]["arguments"]["text"].clone();
                        let out = json!({ "text": text });
                        json!({"content": [{"type": "text", "text": out.to_string()}], "structuredContent": out})
                    }
                    _ => continue,
                };
                writeln!(
                    out,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": m["id"], "result": result})
                )
                .unwrap();
            }
            methods
        });
        (addr, server)
    }

    #[test]
    fn the_planned_scenario_reads_the_tools_and_the_guide_and_calls_what_the_planner_picks() {
        let (addr, server) = mcp_server();
        let opts = Options {
            scenario: Scenario::Planned,
            mcp_direct: true,
            planner: Some(PlannerHandle(Arc::new(Mutex::new(Echo)))),
            ..Options::default()
        };
        let session_new = json!({"jsonrpc": "2.0", "id": 1, "method": methods::SESSION_NEW, "params": {
            "cwd": "/", "mcpServers": [{"name": "eludite", "command": "eludite", "args": ["--mcp-relay", addr],
                                        "env": [{"name": "ELUDITE_MCP_TOKEN", "value": "token"}]}]}});
        let prompt = json!({"jsonrpc": "2.0", "id": 2, "method": methods::SESSION_PROMPT, "params": {
            "sessionId": "fake-session-1", "prompt": [{"type": "text", "text": "hello"}]}});
        let input = format!("{session_new}\n{prompt}\n");
        let mut output = Vec::new();
        // Every permission request is answered from the backlog: the agent reads stdin until it is answered.
        let allow = json!({"jsonrpc": "2.0", "id": "perm-1", "result": {"outcome": {"outcome": "selected", "optionId": "allow-once"}}});
        let input = format!("{input}{allow}\n");
        run(input.as_bytes(), &mut output, opts).unwrap();
        let out = String::from_utf8(output).unwrap();
        let said: String = out
            .lines()
            .map(|l| serde_json::from_str::<Value>(l).unwrap())
            .filter(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
            .map(|m| {
                m["params"]["update"]["content"]["text"]
                    .as_str()
                    .unwrap()
                    .to_owned()
            })
            .collect();
        assert_eq!(
            said,
            "tools 2, guide \"the guide\", echo \"hello\" in 16 bytes"
        );
        assert!(out.contains("toolu_fake_step_1"), "{out}");
        assert!(out.contains(r#""stopReason":"end_turn""#), "{out}");
        assert_eq!(
            server.join().unwrap(),
            [
                "initialize",
                "notifications/initialized",
                "tools/list",
                "tools/list",
                "resources/read",
                "tools/call"
            ]
        );
    }

    /// Brief 0057: `--options` answers `session/new` with modes and config options (the current ones from `_meta`),
    /// echoes `session/set_mode` and `session/set_config_option` with a notification, and refuses unlisted values.
    #[test]
    fn the_options_flag_offers_modes_and_options_and_echoes_changes() {
        let lines = [
            json!({"jsonrpc": "2.0", "id": 1, "method": methods::SESSION_NEW, "params": {
                "cwd": "/", "mcpServers": [], "_meta": {"claudeCode": {"options": {"model": "fast", "effort": "nope"}}}}}),
            json!({"jsonrpc": "2.0", "id": 2, "method": methods::SESSION_SET_MODE, "params": {
                "sessionId": "fake-session-1", "modeId": "plan"}}),
            json!({"jsonrpc": "2.0", "id": 3, "method": methods::SESSION_SET_CONFIG_OPTION, "params": {
                "sessionId": "fake-session-1", "configId": "effort", "value": "high"}}),
            json!({"jsonrpc": "2.0", "id": 4, "method": methods::SESSION_SET_CONFIG_OPTION, "params": {
                "sessionId": "fake-session-1", "configId": "model", "value": "huge"}}),
            json!({"jsonrpc": "2.0", "id": 5, "method": methods::SESSION_SET_MODE, "params": {
                "sessionId": "fake-session-1", "modeId": "bypassPermissions"}}),
            json!({"jsonrpc": "2.0", "id": 6, "method": methods::SESSION_SET_CONFIG_OPTION, "params": {
                "sessionId": "fake-session-1", "configId": "effort", "value": REFUSED_EFFORT}}),
        ];
        let input: String = lines.iter().map(|l| format!("{l}\n")).collect();
        let mut output = Vec::new();
        let opts =
            Options::from_args(["--scenario".into(), "edit".into(), "--options".into()]).unwrap();
        run(input.as_bytes(), &mut output, opts).unwrap();
        let out: Vec<Value> = String::from_utf8(output)
            .unwrap()
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        let answer = |id: u64| out.iter().find(|m| m["id"] == id).unwrap().clone();
        let new = answer(1)["result"].clone();
        assert_eq!(new["modes"]["currentModeId"], "default");
        assert_eq!(new["modes"]["availableModes"], fake_modes());
        assert_eq!(
            new["configOptions"],
            fake_options("fast", "default"),
            "an unlisted effort is ignored"
        );
        assert_eq!(answer(2)["result"], json!({}));
        assert_eq!(
            answer(3)["result"]["configOptions"],
            fake_options("fast", "high")
        );
        assert_eq!(answer(4)["error"]["code"], -32602);
        assert_eq!(answer(5)["error"]["code"], -32602);
        assert_eq!(answer(6)["error"]["data"], REFUSED_WHY);
        let kinds: Vec<&str> = out
            .iter()
            .filter_map(|m| m["params"]["update"]["sessionUpdate"].as_str())
            .collect();
        assert_eq!(kinds, ["current_mode_update", "config_option_update"]);
        // Without the flag: no modes, no options, and the methods are unknown.
        let mut output = Vec::new();
        let opts = Options::from_args(["--scenario".into(), "edit".into()]).unwrap();
        run(input.as_bytes(), &mut output, opts).unwrap();
        let first: Value =
            serde_json::from_str(String::from_utf8(output).unwrap().lines().next().unwrap())
                .unwrap();
        assert_eq!(first["result"], json!({"sessionId": "fake-session-1"}));
    }

    /// Brief 0056: the stream scenario lists its slash commands right after `session/new` answers, and answers a
    /// slash command with one line instead of the stream; other scenarios list none.
    #[test]
    fn the_stream_scenario_lists_its_commands_after_session_new_and_answers_one() {
        let session_new = json!({"jsonrpc": "2.0", "id": 1, "method": methods::SESSION_NEW, "params": {
            "cwd": "/", "mcpServers": []}});
        let prompt = json!({"jsonrpc": "2.0", "id": 2, "method": methods::SESSION_PROMPT, "params": {
            "sessionId": "fake-session-1", "prompt": [{"type": "text", "text": "/compact keep the plan"}]}});
        let input = format!("{session_new}\n{prompt}\n");
        let run_with = |scenario, input: &str| {
            let mut output = Vec::new();
            let opts = Options {
                scenario,
                ..Options::default()
            };
            run(input.as_bytes(), &mut output, opts).unwrap();
            String::from_utf8(output)
                .unwrap()
                .lines()
                .map(|l| serde_json::from_str::<Value>(l).unwrap())
                .collect::<Vec<_>>()
        };
        let out = run_with(Scenario::Stream, &input);
        let answered = out.iter().position(|m| m["id"] == 1).unwrap();
        let listed = out
            .iter()
            .position(|m| m["params"]["update"]["sessionUpdate"] == "available_commands_update")
            .expect("the commands");
        assert!(listed > answered);
        assert_eq!(
            out[listed]["params"]["update"]["availableCommands"],
            stream_commands()
        );
        let said: String = out
            .iter()
            .filter(|m| m["params"]["update"]["sessionUpdate"] == "agent_message_chunk")
            .filter_map(|m| m["params"]["update"]["content"]["text"].as_str())
            .collect();
        assert_eq!(said, slash_reply("/compact keep the plan"));
        assert!(out.iter().any(|m| m["result"]["stopReason"] == "end_turn"));
        let edit = run_with(Scenario::Edit, &format!("{session_new}\n"));
        assert!(
            !edit
                .iter()
                .any(|m| m["params"]["update"]["sessionUpdate"] == "available_commands_update")
        );
    }
}
