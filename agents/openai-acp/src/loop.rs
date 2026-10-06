//! The turn: request, stream, run the tool calls through MCP, request again, until the model answers without tool
//! calls, [`MAX_TURN_REQUESTS`] requests were made, the person cancels, or something fails.
//!
//! The ACP side is a sink of `session/update` objects (JSON in ACP's shape), so the loop is independent of the
//! connection. Each request runs on its own thread ([`Provider::chat`]); the turn awaits its events and the
//! [`Cancel`] signal together, so a cancel answers at once and drops the request. Tool calls run one at a time, in
//! order; the IDE's gate prompts or refuses by its policy and a refusal's text is the tool's result. The adapter
//! never asks for permission itself.

use std::collections::HashSet;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::task::Poll;
use std::time::Instant;

use futures::StreamExt;
use futures::channel::mpsc;
use futures::future::{Either, select};
use futures::task::AtomicWaker;
use serde_json::{Value, json};

use crate::compact;
use crate::log;
use crate::mcp::{self, McpClient, ToolResult, ToolSet};
use crate::provider::{
    self, ChatError, Completion, Provider, RequestLimits, StreamEvent, ToolCallParts,
};

/// Requests one turn may make.
pub const MAX_TURN_REQUESTS: usize = 50;
/// The most of a tool result the model is sent.
pub const TOOL_RESULT_LIMIT: usize = 100_000;
/// The most of a tool result shown in the IDE's tool call.
const UI_RESULT_LIMIT: usize = 10_000;

/// How a turn ended (ACP's `StopReason`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stop {
    EndTurn,
    MaxTokens,
    MaxTurnRequests,
    Refusal,
    Cancelled,
}

/// The cancel signal of the turn in progress: set by `session/cancel`, awaited by the turn.
#[derive(Debug, Default)]
pub struct Cancel {
    flag: AtomicBool,
    waker: AtomicWaker,
}

impl Cancel {
    pub fn cancel(&self) {
        self.flag.store(true, Ordering::SeqCst);
        self.waker.wake();
    }

    pub fn reset(&self) {
        self.flag.store(false, Ordering::SeqCst);
    }

    pub fn is_set(&self) -> bool {
        self.flag.load(Ordering::SeqCst)
    }

    /// Resolves once [`Cancel::cancel`] was called.
    pub fn wait(&self) -> impl Future<Output = ()> + '_ {
        futures::future::poll_fn(move |cx| {
            if self.is_set() {
                return Poll::Ready(());
            }
            self.waker.register(cx.waker());
            if self.is_set() {
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        })
    }
}

/// The model the next request uses, and its context window (0: unknown). `session/set_config_option` changes it
/// between requests.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CurrentModel {
    pub id: String,
    pub window: u64,
}

/// The turn's token totals, for `_meta.eludite.usage` (ACP's `Usage` names).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TurnUsage {
    pub input: u64,
    pub cached: Option<u64>,
    pub output: u64,
    pub thought: Option<u64>,
    pub total: u64,
}

impl TurnUsage {
    fn add(&mut self, u: &provider::Usage) {
        let cached = u.cached_tokens.unwrap_or(0).min(u.prompt_tokens);
        self.input += u.prompt_tokens - cached;
        if u.cached_tokens.is_some() {
            *self.cached.get_or_insert(0) += cached;
        }
        self.output += u.completion_tokens;
        if let Some(r) = u.reasoning_tokens {
            *self.thought.get_or_insert(0) += r;
        }
        self.total += u.prompt_tokens + u.completion_tokens;
    }

    fn to_json(&self, model: &str) -> Value {
        let mut v = json!({
            "inputTokens": self.input,
            "outputTokens": self.output,
            "totalTokens": self.total,
            "model": model,
        });
        if let Some(c) = self.cached {
            v["cachedReadTokens"] = json!(c);
        }
        if let Some(t) = self.thought {
            v["thoughtTokens"] = json!(t);
        }
        v
    }
}

/// Where a turn sends its `session/update`s.
pub type Sink<'a> = &'a mut (dyn FnMut(Value) + Send);

/// What one request came to.
enum Outcome {
    Done(Completion, Vec<(usize, String)>),
    Failed(ChatError),
    Cancelled(String),
}

