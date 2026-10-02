//! The ACP agent: `initialize`, `authenticate`, `session/new`,
//! `session/prompt`, `session/cancel`, and `session/request_permission` raised
//! from the child's `can_use_tool` control requests.
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
    CancelNotification, Implementation, InitializeRequest, InitializeResponse, McpCapabilities,
    NewSessionRequest, NewSessionResponse, PromptRequest, PromptResponse, RequestPermissionOutcome,
    RequestPermissionRequest, SessionNotification, ToolCallUpdate, ToolCallUpdateFields,
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
use crate::process::{ClaudeProcess, Event, Launch, write_mcp_config};
use crate::translate::{Translator, TurnEnd};

/// Environment variable choosing the model (`--model`) when the session does
/// not.
pub const MODEL_ENV: &str = "ELUDITE_CLAUDE_MODEL";

/// The terminal login method's id.
pub const LOGIN_METHOD_ID: &str = "claude-login";

const CHILD_INIT_TIMEOUT: Duration = Duration::from_secs(60);
const INTERRUPT_TIMEOUT: Duration = Duration::from_secs(10);

/// Adapter settings from the command line and environment.
#[derive(Debug, Clone, Default)]
pub struct Config {
    /// `--claude PATH`.
    pub claude: Option<PathBuf>,
    /// `--model M` (else `$ELUDITE_CLAUDE_MODEL`, else the session's `_meta`).
    pub model: Option<String>,
}

struct Session {
    id: String,
    cwd: PathBuf,
    process: Arc<ClaudeProcess>,
    turn: futures::lock::Mutex<(UnboundedReceiver<Event>, Translator)>,
    in_turn: AtomicBool,
    cancelled: AtomicBool,
    mcp_config: PathBuf,
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
    let (s_init, s_auth, s_new, s_prompt, s_cancel) = (
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
                cx.spawn(async move {
                    match new_session(&state, req).await {
                        Ok(r) => responder.respond(r),
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
        .load_session(false)
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

/// The model for a new session: `--model`, then `$ELUDITE_CLAUDE_MODEL`, then
/// the session's `_meta.claudeCode.options.model` (the Node adapter's key).
fn session_model(config: &Config, meta: Option<&serde_json::Map<String, Value>>) -> Option<String> {
    config
        .model
        .clone()
        .or_else(|| std::env::var(MODEL_ENV).ok().filter(|m| !m.is_empty()))
        .or_else(|| {
            meta?
                .get("claudeCode")?
                .pointer("/options/model")?
                .as_str()
                .map(str::to_owned)
        })
}

async fn new_session(state: &State, req: NewSessionRequest) -> Result<NewSessionResponse, Error> {
    let (claude, _version) = state.claude().map_err(failure)?;
    if !req.cwd.is_absolute() {
        return Err(Error::invalid_params().data(json!("cwd must be an absolute path")));
    }
    let id = uuid::Uuid::new_v4().to_string();
    let config_path = std::env::temp_dir().join(format!("eludite-claude-acp-{id}.mcp.json"));
    write_mcp_config(&config_path, &mcp_config(&req.mcp_servers))
        .map_err(|e| failure(format!("could not write the MCP config: {e}")))?;
    let launch = Launch {
        claude,
        cwd: req.cwd.clone(),
        session_id: id.clone(),
        mcp_config: config_path.clone(),
        model: session_model(&state.config, req.meta.as_ref()),
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
    let session = Arc::new(Session {
        id: id.clone(),
        cwd: req.cwd.clone(),
        process: process.clone(),
        turn: futures::lock::Mutex::new((events, Translator::new(req.cwd.clone()))),
        in_turn: AtomicBool::new(false),
        cancelled: AtomicBool::new(false),
        mcp_config: config_path,
    });
    // The SDK handshake. Its reply carries the account; only whether one is
    // present is used (for the log), and nothing of it is logged or kept.
    let reply = process
        .control(json!({"subtype": "initialize"}), CHILD_INIT_TIMEOUT)
        .map_err(|e| failure(format!("could not talk to claude: {e}")))?
        .await
        .map_err(|_| failure("claude exited during initialization"))?
        .map_err(|e| failure(format!("claude initialization failed: {e}")))?;
    // `claude` has read its MCP config by now (it keeps it in memory; checked
    // on 2.1.287 with an `mcp_status` request after deleting the file), so
    // the file, which may hold tokens, does not outlive this point even if
    // the adapter is killed.
    let _ = std::fs::remove_file(&session.mcp_config);
    let logged_in = reply
        .get("account")
        .is_some_and(|a| a.get("tokenSource").and_then(Value::as_str) != Some("none"));
    log::info(format_args!(
        "session {id}: claude pid {:?} ready, logged in: {logged_in}",
        process.id()
    ));
    if let Ok(mut s) = state.sessions.lock() {
        s.insert(id.clone(), session);
    }
    Ok(NewSessionResponse::new(id))
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
