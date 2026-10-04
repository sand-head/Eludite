//! Recorded fixtures and the transports that answer from them, so every forge call can run without a network:
//! [`ReplayTransport`] in process, and [`FixtureServer`], a loopback HTTP/1.1 server on `std::net` that the real
//! [`crate::http::UreqTransport`] talks to (the crate's integration tests and the shell's headless tests).
//!
//! A fixture set is a folder (`crates/forge/testdata/<forge>/`) of JSON files, one exchange each:
//!
//! ```json
//! { "source": "recorded from codeberg.org on 2026-10-04",
//!   "request": { "method": "GET", "path": "/api/v1/repos/o/r/pulls", "query": { "state": "open" },
//!                "body_contains": ["\"event\":\"APPROVE\""] },
//!   "response": { "status": 200, "headers": { "ETag": "\"abc\"" }, "body": [ ... ] },
//!   "times": 1 }
//! ```
//!
//! A request matches a fixture with its method, its path (a `*` segment matches any one segment) and every query
//! member and body piece the fixture names; the fixture naming the most wins, then the first by file name. `times`
//! uses a fixture up so many times, so a sequence (a device flow's pending, then its token) is two files. A response
//! with an `ETag` answers 304 to a request whose `If-None-Match` carries it. `{{base}}` in a response is the
//! server's base url. Unmatched requests answer 404 `no fixture for ...` and are counted as misses.

use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::Result;
use crate::http::{Cancel, Method, Request, Response, Transport};

/// One recorded exchange.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Exchange {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    pub request: FixtureRequest,
    pub response: FixtureResponse,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub times: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureRequest {
    pub method: String,
    pub path: String,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub query: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub body_contains: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FixtureResponse {
    pub status: u16,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub headers: BTreeMap<String, String>,
    #[serde(default, skip_serializing_if = "Value::is_null")]
    pub body: Value,
    /// A body that is not JSON (a log).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub body_text: Option<String>,
}

impl Exchange {
    /// A fixture answering `method path` with `status` and a JSON body.
    pub fn json(method: &str, path: &str, status: u16, body: Value) -> Self {
        Self {
            source: None,
            request: FixtureRequest {
                method: method.to_owned(),
                path: path.to_owned(),
                query: BTreeMap::new(),
                body_contains: Vec::new(),
            },
            response: FixtureResponse {
                status,
                headers: BTreeMap::new(),
                body,
                body_text: None,
            },
            times: None,
        }
    }

    pub fn with_header(mut self, k: &str, v: &str) -> Self {
        self.response.headers.insert(k.to_owned(), v.to_owned());
        self
    }

    pub fn with_query(mut self, k: &str, v: &str) -> Self {
        self.request.query.insert(k.to_owned(), v.to_owned());
        self
    }

    pub fn times(mut self, n: u32) -> Self {
        self.times = Some(n);
        self
    }
}

/// A request as the fixtures saw it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Seen {
    pub method: String,
    /// The path and query (`/repos/o/r/pulls?state=open`).
    pub target: String,
    pub authorization: Option<String>,
    pub if_none_match: Option<String>,
    pub body: String,
    pub status: u16,
}

struct Slot {
    exchange: Exchange,
    left: Option<u32>,
    /// A test's override: it wins over every loaded fixture its request matches.
    first: bool,
}

/// A fixture set, with what it answered.
#[derive(Default)]
pub struct Fixtures {
    slots: Mutex<Vec<Slot>>,
    seen: Mutex<Vec<Seen>>,
    misses: AtomicU64,
    base: Mutex<String>,
}

impl std::fmt::Debug for Fixtures {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Fixtures")
            .field("count", &self.slots.lock().map(|s| s.len()).unwrap_or(0))
            .finish()
    }
}

impl Fixtures {
    pub fn new() -> Self {
        Self::default()
    }

    /// Every `*.json` under `dir` (recursively, `README` and `PIN` files skipped), in file-name order.
    pub fn load(dir: &Path) -> std::io::Result<Self> {
        let f = Self::new();
        f.add_dir(dir)?;
        Ok(f)
    }

