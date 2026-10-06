//! The ACP agent: `initialize`, `authenticate`, `session/new`,
//! `session/prompt`, `session/cancel`, `session/set_mode`,
//! `session/set_config_option`, and `session/request_permission` raised
//! from the child's `can_use_tool` control requests.
//!
//! Modes and config options (brief 0058): `session/new` answers the modes
//! `default` and `plan` and the `model` and `effort` options
//! ([`crate::translate::SessionOptions`]). `session/set_mode` sends the
//! `set_permission_mode` control request; `session/set_config_option` sends
//! `set_model` for the model, and for the effort writes the local command
//! `/effort LEVEL` and consumes its reply, which is not a turn. Both are refused
//! while a turn is in progress, answer with the new state and notify
//! `current_mode_update` or `config_option_update`.
//!
//! Resuming (brief 0061): `initialize` advertises `loadSession`, and
//! `session/load` starts `claude --resume <sessionId>` (with the MCP config and
//! the `initialize` control request as for `session/new`), replays the
//! conversation from Claude Code's own session file
//! ([`crate::process::session_file`], [`crate::translate::replay`]) as
//! `session/update` notifications, then answers with the modes and config
//! options. A `--resume` that `claude` refuses fails the load with its message;
//! an id this adapter already has live is `invalid_params`.
//!
//! Each ACP session owns one `claude --print` child (see [`crate::process`]).
//! Long-running work (`session/new`, each turn, each permission request) runs
//! in tasks spawned on the connection, never in the dispatch loop, so a
//! `session/cancel` is handled while a turn is streaming.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthMethod, AuthMethodTerminal, AuthenticateRequest, AuthenticateResponse,
    CancelNotification, Implementation, InitializeRequest, InitializeResponse, LoadSessionRequest,
    LoadSessionResponse, McpCapabilities, McpServer, NewSessionRequest, NewSessionResponse,
    PromptRequest, PromptResponse, RequestPermissionOutcome, RequestPermissionRequest,
    SessionConfigOption, SessionConfigOptionValue, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, SetSessionModeRequest,
    SetSessionModeResponse, ToolCallUpdate, ToolCallUpdateFields,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Error, Responder};
use futures::StreamExt;
use futures::channel::mpsc::UnboundedReceiver;
use serde_json::{Value, json};

use crate::discovery::{self, Version};
use crate::log;
use crate::mapping::{
    mcp_config, permission_options, permission_response, prompt_content, tool_info, tool_meta,
};
use crate::process::{
    self, ClaudeProcess, Event, InitializeReply, Launch, session_file, write_mcp_config,
};
use crate::translate::{
    DEFAULT_VALUE, EFFORT_OPTION, MODEL_OPTION, SessionOptions, Translator, TurnEnd,
    available_commands_update, local_command, replay,
};

/// Environment variable choosing the model (`--model`) when the session does
/// not.
pub const MODEL_ENV: &str = "ELUDITE_CLAUDE_MODEL";

/// Environment variable choosing the effort (`--effort`) when the session does
/// not (brief 0058).
pub const EFFORT_ENV: &str = "ELUDITE_CLAUDE_EFFORT";

/// The levels `claude --effort` takes.
pub const EFFORT_LEVELS: [&str; 5] = ["low", "medium", "high", "xhigh", "max"];

/// The terminal login method's id.
pub const LOGIN_METHOD_ID: &str = "claude-login";

const CHILD_INIT_TIMEOUT: Duration = Duration::from_secs(60);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long a mode, model or effort change may take (`claude` answers at once).
const OPTION_TIMEOUT: Duration = Duration::from_secs(30);

/// Adapter settings from the command line and environment.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// `--claude PATH`.
    pub claude: Option<PathBuf>,
    /// `--model M` (else `$ELUDITE_CLAUDE_MODEL`, else the session's `_meta`).
    pub model: Option<String>,
    /// `--effort LEVEL` (else `$ELUDITE_CLAUDE_EFFORT`, else the session's `_meta`).
    pub effort: Option<String>,
}

struct Session {
    id: String,
    cwd: PathBuf,
    process: Arc<ClaudeProcess>,
    turn: futures::lock::Mutex<(UnboundedReceiver<Event>, Translator)>,
    in_turn: AtomicBool,
    cancelled: AtomicBool,
    mcp_config: PathBuf,
    /// `claude`'s slash commands from its `initialize` reply (brief 0057), as it lists them.
    commands: Vec<Value>,
    /// The modes and config options (brief 0058).
    options: Mutex<SessionOptions>,
}

