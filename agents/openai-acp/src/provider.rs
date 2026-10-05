//! The HTTP client for an OpenAI-compatible server: `GET {base}/models` and a streamed `POST {base}/chat/completions`,
//! on `ureq` 3 with `rustls`.
//!
//! [`Provider::chat`] blocks: the turn runs it on its own thread (the only thread a request gets) and receives
//! [`StreamEvent`]s over a channel as the server streams. The wire rules live in pure, tested pieces: [`SseParser`]
//! (`data:` lines, events split by blank lines, `[DONE]`) and [`Accumulator`] (content and reasoning deltas,
//! tool-call fragments keyed by `index`, usage on any chunk or alone with empty `choices`, the last `finish_reason`).
//! A server that ignores `stream: true` and answers `application/json` is read as one body. A stream that ends
//! without `[DONE]` is complete when it carried a `finish_reason`, else it is [`ChatError::Dropped`].
//!
//! The key is sent as `Authorization: Bearer KEY` only when there is one (no bare `Bearer`), never logged, and the
//! extra headers can never set `Authorization`.

use std::io::{BufRead, BufReader, Read};
use std::time::Duration;

use futures::channel::mpsc::UnboundedSender;
use serde_json::{Value, json};

use crate::log;
use crate::models::{self, Listing, ModelInfo};

/// `GET /models` gives up after this long.
pub const LIST_TIMEOUT: Duration = Duration::from_secs(30);
/// Connecting to the server gives up after this long.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// How long the server may think before the response's headers arrive (a local model reading a long prompt).
pub const RESPONSE_TIMEOUT: Duration = Duration::from_secs(600);
/// The most of an error body read.
const ERROR_BODY_LIMIT: u64 = 1024 * 1024;

/// Where and how to reach the server.
#[derive(Clone, Default)]
pub struct ProviderConfig {
    /// The API root ending in the version segment (`http://localhost:8080/v1`), one trailing slash stripped.
    pub base_url: String,
    /// `None` or empty: no `Authorization` header at all.
    pub api_key: Option<String>,
    /// Extra request headers (OpenRouter's `HTTP-Referer`, `X-Title`); `Authorization` is dropped.
    pub headers: Vec<(String, String)>,
}

impl std::fmt::Debug for ProviderConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ProviderConfig")
            .field("base_url", &self.base_url)
            .field("api_key", &self.api_key.as_ref().map(|_| log::REDACTED))
            .field("headers", &self.headers)
            .finish()
    }
}

/// `base` with one trailing slash stripped.
pub fn normalize_base(base: &str) -> String {
    base.strip_suffix('/').unwrap_or(base).to_owned()
}

/// Whether `url`'s host is a loopback address (never proxied).
pub fn is_loopback(url: &str) -> bool {
    let rest = url.split("://").nth(1).unwrap_or(url);
    let host = rest.split(['/', '?']).next().unwrap_or("");
    let host = host.rsplit_once('@').map(|(_, h)| h).unwrap_or(host);
    let host = if host.starts_with('[') {
        host.split(']').next().unwrap_or("").trim_start_matches('[')
    } else {
        host.split(':').next().unwrap_or("")
    };
    host == "localhost" || host == "::1" || host.starts_with("127.")
}

/// Token counts of one response.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    /// `prompt_tokens_details.cached_tokens`, when reported.
    pub cached_tokens: Option<u64>,
    /// `completion_tokens_details.reasoning_tokens`, when reported.
    pub reasoning_tokens: Option<u64>,
}

impl Usage {
    fn from_json(v: &Value) -> Option<Self> {
        if !v.is_object() {
            return None;
        }
        let n = |p: &str| v.pointer(p).and_then(Value::as_u64);
        Some(Self {
            prompt_tokens: n("/prompt_tokens").unwrap_or(0),
            completion_tokens: n("/completion_tokens").unwrap_or(0),
            cached_tokens: n("/prompt_tokens_details/cached_tokens"),
            reasoning_tokens: n("/completion_tokens_details/reasoning_tokens"),
        })
    }
}