    /// Add a folder's fixtures after those already loaded.
    pub fn add_dir(&self, dir: &Path) -> std::io::Result<()> {
        let mut files = Vec::new();
        let mut stack = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            for e in std::fs::read_dir(&d)?.flatten() {
                let p = e.path();
                if p.is_dir() {
                    stack.push(p);
                } else if p.extension().is_some_and(|x| x == "json") {
                    files.push(p);
                }
            }
        }
        files.sort();
        for p in files {
            let text = std::fs::read_to_string(&p)?;
            let ex: Exchange = serde_json::from_str(&text).map_err(|e| {
                std::io::Error::new(
                    std::io::ErrorKind::InvalidData,
                    format!("{}: {e}", p.display()),
                )
            })?;
            self.push(ex);
        }
        Ok(())
    }

    /// Add a fixture after the others.
    pub fn push(&self, exchange: Exchange) {
        let left = exchange.times;
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Slot {
                exchange,
                left,
                first: false,
            });
    }

    /// Add a fixture that wins over the others for its requests (a test's override: a 401, a rate limit).
    pub fn push_front(&self, exchange: Exchange) {
        let left = exchange.times;
        self.slots.lock().unwrap_or_else(|e| e.into_inner()).insert(
            0,
            Slot {
                exchange,
                left,
                first: true,
            },
        );
    }

    /// Remove every fixture for `method path`.
    pub fn remove(&self, method: &str, path: &str) {
        self.slots
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .retain(|s| !(s.exchange.request.method == method && s.exchange.request.path == path));
    }

    /// The base url `{{base}}` stands for.
    pub fn set_base(&self, base: &str) {
        *self.base.lock().unwrap_or_else(|e| e.into_inner()) = base.to_owned();
    }

    /// The requests answered so far.
    pub fn seen(&self) -> Vec<Seen> {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// How many requests came.
    pub fn count(&self) -> usize {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).len()
    }

    /// Requests no fixture matched.
    pub fn misses(&self) -> u64 {
        self.misses.load(Ordering::SeqCst)
    }

    pub fn clear_seen(&self) {
        self.seen.lock().unwrap_or_else(|e| e.into_inner()).clear();
        self.misses.store(0, Ordering::SeqCst);
    }

    /// Answer one request: `target` is the path and query.
    pub fn answer(
        &self,
        method: &str,
        target: &str,
        headers: &[(String, String)],
        body: &[u8],
    ) -> Response {
        let (path, query) = split_target(target);
        let body_text = String::from_utf8_lossy(body).into_owned();
        let header = |n: &str| {
            headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(n))
                .map(|(_, v)| v.clone())
        };
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
        let mut best: Option<(usize, usize)> = None;
        for (i, s) in slots.iter().enumerate() {
            if s.left == Some(0) {
                continue;
            }
            let r = &s.exchange.request;
            if !r.method.eq_ignore_ascii_case(method) || !path_matches(&r.path, &path) {
                continue;
            }
            if !r
                .query
                .iter()
                .all(|(k, v)| query.get(k).is_some_and(|x| x == v))
            {
                continue;
            }
            if !r.body_contains.iter().all(|p| body_text.contains(p)) {
                continue;
            }
            let score = r.query.len()
                + r.body_contains.len()
                + usize::from(!r.path.contains('*'))
                + if s.first { 1000 } else { 0 };
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((i, score));
            }
        }
        let base = self.base.lock().unwrap_or_else(|e| e.into_inner()).clone();
        let response = match best {
            Some((i, _)) => {
                let slot = &mut slots[i];
                if let Some(n) = &mut slot.left {
                    *n -= 1;
                }
                let r = &slot.exchange.response;
                let etag = r
                    .headers
                    .iter()
                    .find(|(k, _)| k.eq_ignore_ascii_case("etag"))
                    .map(|(_, v)| v.clone());
                let inm = header("if-none-match");
                if etag.is_some() && etag == inm {
                    Response {
                        status: 304,
                        headers: vec![("ETag".into(), etag.unwrap_or_default())],
                        body: Vec::new(),
                    }
                } else {
                    let body = match &r.body_text {
                        Some(t) => t.clone(),
                        None if r.body.is_null() => String::new(),
                        None => serde_json::to_string(&r.body).expect("serializes"),
                    };
                    Response {
                        status: r.status,
                        headers: r
                            .headers
                            .iter()
                            .map(|(k, v)| (k.clone(), v.replace("{{base}}", &base)))
                            .collect(),
                        body: body.replace("{{base}}", &base).into_bytes(),
                    }
                }
            }
            None => {
                self.misses.fetch_add(1, Ordering::SeqCst);
                Response {
                    status: 404,
                    headers: vec![("Content-Type".into(), "application/json".into())],
                    body: serde_json::to_vec(
                        &serde_json::json!({ "message": format!("no fixture for {method} {target}") }),
                    )
                    .expect("serializes"),
                }
            }
        };
        drop(slots);
        self.seen
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .push(Seen {
                method: method.to_owned(),
                target: target.to_owned(),
                authorization: header("authorization").or_else(|| header("private-token")),
                if_none_match: header("if-none-match"),
                body: body_text,
                status: response.status,
            });
        response
    }
}