/// One session's conversation, tools and model client.
pub struct Engine {
    pub provider: Provider,
    pub limits: RequestLimits,
    pub cwd: PathBuf,
    pub system: String,
    /// The conversation without the system prompt.
    pub messages: Vec<Value>,
    /// The last response's `prompt_tokens` + `completion_tokens` (what the next request carries).
    pub last_used: u64,
    pub mcp: Vec<Arc<McpClient>>,
    pub server_names: Vec<String>,
    pub tools: ToolSet,
    pub model: Arc<Mutex<CurrentModel>>,
    ids: HashSet<String>,
    usage: TurnUsage,
    /// Wall-clock of the last response's end, for the request-to-request budget (logged).
    last_end: Option<Instant>,
}

fn update_text(kind: &str, text: &str) -> Value {
    json!({"sessionUpdate": kind, "content": {"type": "text", "text": text}})
}

fn truncate_chars(s: &str, max: usize) -> (String, bool) {
    match s.char_indices().nth(max) {
        Some((i, _)) => (s[..i].to_owned(), true),
        None => (s.to_owned(), false),
    }
}

/// ACP's tool kind for a tool, from its command's permission class.
pub fn tool_kind(entry: Option<&mcp::ToolEntry>) -> &'static str {
    let Some(e) = entry else { return "other" };
    let id = e.tool.short_id();
    if id.starts_with("search.") || id.ends_with("find_references") {
        return "search";
    }
    match e.tool.permission.as_deref() {
        Some("read") => "read",
        Some("edit_buffer") => "edit",
        Some("execute") | Some("dangerous") => "execute",
        _ => "other",
    }
}

/// The title of a tool call: the tool's name and its first argument.
pub fn tool_title(name: &str, args: &Value) -> String {
    let first = args
        .as_object()
        .and_then(|o| o.values().next())
        .map(|v| match v {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        });
    match first {
        Some(f) if !f.is_empty() => {
            let (f, cut) = truncate_chars(f.lines().next().unwrap_or(""), 80);
            format!("{name} {f}{}", if cut { "…" } else { "" })
        }
        _ => name.to_owned(),
    }
}

impl Engine {
    /// `servers`: each MCP server's name and client, in the order `tools` was built from.
    pub fn new(
        provider: Provider,
        limits: RequestLimits,
        cwd: &Path,
        system: String,
        model: Arc<Mutex<CurrentModel>>,
        servers: Vec<(String, Arc<McpClient>)>,
        tools: ToolSet,
    ) -> Self {
        let (server_names, mcp) = servers.into_iter().unzip();
        Self {
            provider,
            limits,
            cwd: cwd.to_path_buf(),
            system,
            messages: Vec::new(),
            last_used: 0,
            mcp,
            server_names,
            tools,
            model,
            ids: HashSet::new(),
            usage: TurnUsage::default(),
            last_end: None,
        }
    }

    fn current_model(&self) -> CurrentModel {
        self.model.lock().map(|m| m.clone()).unwrap_or_default()
    }

    /// The request's messages: the system prompt, then the conversation.
    pub fn request_messages(&self) -> Vec<Value> {
        let mut m = Vec::with_capacity(self.messages.len() + 1);
        m.push(json!({"role": "system", "content": self.system}));
        m.extend(self.messages.iter().cloned());
        m
    }

    /// The next request's body.
    pub fn body(&self) -> Value {
        provider::request_body(
            &self.current_model().id,
            &self.request_messages(),
            &self.tools.function_tools(),
            &self.limits,
        )
    }

    fn unique_id(&mut self, server_id: &str) -> String {
        if !server_id.is_empty() && self.ids.insert(server_id.to_owned()) {
            return server_id.to_owned();
        }
        loop {
            let id = format!("call_{}", &uuid::Uuid::new_v4().simple().to_string()[..16]);
            if self.ids.insert(id.clone()) {
                return id;
            }
        }
    }

    /// Re-list the tools when a server said they changed.
    pub async fn refresh_tools(&mut self) {
        // Every server's flag is taken (no short circuit), so none stays set for a later turn.
        let changed = self.mcp.iter().filter(|c| c.take_list_changed()).count() > 0;
        if !changed {
            return;
        }
        let mut lists = Vec::new();
        for c in &self.mcp {
            lists.push(c.list_tools().await.unwrap_or_else(|e| {
                log::warn(format_args!("tools/list on {} failed: {e}", c.name()));
                Vec::new()
            }));
        }
        self.tools.set(&self.server_names, lists);
        log::info(format_args!(
            "the tool list changed: {} tools",
            self.tools.len()
        ));
    }