/// One tool call of a response, its fragments joined.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ToolCallParts {
    pub index: usize,
    pub id: String,
    pub name: String,
    /// The `function.arguments` fragments concatenated (JSON text, maybe malformed).
    pub arguments: String,
}

/// A whole response.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Completion {
    pub content: String,
    pub reasoning: String,
    pub tool_calls: Vec<ToolCallParts>,
    pub usage: Option<Usage>,
    pub finish_reason: Option<String>,
}

/// Why a request failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChatError {
    /// A non-2xx status, with the server's `error.message` when the body had one.
    Status {
        status: u16,
        message: Option<String>,
        /// The server said the prompt is longer than the model's context.
        context_length: bool,
    },
    /// The stream ended with neither `[DONE]` nor a `finish_reason`.
    Dropped,
    /// The server could not be reached, or the connection failed before a response.
    Network(String),
    /// The stream carried an `error` object.
    Server(String),
}

impl std::fmt::Display for ChatError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ChatError::Status {
                status,
                message,
                context_length,
            } => {
                if *context_length {
                    write!(
                        f,
                        "the conversation is longer than the model's context window"
                    )?;
                } else {
                    write!(f, "the server answered {status}")?;
                }
                if let Some(m) = message {
                    write!(f, ": {m}")?;
                }
                Ok(())
            }
            ChatError::Dropped => {
                f.write_str("The server closed the stream before the answer was complete")
            }
            ChatError::Network(e) => write!(f, "could not reach the server: {e}"),
            ChatError::Server(e) => write!(f, "the server reported an error: {e}"),
        }
    }
}

/// What the request thread sends while a response streams.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEvent {
    /// The server's first byte of the response body arrived (for the latency budget).
    FirstByte,
    Text(String),
    Thought(String),
    /// A tool call's name is known.
    ToolCallStarted {
        index: usize,
        id: String,
        name: String,
    },
    /// The response is complete, or failed.
    Done(Result<Completion, ChatError>),
}

/// Whether an error status and message mean the prompt did not fit the model's context.
pub fn is_context_length_error(body: &str, message: Option<&str>) -> bool {
    let v: Value = serde_json::from_str(body).unwrap_or(Value::Null);
    let e = v.get("error").unwrap_or(&v);
    let field = |k: &str| {
        e.get(k)
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_lowercase()
    };
    if field("type") == "exceed_context_size_error" || field("code") == "context_length_exceeded" {
        return true;
    }
    let m = message.unwrap_or("").to_lowercase();
    [
        "context length",
        "context_length",
        "maximum context",
        "context window",
        "context size",
        "too many tokens",
        "prompt is too long",
        "exceeds the available context",
    ]
    .iter()
    .any(|p| m.contains(p))
}

/// Server-sent events: `data:` lines joined until a blank line; `[DONE]`; comments and other fields ignored.
#[derive(Debug, Default)]
pub struct SseParser {
    data: Vec<String>,
}

/// One server-sent event's payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SseItem {
    Data(String),
    Done,
}

impl SseParser {
    /// Feed one line (without its line break); returns an event when a blank line ends one.
    pub fn line(&mut self, line: &str) -> Option<SseItem> {
        let line = line.strip_suffix('\r').unwrap_or(line);
        if line.is_empty() {
            return self.flush();
        }
        if let Some(rest) = line.strip_prefix("data:") {
            let rest = rest.strip_prefix(' ').unwrap_or(rest);
            // `[DONE]` alone ends the stream at once.
            if self.data.is_empty() && rest.trim() == "[DONE]" {
                return Some(SseItem::Done);
            }
            self.data.push(rest.to_owned());
        }
        None
    }