fn split_target(target: &str) -> (String, BTreeMap<String, String>) {
    let (path, q) = target.split_once('?').unwrap_or((target, ""));
    let query = q
        .split('&')
        .filter(|p| !p.is_empty())
        .map(|p| {
            let (k, v) = p.split_once('=').unwrap_or((p, ""));
            (crate::util::decode(k), crate::util::decode(v))
        })
        .collect();
    (crate::util::decode(path), query)
}

fn path_matches(pattern: &str, path: &str) -> bool {
    let a: Vec<&str> = pattern.trim_end_matches('/').split('/').collect();
    let b: Vec<&str> = path.trim_end_matches('/').split('/').collect();
    a.len() == b.len() && a.iter().zip(&b).all(|(x, y)| *x == "*" || x == y)
}

/// Fixtures as an in-process transport, with an optional delay per request.
pub struct ReplayTransport {
    pub fixtures: Arc<Fixtures>,
    pub delay: Duration,
}

impl ReplayTransport {
    pub fn new(fixtures: Arc<Fixtures>) -> Self {
        Self {
            fixtures,
            delay: Duration::ZERO,
        }
    }
}

impl Transport for ReplayTransport {
    fn send(&self, request: &Request, cancel: &Cancel) -> Result<Response> {
        cancel.check()?;
        if !self.delay.is_zero() {
            std::thread::sleep(self.delay);
        }
        let rest = request.url.split("://").nth(1).unwrap_or(&request.url);
        let target = rest.find('/').map(|i| &rest[i..]).unwrap_or("/");
        Ok(self.fixtures.answer(
            request.method.as_str(),
            target,
            &request.headers,
            request.body.as_deref().unwrap_or(&[]),
        ))
    }
}

/// A loopback HTTP/1.1 server answering from fixtures, a thread per connection, keep-alive, with an optional delay
/// per request. Dropped, it stops.
pub struct FixtureServer {
    pub fixtures: Arc<Fixtures>,
    port: u16,
    stop: Arc<AtomicBool>,
    delay_ms: Arc<AtomicU64>,
}

impl std::fmt::Debug for FixtureServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("FixtureServer")
            .field("port", &self.port)
            .finish()
    }
}

impl FixtureServer {
    /// Serve `fixtures` on a free loopback port.
    pub fn start(fixtures: Fixtures) -> std::io::Result<Self> {
        let listener = TcpListener::bind("127.0.0.1:0")?;
        let port = listener.local_addr()?.port();
        let fixtures = Arc::new(fixtures);
        fixtures.set_base(&format!("http://127.0.0.1:{port}"));
        let stop = Arc::new(AtomicBool::new(false));
        let delay_ms = Arc::new(AtomicU64::new(0));
        {
            let (fixtures, stop, delay_ms) = (fixtures.clone(), stop.clone(), delay_ms.clone());
            std::thread::Builder::new()
                .name("forge-fixtures".into())
                .spawn(move || {
                    for conn in listener.incoming() {
                        if stop.load(Ordering::SeqCst) {
                            break;
                        }
                        let Ok(conn) = conn else { continue };
                        let (fixtures, stop, delay_ms) =
                            (fixtures.clone(), stop.clone(), delay_ms.clone());
                        std::thread::spawn(move || serve(conn, &fixtures, &stop, &delay_ms));
                    }
                })?;
        }
        Ok(Self {
            fixtures,
            port,
            stop,
            delay_ms,
        })
    }