    async fn request(
        &mut self,
        body: Value,
        visible: bool,
        cancel: &Cancel,
        sink: Sink<'_>,
    ) -> Outcome {
        let (tx, mut rx) = mpsc::unbounded();
        let provider = self.provider.clone();
        if let Some(end) = self.last_end.take() {
            log::info(format_args!(
                "next request {} µs after the last response",
                end.elapsed().as_micros()
            ));
        }
        let spawned = std::thread::Builder::new()
            .name("openai-request".into())
            .spawn(move || provider.chat(&body, &tx));
        if let Err(e) = spawned {
            return Outcome::Failed(ChatError::Network(e.to_string()));
        }
        let mut partial = String::new();
        let mut started: Vec<(usize, String)> = Vec::new();
        loop {
            let event = {
                let next = rx.next();
                let stop = cancel.wait();
                futures::pin_mut!(next, stop);
                match select(next, stop).await {
                    Either::Left((e, _)) => e,
                    Either::Right(_) => {
                        // Dropping the receiver ends the request thread at its next event.
                        return Outcome::Cancelled(partial);
                    }
                }
            };
            let Some(event) = event else {
                return Outcome::Failed(ChatError::Dropped);
            };
            match event {
                StreamEvent::FirstByte => {}
                StreamEvent::Text(t) => {
                    if visible {
                        partial.push_str(&t);
                        sink(update_text("agent_message_chunk", &t));
                    }
                }
                StreamEvent::Thought(t) => {
                    if visible {
                        sink(update_text("agent_thought_chunk", &t));
                    }
                }
                StreamEvent::ToolCallStarted { index, id, name } => {
                    if visible {
                        let acp = self.unique_id(&id);
                        let kind = tool_kind(self.tools.find(&name));
                        sink(
                            json!({"sessionUpdate": "tool_call", "toolCallId": acp, "title": name,
                            "kind": kind, "status": "pending"}),
                        );
                        started.push((index, acp));
                    }
                }
                StreamEvent::Done(Ok(c)) => {
                    self.last_end = Some(Instant::now());
                    return Outcome::Done(c, started);
                }
                StreamEvent::Done(Err(e)) => {
                    for (_, id) in &started {
                        sink(
                            json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": "failed"}),
                        );
                    }
                    return Outcome::Failed(e);
                }
            }
        }
    }

    fn send_usage(&self, sink: Sink<'_>) {
        let model = self.current_model();
        sink(json!({
            "sessionUpdate": "usage_update",
            "used": self.last_used,
            "size": model.window,
            "_meta": {"eludite": {"usage": self.usage.to_json(&model.id)}},
        }));
    }

    /// Summarize the messages before `at`. `Ok(Some)`: cancelled.
    async fn compact(
        &mut self,
        at: usize,
        cancel: &Cancel,
        sink: Sink<'_>,
    ) -> Result<Option<Stop>, String> {
        let before = if self.last_used > 0 {
            self.last_used
        } else {
            compact::estimate_tokens(&self.request_messages())
        };
        let body = provider::request_body(
            &self.current_model().id,
            &compact::summary_request(&self.messages[..at]),
            &[],
            &self.limits,
        );
        let summary = match self.request(body, false, cancel, sink).await {
            Outcome::Done(c, _) => c.content,
            Outcome::Cancelled(_) => return Ok(Some(Stop::Cancelled)),
            Outcome::Failed(e) => return Err(format!("Compacting the conversation failed: {e}")),
        };
        if summary.trim().is_empty() {
            return Err(
                "Compacting the conversation failed: the model returned an empty summary".into(),
            );
        }
        self.messages = compact::apply_summary(&self.messages, at, &summary);
        let after = compact::estimate_tokens(&self.request_messages());
        self.last_used = after;
        sink(update_text(
            "agent_message_chunk",
            &format!("Compacted the conversation ({before} tokens to {after})\n\n"),
        ));
        Ok(None)
    }

