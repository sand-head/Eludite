//! One `claude --print` child process speaking stream-json on stdio.
//!
//! A reader thread parses the child's stdout, one JSON object per line. Replies
//! to the adapter's own control requests (`initialize`, `interrupt`) are routed
//! to their waiters by `request_id`; everything else goes, in order, to an
//! unbounded channel the session's prompt task drains. A second thread copies
//! the child's stderr to the adapter's log. Writes to the child's stdin are
//! single lines under a mutex.
//!
//! Brief 0058: the launch passes `--effort` beside `--model`, and the
//! `initialize` reply's `models` are kept for the session's model and effort
//! options; [`set_permission_mode`] and [`set_model`] are the control requests
//! that change them (verified on 2.1.289: `set_permission_mode` answers
//! `{mode}` and a `system` `status` message, `set_model` answers success;
//! there is no `set_effort` control request, so the effort is set with the
//! local command `/effort`).
//!
//! Brief 0060: [`Launch::resume`] starts `claude --resume <session id>` in
//! place of `--session-id` (verified on 2.1.289: it continues the session in
//! its own file under the same id; a session with nothing in it yet, such as
//! one that only ran `initialize`, is refused with "No conversation found with
//! session ID" in a `result` message's `errors` and exit status 1), and
//! [`session_file`] is where Claude Code keeps a session's conversation, which
//! the adapter replays to the client.

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
    /// `--effort` (`low`, `medium`, `high`, `xhigh`, `max`; brief 0058).
    pub effort: Option<String>,
    /// Resume `session_id` (`--resume`) instead of starting it (`--session-id`; brief 0060).
    pub resume: bool,
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
        ]
        .iter()
        .map(OsString::from)
        .collect();
        a.push(
            if self.resume {
                "--resume"
            } else {
                "--session-id"
            }
            .into(),
        );
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
/// `[{name, description, argumentHint, aliases?, builtin?}]` (verified on 2.1.289; brief 0057), and the models:
/// `[{value, resolvedModel, displayName, description, supportsEffort?, supportedEffortLevels?, ...}]` (verified on
/// 2.1.289; brief 0058).
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

/// Where Claude Code keeps session `id`'s conversation for `cwd` (brief 0060): `<config>/projects/<cwd with every
/// character but ASCII letters and digits as `-`>/<id>.jsonl`, `<config>` being `$CLAUDE_CONFIG_DIR` or `~/.claude`
/// (verified on 2.1.289: `/a/b_c.d e` is `-a-b-c-d-e`). When that file is missing, the session is looked for under
/// every project folder (Claude Code shortens very long folder names). `None` when it is nowhere.
pub fn session_file(cwd: &Path, id: &str) -> Option<PathBuf> {
    if id.is_empty() || id.contains(['/', '\\']) || id.contains("..") {
        return None;
    }
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|d| !d.is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var_os(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .map(|h| PathBuf::from(h).join(".claude"))
        })?;
    session_file_in(&config.join("projects"), cwd, id)
}

/// [`session_file`] under `projects`.
pub fn session_file_in(projects: &Path, cwd: &Path, id: &str) -> Option<PathBuf> {
    let folder: String = cwd
        .to_string_lossy()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    let file = format!("{id}.jsonl");
    let direct = projects.join(folder).join(&file);
    if direct.is_file() {
        return Some(direct);
    }
    std::fs::read_dir(projects)
        .ok()?
        .flatten()
        .map(|e| e.path().join(&file))
        .find(|p| p.is_file())
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

#[cfg(test)]
mod tests {
    use super::*;

    fn launch(resume: bool) -> Launch {
        Launch {
            claude: "claude".into(),
            cwd: "/w".into(),
            session_id: "s-1".into(),
            mcp_config: "/tmp/m.json".into(),
            model: None,
            effort: None,
            resume,
        }
    }

    /// Brief 0060: a resumed launch names the session with `--resume`, never `--session-id`.
    #[test]
    fn a_resumed_launch_passes_resume_instead_of_session_id() {
        let args = |l: Launch| -> Vec<String> {
            l.args()
                .into_iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let new = args(launch(false));
        let at = new.iter().position(|a| a == "--session-id").unwrap();
        assert_eq!(new[at + 1], "s-1");
        assert!(!new.iter().any(|a| a == "--resume"));
        let resumed = args(launch(true));
        let at = resumed.iter().position(|a| a == "--resume").unwrap();
        assert_eq!(resumed[at + 1], "s-1");
        assert!(!resumed.iter().any(|a| a == "--session-id"));
        assert_eq!(new.len(), resumed.len());
    }

    /// Brief 0060: the session file is under the project folder named after the cwd (every character but ASCII
    /// letters and digits a `-`), else under any project folder; a missing file or an id with a path in it is none.
    #[test]
    fn the_session_file_is_found_under_the_cwds_project_folder() {
        let root = std::env::temp_dir().join(format!(
            "eludite-claude-acp-projects-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        let direct = root.join("-w-a-b-c-d-e");
        std::fs::create_dir_all(&direct).unwrap();
        std::fs::write(direct.join("s-1.jsonl"), "{}").unwrap();
        assert_eq!(
            session_file_in(&root, Path::new("/w/a_b.c d/e"), "s-1"),
            Some(direct.join("s-1.jsonl"))
        );
        let other = root.join("-shortened-1234");
        std::fs::create_dir_all(&other).unwrap();
        std::fs::write(other.join("s-2.jsonl"), "{}").unwrap();
        assert_eq!(
            session_file_in(&root, Path::new("/elsewhere"), "s-2"),
            Some(other.join("s-2.jsonl"))
        );
        assert_eq!(session_file_in(&root, Path::new("/w"), "s-3"), None);
        assert_eq!(
            session_file_in(&root.join("none"), Path::new("/w"), "s-1"),
            None
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