    /// Serve the fixture folder `dir`.
    pub fn serve_dir(dir: &Path) -> std::io::Result<Self> {
        Self::start(Fixtures::load(dir)?)
    }

    /// `http://127.0.0.1:<port>`.
    pub fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// Wait this long before answering each request (the budget test's 20 ms).
    pub fn set_delay(&self, d: Duration) {
        self.delay_ms.store(d.as_millis() as u64, Ordering::SeqCst);
    }
}

impl Drop for FixtureServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = TcpStream::connect(("127.0.0.1", self.port));
    }
}

fn serve(conn: TcpStream, fixtures: &Fixtures, stop: &AtomicBool, delay_ms: &AtomicU64) {
    let _ = conn.set_read_timeout(Some(Duration::from_secs(30)));
    let _ = conn.set_nodelay(true);
    let mut reader = BufReader::new(match conn.try_clone() {
        Ok(c) => c,
        Err(_) => return,
    });
    let mut writer = conn;
    loop {
        if stop.load(Ordering::SeqCst) {
            return;
        }
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) | Err(_) => return,
            Ok(_) => {}
        }
        let mut parts = line.split_whitespace();
        let (Some(method), Some(target)) = (parts.next(), parts.next()) else {
            return;
        };
        let (method, mut target) = (method.to_owned(), target.to_owned());
        if let Some(rest) = target.strip_prefix("http://") {
            target = rest
                .find('/')
                .map(|i| rest[i..].to_owned())
                .unwrap_or("/".into());
        }
        let mut headers = Vec::new();
        let mut length = 0usize;
        let mut close = false;
        loop {
            let mut h = String::new();
            if reader.read_line(&mut h).is_err() {
                return;
            }
            let h = h.trim_end();
            if h.is_empty() {
                break;
            }
            if let Some((k, v)) = h.split_once(':') {
                let (k, v) = (k.trim().to_owned(), v.trim().to_owned());
                if k.eq_ignore_ascii_case("content-length") {
                    length = v.parse().unwrap_or(0);
                }
                if k.eq_ignore_ascii_case("connection") && v.eq_ignore_ascii_case("close") {
                    close = true;
                }
                headers.push((k, v));
            }
        }
        if headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("expect") && v.eq_ignore_ascii_case("100-continue")
        }) && writer.write_all(b"HTTP/1.1 100 Continue\r\n\r\n").is_err()
        {
            return;
        }
        let mut body = vec![0; length];
        if length > 0 && reader.read_exact(&mut body).is_err() {
            return;
        }
        let delay = delay_ms.load(Ordering::SeqCst);
        if delay > 0 {
            std::thread::sleep(Duration::from_millis(delay));
        }
        let _ = Method::parse(&method);
        let r = fixtures.answer(&method, &target, &headers, &body);
        let mut out = format!("HTTP/1.1 {} {}\r\n", r.status, reason(r.status));
        let mut has_type = false;
        for (k, v) in &r.headers {
            if k.eq_ignore_ascii_case("content-length")
                || k.eq_ignore_ascii_case("transfer-encoding")
            {
                continue;
            }
            has_type |= k.eq_ignore_ascii_case("content-type");
            out.push_str(&format!("{k}: {v}\r\n"));
        }
        if !has_type && !r.body.is_empty() {
            out.push_str("Content-Type: application/json\r\n");
        }
        out.push_str(&format!("Content-Length: {}\r\n\r\n", r.body.len()));
        let mut bytes = out.into_bytes();
        bytes.extend_from_slice(&r.body);
        if writer.write_all(&bytes).is_err() {
            return;
        }
        let _ = writer.flush();
        if close {
            return;
        }
    }
}

fn reason(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        202 => "Accepted",
        204 => "No Content",
        304 => "Not Modified",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        409 => "Conflict",
        422 => "Unprocessable Entity",
        429 => "Too Many Requests",
        _ => "Status",
    }
}

