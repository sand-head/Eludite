//! A loopback fake of an OpenAI-compatible server: `std::net::TcpListener` and hand-written HTTP/1.1 (as
//! `crates/forge`'s fixture server), scripted per test. `GET /models` answers [`FakeServer::set_models`];
//! each `POST /chat/completions` takes the next scripted [`Reply`] (or the fallback). Every request is recorded with
//! its headers and body. Streams are chunked, as real servers send them, so a stream can end cleanly without
//! `[DONE]`, be cut mid-way ([`End::Drop`]) or stall until the client goes away ([`End::Stall`]).

#![allow(dead_code)]

use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

/// One recorded request.
#[derive(Debug, Clone)]
pub struct Recorded {
    pub method: String,
    pub path: String,
    pub headers: Vec<(String, String)>,
    pub body: String,
    pub at: Instant,
}

impl Recorded {
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap_or(Value::Null)
    }
}

/// One step of a scripted stream.
#[derive(Debug, Clone)]
pub enum Ev {
    /// `data: <json>` and a blank line.
    Data(Value),
    /// Bytes sent as they are (several lines, comments, a split event).
    Raw(String),
    Delay(u64),
}

/// How a scripted stream ends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum End {
    /// `data: [DONE]`, then the last chunk.
    Done,
    /// The last chunk with no `[DONE]`.
    NoDone,
    /// The connection is closed without the last chunk.
    Drop,
    /// Keep-alive comments every 20 ms until the client closes the connection.
    Stall,
}

/// A scripted answer.
#[derive(Debug, Clone)]
pub enum Reply {
    /// A whole body with this status and content type.
    Body {
        status: u16,
        content_type: String,
        body: String,
    },
    /// A chunked `text/event-stream`.
    Sse { events: Vec<Ev>, end: End },
}

impl Reply {
    pub fn json(status: u16, body: Value) -> Self {
        Reply::Body {
            status,
            content_type: "application/json".into(),
            body: body.to_string(),
        }
    }

    pub fn html(body: &str) -> Self {
        Reply::Body {
            status: 200,
            content_type: "text/html".into(),
            body: body.into(),
        }
    }

    pub fn sse(events: Vec<Ev>) -> Self {
        Reply::Sse {
            events,
            end: End::Done,
        }
    }
}

pub fn chunk(delta: Value, finish: Option<&str>) -> Value {
    json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "created": 1, "model": "m",
        "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]})
}

pub fn text(t: &str) -> Ev {
    Ev::Data(chunk(json!({"content": t}), None))
}

pub fn thought(t: &str) -> Ev {
    Ev::Data(chunk(json!({"reasoning_content": t}), None))
}

pub fn finish(reason: &str) -> Ev {
    Ev::Data(chunk(json!({}), Some(reason)))
}

/// The usage-only final chunk (`choices: []`), as with `stream_options.include_usage`.
pub fn usage(prompt: u64, completion: u64) -> Ev {
    Ev::Data(
        json!({"id": "chatcmpl-1", "object": "chat.completion.chunk", "choices": [],
        "usage": {"prompt_tokens": prompt, "completion_tokens": completion, "total_tokens": prompt + completion}}),
    )
}

/// A tool call fragment.
pub fn tool_frag(index: u64, id: Option<&str>, name: Option<&str>, args: &str) -> Ev {
    let mut f = json!({"index": index, "function": {"arguments": args}});
    if let Some(id) = id {
        f["id"] = json!(id);
        f["type"] = json!("function");
    }
    if let Some(n) = name {
        f["function"]["name"] = json!(n);
    }
    Ev::Data(chunk(json!({"tool_calls": [f]}), None))
}

/// A streamed text answer with usage.
pub fn text_reply(t: &str, prompt: u64, completion: u64) -> Reply {
    Reply::sse(vec![text(t), finish("stop"), usage(prompt, completion)])
}

/// A streamed answer calling one tool.
pub fn tool_reply(id: &str, name: &str, args: &str, prompt: u64) -> Reply {
    Reply::sse(vec![
        tool_frag(0, Some(id), Some(name), ""),
        tool_frag(0, None, None, args),
        finish("tool_calls"),
        usage(prompt, 10),
    ])
}

#[derive(Default)]
struct State {
    models: Option<Reply>,
    chats: VecDeque<Reply>,
    fallback: Option<Reply>,
    requests: Vec<Recorded>,
}

/// The server. Dropping it leaves the listener thread parked on `accept`; each test uses its own.
pub struct FakeServer {
    /// `http://127.0.0.1:PORT/v1`.
    pub url: String,
    state: Arc<Mutex<State>>,
    /// Set when a stream's write failed because the client went away.
    pub client_closed: Arc<AtomicBool>,
}

