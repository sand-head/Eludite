//! A CDP connection: one websocket to a browser endpoint, a reader thread, and requests correlated by id.
//!
//! - **Messages.** The envelope (`id`, `method`, `params`, `sessionId`, `result`, `error`) is typed here
//!   ([`Outgoing`], [`Incoming`]); the protocol JSON does not describe it, so it is not generated.
//! - **Threads.** The websocket is `tungstenite` over a `std::net::TcpStream`, no TLS and no async runtime. The
//!   reader thread (`cdp-reader`) owns the read half; writes go through a second `tungstenite` socket over a clone
//!   of the same stream, under a mutex, so a caller's write never waits for a read. (The read half would answer a
//!   ping by writing; CDP endpoints never send pings.)
//! - **Requests.** [`Connection::call`] waits for its answer up to a timeout; [`Connection::call_many`] pipelines
//!   several and waits for all; [`Connection::send`] only queues one and answers a receiver. Nothing blocks a
//!   caller that did not ask to wait.
//! - **Events** go to every subscriber of their session ([`Connection::subscribe`]; `None` is the browser's own
//!   session) through a channel, from the reader thread.
//! - **Closing.** When the socket closes or fails, every pending request fails with [`CdpError::Closed`], every
//!   subscription ends (its channel disconnects) and the close hook runs once.

use std::collections::HashMap;
use std::io;
use std::net::{TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use serde_json::Value;
use tungstenite::protocol::{Role, WebSocketConfig};
use tungstenite::{Message, WebSocket};

/// How long a request waits for its answer by default.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// A request on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outgoing {
    pub id: u64,
    pub method: String,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub params: Value,
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// An error answer's body.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireError {
    pub code: i64,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

/// A message from the browser: an answer (`id` with `result` or `error`) or an event (`method` and `params`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Incoming {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub id: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<WireError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(rename = "sessionId", default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
}