/// A transport that sends through another and writes each exchange as a fixture file (the recorder,
/// `tools/forge-corpus/record.sh`): request headers are never written (so no token is), response headers are cut to
/// the validators and paging, and [`scrub`] removes personal data and anything shaped like a token from the body.
/// `hosts` maps a host other than the API's to a path prefix (`plc.directory` → `/plc`), as the replay expects it.
pub struct RecordingTransport {
    pub inner: Arc<dyn Transport>,
    pub dir: std::path::PathBuf,
    pub source: String,
    pub hosts: Vec<(String, String)>,
    /// The API's host: any other host not in `hosts` is recorded under `/didweb/<host>` (a `did:web` document).
    pub api_host: Option<String>,
    seq: AtomicU64,
    written: Mutex<std::collections::HashSet<String>>,
}

impl RecordingTransport {
    pub fn new(
        inner: Arc<dyn Transport>,
        dir: impl Into<std::path::PathBuf>,
        source: impl Into<String>,
    ) -> Self {
        Self {
            inner,
            dir: dir.into(),
            source: source.into(),
            hosts: Vec::new(),
            api_host: None,
            seq: AtomicU64::new(0),
            written: Mutex::new(std::collections::HashSet::new()),
        }
    }

    pub fn api_host(mut self, host: &str) -> Self {
        self.api_host = Some(host.to_owned());
        self
    }

    pub fn map_host(mut self, host: &str, prefix: &str) -> Self {
        self.hosts.push((host.to_owned(), prefix.to_owned()));
        self
    }
}

/// Response headers a fixture keeps.
const KEPT_HEADERS: [&str; 6] = [
    "content-type",
    "etag",
    "link",
    "x-total-count",
    "x-total",
    "x-next-page",
];

/// The longest array a fixture keeps.
pub const MAX_FIXTURE_ARRAY: usize = 50;

/// The largest string a fixture keeps (logs and patches are cut).
pub const MAX_FIXTURE_STRING: usize = 4000;

impl Transport for RecordingTransport {
    fn send(&self, request: &Request, cancel: &Cancel) -> Result<Response> {
        let response = self.inner.send(request, cancel)?;
        let rest = request.url.split("://").nth(1).unwrap_or(&request.url);
        let (host, target) = match rest.find('/') {
            Some(i) => (&rest[..i], &rest[i..]),
            None => (rest, "/"),
        };
        let prefix = match self.hosts.iter().find(|(h, _)| h == host) {
            Some((_, p)) => p.clone(),
            None if self.api_host.as_deref().is_some_and(|a| a != host) => {
                format!("/didweb/{host}")
            }
            None => String::new(),
        };
        let prefix = prefix.as_str();
        let target = format!("{prefix}{target}");
        let (path, query) = split_target(&target);
        let body_text = request
            .body
            .as_ref()
            .map(|b| String::from_utf8_lossy(b).into_owned())
            .unwrap_or_default();
        if matches!(response.status, 203 | 401 | 403) {
            // An anonymous recording's refusals are not fixtures: the signed-in runs' answers are.
            eprintln!(
                "not recorded (needs a token): {} {target} answered {}",
                request.method.as_str(),
                response.status
            );
            return Ok(response);
        }
        let key = format!("{} {target} {body_text}", request.method.as_str());
        if !self
            .written
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .insert(key)
        {
            return Ok(response);
        }
        let origin = format!(
            "{}://{host}",
            request.url.split("://").next().unwrap_or("https")
        );
        let headers: BTreeMap<String, String> = response
            .headers
            .iter()
            .filter(|(k, _)| KEPT_HEADERS.contains(&k.to_ascii_lowercase().as_str()))
            .map(|(k, v)| {
                (
                    k.clone(),
                    v.replace(&origin, &format!("{{{{base}}}}{prefix}")),
                )
            })
            .collect();
        let (body, body_text_out) = match serde_json::from_slice::<Value>(&response.body) {
            Ok(v) => (scrub(v), None),
            Err(_) if response.body.is_empty() => (Value::Null, None),
            Err(_) => {
                let t = String::from_utf8_lossy(&response.body).into_owned();
                (
                    Value::Null,
                    Some(cut(
                        &scrub_text(&t),
                        if t.trim_start().starts_with('<') {
                            2_000
                        } else {
                            20_000
                        },
                    )),
                )
            }
        };
        let mut body_contains = Vec::new();
        if let Ok(Value::Object(o)) = serde_json::from_str::<Value>(&body_text) {
            // A write's fixture names its body's first members, so a different write does not match it.
            for (k, v) in o.iter().take(2) {
                if let Some(s) = v
                    .as_str()
                    .filter(|s| s.len() < 80 && !k.contains("password"))
                {
                    body_contains.push(format!(
                        "\"{k}\":{}",
                        serde_json::to_string(s).unwrap_or_default()
                    ));
                }
            }
        }
        let ex = Exchange {
            source: Some(self.source.clone()),
            request: FixtureRequest {
                method: request.method.as_str().to_owned(),
                path,
                query,
                body_contains,
            },
            response: FixtureResponse {
                status: response.status,
                headers,
                body,
                body_text: body_text_out,
            },
            times: None,
        };
        let n = self.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let slug: String = target
            .split('?')
            .next()
            .unwrap_or("")
            .rsplit('/')
            .take(2)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect::<Vec<_>>()
            .join("-")
            .chars()
            .map(|c| {
                if c.is_ascii_alphanumeric() || c == '-' {
                    c
                } else {
                    '_'
                }
            })
            .take(48)
            .collect();
        let _ = std::fs::create_dir_all(&self.dir);
        let file = self.dir.join(format!(
            "{n:03}-{}-{slug}.json",
            request.method.as_str().to_ascii_lowercase()
        ));
        let text = serde_json::to_string_pretty(&ex).expect("serializes");
        let _ = std::fs::write(file, text + "\n");
        Ok(response)
    }
}