    /// The pending event at the end of the stream.
    pub fn flush(&mut self) -> Option<SseItem> {
        if self.data.is_empty() {
            return None;
        }
        let data = std::mem::take(&mut self.data).join("\n");
        Some(if data.trim() == "[DONE]" {
            SseItem::Done
        } else {
            SseItem::Data(data)
        })
    }
}

/// Builds a [`Completion`] from stream chunks (or one body), emitting the deltas as [`StreamEvent`]s.
#[derive(Debug, Default)]
pub struct Accumulator {
    completion: Completion,
    /// Index of the call each fragment without an `index` continues.
    last_index: Option<usize>,
}

impl Accumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether a `finish_reason` arrived.
    pub fn finished(&self) -> bool {
        self.completion.finish_reason.is_some()
    }

    /// One `chat.completion.chunk`.
    pub fn chunk(&mut self, chunk: &Value, out: &mut Vec<StreamEvent>) {
        if let Some(u) = chunk.get("usage").and_then(Usage::from_json) {
            self.completion.usage = Some(u);
        }
        let Some(choices) = chunk.get("choices").and_then(Value::as_array) else {
            return;
        };
        for choice in choices {
            if choice.get("index").and_then(Value::as_u64).unwrap_or(0) != 0 {
                continue;
            }
            if let Some(delta) = choice.get("delta").or_else(|| choice.get("message")) {
                self.delta(delta, out);
            }
            if let Some(r) = choice.get("finish_reason").and_then(Value::as_str) {
                self.completion.finish_reason = Some(r.to_owned());
            }
        }
    }

    fn delta(&mut self, delta: &Value, out: &mut Vec<StreamEvent>) {
        if let Some(t) = delta.get("content").and_then(Value::as_str)
            && !t.is_empty()
        {
            self.completion.content.push_str(t);
            out.push(StreamEvent::Text(t.to_owned()));
        }
        let thought = delta
            .get("reasoning_content")
            .and_then(Value::as_str)
            .or_else(|| delta.get("reasoning").and_then(Value::as_str));
        if let Some(t) = thought
            && !t.is_empty()
        {
            self.completion.reasoning.push_str(t);
            out.push(StreamEvent::Thought(t.to_owned()));
        }
        let Some(calls) = delta.get("tool_calls").and_then(Value::as_array) else {
            return;
        };
        for (position, frag) in calls.iter().enumerate() {
            let id = frag.get("id").and_then(Value::as_str).unwrap_or("");
            let index = match frag.get("index").and_then(Value::as_u64) {
                Some(i) => i as usize,
                // No index: a new id starts a call, anything else continues the last one.
                None => match (self.last_index, id.is_empty()) {
                    (Some(last), true) => last,
                    (Some(last), false)
                        if self
                            .completion
                            .tool_calls
                            .iter()
                            .any(|c| c.index == last && (c.id.is_empty() || c.id == id)) =>
                    {
                        last
                    }
                    _ => self
                        .completion
                        .tool_calls
                        .iter()
                        .map(|c| c.index + 1)
                        .max()
                        .unwrap_or(0)
                        .max(position),
                },
            };
            self.last_index = Some(index);
            let pos = match self
                .completion
                .tool_calls
                .iter()
                .position(|c| c.index == index)
            {
                Some(p) => p,
                None => {
                    self.completion.tool_calls.push(ToolCallParts {
                        index,
                        ..ToolCallParts::default()
                    });
                    self.completion.tool_calls.len() - 1
                }
            };
            let call = &mut self.completion.tool_calls[pos];
            let had_name = !call.name.is_empty();
            if call.id.is_empty() && !id.is_empty() {
                call.id = id.to_owned();
            }
            if let Some(f) = frag.get("function") {
                if let Some(n) = f.get("name").and_then(Value::as_str)
                    && call.name.is_empty()
                {
                    call.name = n.to_owned();
                }
                match f.get("arguments") {
                    Some(Value::String(a)) => call.arguments.push_str(a),
                    // Some servers send the arguments as an object.
                    Some(v @ Value::Object(_)) => call.arguments.push_str(&v.to_string()),
                    _ => {}
                }
            }
            if !had_name && !call.name.is_empty() {
                out.push(StreamEvent::ToolCallStarted {
                    index,
                    id: call.id.clone(),
                    name: call.name.clone(),
                });
            }
        }
    }

    /// The whole response, tool calls in index order.
    pub fn finish(mut self) -> Completion {
        self.completion.tool_calls.sort_by_key(|c| c.index);
        self.completion
    }
}