    /// Compact the conversation (see [`crate::compact`]). `Ok(Ok(false))`: there was nothing to compact;
    /// `Ok(Err(stop))`: cancelled.
    pub async fn try_compact(
        &mut self,
        cancel: &Cancel,
        sink: Sink<'_>,
    ) -> Result<Result<bool, Stop>, String> {
        let Some(at) = compact::split_point(&self.messages) else {
            return Ok(Ok(false));
        };
        match self.compact(at, cancel, sink).await? {
            None => Ok(Ok(true)),
            Some(stop) => Ok(Err(stop)),
        }
    }

    /// One prompt turn. `/clear` and `/compact` are handled here.
    pub async fn turn(
        &mut self,
        prompt: &str,
        cancel: &Cancel,
        sink: Sink<'_>,
    ) -> Result<Stop, String> {
        self.usage = TurnUsage::default();
        let command = prompt.split_whitespace().next().unwrap_or("");
        if command == "/clear" {
            self.messages.clear();
            self.last_used = 0;
            sink(update_text(
                "agent_message_chunk",
                "Cleared the conversation. The next prompt starts fresh.",
            ));
            self.send_usage(sink);
            return Ok(Stop::EndTurn);
        }
        if command == "/compact" {
            return match self.try_compact(cancel, sink).await? {
                Ok(true) => {
                    self.send_usage(sink);
                    Ok(Stop::EndTurn)
                }
                Ok(false) => {
                    sink(update_text(
                        "agent_message_chunk",
                        "There is nothing to compact yet.",
                    ));
                    Ok(Stop::EndTurn)
                }
                Err(stop) => Ok(stop),
            };
        }
        self.messages
            .push(json!({"role": "user", "content": prompt}));
        let len = self.messages.len();
        let result = self.requests(cancel, sink).await;
        // A turn that failed before any answer leaves no unanswered prompt behind (roles keep alternating).
        if result.is_err() && self.messages.len() == len {
            self.messages.pop();
        }
        result
    }

    /// Request, run the tool calls, and request again until the turn ends.
    async fn requests(&mut self, cancel: &Cancel, sink: Sink<'_>) -> Result<Stop, String> {
        let mut requests = 0;
        loop {
            if cancel.is_set() {
                return Ok(Stop::Cancelled);
            }
            if requests >= MAX_TURN_REQUESTS {
                return Ok(Stop::MaxTurnRequests);
            }
            self.refresh_tools().await;
            compact::trim_tool_results(&mut self.messages);
            if compact::should_compact(self.last_used, self.current_model().window)
                && let Err(stop) = self.try_compact(cancel, sink).await?
            {
                return Ok(stop);
            }
            let mut retried = false;
            let mut compacted = false;
            let (completion, started) = loop {
                let body = self.body();
                match self.request(body, true, cancel, sink).await {
                    Outcome::Done(c, s) => break (c, s),
                    Outcome::Cancelled(partial) => {
                        self.messages.push(json!({"role": "assistant",
                            "content": if partial.is_empty() { "(cancelled)".to_owned() } else { partial }}));
                        return Ok(Stop::Cancelled);
                    }
                    Outcome::Failed(ChatError::Dropped) if !retried => {
                        log::info(format_args!("the stream was cut; retrying once"));
                        retried = true;
                    }
                    Outcome::Failed(
                        e @ ChatError::Status {
                            context_length: true,
                            ..
                        },
                    ) if !compacted => {
                        compacted = true;
                        match self.try_compact(cancel, sink).await? {
                            Ok(true) => {}
                            Ok(false) => return Err(capitalize(&e.to_string())),
                            Err(stop) => return Ok(stop),
                        }
                    }
                    Outcome::Failed(e) => return Err(capitalize(&e.to_string())),
                }
            };
            requests += 1;
            if let Some(u) = &completion.usage {
                self.usage.add(u);
                self.last_used = u.prompt_tokens + u.completion_tokens;
            } else {
                self.last_used = compact::estimate_tokens(&self.request_messages())
                    + (completion.content.len() as u64).div_ceil(4);
            }
            self.send_usage(sink);
            if completion.tool_calls.is_empty() {
                self.messages
                    .push(json!({"role": "assistant", "content": completion.content}));
                return Ok(match completion.finish_reason.as_deref() {
                    Some("length") => Stop::MaxTokens,
                    Some("content_filter") => Stop::Refusal,
                    _ => Stop::EndTurn,
                });
            }
            if let Some(stop) = self.run_tools(completion, started, cancel, sink).await {
                return Ok(stop);
            }
        }
    }

