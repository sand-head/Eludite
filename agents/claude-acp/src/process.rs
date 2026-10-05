//! One `claude --print` child process speaking stream-json on stdio.
//!
//! A reader thread parses the child's stdout, one JSON object per line. Replies
//! to the adapter's own control requests (`initialize`, `interrupt`) are routed
//! to their waiters by `request_id`; everything else goes, in order, to an
//! unbounded channel the session's prompt task drains. A second thread copies
//! the child's stderr to the adapter's log. Writes to the child's stdin are
//! single lines under a mutex.
//!
//! Brief 0057: the launch passes `--effort` beside `--model`, and the
//! `initialize` reply's `models` are kept for the session's model and effort
//! options; [`set_permission_mode`] and [`set_model`] are the control requests
//! that change them (verified on 2.1.289: `set_permission_mode` answers
//! `{mode}` and a `system` `status` message, `set_model` answers success;
//! there is no `set_effort` control request, so the effort is set with the
//! local command `/effort`).

use std::collections::HashMap;
use std::ffi::OsString;
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use futures::channel::{mpsc, oneshot};
use serde_json::{Value, json};

use crate::discovery::CLAUDE_SESSION_ENV;
use crate::log;

/// What the child sent that the session must handle.
#[derive(Debug)]
pub enum Event {
    /// Any stream-json message except control traffic: `system`, `assistant`,
    /// `user`, `stream_event`, `result`, `rate_limit_event`, ...
    Message(Value),
    /// A `control_request` from the child (`can_use_tool`, ...). Answer with
    /// [`ClaudeProcess::respond`].
    ControlRequest { request_id: String, request: Value },
    /// The child's stdout closed.
    Exited,
}

/// How to launch the child.
#[derive(Debug, Clone)]
pub struct Launch {
    pub claude: PathBuf,
    pub cwd: PathBuf,
    pub session_id: String,
    /// The `--mcp-config` file (always passed, with `--strict-mcp-config`).
    pub mcp_config: PathBuf,
    pub model: Option<String>,
    /// `--effort` (`low`, `medium`, `high`, `xhigh`, `max`; brief 0057).
    pub effort: Option<String>,
}

impl Launch {
    /// The child's arguments (brief 0006 Contract, "Child side").
    pub fn args(&self) -> Vec<OsString> {
        let mut a: Vec<OsString> = [
            "--print",
            "--input-format",
            "stream-json",
            "--output-format",
            "stream-json",
            "--include-partial-messages",
            "--replay-user-messages",
            "--verbose",
            "--permission-prompt-tool",
            "stdio",
            "--permission-mode",
            "default",
            "--session-id",
        ]
        .iter()
        .map(OsString::from)
        .collect();
        a.push(self.session_id.clone().into());
        a.push("--mcp-config".into());
        a.push(self.mcp_config.clone().into());
        a.push("--strict-mcp-config".into());
        if let Some(m) = &self.model {
            a.push("--model".into());
            a.push(m.into());
        }
        if let Some(e) = &self.effort {
            a.push("--effort".into());
            a.push(e.into());
        }
        a
    }
}

/// What the adapter keeps of `claude`'s reply to its `initialize` control request: whether an account is logged in
/// (only that; nothing of the account is logged or kept), the slash commands, as `claude` lists them:
/// `[{name, description, argumentHint, aliases?, builtin?}]` (verified on 2.1.289; brief 0056), and the models:
/// `[{value, resolvedModel, displayName, description, supportsEffort?, supportedEffortLevels?, ...}]` (verified on
/// 2.1.289; brief 0057).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct InitializeReply {
    pub logged_in: bool,
    pub commands: Vec<Value>,
    pub models: Vec<Value>,
}

impl InitializeReply {
    pub fn parse(reply: &Value) -> Self {
        Self {
            logged_in: reply
                .get("account")
                .is_some_and(|a| a.get("tokenSource").and_then(Value::as_str) != Some("none")),
            commands: reply
                .get("commands")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
            models: reply
                .get("models")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        }
    }
}

/// The `set_permission_mode` control request.
pub fn set_permission_mode(mode: &str) -> Value {
    json!({"subtype": "set_permission_mode", "mode": mode})
}

/// The `set_model` control request.
pub fn set_model(model: &str) -> Value {
    json!({"subtype": "set_model", "model": model})
}

type Waiters = Mutex<HashMap<String, oneshot::Sender<Result<Value, String>>>>;

/// A running child.
pub struct ClaudeProcess {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
    waiters: Arc<Waiters>,
    next_id: AtomicU64,
}

impl std::fmt::Debug for ClaudeProcess {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClaudeProcess").finish_non_exhaustive()
    }
}