/// A `chat.completion` body (a server that ignored `stream: true`), with its deltas as events.
pub fn completion_from_body(body: &Value) -> (Vec<StreamEvent>, Completion) {
    let mut acc = Accumulator::new();
    let mut events = Vec::new();
    acc.chunk(body, &mut events);
    (events, acc.finish())
}

/// The client for one server.
#[derive(Clone)]
pub struct Provider {
    config: ProviderConfig,
    list: ureq::Agent,
    chat: ureq::Agent,
}

impl std::fmt::Debug for Provider {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Provider")
            .field("config", &self.config)
            .finish()
    }
}

/// `SSL_CERT_FILE`'s certificates as the roots, when it names a readable PEM bundle (a proxy that re-signs TLS).
fn env_roots() -> Option<ureq::tls::RootCerts> {
    let path = std::env::var_os("SSL_CERT_FILE")?;
    let pem = std::fs::read(path).ok()?;
    let certs: Vec<ureq::tls::Certificate<'static>> = ureq::tls::parse_pem(&pem)
        .filter_map(|item| match item {
            Ok(ureq::tls::PemItem::Certificate(c)) => Some(c),
            _ => None,
        })
        .collect();
    (!certs.is_empty()).then(|| ureq::tls::RootCerts::new_with_certs(&certs))
}

fn agent(global: Option<Duration>, proxied: bool) -> ureq::Agent {
    let mut tls = ureq::tls::TlsConfig::builder();
    if let Some(roots) = env_roots() {
        tls = tls.root_certs(roots);
    }
    ureq::Agent::config_builder()
        .http_status_as_error(false)
        .timeout_global(global)
        .timeout_connect(Some(CONNECT_TIMEOUT))
        .timeout_recv_response(Some(RESPONSE_TIMEOUT))
        .max_redirects(5)
        .proxy(if proxied {
            ureq::Proxy::try_from_env()
        } else {
            None
        })
        .tls_config(tls.build())
        .user_agent(concat!("eludite-openai-acp/", env!("CARGO_PKG_VERSION")))
        .build()
        .into()
}

impl Provider {
    pub fn new(mut config: ProviderConfig) -> Self {
        config.base_url = normalize_base(&config.base_url);
        config.api_key = config.api_key.filter(|k| !k.is_empty());
        config
            .headers
            .retain(|(k, _)| !k.eq_ignore_ascii_case("authorization"));
        if let Some(k) = &config.api_key {
            log::add_secret(k);
        }
        let proxied = !is_loopback(&config.base_url);
        Self {
            list: agent(Some(LIST_TIMEOUT), proxied),
            chat: agent(None, proxied),
            config,
        }
    }

    pub fn base_url(&self) -> &str {
        &self.config.base_url
    }

    fn headers(&self) -> Vec<(String, String)> {
        let mut h = self.config.headers.clone();
        if let Some(k) = &self.config.api_key {
            h.push(("Authorization".into(), format!("Bearer {k}")));
        }
        h
    }

    /// The header names and values for the log: everything but `Authorization`.
    fn loggable_headers(&self) -> String {
        self.config
            .headers
            .iter()
            .map(|(k, v)| format!("{k}: {v}"))
            .collect::<Vec<_>>()
            .join(", ")
    }