fn cut(s: &str, n: usize) -> String {
    if s.len() <= n {
        return s.to_owned();
    }
    let mut end = n;
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    format!("{}…", &s[..end])
}

/// Members of a user object blanked in fixtures.
const PERSONAL: [&str; 12] = [
    "email",
    "full_name",
    "location",
    "website",
    "description",
    "avatar_url",
    "pronouns",
    "last_login",
    "public_email",
    "imageUrl",
    "skype",
    "linkedin",
];

/// Remove personal data and token-shaped strings from a recorded body; cut long strings.
pub fn scrub(v: Value) -> Value {
    match v {
        Value::Object(o) => Value::Object(
            o.into_iter()
                .map(|(k, v)| {
                    let blank = PERSONAL.contains(&k.as_str())
                        || k.to_ascii_lowercase().contains("email")
                        || k == "avatar";
                    if blank && v.is_string() {
                        let replacement = if k.to_ascii_lowercase().contains("email") {
                            "user@example.invalid"
                        } else {
                            ""
                        };
                        (k, Value::String(replacement.into()))
                    } else if k == "event_payload" || k == "Raw" || k == "BodyAppendix" {
                        (k, Value::String(String::new()))
                    } else if k == "Lines" && v.is_array() {
                        // Tangled's compare: the patch line by line, of which Eludite reads only the counts.
                        (k, Value::Array(Vec::new()))
                    } else {
                        (k, scrub(v))
                    }
                })
                .collect(),
        ),
        Value::Array(a) => Value::Array(a.into_iter().take(MAX_FIXTURE_ARRAY).map(scrub).collect()),
        Value::String(s) => Value::String(cut(&scrub_text(&s), MAX_FIXTURE_STRING)),
        other => other,
    }
}

