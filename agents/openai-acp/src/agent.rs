//! The ACP agent: `initialize`, `authenticate`, `session/new`, `session/prompt`, `session/cancel` and
//! `session/set_config_option` (the `model` option, brief 0058's shape).
//!
//! `session/new` connects the MCP servers it is given (Eludite's endpoint), lists their tools, reads the guides
//! into the system prompt and lists the server's models, then answers with the `model` config option and sends
//! `available_commands_update` (`/compact`, `/clear`). Each turn runs in a task spawned on the connection, never in
//! the dispatch loop, so `session/cancel` is handled while a turn streams. The agent never raises
//! `session/request_permission`: it has no tools of its own, and the IDE's gate decides every MCP call.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

use agent_client_protocol::schema::ProtocolVersion;
use agent_client_protocol::schema::v1::{
    AgentCapabilities, AuthenticateRequest, AuthenticateResponse, CancelNotification,
    Implementation, InitializeRequest, InitializeResponse, McpCapabilities, NewSessionRequest,
    NewSessionResponse, PromptRequest, PromptResponse, SessionNotification, SessionUpdate,
    SetSessionConfigOptionRequest, SetSessionConfigOptionResponse, StopReason,
};
use agent_client_protocol::{Agent, Client, ConnectTo, ConnectionTo, Error, Responder};
use futures::channel::oneshot;
use serde_json::{Value, json};

use crate::log;
use crate::mcp::{McpClient, ServerSpec, ToolSet, ToolsMode};
use crate::models::{self, ModelInfo};
use crate::prompt;
use crate::provider::{Provider, ProviderConfig, RequestLimits};
use crate::turn::{Cancel, CurrentModel, Engine, Stop};

/// Environment variable carrying the key (set by the IDE per launch from its credential store).
pub const API_KEY_ENV: &str = "ELUDITE_OPENAI_API_KEY";

/// Adapter settings from the command line and environment.
#[derive(Clone, Default)]
pub struct Config {
    /// `--base-url`: the API root ending in the version segment.
    pub base_url: String,
    /// `$ELUDITE_OPENAI_API_KEY`.
    pub api_key: Option<String>,
    /// `--model`: the session's first model.
    pub model: Option<String>,
    /// `--header NAME=VALUE`.
    pub headers: Vec<(String, String)>,
    /// `--tools core|all`.
    pub tools: ToolsMode,
    /// `--catalog`: the person's model list (the server is then not asked).
    pub catalog: Vec<ModelInfo>,
    /// `--max-completion-tokens`, `--chat-template-kwargs`.
    pub limits: RequestLimits,
    /// `--name`: shown as the agent's title.
    pub name: Option<String>,
}

impl std::fmt::Debug for Config {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Config")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| log::REDACTED))
            .field("model", &self.model)
            .field("headers", &self.headers)
            .field("tools", &self.tools)
            .finish_non_exhaustive()
    }
}

impl Config {
    pub fn provider(&self) -> Provider {
        Provider::new(ProviderConfig {
            base_url: self.base_url.clone(),
            api_key: self.api_key.clone(),
            headers: self.headers.clone(),
        })
    }
}

struct Session {
    engine: futures::lock::Mutex<Engine>,
    model: Arc<Mutex<CurrentModel>>,
    models: Vec<ModelInfo>,
    cancel: Cancel,
    in_turn: AtomicBool,
}

struct State {
    config: Config,
    provider: Provider,
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
}

fn failure(message: impl Into<String>) -> Error {
    Error::new(-32603, message.into())
}

/// Run `f` on its own thread and await its result.
async fn off_thread<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> Result<T, Error> {
    let (tx, rx) = oneshot::channel();
    std::thread::Builder::new()
        .name("openai-acp-work".into())
        .spawn(move || {
            let _ = tx.send(f());
        })
        .map_err(|e| failure(e.to_string()))?;
    rx.await.map_err(|_| failure("the worker thread failed"))
}

/// Serve ACP on stdin/stdout until the client disconnects.
pub fn run_stdio(config: Config) -> Result<(), Error> {
    futures::executor::block_on(serve(config, agent_client_protocol::Stdio::new()))
}