    /// Run a response's tool calls in order and append the assistant message and the results. `Some`: cancelled.
    async fn run_tools(
        &mut self,
        completion: Completion,
        started: Vec<(usize, String)>,
        cancel: &Cancel,
        sink: Sink<'_>,
    ) -> Option<Stop> {
        struct Call {
            id: String,
            parts: ToolCallParts,
            args: Result<Value, String>,
        }
        let mut calls = Vec::new();
        for parts in completion.tool_calls {
            let id = match started.iter().find(|(i, _)| *i == parts.index) {
                Some((_, id)) => id.clone(),
                None => {
                    let id = self.unique_id(&parts.id);
                    sink(
                        json!({"sessionUpdate": "tool_call", "toolCallId": id, "title": parts.name,
                        "kind": tool_kind(self.tools.find(&parts.name)), "status": "pending"}),
                    );
                    id
                }
            };
            let text = parts.arguments.trim();
            let args = if text.is_empty() {
                Ok(json!({}))
            } else {
                match serde_json::from_str::<Value>(text) {
                    Ok(v @ Value::Object(_)) => Ok(v),
                    Ok(_) => Err("the arguments are not a JSON object".to_owned()),
                    Err(e) => Err(format!("the arguments are not valid JSON ({e})")),
                }
            };
            calls.push(Call { id, parts, args });
        }
        // Malformed arguments are replayed as `{}` (servers parse the history's arguments); the tool result says why.
        let tool_calls: Vec<Value> = calls
            .iter()
            .map(|c| {
                let arguments = match &c.args {
                    Ok(_) => c.parts.arguments.trim().to_owned(),
                    Err(_) => "{}".to_owned(),
                };
                let arguments = if arguments.is_empty() { "{}".to_owned() } else { arguments };
                json!({"id": c.id, "type": "function", "function": {"name": c.parts.name, "arguments": arguments}})
            })
            .collect();
        let content = (!completion.content.is_empty()).then(|| completion.content.clone());
        self.messages
            .push(json!({"role": "assistant", "content": content, "tool_calls": tool_calls}));
        let mut images: Vec<(String, String, String)> = Vec::new();
        for (n, call) in calls.iter().enumerate() {
            if cancel.is_set() {
                self.cancel_rest(
                    &calls[n..].iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
                    false,
                    sink,
                );
                return Some(Stop::Cancelled);
            }
            let name = call.parts.name.clone();
            let args = call.args.clone().unwrap_or_else(|_| json!({}));
            let mut update = json!({"sessionUpdate": "tool_call_update", "toolCallId": call.id,
                "status": "in_progress", "title": tool_title(&name, &args), "rawInput": args});
            if let Some(p) = args.get("path").and_then(Value::as_str) {
                let path = self.cwd.join(p);
                let mut loc = json!({"path": path.to_string_lossy()});
                if let Some(l) = args
                    .get("line")
                    .or_else(|| args.get("startLine"))
                    .and_then(Value::as_u64)
                {
                    loc["line"] = json!(l);
                }
                update["locations"] = json!([loc]);
            }
            sink(update);
            let result = match &call.args {
                Err(why) => ToolResult {
                    text: format!(
                        "The call was not run: {why}. The text received was: {}\nCall {name} again with a JSON object that matches its schema.",
                        truncate_chars(&call.parts.arguments, 2_000).0
                    ),
                    images: Vec::new(),
                    is_error: true,
                },
                Ok(args) if name == mcp::META_TOOL => ToolResult {
                    text: self.tools.meta_call(args),
                    images: Vec::new(),
                    is_error: false,
                },
                Ok(args) => match self.tools.find(&name).cloned() {
                    None => ToolResult {
                        text: format!(
                            "There is no tool named {name:?}. Use one of the tools you were given{}.",
                            if self.tools.mode == mcp::ToolsMode::Core {
                                ", or call eludite-tools to list the others"
                            } else {
                                ""
                            }
                        ),
                        images: Vec::new(),
                        is_error: true,
                    },
                    Some(entry) => {
                        let client = self.mcp[entry.server].clone();
                        let fut = client.call_tool(&entry.tool.name, args, &call.id);
                        let stop = cancel.wait();
                        futures::pin_mut!(fut, stop);
                        match select(fut, stop).await {
                            Either::Left((r, _)) => r,
                            Either::Right(_) => {
                                self.cancel_rest(
                                    &calls[n..].iter().map(|c| c.id.clone()).collect::<Vec<_>>(),
                                    true,
                                    sink,
                                );
                                return Some(Stop::Cancelled);
                            }
                        }
                    }
                },
            };
            let (text, cut) = truncate_chars(&result.text, TOOL_RESULT_LIMIT);
            let mut text = if cut {
                format!("{text}\n[the result was cut at {TOOL_RESULT_LIMIT} characters]")
            } else {
                text
            };
            if !result.images.is_empty() {
                text.push_str(&format!(
                    "\n[{} image(s) from this tool follow in the next user message]",
                    result.images.len()
                ));
                images.extend(
                    result
                        .images
                        .iter()
                        .map(|(m, d)| (name.clone(), m.clone(), d.clone())),
                );
            }
            let (shown, _) = truncate_chars(&text, UI_RESULT_LIMIT);
            sink(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": call.id,
                "status": if result.is_error { "failed" } else { "completed" },
                "content": [{"type": "content", "content": {"type": "text", "text": shown}}]}),
            );
            self.messages
                .push(json!({"role": "tool", "tool_call_id": call.id, "content": text}));
        }
        if !images.is_empty() {
            let mut parts =
                vec![json!({"type": "text", "text": "Images returned by the tools above:"})];
            for (tool, mime, data) in images {
                parts.push(json!({"type": "text", "text": format!("From {tool}:")}));
                parts.push(json!({"type": "image_url", "image_url": {"url": format!("data:{mime};base64,{data}")}}));
            }
            self.messages
                .push(json!({"role": "user", "content": parts}));
        }
        None
    }

    /// Answer the calls in `ids` that never got a result, so the history stays valid for the next prompt.
    fn cancel_rest(&mut self, ids: &[String], first_running: bool, sink: Sink<'_>) {
        for (i, id) in ids.iter().enumerate() {
            let text = if i == 0 && first_running {
                "The person cancelled the turn while this tool was running; its result was not received."
            } else {
                "The person cancelled the turn before this tool ran."
            };
            sink(
                json!({"sessionUpdate": "tool_call_update", "toolCallId": id, "status": "failed"}),
            );
            self.messages
                .push(json!({"role": "tool", "tool_call_id": id, "content": text}));
        }
    }
}