impl Session {
    /// The `available_commands_update` sent right after `session/new` answers.
    fn commands_update(&self) -> SessionNotification {
        SessionNotification::new(self.id.clone(), available_commands_update(&self.commands))
    }

    fn options(&self) -> std::sync::MutexGuard<'_, SessionOptions> {
        self.options.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn notification(&self, update: SessionUpdate) -> SessionNotification {
        SessionNotification::new(self.id.clone(), update)
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        self.process.kill();
        let _ = std::fs::remove_file(&self.mcp_config);
    }
}

#[derive(Default)]
struct State {
    config: Config,
    auth_terminal: AtomicBool,
    claude: OnceLock<Result<(PathBuf, Version), String>>,
    sessions: Mutex<HashMap<String, Arc<Session>>>,
}

impl State {
    fn session(&self, id: &str) -> Result<Arc<Session>, Error> {
        self.sessions
            .lock()
            .ok()
            .and_then(|s| s.get(id).cloned())
            .ok_or_else(|| Error::invalid_params().data(json!(format!("unknown session {id}"))))
    }

    fn claude(&self) -> Result<(PathBuf, Version), String> {
        self.claude
            .get_or_init(|| {
                let (path, source) = discovery::discover_from_env(self.config.claude.as_deref())?;
                let version = discovery::version_of(&path)?;
                log::info(format_args!(
                    "claude {version} at {} ({source:?})",
                    path.display()
                ));
                if version > discovery::MIN_CLAUDE_VERSION {
                    log::info(format_args!(
                        "claude {version} is newer than the validated {}",
                        discovery::MIN_CLAUDE_VERSION
                    ));
                }
                discovery::check_version(version)?;
                Ok((path, version))
            })
            .clone()
    }
}

fn failure(message: impl Into<String>) -> Error {
    Error::new(-32603, message.into())
}

/// Serve ACP on stdin/stdout until the client disconnects.
pub fn run_stdio(config: Config) -> Result<(), Error> {
    futures::executor::block_on(serve(config, agent_client_protocol::Stdio::new()))
}