/// Serve ACP over `transport`.
pub async fn serve(config: Config, transport: impl ConnectTo<Agent>) -> Result<(), Error> {
    if let Some(k) = &config.api_key {
        log::add_secret(k);
    }
    let state = Arc::new(State {
        provider: config.provider(),
        config,
        sessions: Mutex::default(),
    });
    let (s_init, s_new, s_prompt, s_cancel, s_config) = (
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
        state.clone(),
    );
    let result = Agent
        .builder()
        .name("eludite-openai-acp")
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
                // No login: the key comes from the environment.
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
                        Ok((id, response)) => {
                            responder.respond(response)?;
                            let commands = json!({"sessionUpdate": "available_commands_update", "availableCommands": [
                                {"name": "compact", "description": "Summarize the conversation so far to free context"},
                                {"name": "clear", "description": "Forget the conversation; the next prompt starts fresh"},
                            ]});
                            notify(&task_cx, &id, commands);
                            Ok(())
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
            async move |req: SetSessionConfigOptionRequest,
                        responder: Responder<SetSessionConfigOptionResponse>,
                        cx: ConnectionTo<Client>| {
                match set_config_option(&s_config, &req) {
                    Ok((id, options)) => {
                        let response: SetSessionConfigOptionResponse =
                            serde_json::from_value(json!({"configOptions": options}))
                                .map_err(|e| failure(e.to_string()))?;
                        responder.respond(response)?;
                        notify(
                            &cx,
                            &id,
                            json!({"sessionUpdate": "config_option_update", "configOptions": options}),
                        );
                        Ok(())
                    }
                    Err(e) => responder.respond_with_error(e),
                }
            },
            agent_client_protocol::on_receive_request!(),
        )
        .on_receive_notification(
            async move |n: CancelNotification, _cx: ConnectionTo<Client>| {
                if let Ok(s) = s_cancel.session(&n.session_id.0)
                    && s.in_turn.load(Ordering::Acquire)
                {
                    s.cancel.cancel();
                }
                Ok(())
            },
            agent_client_protocol::on_receive_notification!(),
        )
        .connect_to(transport)
        .await;
    if let Ok(mut sessions) = state.sessions.lock() {
        sessions.clear();
    }
    result
}

/// Send one `session/update` (built as ACP JSON).
fn notify(cx: &ConnectionTo<Client>, session: &str, update: Value) {
    match serde_json::from_value::<SessionUpdate>(update) {
        Ok(u) => {
            if let Err(e) = cx.send_notification(SessionNotification::new(session.to_owned(), u)) {
                log::warn(format_args!("could not send an update: {e:?}"));
            }
        }
        Err(e) => log::warn(format_args!("an update did not match ACP's schema: {e}")),
    }
}

fn initialize(state: &State, req: InitializeRequest) -> InitializeResponse {
    let version = req.protocol_version.min(ProtocolVersion::V1);
    let caps = AgentCapabilities::new()
        .load_session(false)
        .mcp_capabilities(McpCapabilities::new().http(true).sse(false));
    InitializeResponse::new(version)
        .agent_capabilities(caps)
        .agent_info(
            Implementation::new("eludite-openai-acp", env!("CARGO_PKG_VERSION")).title(
                state
                    .config
                    .name
                    .clone()
                    .unwrap_or_else(|| "OpenAI-compatible".to_owned()),
            ),
        )
}

async fn new_session(
    state: &State,
    req: NewSessionRequest,
) -> Result<(String, NewSessionResponse), Error> {
    let cwd: PathBuf = req.cwd.clone();
    if !cwd.is_absolute() {
        return Err(Error::invalid_params().data(json!("cwd must be an absolute path")));
    }
    // The models first: a rejected key or an unreachable server fails the session before anything starts.
    let provider = state.provider.clone();
    let catalog = state.config.catalog.clone();
    let listing = off_thread(move || provider.list_models(&catalog)).await?;
    if listing.key_rejected {
        return Err(Error::auth_required().data(json!(format!(
            "{}: {}",
            state.provider.base_url(),
            listing.message.as_deref().unwrap_or("the key was rejected")
        ))));
    }
    let models = models::session_models(&listing.models, state.config.model.as_deref());
    let Some(first) = models.first() else {
        return Err(failure(format!(
            "{}: {}; set a default model for this server",
            state.provider.base_url(),
            listing.message.as_deref().unwrap_or("no models")
        )));
    };
    let current = state
        .config
        .model
        .clone()
        .unwrap_or_else(|| first.id.clone());
    let window = models
        .iter()
        .find(|m| m.id == current)
        .map(|m| m.context_window)
        .unwrap_or(0);

    let mut clients = Vec::new();
    let mut names = Vec::new();
    let mut lists = Vec::new();
    for server in &req.mcp_servers {
        let Some(spec) = serde_json::to_value(server)
            .ok()
            .as_ref()
            .and_then(ServerSpec::from_acp)
        else {
            log::warn(format_args!(
                "skipped an MCP server this adapter cannot reach (sse)"
            ));
            continue;
        };
        let client = match McpClient::connect(&spec, &cwd) {
            Ok(c) => c,
            Err(e) => {
                log::warn(format_args!("{e}"));
                continue;
            }
        };
        if let Err(e) = client.initialize().await {
            log::warn(format_args!("MCP server {}: {e}", spec.name()));
            continue;
        }
        let tools = client.list_tools().await.unwrap_or_else(|e| {
            log::warn(format_args!("tools/list on {}: {e}", spec.name()));
            Vec::new()
        });
        names.push(spec.name().to_owned());
        lists.push(tools);
        clients.push(client);
    }
    let mut guides = Vec::new();
    for (uri, heading) in prompt::GUIDES {
        for c in &clients {
            if let Ok(text) = c.read_resource(uri).await {
                guides.push(((*uri).to_owned(), (*heading).to_owned(), text));
                break;
            }
        }
    }
    let mut tools = ToolSet::new(state.config.tools);
    tools.set(&names, lists);
    let system = prompt::system_prompt(&cwd, None, None, !tools.is_empty(), &guides);
    log::info(format_args!(
        "session: model {current} (window {window}), {} tools ({} sent), system prompt ~{} tokens",
        tools.len(),
        tools.function_tools().len(),
        prompt::estimate_tokens(&system)
    ));
    let model = Arc::new(Mutex::new(CurrentModel {
        id: current.clone(),
        window,
    }));
    let engine = Engine::new(
        state.provider.clone(),
        state.config.limits.clone(),
        &cwd,
        system,
        model.clone(),
        names.into_iter().zip(clients).collect(),
        tools,
    );
    let id = uuid::Uuid::new_v4().to_string();
    let option = models::model_config_option(&models, &current);
    let session = Arc::new(Session {
        engine: futures::lock::Mutex::new(engine),
        model,
        models,
        cancel: Cancel::default(),
        in_turn: AtomicBool::new(false),
    });
    if let Ok(mut s) = state.sessions.lock() {
        s.insert(id.clone(), session);
    }
    let response: NewSessionResponse =
        serde_json::from_value(json!({"sessionId": id, "configOptions": [option]}))
            .map_err(|e| failure(e.to_string()))?;
    Ok((id, response))
}