    /// `GET {base}/models`, or the catalog without asking when there is one.
    pub fn list_models(&self, catalog: &[ModelInfo]) -> Listing {
        if !catalog.is_empty() {
            return Listing::catalog(catalog);
        }
        let url = format!("{}/models", self.config.base_url);
        log::info(format_args!(
            "GET {url} (headers: {})",
            self.loggable_headers()
        ));
        let mut req = self.list.get(&url).header("Accept", "application/json");
        for (k, v) in self.headers() {
            req = req.header(k.as_str(), v.as_str());
        }
        match req.call() {
            Ok(resp) => {
                let status = resp.status().as_u16();
                let body = resp
                    .into_body()
                    .into_with_config()
                    .limit(16 * 1024 * 1024)
                    .read_to_string()
                    .unwrap_or_default();
                log::info(format_args!("GET {url}: {status}, {} bytes", body.len()));
                models::from_response(status, &body, catalog, None)
            }
            Err(e) => models::from_response(0, "", catalog, Some(&e.to_string())),
        }
    }

    /// `POST {base}/chat/completions` with `body`, streaming [`StreamEvent`]s to `tx` and ending with
    /// [`StreamEvent::Done`]. Returns early, dropping the connection, when `tx`'s receiver is gone (a cancel).
    pub fn chat(&self, body: &Value, tx: &UnboundedSender<StreamEvent>) {
        let url = format!("{}/chat/completions", self.config.base_url);
        let text = body.to_string();
        if log::verbose() {
            log::info(format_args!(
                "POST {url} (headers: {}) {text}",
                self.loggable_headers()
            ));
        }
        let result = self.chat_inner(&url, &text, tx);
        if let Some(result) = result {
            if let Err(e) = &result {
                log::info(format_args!("POST {url}: {e}"));
            }
            let _ = tx.unbounded_send(StreamEvent::Done(result));
        }
    }