impl FakeServer {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/v1", listener.local_addr().unwrap());
        let state: Arc<Mutex<State>> = Arc::default();
        let closed: Arc<AtomicBool> = Arc::default();
        let (s, c) = (state.clone(), closed.clone());
        std::thread::spawn(move || {
            for conn in listener.incoming() {
                let Ok(conn) = conn else { continue };
                let (s, c) = (s.clone(), c.clone());
                std::thread::spawn(move || {
                    let _ = serve(conn, &s, &c);
                });
            }
        });
        Self {
            url,
            state,
            client_closed: closed,
        }
    }

    pub fn set_models(&self, reply: Reply) {
        self.state.lock().unwrap().models = Some(reply);
    }

    /// A llama.cpp-shaped `/models` listing of `ids` with `n_ctx_train`.
    pub fn llama_models(&self, ids: &[&str], n_ctx: u64) {
        let data: Vec<Value> = ids
            .iter()
            .map(|id| json!({"id": id, "object": "model", "owned_by": "llamacpp", "meta": {"n_ctx_train": n_ctx}}))
            .collect();
        self.set_models(Reply::json(200, json!({"object": "list", "data": data})));
    }

    pub fn push(&self, reply: Reply) {
        self.state.lock().unwrap().chats.push_back(reply);
    }

    pub fn set_fallback(&self, reply: Reply) {
        self.state.lock().unwrap().fallback = Some(reply);
    }

    pub fn requests(&self) -> Vec<Recorded> {
        self.state.lock().unwrap().requests.clone()
    }

    /// The chat requests' bodies.
    pub fn chats(&self) -> Vec<Value> {
        self.requests()
            .iter()
            .filter(|r| r.path.ends_with("/chat/completions"))
            .map(Recorded::json)
            .collect()
    }

    pub fn chat_requests(&self) -> Vec<Recorded> {
        self.requests()
            .into_iter()
            .filter(|r| r.path.ends_with("/chat/completions"))
            .collect()
    }
}

fn read_request(reader: &mut BufReader<TcpStream>) -> std::io::Result<Option<Recorded>> {
    let mut line = String::new();
    if reader.read_line(&mut line)? == 0 {
        return Ok(None);
    }
    let mut parts = line.split_whitespace();
    let method = parts.next().unwrap_or("").to_owned();
    let path = parts.next().unwrap_or("").to_owned();
    let mut headers = Vec::new();
    loop {
        let mut h = String::new();
        reader.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.push((k.trim().to_owned(), v.trim().to_owned()));
        }
    }
    let len = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-length"))
        .and_then(|(_, v)| v.parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; len];
    reader.read_exact(&mut body)?;
    Ok(Some(Recorded {
        method,
        path,
        headers,
        body: String::from_utf8_lossy(&body).into_owned(),
        at: Instant::now(),
    }))
}

fn status_text(status: u16) -> &'static str {
    match status {
        200 => "OK",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        500 => "Internal Server Error",
        _ => "Status",
    }
}

fn write_chunk(w: &mut TcpStream, data: &str) -> std::io::Result<()> {
    write!(w, "{:x}\r\n{data}\r\n", data.len())?;
    w.flush()
}

fn serve(conn: TcpStream, state: &Mutex<State>, closed: &AtomicBool) -> std::io::Result<()> {
    let _ = conn.set_nodelay(true);
    let mut reader = BufReader::new(conn.try_clone()?);
    let mut w = conn;
    while let Some(req) = read_request(&mut reader)? {
        let reply = {
            let mut s = state.lock().unwrap();
            s.requests.push(req.clone());
            if req.path.ends_with("/models") {
                s.models
                    .clone()
                    .unwrap_or_else(|| Reply::json(404, json!({"error": {"message": "not found"}})))
            } else {
                s.chats
                    .pop_front()
                    .or_else(|| s.fallback.clone())
                    .unwrap_or_else(|| {
                        Reply::json(500, json!({"error": {"message": "nothing scripted"}}))
                    })
            }
        };
        match reply {
            Reply::Body {
                status,
                content_type,
                body,
            } => {
                write!(
                    w,
                    "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\n\r\n{body}",
                    status_text(status),
                    body.len()
                )?;
                w.flush()?;
            }
            Reply::Sse { events, end } => {
                write!(
                    w,
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nCache-Control: no-cache\r\nTransfer-Encoding: chunked\r\n\r\n"
                )?;
                w.flush()?;
                for ev in events {
                    let r = match ev {
                        Ev::Data(v) => write_chunk(&mut w, &format!("data: {v}\n\n")),
                        Ev::Raw(s) => write_chunk(&mut w, &s),
                        Ev::Delay(ms) => {
                            std::thread::sleep(Duration::from_millis(ms));
                            Ok(())
                        }
                    };
                    if r.is_err() {
                        closed.store(true, Ordering::SeqCst);
                        return Ok(());
                    }
                }
                match end {
                    End::Done => {
                        write_chunk(&mut w, "data: [DONE]\n\n")?;
                        w.write_all(b"0\r\n\r\n")?;
                        w.flush()?;
                    }
                    End::NoDone => {
                        w.write_all(b"0\r\n\r\n")?;
                        w.flush()?;
                    }
                    End::Drop => {
                        let _ = w.shutdown(std::net::Shutdown::Both);
                        return Ok(());
                    }
                    End::Stall => {
                        for _ in 0..500 {
                            std::thread::sleep(Duration::from_millis(20));
                            if write_chunk(&mut w, ": keep-alive\n\n").is_err() {
                                closed.store(true, Ordering::SeqCst);
                                return Ok(());
                            }
                        }
                        return Ok(());
                    }
                }
            }
        }
    }
    Ok(())
}
