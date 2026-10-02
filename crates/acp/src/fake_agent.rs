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
//! Run it with [`run`] over any streams, or as the `eludite-fake-acp-agent`
//! binary (`--scenario NAME`, `--chunks N`, `--rate HZ`, `--edit RELPATH`).

use std::collections::{BTreeMap, VecDeque};
use std::io::{self, BufRead, BufReader, Write};
use std::process::{Command, Stdio};
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
}

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
                self.reply(&id, json!({"sessionId": session}))?;
                let status = if self.opts.scenario == Scenario::LoginRequired {
                    json!({"kind": "none", "label": "Not logged in"})
                } else {
                    json!({"kind": "account", "label": "Fake subscription"})
                };
                self.write(json!({"jsonrpc": "2.0", "method": "_auth/status_update", "params": {"authStatus": status}}))
            }
            (methods::SESSION_PROMPT, Some(id)) => {
                self.cancelled = false;
                let stop = match self.opts.scenario {
                    Scenario::LoginRequired => {
                        return self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": AUTH_REQUIRED, "message": "Authentication required"}}));
                    }
                    Scenario::Stream => self.stream()?,
                    Scenario::Diagnostics => self.diagnostics(false)?,
                    Scenario::DiagnosticsThenShell => self.diagnostics(true)?,
                    Scenario::Edit => self.edit()?,
                    Scenario::Write => self.write_file()?,
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
            (_, Some(id)) if !method.is_empty() => self.write(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("fake agent: no {method}")}})),
            _ => Ok(()),
        }
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
        let Some(McpServer::Stdio {
            command,
            args: argv,
            env,
            ..
        }) = self
            .mcp
            .iter()
            .find(|s| matches!(s, McpServer::Stdio { .. }))
        else {
            return Err("no MCP server was passed".into());
        };
        let mut child = Command::new(command)
            .args(argv)
            .envs(env.iter().map(|e| (&e.name, &e.value)))
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn {command}: {e}"))?;
        let mut stdin = child.stdin.take().expect("piped");
        let mut stdout = BufReader::new(child.stdout.take().expect("piped"));
        let mut send = |v: Value| {
            writeln!(stdin, "{v}")
                .and_then(|()| stdin.flush())
                .map_err(|e| e.to_string())
        };
        send(
            json!({"jsonrpc": "2.0", "id": 1, "method": "initialize", "params": {"protocolVersion": "2025-06-18", "capabilities": {}, "clientInfo": {"name": "eludite-fake-acp-agent", "version": "0"}}}),
        )?;
        send(json!({"jsonrpc": "2.0", "method": "notifications/initialized"}))?;
        send(
            json!({"jsonrpc": "2.0", "id": 2, "method": "tools/call", "params": {"name": tool, "arguments": args, "_meta": {"claudecode/toolUseId": tool_use_id}}}),
        )?;
        let mut result = None;
        let mut line = String::new();
        while result.is_none() {
            line.clear();
            if stdout.read_line(&mut line).map_err(|e| e.to_string())? == 0 {
                break;
            }
            let v: Value = serde_json::from_str(&line).map_err(|e| e.to_string())?;
            if v["id"] == 2 {
                result = Some(v);
            }
        }
        let _ = child.kill();
        let _ = child.wait();
        let v = result.ok_or("MCP server closed before answering")?;
        if let Some(e) = v.get("error") {
            return Err(e.to_string());
        }
        Ok(v["result"].clone())
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