    /// `None`: the receiver is gone.
    fn chat_inner(
        &self,
        url: &str,
        text: &str,
        tx: &UnboundedSender<StreamEvent>,
    ) -> Option<Result<Completion, ChatError>> {
        let mut req = self
            .chat
            .post(url)
            .header("Content-Type", "application/json")
            .header("Accept", "text/event-stream, application/json");
        for (k, v) in self.headers() {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = match req.send(text.as_bytes()) {
            Ok(r) => r,
            Err(e) => return Some(Err(ChatError::Network(e.to_string()))),
        };
        let status = resp.status().as_u16();
        let content_type = resp
            .headers()
            .get("content-type")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_ascii_lowercase();
        let reader = resp.into_body().into_with_config().limit(u64::MAX).reader();
        if !(200..300).contains(&status) {
            let mut body = String::new();
            let _ = reader.take(ERROR_BODY_LIMIT).read_to_string(&mut body);
            let message = models::error_message(&body).or_else(|| {
                let t = body.trim();
                (!t.is_empty() && !t.starts_with('<')).then(|| t.chars().take(300).collect())
            });
            let context_length = is_context_length_error(&body, message.as_deref());
            return Some(Err(ChatError::Status {
                status,
                message,
                context_length,
            }));
        }
        if content_type.starts_with("application/json") {
            let mut body = String::new();
            let mut reader = reader;
            if let Err(e) = reader.read_to_string(&mut body) {
                return Some(Err(ChatError::Network(e.to_string())));
            }
            tx.unbounded_send(StreamEvent::FirstByte).ok()?;
            let v: Value = match serde_json::from_str(&body) {
                Ok(v) => v,
                Err(e) => {
                    return Some(Err(ChatError::Server(format!(
                        "the answer is not JSON: {e}"
                    ))));
                }
            };
            if let Some(e) = v.get("error") {
                return Some(Err(ChatError::Server(error_text(e))));
            }
            let (events, completion) = completion_from_body(&v);
            for e in events {
                tx.unbounded_send(e).ok()?;
            }
            return Some(Ok(completion));
        }
        self.read_stream(reader, tx)
    }

    fn read_stream(
        &self,
        reader: impl Read,
        tx: &UnboundedSender<StreamEvent>,
    ) -> Option<Result<Completion, ChatError>> {
        let mut reader = BufReader::with_capacity(16 * 1024, reader);
        let mut sse = SseParser::default();
        let mut acc = Accumulator::new();
        let mut events = Vec::new();
        let mut line = String::new();
        let mut first = true;
        let mut done = false;
        loop {
            line.clear();
            let item = match reader.read_line(&mut line) {
                Ok(0) => match sse.flush() {
                    Some(item) => Some(item),
                    None => break,
                },
                Ok(_) => {
                    // A cancel drops the receiver: stop reading (and drop the connection) even while the server
                    // sends only keep-alive comments.
                    if tx.is_closed() {
                        return None;
                    }
                    if first {
                        first = false;
                        tx.unbounded_send(StreamEvent::FirstByte).ok()?;
                    }
                    sse.line(line.trim_end_matches('\n'))
                }
                Err(e) => {
                    log::info(format_args!("the stream failed: {e}"));
                    break;
                }
            };
            match item {
                None => {}
                Some(SseItem::Done) => {
                    done = true;
                    break;
                }
                Some(SseItem::Data(data)) => {
                    let chunk: Value = match serde_json::from_str(&data) {
                        Ok(v) => v,
                        Err(_) => {
                            log::info(format_args!("skipped an event that is not JSON"));
                            continue;
                        }
                    };
                    if let Some(e) = chunk.get("error") {
                        return Some(Err(ChatError::Server(error_text(e))));
                    }
                    acc.chunk(&chunk, &mut events);
                    for e in events.drain(..) {
                        tx.unbounded_send(e).ok()?;
                    }
                }
            }
        }
        if done || acc.finished() {
            Some(Ok(acc.finish()))
        } else {
            Some(Err(ChatError::Dropped))
        }
    }
}

fn error_text(e: &Value) -> String {
    e.get("message")
        .and_then(Value::as_str)
        .or_else(|| e.as_str())
        .map(str::to_owned)
        .unwrap_or_else(|| e.to_string())
}

/// The request body: `model`, `messages`, `tools` (when any), streaming with usage, the token limit, and
/// `chat_template_kwargs` when configured. No `temperature`, no `tool_choice`.
pub fn request_body(
    model: &str,
    messages: &[Value],
    tools: &[Value],
    limits: &RequestLimits,
) -> Value {
    let mut body = json!({
        "model": model,
        "messages": messages,
        "stream": true,
        "stream_options": {"include_usage": true},
    });
    if !tools.is_empty() {
        body["tools"] = Value::Array(tools.to_vec());
    }
    match limits.max_completion_tokens {
        Some(n) => body["max_completion_tokens"] = json!(n),
        None => body["max_tokens"] = json!(limits.max_tokens),
    }
    if let Some(k) = &limits.chat_template_kwargs {
        body["chat_template_kwargs"] = k.clone();
    }
    body
}

/// What the request carries besides the conversation.
#[derive(Debug, Clone, PartialEq)]
pub struct RequestLimits {
    /// `max_tokens` (8,192 by default).
    pub max_tokens: u64,
    /// `--max-completion-tokens`: sent as `max_completion_tokens` instead (OpenAI's reasoning models).
    pub max_completion_tokens: Option<u64>,
    /// `--chat-template-kwargs JSON`: llama.cpp's `chat_template_kwargs`, verbatim.
    pub chat_template_kwargs: Option<Value>,
}

impl Default for RequestLimits {
    fn default() -> Self {
        Self {
            max_tokens: 8_192,
            max_completion_tokens: None,
            chat_template_kwargs: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(chunks: &[Value]) -> (Vec<StreamEvent>, Completion) {
        let mut acc = Accumulator::new();
        let mut ev = Vec::new();
        for c in chunks {
            acc.chunk(c, &mut ev);
        }
        (ev, acc.finish())
    }

    #[test]
    fn sse_lines_events_and_done() {
        let mut p = SseParser::default();
        assert_eq!(p.line(": keep-alive"), None);
        assert_eq!(p.line("event: message"), None);
        assert_eq!(p.line(r#"data: {"a":1}"#), None);
        assert_eq!(p.line(""), Some(SseItem::Data(r#"{"a":1}"#.into())));
        assert_eq!(p.line("data:{\"b\":"), None);
        assert_eq!(p.line("data: 2}\r"), None);
        assert_eq!(p.line("\r"), Some(SseItem::Data("{\"b\":\n2}".into())));
        assert_eq!(p.line("data: [DONE]"), Some(SseItem::Done));
        assert_eq!(p.line(r#"data: {"c":3}"#), None);
        assert_eq!(p.flush(), Some(SseItem::Data(r#"{"c":3}"#.into())));
        assert_eq!(p.flush(), None);
    }

    #[test]
    fn tool_calls_accumulate_by_index() {
        let (ev, c) = run(&[
            json!({"choices": [{"index": 0, "delta": {"role": "assistant", "content": null, "tool_calls": [
                {"index": 0, "id": "call_a", "type": "function", "function": {"name": "eludite-file-read", "arguments": ""}}]}}]}),
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [{"index": 0, "function": {"arguments": "{\"pa"}}]}}]}),
            json!({"choices": [{"index": 0, "delta": {"tool_calls": [
                {"index": 1, "id": "call_b", "function": {"name": "diagnostics-list", "arguments": "{}"}},
                {"index": 0, "function": {"arguments": "th\": \"a.rs\"}"}}]}}]}),
            json!({"choices": [{"index": 0, "delta": {}, "finish_reason": "tool_calls"}]}),
            json!({"choices": [], "usage": {"prompt_tokens": 10, "completion_tokens": 5, "prompt_tokens_details": {"cached_tokens": 4}}}),
        ]);
        assert_eq!(c.tool_calls.len(), 2);
        assert_eq!(c.tool_calls[0].arguments, r#"{"path": "a.rs"}"#);
        assert_eq!(c.tool_calls[1].name, "diagnostics-list");
        assert_eq!(c.finish_reason.as_deref(), Some("tool_calls"));
        let u = c.usage.unwrap();
        assert_eq!(
            (u.prompt_tokens, u.completion_tokens, u.cached_tokens),
            (10, 5, Some(4))
        );
        let started: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                StreamEvent::ToolCallStarted { name, .. } => Some(name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(started, ["eludite-file-read", "diagnostics-list"]);
    }

    #[test]
    fn fragments_without_an_index_continue_the_last_call() {
        let (_, c) = run(&[
            json!({"choices": [{"delta": {"tool_calls": [{"id": "x", "function": {"name": "a", "arguments": "{\"k\""}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"function": {"arguments": ":1}"}}]}}]}),
            json!({"choices": [{"delta": {"tool_calls": [{"id": "y", "function": {"name": "b", "arguments": {"z": 2}}}]}}]}),
        ]);
        assert_eq!(c.tool_calls.len(), 2);
        assert_eq!(c.tool_calls[0].arguments, r#"{"k":1}"#);
        assert_eq!(c.tool_calls[1].arguments, r#"{"z":2}"#);
    }

    #[test]
    fn content_reasoning_and_bodies() {
        let (ev, c) = run(&[
            json!({"choices": [{"delta": {"reasoning_content": "hm"}}]}),
            json!({"choices": [{"delta": {"reasoning": " ok"}}]}),
            json!({"choices": [{"delta": {"content": "Hi"}, "finish_reason": null}]}),
            json!({"choices": [{"delta": {"content": "!"}, "finish_reason": "stop"}],
                   "usage": {"prompt_tokens": 3, "completion_tokens": 2, "completion_tokens_details": {"reasoning_tokens": 1}}}),
        ]);
        assert_eq!(c.content, "Hi!");
        assert_eq!(c.reasoning, "hm ok");
        assert_eq!(c.usage.unwrap().reasoning_tokens, Some(1));
        assert_eq!(ev[0], StreamEvent::Thought("hm".into()));
        let (ev, c) = completion_from_body(&json!({
            "choices": [{"index": 0, "message": {"role": "assistant", "content": "Done.", "tool_calls": [
                {"id": "c1", "type": "function", "function": {"name": "t", "arguments": "{}"}}]}, "finish_reason": "tool_calls"}],
            "usage": {"prompt_tokens": 1, "completion_tokens": 1}
        }));
        assert_eq!(c.content, "Done.");
        assert_eq!(c.tool_calls[0].id, "c1");
        assert!(ev.contains(&StreamEvent::Text("Done.".into())));
    }

    #[test]
    fn context_length_errors_and_bodies() {
        let llama = r#"{"error":{"code":400,"message":"the request exceeds the available context size, try increasing it","type":"exceed_context_size_error","n_prompt_tokens":17000,"n_ctx":16384}}"#;
        assert!(is_context_length_error(
            llama,
            models::error_message(llama).as_deref()
        ));
        let openai = r#"{"error":{"message":"This model's maximum context length is 8192 tokens.","type":"invalid_request_error","code":"context_length_exceeded"}}"#;
        assert!(is_context_length_error(openai, None));
        assert!(is_context_length_error("", Some("Prompt is too long")));
        assert!(!is_context_length_error(
            r#"{"error":{"message":"bad model"}}"#,
            Some("bad model")
        ));
        let body = request_body(
            "m",
            &[json!({"role": "user", "content": "x"})],
            &[],
            &RequestLimits::default(),
        );
        assert_eq!(body["max_tokens"], 8192);
        assert!(body.get("tools").is_none());
        assert!(body.get("temperature").is_none() && body.get("tool_choice").is_none());
        let limits = RequestLimits {
            max_completion_tokens: Some(100),
            chat_template_kwargs: Some(json!({"enable_thinking": false})),
            ..RequestLimits::default()
        };
        let body = request_body("m", &[], &[json!({"type": "function"})], &limits);
        assert_eq!(body["max_completion_tokens"], 100);
        assert!(body.get("max_tokens").is_none());
        assert_eq!(body["chat_template_kwargs"]["enable_thinking"], false);
        assert_eq!(body["stream_options"]["include_usage"], true);
    }

    #[test]
    fn urls_and_debug_never_show_the_key() {
        assert_eq!(normalize_base("http://h/v1/"), "http://h/v1");
        assert!(is_loopback("http://127.0.0.1:8080/v1"));
        assert!(is_loopback("http://localhost:11434/v1"));
        assert!(!is_loopback("https://openrouter.ai/api/v1"));
        let p = Provider::new(ProviderConfig {
            base_url: "http://127.0.0.1:1/v1/".into(),
            api_key: Some("sk-debug-secret".into()),
            headers: vec![
                ("Authorization".into(), "x".into()),
                ("X-Title".into(), "E".into()),
            ],
        });
        assert_eq!(p.base_url(), "http://127.0.0.1:1/v1");
        assert!(!format!("{p:?}").contains("sk-debug-secret"));
        assert!(!p.loggable_headers().contains("Authorization"));
        assert_eq!(
            p.headers()
                .iter()
                .filter(|(k, _)| k == "Authorization")
                .count(),
            1,
            "only the key's header"
        );
        let none = Provider::new(ProviderConfig {
            base_url: "http://127.0.0.1:1/v1".into(),
            api_key: Some(String::new()),
            headers: vec![],
        });
        assert!(none.headers().is_empty(), "no bare Bearer");
    }
}