/// Token shapes (`ghp_`, `gho_`, `github_pat_`, `glpat-`, JWTs) and email addresses in text replaced.
pub fn scrub_text(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for word in s.split_inclusive(|c: char| {
        c.is_whitespace() || c == '"' || c == '\'' || c == '<' || c == '>' || c == '(' || c == ')'
    }) {
        let trimmed = word.trim_end_matches(|c: char| {
            c.is_whitespace()
                || c == '"'
                || c == '\''
                || c == '<'
                || c == '>'
                || c == '('
                || c == ')'
        });
        let tail = &word[trimmed.len()..];
        let secret = [
            "ghp_",
            "gho_",
            "ghu_",
            "ghs_",
            "github_pat_",
            "glpat-",
            "gloas-",
        ]
        .iter()
        .any(|p| trimmed.starts_with(p))
            || (trimmed.starts_with("eyJ") && trimmed.len() > 40);
        let email = trimmed.contains('@')
            && trimmed
                .rsplit_once('@')
                .is_some_and(|(u, d)| !u.is_empty() && d.contains('.') && !d.starts_with('.'))
            && !trimmed.contains("://")
            && trimmed
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "@._+-".contains(c));
        if secret {
            out.push_str("<scrubbed>");
        } else if email {
            out.push_str("user@example.invalid");
        } else {
            out.push_str(trimmed);
        }
        out.push_str(tail);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn matching_prefers_the_most_specific_fixture_and_uses_times() {
        let f = Fixtures::new();
        f.push(Exchange::json(
            "GET",
            "/repos/*/*/pulls",
            200,
            json!(["any"]),
        ));
        f.push(Exchange::json(
            "GET",
            "/repos/o/r/pulls",
            200,
            json!(["exact"]),
        ));
        f.push(
            Exchange::json("GET", "/repos/o/r/pulls", 200, json!(["closed"]))
                .with_query("state", "closed"),
        );
        f.push(Exchange::json("POST", "/x", 200, json!("once")).times(1));
        f.push(Exchange::json("POST", "/x", 200, json!("after")));
        let body = |r: Response| String::from_utf8(r.body).unwrap();
        assert_eq!(
            body(f.answer("GET", "/repos/a/b/pulls", &[], b"")),
            "[\"any\"]"
        );
        assert_eq!(
            body(f.answer("GET", "/repos/o/r/pulls", &[], b"")),
            "[\"exact\"]"
        );
        assert_eq!(
            body(f.answer("GET", "/repos/o/r/pulls?state=closed&page=1", &[], b"")),
            "[\"closed\"]"
        );
        assert_eq!(body(f.answer("POST", "/x", &[], b"")), "\"once\"");
        assert_eq!(body(f.answer("POST", "/x", &[], b"")), "\"after\"");
        assert_eq!(f.answer("GET", "/nothing", &[], b"").status, 404);
        assert_eq!(f.misses(), 1);
        assert_eq!(f.count(), 6);
    }

    #[test]
    fn scrubbing_removes_personal_data_and_tokens() {
        let v = json!({
            "user": {"login": "alice", "email": "alice@corp.example", "full_name": "Alice A", "avatar_url": "https://x/a.png"},
            "body": "token ghp_abcdefghijklmnopqrstuvwxyz0123456789 and mail bob@corp.example here",
            "commit": {"author": {"name": "Alice", "email": "alice@corp.example"}},
        });
        let s = scrub(v).to_string();
        assert!(!s.contains("corp.example"), "{s}");
        assert!(!s.contains("ghp_"), "{s}");
        assert!(!s.contains("Alice A"), "{s}");
        assert!(s.contains("\"login\":\"alice\""), "{s}");
    }

    #[test]
    fn an_etag_answers_304() {
        let f = Fixtures::new();
        f.push(Exchange::json("GET", "/x", 200, json!([1])).with_header("ETag", "\"v1\""));
        let h = vec![("If-None-Match".to_owned(), "\"v1\"".to_owned())];
        assert_eq!(f.answer("GET", "/x", &h, b"").status, 304);
        assert_eq!(f.answer("GET", "/x", &[], b"").status, 200);
    }

    #[test]
    fn the_server_answers_over_http_with_the_real_transport() {
        let f = Fixtures::new();
        f.push(
            Exchange::json(
                "GET",
                "/api/v1/version",
                200,
                json!({"version": "1.0", "self": "{{base}}/x"}),
            )
            .with_header("ETag", "\"e\""),
        );
        let server = FixtureServer::start(f).unwrap();
        let t = crate::http::UreqTransport::default();
        let r = t
            .send(
                &Request::new(Method::Get, format!("{}/api/v1/version", server.base())),
                &Cancel::new(),
            )
            .unwrap();
        assert_eq!(r.status, 200);
        let v = r.json().unwrap();
        assert_eq!(v["self"], format!("{}/x", server.base()));
        let r = t
            .send(
                &Request::new(Method::Get, format!("{}/api/v1/version", server.base()))
                    .header("If-None-Match", "\"e\""),
                &Cancel::new(),
            )
            .unwrap();
        assert_eq!(r.status, 304);
        assert_eq!(server.fixtures.count(), 2);
    }
}