impl ClaudeProcess {
    /// Spawn the child. Events arrive on the returned receiver.
    pub fn spawn(launch: &Launch) -> io::Result<(Arc<Self>, mpsc::UnboundedReceiver<Event>)> {
        let mut cmd = Command::new(&launch.claude);
        // `Command` quotes arguments for CreateProcess on Windows itself;
        // `claude.exe` is a real executable, so no `cmd.exe` quoting applies.
        cmd.args(launch.args())
            .current_dir(&launch.cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for var in CLAUDE_SESSION_ENV {
            cmd.env_remove(var);
        }
        let mut child = cmd.spawn()?;
        let stdin = child.stdin.take().expect("piped stdin");
        let stdout = child.stdout.take().expect("piped stdout");
        let stderr = child.stderr.take().expect("piped stderr");
        let waiters: Arc<Waiters> = Arc::default();
        let (tx, rx) = mpsc::unbounded();

        thread::Builder::new()
            .name("claude-stderr".into())
            .spawn(move || {
                for line in BufReader::new(stderr).lines() {
                    let Ok(line) = line else { break };
                    log::info(format_args!("[claude stderr] {line}"));
                }
            })?;
        let reader_waiters = waiters.clone();
        thread::Builder::new()
            .name("claude-stdout".into())
            .spawn(move || read_loop(stdout, tx, reader_waiters))?;

        Ok((
            Arc::new(Self {
                stdin: Mutex::new(stdin),
                child: Mutex::new(child),
                waiters,
                next_id: AtomicU64::new(1),
            }),
            rx,
        ))
    }

    /// Write one JSON message as a line.
    pub fn send(&self, msg: &Value) -> io::Result<()> {
        let mut line = serde_json::to_vec(msg).map_err(io::Error::other)?;
        line.push(b'\n');
        let mut w = self
            .stdin
            .lock()
            .map_err(|_| io::Error::other("stdin lock"))?;
        w.write_all(&line)?;
        w.flush()
    }

    /// Send a user message (one turn).
    pub fn send_user(&self, session_id: &str, content: Vec<Value>) -> io::Result<()> {
        self.send(&json!({
            "type": "user",
            "message": {"role": "user", "content": content},
            "parent_tool_use_id": null,
            "session_id": session_id,
        }))
    }

    /// Send a control request (`initialize`, `interrupt`, ...) and return a
    /// receiver for its `response` payload. After `timeout` the receiver gets
    /// an error.
    pub fn control(
        &self,
        request: Value,
        timeout: Duration,
    ) -> io::Result<oneshot::Receiver<Result<Value, String>>> {
        let id = format!("eludite_{}", self.next_id.fetch_add(1, Ordering::Relaxed));
        let (tx, rx) = oneshot::channel();
        if let Ok(mut w) = self.waiters.lock() {
            w.insert(id.clone(), tx);
        }
        if let Err(e) = self.send(&json!({
            "type": "control_request",
            "request_id": id,
            "request": request,
        })) {
            if let Ok(mut w) = self.waiters.lock() {
                w.remove(&id);
            }
            return Err(e);
        }
        let waiters = self.waiters.clone();
        thread::Builder::new()
            .name("claude-control-timeout".into())
            .spawn(move || {
                thread::sleep(timeout);
                if let Some(tx) = waiters.lock().ok().and_then(|mut w| w.remove(&id)) {
                    let _ = tx.send(Err(format!("no reply to control request {id}")));
                }
            })?;
        Ok(rx)
    }

    /// Answer a child's control request with a success payload.
    pub fn respond(&self, request_id: &str, response: Value) -> io::Result<()> {
        self.send(&json!({
            "type": "control_response",
            "response": {"subtype": "success", "request_id": request_id, "response": response},
        }))
    }

    /// Answer a child's control request with an error.
    pub fn respond_error(&self, request_id: &str, error: &str) -> io::Result<()> {
        self.send(&json!({
            "type": "control_response",
            "response": {"subtype": "error", "request_id": request_id, "error": error},
        }))
    }

    /// The child's process id.
    pub fn id(&self) -> Option<u32> {
        self.child.lock().ok().map(|c| c.id())
    }

    /// Kill the child and reap it.
    pub fn kill(&self) {
        if let Ok(mut c) = self.child.lock() {
            let _ = c.kill();
            let _ = c.wait();
        }
    }
}

fn read_loop(stdout: impl io::Read, tx: mpsc::UnboundedSender<Event>, waiters: Arc<Waiters>) {
    let mut reader = BufReader::new(stdout);
    let mut line = String::new();
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => break,
            Ok(_) => {}
        }
        let text = line.trim();
        if text.is_empty() {
            continue;
        }
        let msg: Value = match serde_json::from_str(text) {
            Ok(v) => v,
            Err(e) => {
                log::warn(format_args!("claude wrote a non-JSON line ({e})"));
                continue;
            }
        };
        match msg.get("type").and_then(Value::as_str) {
            Some("control_response") => {
                let id = msg
                    .pointer("/response/request_id")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                // The child also echoes the adapter's own answers to its
                // `can_use_tool` requests; those have no waiter and are dropped.
                if let Some(w) = waiters.lock().ok().and_then(|mut w| w.remove(id)) {
                    let r = &msg["response"];
                    let _ = w.send(if r["subtype"] == "success" {
                        Ok(r.get("response").cloned().unwrap_or(Value::Null))
                    } else {
                        Err(r["error"].as_str().unwrap_or("error").to_owned())
                    });
                }
            }
            Some("control_request") => {
                let request_id = msg["request_id"].as_str().unwrap_or_default().to_owned();
                let request = msg.get("request").cloned().unwrap_or(Value::Null);
                let _ = tx.unbounded_send(Event::ControlRequest {
                    request_id,
                    request,
                });
            }
            _ => {
                let _ = tx.unbounded_send(Event::Message(msg));
            }
        }
    }
    if let Ok(mut w) = waiters.lock() {
        for (_, tx) in w.drain() {
            let _ = tx.send(Err("claude exited".into()));
        }
    }
    let _ = tx.unbounded_send(Event::Exited);
}

/// Write the client's MCP servers as a `--mcp-config` file, readable only by
/// the user (it can carry tokens in `env` or headers).
pub fn write_mcp_config(path: &Path, config: &Value) -> io::Result<()> {
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut f = opts.open(path)?;
    f.write_all(&serde_json::to_vec_pretty(config).map_err(io::Error::other)?)?;
    f.flush()
}