/// Serve ACP over `transport`.
pub async fn serve(config: Config, transport: impl ConnectTo<Agent>) -> Result<(), Error> {
    let state = Arc::new(State {
        config,
        ..State::default()
    });
    let (s_init, s_auth, s_new, s_load, s_prompt, s_cancel, s_mode, s_option) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    let result = Agent
        .builder()
        .name("eludite-claude-acp")
        .on_receive_request(
            async move |req: InitializeRequest,
                        responder: Responder<InitializeResponse>,
                        _cx: ConnectionTo<Client>| {
                responder.respond(initialize(&s_init, req))
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |_req: AuthenticateRequest,
                        responder: Responder<AuthenticateResponse>,
                        _cx: ConnectionTo<Client>| {
                // Login happens in a terminal (`claude auth login`); the next
                // session picks it up. Nothing to do here.
                let _ = &s_auth;
                responder.respond(AuthenticateResponse::new())
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: NewSessionRequest,
                        responder: Responder<NewSessionResponse>,
                        cx: ConnectionTo<Client>| {
                let state = s_new.clone();
                let task_cx = cx.clone();
                cx.spawn(async move {
                    match new_session(&state, req).await {
                        Ok((r, session)) => {
                            responder.respond(r)?;
                            // ACP clients expect the slash commands before the first prompt (brief 0057): one
                            // `available_commands_update` right after the answer, from this task, so it follows it.
                            task_cx.send_notification(session.commands_update())
                        }
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: LoadSessionRequest,
                        responder: Responder<LoadSessionResponse>,
                        cx: ConnectionTo<Client>| {
                let state = s_load.clone();
                let task_cx = cx.clone();
                cx.spawn(async move {
                    match load_session(&state, req).await {
                        Ok((r, session, updates)) => {
                            // ACP: the conversation is replayed before the answer (brief 0061), then the slash
                            // commands follow it as after `session/new`.
                            for u in updates {
                                task_cx.send_notification(session.notification(u))?;
                            }
                            responder.respond(r)?;
                            task_cx.send_notification(session.commands_update())
                        }
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: PromptRequest,
                        responder: Responder<PromptResponse>,
                        cx: ConnectionTo<Client>| {
                let session = match s_prompt.session(&req.session_id.0) {
                    Ok(s) => s,
                    Err(e) => return responder.respond_with_error(e),
                };
                let task_cx = cx.clone();
                cx.spawn(async move {
                    match run_turn(session, req, task_cx).await {
                        Ok(r) => responder.respond(r),
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: SetSessionModeRequest,
                        responder: Responder<SetSessionModeResponse>,
                        cx: ConnectionTo<Client>| {
                let session = match s_mode.session(&req.session_id.0) {
                    Ok(s) => s,
                    Err(e) => return responder.respond_with_error(e),
                };
                let task_cx = cx.clone();
                cx.spawn(async move {
                    match set_mode(&session, &req.mode_id.0).await {
                        Ok(update) => {
                            responder.respond(SetSessionModeResponse::new())?;
                            task_cx.send_notification(session.notification(update))
                        }
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_request(
            async move |req: SetSessionConfigOptionRequest,
                        responder: Responder<SetSessionConfigOptionResponse>,
                        cx: ConnectionTo<Client>| {
                let session = match s_option.session(&req.session_id.0) {
                    Ok(s) => s,
                    Err(e) => return responder.respond_with_error(e),
                };
                let task_cx = cx.clone();
                cx.spawn(async move {
                    match set_option(&session, &req.config_id.0, &req.value).await {
                        Ok(options) => {
                            responder.respond(SetSessionConfigOptionResponse::new(options))?;
                            let update = session.options().update();
                            task_cx.send_notification(session.notification(update))
                        }
                        Err(e) => responder.respond_with_error(e),
                    }
                })
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |n: CancelNotification, _cx: ConnectionTo<Client>| {
                if let Ok(s) = s_cancel.session(&n.session_id.0) {
                    cancel(&s);
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await;
    // The client is gone: stop every child and remove the config files.
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.clear();
    }
    result
}

fn initialize(state: &State, req: InitializeRequest) -> InitializeResponse {
    let auth_terminal = req.client_capabilities.auth.terminal;
    state.auth_terminal.store(auth_terminal, Ordering::Relaxed);
    let version = req.protocol_version.min(ProtocolVersion::V1);
    let caps = AgentCapabilities::new()
        .load_session(true)
        .mcp_capabilities(McpCapabilities::new().http(true).sse(false));
    let mut resp = InitializeResponse::new(version)
        .agent_capabilities(caps)
        .agent_info(
            Implementation::new("eludite-claude-acp", env!("CARGO_PKG_VERSION"))
                .title("Claude Code".to_owned()),
        );
    // Like the Node adapter, list the terminal login only to clients that can
    // show it. Its args are appended to this adapter's own command line:
    // `eludite-claude-acp auth login` runs `claude auth login`.
    if auth_terminal {
        resp = resp.auth_methods(vec![AuthMethod::Terminal(
            AuthMethodTerminal::new(LOGIN_METHOD_ID, "Log in to Claude Code")
                .description(
                    "Sign in with your Claude subscription or Anthropic Console account \
                     (runs `claude auth login`)."
                        .to_owned(),
                )
                .args(vec!["auth".into(), "login".into()]),
        )]);
    }
    resp
}

/// `_meta.claudeCode.options.<key>` of `session/new` (the Node adapter's keys).
fn meta_option(meta: Option<&serde_json::Map<String, Value>>, key: &str) -> Option<String> {
    meta?
        .get("claudeCode")?
        .get("options")?
        .get(key)?
        .as_str()
        .filter(|v| !v.is_empty() && *v != DEFAULT_VALUE)
        .map(str::to_owned)
}

/// The model for a new session: `--model`, then `$ELUDITE_CLAUDE_MODEL`, then
/// the session's `_meta.claudeCode.options.model` (the Node adapter's key).
fn session_model(config: &Config, meta: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    config
        .model
        .clone()
        .or_else(|| std::env::var(MODEL_ENV).ok().filter(|m| !m.is_empty()))
        .or_else(|| meta_option(meta, "model"))
}

/// The effort for a new session: `--effort`, then `$ELUDITE_CLAUDE_EFFORT`,
/// then the session's `_meta.claudeCode.options.effort`; only a level `claude
/// --effort` takes.
fn session_effort(
    config: &Config,
    meta: Option<&serde_json::Map<String, Value>>,
) -> Option<String> {
    config
        .effort
        .clone()
        .or_else(|| std::env::var(EFFORT_ENV).ok().filter(|e| !e.is_empty()))
        .or_else(|| meta_option(meta, "effort"))
        .filter(|e| EFFORT_LEVELS.contains(&e.as_str()))
}

async fn new_session(
    state: &State,
    req: NewSessionRequest,
) -> Result<(NewSessionResponse, Arc<Session>), Error> {
    let (session, options) =
        open_session(state, &req.cwd, &req.mcp_servers, req.meta.as_ref(), None).await?;
    let response = NewSessionResponse::new(session.id.clone())
        .modes(options.modes())
        .config_options(options.config_options());
    Ok((response, session))
}

/// `session/load` (brief 0061): `claude --resume`, then the conversation from Claude Code's session file to replay.
async fn load_session(
    state: &State,
    req: LoadSessionRequest,
) -> Result<(LoadSessionResponse, Arc<Session>, Vec<SessionUpdate>), Error> {
    let id = req.session_id.0.to_string();
    if state.session(&id).is_ok() {
        return Err(invalid(format!("session {id} is already live")));
    }
    let (session, options) =
        open_session(state, &req.cwd, &req.mcp_servers, None, Some(id.clone())).await?;
    // The file Claude Code wrote the session to; read while the client waits for the answer.
    let replayed = session_file(&req.cwd, &id)
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|text| replay(&text, &req.cwd))
        .unwrap_or_default();
    log::info(format_args!(
        "session {id}: resumed, replaying {} updates ({} records skipped)",
        replayed.updates.len(),
        replayed.skipped
    ));
    let response = LoadSessionResponse::new()
        .modes(options.modes())
        .config_options(options.config_options());
    Ok((response, session, replayed.updates))
}

/// A `claude` child for a new session, or (`resume`) for session `resume` with `--resume`, after its `initialize`
/// control request; the session is live once this returns.
async fn open_session(
    state: &State,
    cwd: &std::path::Path,
    mcp_servers: &[McpServer],
    meta: Option<&serde_json::Map<String, Value>>,
    resume: Option<String>,
) -> Result<(Arc<Session>, SessionOptions), Error> {
    let (claude, _version) = state.claude().map_err(failure)?;
    if !cwd.is_absolute() {
        return Err(Error::invalid_params().data(json!("cwd must be an absolute path")));
    }
    let resuming = resume.is_some();
    let id = resume.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
    // A resumed id may have been loaded before by another adapter: its config file name is new each time.
    let file = if resuming {
        format!("eludite-claude-acp-{id}-{}.mcp.json", uuid::Uuid::new_v4())
    } else {
        format!("eludite-claude-acp-{id}.mcp.json")
    };
    let config_path = std::env::temp_dir().join(file);
    write_mcp_config(&config_path, &mcp_config(mcp_servers))
        .map_err(|e| failure(format!("could not write the MCP config: {e}")))?;
    let launch = Launch {
        claude,
        cwd: cwd.to_path_buf(),
        session_id: id.clone(),
        mcp_config: config_path.clone(),
        model: session_model(&state.config, meta),
        effort: session_effort(&state.config, meta),
        resume: resuming,
    };
    let (process, events) = match ClaudeProcess::spawn(&launch) {
        Ok(p) => p,
        Err(e) => {
            let _ = std::fs::remove_file(&config_path);
            return Err(failure(format!(
                "could not start {}: {e}",
                launch.claude.display()
            )));
        }
    };
    let mut session = Session {
        id: id.clone(),
        cwd: cwd.to_path_buf(),
        process: process.clone(),
        turn: futures::lock::Mutex::new((events, Translator::new(cwd.to_path_buf()))),
        in_turn: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        mcp_config: config_path,
        commands: Vec::new(),
        options: Mutex::new(SessionOptions::new(Vec::new(), None, None)),
    };
    // The SDK handshake. Its reply carries the account; only whether one is
    // present is used (for the log), and nothing of it is logged or kept. Its
    // slash commands are kept for the client (brief 0057).
    let answer = process
        .control(json!({"subtype": "initialize"}), CHILD_INIT_TIMEOUT)
        .map_err(|e| failure(format!("could not talk to claude: {e}")))?
        .await;
    let reply = match answer {
        Ok(Ok(reply)) => reply,
        failed => {
            // A refused `--resume` (brief 0061): `claude` says why in a `result`'s `errors`, then exits.
            if resuming && let Some(why) = refusal(&session).await {
                return Err(failure(format!("claude could not resume {id}: {why}")));
            }
            return Err(match failed {
                Ok(Err(e)) => failure(format!("claude initialization failed: {e}")),
                _ => failure("claude exited during initialization"),
            });
        }
    };
    // `claude` has read its MCP config by now (it keeps it in memory; checked
    // on 2.1.287 with an `mcp_status` request after deleting the file), so
    // the file, which may hold tokens, does not outlive this point even if
    // the adapter is killed.
    let _ = std::fs::remove_file(&session.mcp_config);
    let InitializeReply {
        logged_in,
        commands,
        models,
    } = InitializeReply::parse(&reply);
    log::info(format_args!(
        "session {id}: claude pid {:?} ready, logged in: {logged_in}, {} commands, {} models",
        process.id(),
        commands.len(),
        models.len()
    ));
    session.commands = commands;
    let options = SessionOptions::new(models, launch.model.as_deref(), launch.effort.as_deref());
    session.options = Mutex::new(options.clone());
    let session = Arc::new(session);
    if let Ok(mut s) = state.sessions.lock() {
        s.insert(id.clone(), session.clone());
    }
    Ok((session, options))
}

/// Why `claude` refused to start a session: the `errors` of the error `result` it wrote before exiting (2.1.289: "No
/// conversation found with session ID: ..." for a `--resume` of a session with nothing in it).
async fn refusal(session: &Session) -> Option<String> {
    let mut turn = session.turn.lock().await;
    while let Some(event) = turn.0.next().await {
        match event {
            Event::Message(m)
                if m.get("type").and_then(Value::as_str) == Some("result")
                    && m.get("is_error").and_then(Value::as_bool) == Some(true) =>
            {
                let errors: Vec<&str> = m
                    .get("errors")
                    .and_then(Value::as_array)
                    .map(|e| e.iter().filter_map(Value::as_str).collect())
                    .unwrap_or_default();
                return Some(if errors.is_empty() {
                    m.get("subtype")
                        .and_then(Value::as_str)
                        .unwrap_or("error")
                        .to_owned()
                } else {
                    errors.join("; ")
                });
            }
            Event::Exited => return None,
            _ => {}
        }
    }
    None
}

/// `invalid_params` with a message for the client.
fn invalid(message: impl Into<String>) -> Error {
    Error::invalid_params().data(json!(message.into()))
}

/// The session's turn, when none is in progress: a change made while a turn
/// runs is refused (`in_turn`), so nothing is written to `claude` then.
fn idle(
    session: &Session,
) -> Result<futures::lock::MutexGuard<'_, (UnboundedReceiver<Event>, Translator)>, Error> {
    let busy = || {
        Error::invalid_request().data(json!(
            "in_turn: a turn is in progress; change it when the turn ends"
        ))
    };
    if session.in_turn.load(Ordering::Acquire) {
        return Err(busy());
    }
    session.turn.try_lock().ok_or_else(busy)
}

/// Send a control request to `claude` and wait for its answer.
async fn control(session: &Session, request: Value) -> Result<Value, Error> {
    let subtype = request["subtype"].as_str().unwrap_or("?").to_owned();
    session
        .process
        .control(request, OPTION_TIMEOUT)
        .map_err(|e| failure(format!("could not write to claude: {e}")))?
        .await
        .map_err(|_| failure("claude exited"))?
        .map_err(|e| failure(format!("claude refused {subtype}: {e}")))
}

/// `session/set_mode`: `set_permission_mode`, then the `current_mode_update` to send.
async fn set_mode(session: &Session, mode: &str) -> Result<SessionUpdate, Error> {
    let _turn = idle(session)?;
    if !session.options().has_mode(mode) {
        return Err(invalid(format!("unknown mode {mode}")));
    }
    control(session, process::set_permission_mode(mode)).await?;
    let mut o = session.options();
    o.mode = mode.to_owned();
    Ok(o.mode_update())
}

/// `session/set_config_option`: the model by `set_model`, the effort by the
/// local command `/effort`. Returns every option.
async fn set_option(
    session: &Session,
    id: &str,
    value: &SessionConfigOptionValue,
) -> Result<Vec<SessionConfigOption>, Error> {
    let mut turn = idle(session)?;
    let Some(value) = value.as_value_id().map(|v| v.0.to_string()) else {
        return Err(invalid(format!("{id} takes one of its values")));
    };
    match id {
        MODEL_OPTION => {
            if !session.options().has_model(&value) {
                return Err(invalid(format!("unknown value {value} for {id}")));
            }
            control(session, process::set_model(&value)).await?;
            session.options().set_model(&value);
        }
        EFFORT_OPTION => {
            {
                let o = session.options();
                if o.effort_levels().is_empty() {
                    return Err(invalid(format!("unknown option {id}")));
                }
                if value == DEFAULT_VALUE {
                    return Err(invalid(
                        "Claude Code cannot go back to the model's own effort in a session; \
                         start a new session for it",
                    ));
                }
                if !o.can_set_effort(&value) {
                    return Err(invalid(format!("unknown value {value} for {id}")));
                }
            }
            let (events, _) = &mut *turn;
            run_local_command(session, events, EFFORT_OPTION, &value).await?;
            session.options().effort = value;
        }
        other => return Err(invalid(format!("unknown option {other}"))),
    }
    Ok(session.options().config_options())
}

/// Write `/<command> <arg>` and consume what `claude` answers up to the
/// `result` of that local command: none of it is a turn for the client.
async fn run_local_command(
    session: &Session,
    events: &mut UnboundedReceiver<Event>,
    command: &str,
    arg: &str,
) -> Result<(), Error> {
    session
        .process
        .send_user(
            &session.id,
            vec![json!({"type": "text", "text": format!("/{command} {arg}")})],
        )
        .map_err(|e| failure(format!("could not write to claude: {e}")))?;
    let (timeout_tx, mut timeout) = futures::channel::oneshot::channel::<()>();
    let _ = std::thread::Builder::new()
        .name("claude-local-command-timeout".into())
        .spawn(move || {
            std::thread::sleep(OPTION_TIMEOUT);
            let _ = timeout_tx.send(());
        });
    loop {
        let next = match futures::future::select(events.next(), &mut timeout).await {
            futures::future::Either::Left((next, _)) => next,
            futures::future::Either::Right(_) => {
                return Err(failure(format!("claude did not answer /{command}")));
            }
        };
        match next {
            Some(Event::Message(msg)) => {
                if local_command(&msg) == Some(command) {
                    return Ok(());
                }
                if msg.get("type").and_then(Value::as_str) == Some("result") {
                    let why = msg.get("result").and_then(Value::as_str).unwrap_or("");
                    return Err(failure(format!("claude did not run /{command}: {why}")));
                }
            }
            Some(Event::ControlRequest { request_id, .. }) => {
                let _ = session
                    .process
                    .respond_error(&request_id, "not supported by eludite-claude-acp");
            }
            Some(Event::Exited) | None => return Err(failure("claude exited")),
        }
    }
}

fn cancel(session: &Session) {
    if !session.in_turn.load(Ordering::Acquire) {
        return;
    }
    session.cancelled.store(true, Ordering::Release);
    if let Err(e) = session
        .process
        .control(json!({"subtype": "interrupt"}), INTERRUPT_TIMEOUT)
    {
        log::warn(format_args!("could not send interrupt: {e}"));
    }
}

async fn run_turn(
    session: Arc<Session>,
    req: PromptRequest,
    cx: ConnectionTo<Client>,
) -> Result<PromptResponse, Error> {
    let mut guard = session.turn.lock().await;
    let (events, translator) = &mut *guard;
    translator.begin_turn();
    session.cancelled.store(false, Ordering::Release);
    session.in_turn.store(true, Ordering::Release);
    let result = drive_turn(&session, &req, events, translator, &cx).await;
    session.in_turn.store(false, Ordering::Release);
    result
}

async fn drive_turn(
    session: &Arc<Session>,
    req: &PromptRequest,
    events: &mut UnboundedReceiver<Event>,
    translator: &mut Translator,
    cx: &ConnectionTo<Client>,
) -> Result<PromptResponse, Error> {
    let content = prompt_content(&req.prompt);
    if content.is_empty() {
        return Err(Error::invalid_params().data(json!("the prompt has no text")));
    }
    // A local command typed as the prompt (`/model opus`) is matched against it (brief 0058).
    let prompt_text = content
        .iter()
        .filter_map(|c| c.get("text").and_then(Value::as_str))
        .collect::<Vec<_>>()
        .join("\n");
    session
        .process
        .send_user(&session.id, content)
        .map_err(|e| failure(format!("could not write to claude: {e}")))?;
    let mut updates = Vec::new();
    while let Some(event) = events.next().await {
        match event {
            Event::Message(msg) => {
                let cancelled = session.cancelled.load(Ordering::Acquire);
                let end = translator.on_message(&msg, cancelled, &mut updates);
                for u in updates.drain(..) {
                    cx.send_notification(SessionNotification::new(session.id.clone(), u))?;
                }
                // `/model X` or `/effort X` typed as a prompt moves its option; a turn's model corrects the model
                // option (brief 0058).
                if end.is_some() {
                    let changed = match local_command(&msg) {
                        Some(command) => session.options().on_local_command(command, &prompt_text),
                        None => translator
                            .model()
                            .is_some_and(|m| session.options().on_turn_model(m)),
                    };
                    if changed {
                        let update = session.options().update();
                        cx.send_notification(session.notification(update))?;
                    }
                }
                match end {
                    None => {}
                    Some(TurnEnd::Stop(reason)) => return Ok(PromptResponse::new(reason)),
                    Some(TurnEnd::AuthRequired) => {
                        return Err(Error::auth_required().data(json!(
                            "Claude Code is not logged in. Run `claude auth login` in a terminal, then start a new session."
                        )));
                    }
                    Some(TurnEnd::Error(m)) => return Err(failure(m)),
                }
            }
            Event::ControlRequest {
                request_id,
                request,
            } => {
                if request.get("subtype").and_then(Value::as_str) == Some("can_use_tool") {
                    let (session, cx2) = (session.clone(), cx.clone());
                    cx.spawn(async move {
                        ask_permission(&session, &request_id, &request, &cx2).await;
                        Ok(())
                    })?;
                } else {
                    let subtype = request["subtype"].as_str().unwrap_or("?");
                    log::info(format_args!("unsupported control request {subtype}"));
                    let _ = session
                        .process
                        .respond_error(&request_id, "not supported by eludite-claude-acp");
                }
            }
            Event::Exited => break,
        }
    }
    Err(failure("claude exited during the turn"))
}

/// Raise `session/request_permission` for a `can_use_tool` request and answer
/// the child with the matching `control_response`.
async fn ask_permission(
    session: &Session,
    request_id: &str,
    request: &Value,
    cx: &ConnectionTo<Client>,
) {
    let name = request["tool_name"].as_str().unwrap_or("tool");
    let input = request.get("input").cloned().unwrap_or_else(|| json!({}));
    let suggestions = request
        .get("permission_suggestions")
        .cloned()
        .unwrap_or(Value::Null);
    let tool_use_id = request["tool_use_id"].as_str().unwrap_or(request_id);
    let display = request["display_name"].as_str().unwrap_or(name);
    let info = tool_info(name, &input, &session.cwd);
    let fields = ToolCallUpdateFields::new()
        .title(info.title)
        .kind(info.kind)
        .locations((!info.locations.is_empty()).then_some(info.locations))
        .content(if info.content.is_empty() {
            None
        } else {
            Some(info.content)
        })
        .raw_input(input.clone());
    let tool_call = ToolCallUpdate::new(tool_use_id.to_owned(), fields)
        .meta(tool_meta(name, request.get("mcp_server")));
    let ask = RequestPermissionRequest::new(
        session.id.clone(),
        tool_call,
        permission_options(name, display, &suggestions),
    );
    let selected = match cx.send_request(ask).block_task().await {
        Ok(r) => match r.outcome {
            RequestPermissionOutcome::Selected(s) => Some(s.option_id.0.to_string()),
            _ => None,
        },
        Err(e) => {
            log::warn(format_args!("permission request failed: {e:?}"));
            None
        }
    };
    let selected = selected.filter(|_| !session.cancelled.load(Ordering::Acquire));
    let response = permission_response(selected.as_deref(), name, &input, &suggestions);
    if let Err(e) = session.process.respond(request_id, response) {
        log::warn(format_args!("could not answer can_use_tool: {e}"));
    }
}