fn capitalize(s: &str) -> String {
    let mut c = s.chars();
    match c.next() {
        Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
        None => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn titles_kinds_and_usage() {
        assert_eq!(
            tool_title(
                "eludite-file-read",
                &json!({"path": "src/a.rs", "startLine": 3})
            ),
            "eludite-file-read src/a.rs"
        );
        assert_eq!(
            tool_title("diagnostics-list", &json!({})),
            "diagnostics-list"
        );
        assert_eq!(tool_title("t", &json!({"n": 5})), "t 5");
        assert_eq!(tool_kind(None), "other");
        let mut u = TurnUsage::default();
        u.add(&provider::Usage {
            prompt_tokens: 100,
            completion_tokens: 10,
            cached_tokens: Some(40),
            reasoning_tokens: None,
        });
        u.add(&provider::Usage {
            prompt_tokens: 120,
            completion_tokens: 5,
            cached_tokens: None,
            reasoning_tokens: Some(2),
        });
        let v = u.to_json("m");
        assert_eq!(v["inputTokens"], 180);
        assert_eq!(v["cachedReadTokens"], 40);
        assert_eq!(v["outputTokens"], 15);
        assert_eq!(v["thoughtTokens"], 2);
        assert_eq!(v["totalTokens"], 235);
        assert!(
            TurnUsage::default()
                .to_json("m")
                .get("cachedReadTokens")
                .is_none()
        );
    }

    #[test]
    fn cancel_wakes_the_waiter() {
        let c = Arc::new(Cancel::default());
        let c2 = c.clone();
        let t = std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_millis(20));
            c2.cancel();
        });
        futures::executor::block_on(c.wait());
        assert!(c.is_set());
        c.reset();
        assert!(!c.is_set());
        t.join().unwrap();
    }
}