fn set_config_option(
    state: &State,
    req: &SetSessionConfigOptionRequest,
) -> Result<(String, Vec<Value>), Error> {
    let v = serde_json::to_value(req).map_err(|e| failure(e.to_string()))?;
    let id = v["sessionId"].as_str().unwrap_or_default().to_owned();
    let session = state.session(&id)?;
    if v["configId"] != "model" {
        return Err(
            Error::invalid_params().data(json!(format!("unknown config option {}", v["configId"])))
        );
    }
    let Some(value) = v["value"].as_str() else {
        return Err(Error::invalid_params().data(json!("the model is a string")));
    };
    let Some(m) = session.models.iter().find(|m| m.id == value) else {
        return Err(Error::invalid_params()
            .data(json!(format!("{value} is not one of the server's models"))));
    };
    if let Ok(mut cur) = session.model.lock() {
        *cur = CurrentModel {
            id: m.id.clone(),
            window: m.context_window,
        };
    }
    log::info(format_args!("model changed to {value}"));
    Ok((
        id,
        vec![models::model_config_option(&session.models, value)],
    ))
}

/// The prompt's text: text blocks, resource links as `[name](uri)`, embedded resources' text.
fn prompt_text(req: &PromptRequest) -> String {
    let mut parts = Vec::new();
    for block in &req.prompt {
        let Ok(b) = serde_json::to_value(block) else {
            continue;
        };
        match b["type"].as_str() {
            Some("text") => parts.push(b["text"].as_str().unwrap_or("").to_owned()),
            Some("resource_link") => parts.push(format!(
                "[{}]({})",
                b["name"].as_str().unwrap_or(""),
                b["uri"].as_str().unwrap_or("")
            )),
            Some("resource") => {
                if let Some(t) = b.pointer("/resource/text").and_then(Value::as_str) {
                    parts.push(format!(
                        "{}:\n{t}",
                        b.pointer("/resource/uri")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                    ));
                }
            }
            _ => {}
        }
    }
    parts.join("\n")
}

async fn run_turn(
    session: Arc<Session>,
    req: PromptRequest,
    cx: ConnectionTo<Client>,
) -> Result<PromptResponse, Error> {
    let text = prompt_text(&req);
    if text.trim().is_empty() {
        return Err(Error::invalid_params().data(json!("the prompt has no text")));
    }
    let id = req.session_id.0.to_string();
    let mut engine = session.engine.lock().await;
    session.cancel.reset();
    session.in_turn.store(true, Ordering::Release);
    let mut sink = |update: Value| notify(&cx, &id, update);
    let result = engine.turn(&text, &session.cancel, &mut sink).await;
    session.in_turn.store(false, Ordering::Release);
    let stop = result.map_err(failure)?;
    Ok(PromptResponse::new(match stop {
        Stop::EndTurn => StopReason::EndTurn,
        Stop::MaxTokens => StopReason::MaxTokens,
        Stop::MaxTurnRequests => StopReason::MaxTurnRequests,
        Stop::Refusal => StopReason::Refusal,
        Stop::Cancelled => StopReason::Cancelled,
    }))
}