/// An event, as subscribers receive it.
#[derive(Debug, Clone, PartialEq)]
pub struct CdpEvent {
    pub method: String,
    pub params: Value,
    pub session_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CdpError {
    /// The connection is closed (the browser exited or hung up).
    Closed,
    /// No answer in time.
    Timeout { method: String, after: Duration },
    /// The browser answered with an error.
    Protocol {
        method: String,
        code: i64,
        message: String,
    },
    /// Connecting or writing failed.
    Io(String),
}

impl std::fmt::Display for CdpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CdpError::Closed => write!(f, "the browser connection is closed"),
            CdpError::Timeout { method, after } => {
                write!(
                    f,
                    "the browser did not answer {method} in {} ms",
                    after.as_millis()
                )
            }
            CdpError::Protocol {
                method, message, ..
            } => write!(f, "{method}: {message}"),
            CdpError::Io(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for CdpError {}

type Reply = mpsc::Sender<Result<Value, CdpError>>;

struct Inner {
    writer: Mutex<Option<WebSocket<TcpStream>>>,
    /// The stream, to shut it down on close.
    stream: TcpStream,
    next_id: AtomicU64,
    pending: Mutex<HashMap<u64, (String, Reply)>>,
    subscribers: Mutex<HashMap<Option<String>, Vec<mpsc::Sender<CdpEvent>>>>,
    closed: AtomicBool,
    on_close: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    endpoint: String,
}

/// A connection to one CDP endpoint. Cheap to clone; every clone shares the socket.
#[derive(Clone)]
pub struct Connection {
    inner: Arc<Inner>,
}

impl std::fmt::Debug for Connection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Connection")
            .field("endpoint", &self.inner.endpoint)
            .field("closed", &self.is_closed())
            .finish()
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// Big enough for a full-page screenshot or a large accessibility tree in one message.
fn config() -> WebSocketConfig {
    WebSocketConfig::default()
        .max_message_size(Some(512 << 20))
        .max_frame_size(None)
}

impl Connection {
    /// Connect to `ws://host:port/path` (no TLS) within `timeout`.
    pub fn connect(url: &str, timeout: Duration) -> Result<Connection, CdpError> {
        let rest = url
            .strip_prefix("ws://")
            .ok_or_else(|| CdpError::Io(format!("not a ws:// endpoint: {url}")))?;
        let host = rest.split('/').next().unwrap_or_default();
        let addr = host
            .to_socket_addrs()
            .map_err(|e| CdpError::Io(format!("{host}: {e}")))?
            .next()
            .ok_or_else(|| CdpError::Io(format!("{host}: no address")))?;
        let stream = TcpStream::connect_timeout(&addr, timeout)
            .map_err(|e| CdpError::Io(format!("{url}: {e}")))?;
        let _ = stream.set_nodelay(true);
        stream
            .set_read_timeout(Some(timeout))
            .map_err(|e| CdpError::Io(e.to_string()))?;
        let (reader, _) = tungstenite::client::client_with_config(
            url,
            stream.try_clone().map_err(io_err)?,
            Some(config()),
        )
        .map_err(|e| CdpError::Io(format!("websocket handshake with {url}: {e}")))?;
        // Reads block until a message arrives from now on.
        stream.set_read_timeout(None).map_err(io_err)?;
        let writer = WebSocket::from_raw_socket(
            stream.try_clone().map_err(io_err)?,
            Role::Client,
            Some(config()),
        );
        let inner = Arc::new(Inner {
            writer: Mutex::new(Some(writer)),
            stream,
            next_id: AtomicU64::new(1),
            pending: Mutex::default(),
            subscribers: Mutex::default(),
            closed: AtomicBool::new(false),
            on_close: Mutex::new(None),
            endpoint: url.to_owned(),
        });
        let reader_inner = inner.clone();
        std::thread::Builder::new()
            .name("cdp-reader".into())
            .spawn(move || reader_loop(reader, reader_inner))
            .map_err(io_err)?;
        Ok(Connection { inner })
    }

    pub fn endpoint(&self) -> &str {
        &self.inner.endpoint
    }

    pub fn is_closed(&self) -> bool {
        self.inner.closed.load(Ordering::Acquire)
    }

    /// Run `f` once when the connection closes (at once when it is closed already).
    pub fn on_close(&self, f: impl FnOnce() + Send + 'static) {
        if self.is_closed() {
            f();
            return;
        }
        *lock(&self.inner.on_close) = Some(Box::new(f));
        // The reader may have closed between the check and the store.
        if self.is_closed()
            && let Some(f) = lock(&self.inner.on_close).take()
        {
            f();
        }
    }

    /// Queue a request; its answer arrives on the returned receiver.
    pub fn send(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
    ) -> Result<mpsc::Receiver<Result<Value, CdpError>>, CdpError> {
        if self.is_closed() {
            return Err(CdpError::Closed);
        }
        let id = self.inner.next_id.fetch_add(1, Ordering::AcqRel);
        let msg = Outgoing {
            id,
            method: method.to_owned(),
            params: if params.is_null() {
                Value::Object(Default::default())
            } else {
                params
            },
            session_id: session.map(str::to_owned),
        };
        let text = serde_json::to_string(&msg).map_err(|e| CdpError::Io(e.to_string()))?;
        let (tx, rx) = mpsc::channel();
        lock(&self.inner.pending).insert(id, (method.to_owned(), tx));
        let written = match lock(&self.inner.writer).as_mut() {
            Some(w) => w
                .send(Message::text(text))
                .map_err(|e| CdpError::Io(e.to_string())),
            None => Err(CdpError::Closed),
        };
        if let Err(e) = written {
            lock(&self.inner.pending).remove(&id);
            if self.is_closed() {
                return Err(CdpError::Closed);
            }
            return Err(e);
        }
        Ok(rx)
    }

    /// Send a request and wait for its answer up to `timeout`.
    pub fn call(
        &self,
        session: Option<&str>,
        method: &str,
        params: Value,
        timeout: Duration,
    ) -> Result<Value, CdpError> {
        let rx = self.send(session, method, params)?;
        wait(&rx, method, Instant::now() + timeout, timeout)
    }

    /// Send several requests at once and wait for every answer (each up to `timeout` from now), in order.
    pub fn call_many(
        &self,
        session: Option<&str>,
        calls: Vec<(String, Value)>,
        timeout: Duration,
    ) -> Vec<Result<Value, CdpError>> {
        let deadline = Instant::now() + timeout;
        let sent: Vec<(String, Result<_, CdpError>)> = calls
            .into_iter()
            .map(|(m, p)| {
                let r = self.send(session, &m, p);
                (m, r)
            })
            .collect();
        sent.into_iter()
            .map(|(m, r)| r.and_then(|rx| wait(&rx, &m, deadline, timeout)))
            .collect()
    }

    /// The events of `session` (`None`: the browser's own, without a `sessionId`). The channel disconnects when the
    /// connection closes or [`Connection::unsubscribe`] drops the session's subscribers.
    pub fn subscribe(&self, session: Option<&str>) -> mpsc::Receiver<CdpEvent> {
        let (tx, rx) = mpsc::channel();
        if !self.is_closed() {
            lock(&self.inner.subscribers)
                .entry(session.map(str::to_owned))
                .or_default()
                .push(tx);
        }
        rx
    }

    /// Drop every subscriber of `session` (their receivers disconnect).
    pub fn unsubscribe(&self, session: Option<&str>) {
        lock(&self.inner.subscribers).remove(&session.map(str::to_owned));
    }

    /// Close the socket; the reader thread then fails what is pending and runs the close hook.
    pub fn close(&self) {
        if let Some(mut w) = lock(&self.inner.writer).take() {
            let _ = w.close(None);
            let _ = w.flush();
        }
        let _ = self.inner.stream.shutdown(std::net::Shutdown::Both);
    }
}

fn io_err(e: io::Error) -> CdpError {
    CdpError::Io(e.to_string())
}

fn wait(
    rx: &mpsc::Receiver<Result<Value, CdpError>>,
    method: &str,
    deadline: Instant,
    timeout: Duration,
) -> Result<Value, CdpError> {
    let left = deadline.saturating_duration_since(Instant::now());
    match rx.recv_timeout(left) {
        Ok(r) => r,
        Err(mpsc::RecvTimeoutError::Timeout) => Err(CdpError::Timeout {
            method: method.to_owned(),
            after: timeout,
        }),
        Err(mpsc::RecvTimeoutError::Disconnected) => Err(CdpError::Closed),
    }
}

fn reader_loop(mut ws: WebSocket<TcpStream>, inner: Arc<Inner>) {
    loop {
        let text = match ws.read() {
            Ok(Message::Text(t)) => t,
            Ok(Message::Binary(b)) => match String::from_utf8(b.to_vec()) {
                Ok(s) => s.into(),
                Err(_) => continue,
            },
            Ok(Message::Close(_)) | Err(_) => break,
            Ok(_) => continue,
        };
        let Ok(msg) = serde_json::from_str::<Incoming>(text.as_str()) else {
            continue;
        };
        if let Some(id) = msg.id {
            let Some((method, reply)) = lock(&inner.pending).remove(&id) else {
                // An answer nobody waits for any more (it timed out).
                continue;
            };
            let result = match msg.error {
                Some(e) => Err(CdpError::Protocol {
                    method,
                    code: e.code,
                    message: e.message,
                }),
                None => Ok(msg.result.unwrap_or(Value::Null)),
            };
            let _ = reply.send(result);
        } else if let Some(method) = msg.method {
            let event = CdpEvent {
                method,
                params: msg.params.unwrap_or(Value::Null),
                session_id: msg.session_id,
            };
            let mut subs = lock(&inner.subscribers);
            if let Some(list) = subs.get_mut(&event.session_id) {
                list.retain(|tx| tx.send(event.clone()).is_ok());
            }
        }
    }
    inner.closed.store(true, Ordering::Release);
    let _ = inner.stream.shutdown(std::net::Shutdown::Both);
    lock(&inner.writer).take();
    for (_, (_, reply)) in lock(&inner.pending).drain() {
        let _ = reply.send(Err(CdpError::Closed));
    }
    lock(&inner.subscribers).clear();
    let hook = lock(&inner.on_close).take();
    if let Some(f) = hook {
        f();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::net::TcpListener;

    /// A fake CDP endpoint in this process: accepts one websocket and runs `script` with it.
    fn fake_endpoint(script: impl FnOnce(WebSocket<TcpStream>) + Send + 'static) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let ws = tungstenite::accept(stream).unwrap();
            script(ws);
        });
        format!("ws://127.0.0.1:{port}/devtools/browser/fake")
    }

    fn read_request(ws: &mut WebSocket<TcpStream>) -> Outgoing {
        loop {
            if let Message::Text(t) = ws.read().unwrap() {
                return serde_json::from_str(t.as_str()).unwrap();
            }
        }
    }

    fn write(ws: &mut WebSocket<TcpStream>, v: Value) {
        ws.send(Message::text(v.to_string())).unwrap();
    }

    #[test]
    fn call_many_pipelines_and_answers_in_order() {
        let url = fake_endpoint(|mut ws| {
            // Both requests arrive before either is answered.
            let a = read_request(&mut ws);
            let b = read_request(&mut ws);
            write(&mut ws, json!({"id": b.id, "result": {"n": 2}}));
            write(&mut ws, json!({"id": a.id, "result": {"n": 1}}));
            let _ = ws.read();
        });
        let conn = Connection::connect(&url, Duration::from_secs(5)).unwrap();
        let answers = conn.call_many(
            None,
            vec![("A.a".into(), Value::Null), ("B.b".into(), json!({"x": 1}))],
            Duration::from_secs(5),
        );
        assert_eq!(answers, vec![Ok(json!({"n": 1})), Ok(json!({"n": 2}))]);
        conn.close();
    }

    #[test]
    fn frames_and_correlates_out_of_order_answers_and_routes_events() {
        let url = fake_endpoint(|mut ws| {
            let a = read_request(&mut ws);
            let b = read_request(&mut ws);
            assert_eq!(a.method, "Browser.getVersion");
            assert_eq!(a.session_id, None);
            assert_eq!(b.method, "Runtime.evaluate");
            assert_eq!(b.session_id.as_deref(), Some("S1"));
            assert_eq!(b.params, json!({"expression": "1+1"}));
            // An event for the session and one for the browser, then the answers in reverse order.
            write(
                &mut ws,
                json!({"method": "Runtime.consoleAPICalled", "params": {"type": "log"}, "sessionId": "S1"}),
            );
            write(
                &mut ws,
                json!({"method": "Target.targetCreated", "params": {"targetInfo": {}}}),
            );
            write(
                &mut ws,
                json!({"id": b.id, "result": {"value": 2}, "sessionId": "S1"}),
            );
            write(
                &mut ws,
                json!({"id": a.id, "result": {"product": "Fake/1.0"}}),
            );
            let c = read_request(&mut ws);
            write(
                &mut ws,
                json!({"id": c.id, "error": {"code": -32601, "message": "'Nope.nope' wasn't found"}}),
            );
            let _ = ws.read();
        });
        let conn = Connection::connect(&url, Duration::from_secs(5)).unwrap();
        let session_events = conn.subscribe(Some("S1"));
        let browser_events = conn.subscribe(None);
        let a = conn.send(None, "Browser.getVersion", Value::Null).unwrap();
        let b = conn
            .send(Some("S1"), "Runtime.evaluate", json!({"expression": "1+1"}))
            .unwrap();
        let t = Duration::from_secs(5);
        assert_eq!(b.recv_timeout(t).unwrap().unwrap(), json!({"value": 2}));
        assert_eq!(
            a.recv_timeout(t).unwrap().unwrap(),
            json!({"product": "Fake/1.0"})
        );
        let e = session_events.recv_timeout(t).unwrap();
        assert_eq!(e.method, "Runtime.consoleAPICalled");
        assert_eq!(e.session_id.as_deref(), Some("S1"));
        assert_eq!(
            browser_events.recv_timeout(t).unwrap().method,
            "Target.targetCreated"
        );
        assert!(
            session_events.try_recv().is_err(),
            "the browser's event is not the session's"
        );
        match conn.call(None, "Nope.nope", Value::Null, t) {
            Err(CdpError::Protocol { code, method, .. }) => {
                assert_eq!(code, -32601);
                assert_eq!(method, "Nope.nope");
            }
            other => panic!("{other:?}"),
        }
        conn.close();
    }

    #[test]
    fn a_request_times_out_and_its_late_answer_is_dropped() {
        let url = fake_endpoint(|mut ws| {
            let a = read_request(&mut ws);
            std::thread::sleep(Duration::from_millis(300));
            write(&mut ws, json!({"id": a.id, "result": {}}));
            let b = read_request(&mut ws);
            write(&mut ws, json!({"id": b.id, "result": {"ok": true}}));
            let _ = ws.read();
        });
        let conn = Connection::connect(&url, Duration::from_secs(5)).unwrap();
        let started = Instant::now();
        let r = conn.call(None, "Slow.call", Value::Null, Duration::from_millis(100));
        assert!(matches!(r, Err(CdpError::Timeout { .. })), "{r:?}");
        crate::assert_budget(
            "the timed-out call",
            started.elapsed(),
            Duration::from_millis(250),
        );
        // The late answer to the first request does not satisfy the second.
        let r = conn
            .call(None, "Next.call", Value::Null, Duration::from_secs(5))
            .unwrap();
        assert_eq!(r, json!({"ok": true}));
        // A caller that did not ask to wait is never blocked: `send` returns at once.
        let started = Instant::now();
        let _rx = conn.send(None, "Never.answered", Value::Null).unwrap();
        crate::assert_budget("send", started.elapsed(), Duration::from_millis(50));
        conn.close();
    }

    #[test]
    fn a_closed_connection_fails_pending_requests_and_ends_subscriptions() {
        let url = fake_endpoint(|mut ws| {
            let _ = read_request(&mut ws);
            // Hang up without answering.
            let _ = ws.close(None);
            let _ = ws.flush();
        });
        let conn = Connection::connect(&url, Duration::from_secs(5)).unwrap();
        let events = conn.subscribe(Some("S"));
        let (tx, closed) = mpsc::channel();
        conn.on_close(move || {
            let _ = tx.send(());
        });
        let started = Instant::now();
        let r = conn.call(
            None,
            "Page.navigate",
            json!({"url": "about:blank"}),
            Duration::from_secs(10),
        );
        assert_eq!(r, Err(CdpError::Closed));
        assert!(started.elapsed() < Duration::from_secs(5));
        closed.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(conn.is_closed());
        assert!(matches!(
            events.recv_timeout(Duration::from_secs(5)),
            Err(mpsc::RecvTimeoutError::Disconnected)
        ));
        assert_eq!(
            conn.call(None, "X.y", Value::Null, Duration::from_secs(1)),
            Err(CdpError::Closed)
        );
    }

    #[test]
    fn message_shapes() {
        let o = Outgoing {
            id: 3,
            method: "Page.navigate".into(),
            params: json!({"url": "about:blank"}),
            session_id: Some("S".into()),
        };
        assert_eq!(
            serde_json::to_value(&o).unwrap(),
            json!({"id": 3, "method": "Page.navigate", "params": {"url": "about:blank"}, "sessionId": "S"})
        );
        let i: Incoming =
            serde_json::from_value(json!({"id": 3, "error": {"code": -32000, "message": "x"}}))
                .unwrap();
        assert_eq!(i.error.unwrap().code, -32000);
        let e: Incoming =
            serde_json::from_value(json!({"method": "Page.loadEventFired", "params": {"timestamp": 1.5}, "sessionId": "S"}))
                .unwrap();
        assert_eq!(e.id, None);
        assert_eq!(e.session_id.as_deref(), Some("S"));
    }
}
