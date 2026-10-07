//! One tab's state, fed by its CDP events on a pump thread: the page generation, refs, the console and network
//! rings, and what navigation waits look at (lifecycle events per loader, the main document's status, requests in
//! flight).
//!
//! - **Page generation** grows on `Page.frameNavigated` of the main frame and on `DOM.documentUpdated`.
//! - **Refs** (`e1`, `e2`, ...) map to CDP backend node ids and belong to the generation they were issued in; a ref
//!   from another generation is refused with "stale ref: the page changed (generation N, ref from M); call
//!   read_page again". Numbers never repeat in a tab, so only the current generation's map is kept, and the
//!   generation of an older ref is found from where each generation's numbers started.
//! - **Rings**: console messages (`Runtime.consoleAPICalled`, `Runtime.exceptionThrown`, `Log.entryAdded`, 2,000
//!   per tab) and network requests (`Network.requestWillBeSent`, `responseReceived`, `loadingFinished`,
//!   `loadingFailed`, 5,000 per tab).
//!
//! Events are read field by field rather than through the generated event types: a Chrome older or newer than the
//! pinned protocol may omit a member the pin marks required, and a dropped event would corrupt the state.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::sync::{Condvar, Mutex};
use std::time::Instant;

use eludite_commands::browser::{
    ConsoleError, ConsoleLevel, ConsoleMessage, InitiatorRow, NetworkRequestRow,
};
use serde_json::Value;

use crate::connection::CdpEvent;
use crate::ring::Ring;

pub const CONSOLE_RING: usize = 2000;
pub const NETWORK_RING: usize = 5000;
/// Loaders whose lifecycle is remembered.
const LOADERS: usize = 32;

/// A ref that cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RefError {
    Stale { current: u64, issued: u64 },
    Unknown(String),
}

impl std::fmt::Display for RefError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RefError::Stale { current, issued } => write!(
                f,
                "stale ref: the page changed (generation {current}, ref from {issued}); call read_page again"
            ),
            RefError::Unknown(r) => write!(
                f,
                "unknown ref `{r}`: refs come from eludite.browser.read_page and find on this tab"
            ),
        }
    }
}

/// Refs of one tab.
#[derive(Debug, Default)]
pub struct Refs {
    next: u64,
    /// The current generation's refs: number to backend node id, and back.
    by_number: HashMap<u64, i64>,
    by_node: HashMap<i64, u64>,
    /// (first number, generation) for each generation that issued refs, oldest first.
    starts: Vec<(u64, u64)>,
}

impl Refs {
    /// The ref of `backend_node_id` in `generation` (the same node keeps its ref within a generation).
    pub fn issue(&mut self, backend_node_id: i64, generation: u64) -> String {
        if self.starts.last().is_none_or(|(_, g)| *g != generation) {
            self.by_number.clear();
            self.by_node.clear();
            self.starts.push((self.next + 1, generation));
        }
        if let Some(n) = self.by_node.get(&backend_node_id) {
            return format!("e{n}");
        }
        self.next += 1;
        self.by_number.insert(self.next, backend_node_id);
        self.by_node.insert(backend_node_id, self.next);
        format!("e{}", self.next)
    }

    /// The backend node id of `r`, if it is from `generation`.
    pub fn resolve(&self, r: &str, generation: u64) -> Result<i64, RefError> {
        let n: u64 = r
            .strip_prefix('e')
            .and_then(|n| n.parse().ok())
            .ok_or_else(|| RefError::Unknown(r.to_owned()))?;
        if n == 0 || n > self.next {
            return Err(RefError::Unknown(r.to_owned()));
        }
        let issued = self
            .starts
            .iter()
            .rev()
            .find(|(start, _)| *start <= n)
            .map(|(_, g)| *g)
            .ok_or_else(|| RefError::Unknown(r.to_owned()))?;
        if issued != generation {
            return Err(RefError::Stale {
                current: generation,
                issued,
            });
        }
        self.by_number
            .get(&n)
            .copied()
            .ok_or_else(|| RefError::Unknown(r.to_owned()))
    }
}

/// A console entry in the ring.
pub type ConsoleEntry = ConsoleMessage;

/// A network entry in the ring, with the monotonic start time for its duration and the response headers
/// (`eludite.browser.network_body`, brief 0024).
#[derive(Debug, Clone, PartialEq)]
pub struct NetworkEntry {
    pub row: NetworkRequestRow,
    /// `Network.MonotonicTime` of the request, in seconds.
    pub started: f64,
    /// The response headers, once the response arrived (at most [`HEADERS`] of them).
    pub headers: BTreeMap<String, String>,
}

/// Response headers kept per request.
pub const HEADERS: usize = 64;

/// Lifecycle events seen for one loader of the main frame.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Lifecycle {
    pub dom_content_loaded: bool,
    pub load: bool,
}

#[derive(Debug)]
pub struct TabState {
    pub id: String,
    pub page_generation: u64,
    pub main_frame: Option<String>,
    /// The main frame's current loader and url (from `Page.frameNavigated`).
    pub main_loader: Option<String>,
    pub url: String,
    pub loading: bool,
    /// Main-frame navigations seen (cross-document and same-document).
    pub navigations: u64,
    pub lifecycle: VecDeque<(String, Lifecycle)>,
    /// The main document's HTTP status by loader id (`requestId == loaderId` for navigations).
    pub document_status: VecDeque<(String, u16)>,
    pub refs: Refs,
    pub console: Ring<ConsoleEntry>,
    pub network: Ring<NetworkEntry>,
    /// Ring seq of each request id's latest entry.
    request_seq: HashMap<String, u64>,
    /// Requests sent and not answered yet. A request leaves this set at its response headers, not at
    /// `loadingFinished`: Chrome reports a fetch whose body the page never reads as loading forever, which would
    /// keep `network_idle` from ever holding. Body data still arriving counts as activity (`Network.dataReceived`).
    pub in_flight: HashSet<String>,
    /// The loader of each request in flight: a main-frame navigation forgets the old page's requests, which Chrome
    /// may never finish or fail (a favicon request cut off by the navigation).
    flight_loader: HashMap<String, String>,
    pub last_network_activity: Instant,
    /// The tab's target went away (closed or crashed).
    pub gone: bool,
    /// A JavaScript dialog the page waits on, as CDP reported it (`Page.javascriptDialogOpening`; brief 0032).
    pub dialog: Option<crate::engine::PendingDialog>,
}

impl TabState {
    pub fn new(id: &str) -> Self {
        Self {
            id: id.to_owned(),
            page_generation: 1,
            main_frame: None,
            main_loader: None,
            url: String::new(),
            loading: false,
            navigations: 0,
            lifecycle: VecDeque::new(),
            document_status: VecDeque::new(),
            refs: Refs::default(),
            console: Ring::new(CONSOLE_RING),
            network: Ring::new(NETWORK_RING),
            request_seq: HashMap::new(),
            in_flight: HashSet::new(),
            flight_loader: HashMap::new(),
            last_network_activity: Instant::now(),
            gone: false,
            dialog: None,
        }
    }

    pub fn lifecycle_of(&self, loader: &str) -> Lifecycle {
        self.lifecycle
            .iter()
            .find(|(l, _)| l == loader)
            .map(|(_, s)| s.clone())
            .unwrap_or_default()
    }

    pub fn status_of(&self, loader: &str) -> Option<u16> {
        self.document_status
            .iter()
            .find(|(l, _)| l == loader)
            .map(|(_, s)| *s)
    }

    fn lifecycle_mut(&mut self, loader: &str) -> &mut Lifecycle {
        if let Some(i) = self.lifecycle.iter().position(|(l, _)| l == loader) {
            return &mut self.lifecycle[i].1;
        }
        if self.lifecycle.len() == LOADERS {
            self.lifecycle.pop_front();
        }
        self.lifecycle
            .push_back((loader.to_owned(), Lifecycle::default()));
        &mut self.lifecycle.back_mut().expect("just pushed").1
    }

    fn is_main(&self, frame: Option<&str>) -> bool {
        match (frame, &self.main_frame) {
            (Some(f), Some(m)) => f == m,
            // Before the frame tree is known, take what comes.
            (Some(_), None) => true,
            (None, _) => false,
        }
    }

    /// Errors logged since `seq`.
    pub fn console_errors_since(&self, seq: u64) -> u64 {
        self.console
            .since(seq)
            .filter(|(_, m)| m.level == ConsoleLevel::Error)
            .count() as u64
    }

    /// The errors logged since `seq`, oldest first (an action's `console_errors`).
    pub fn console_error_list_since(&self, seq: u64) -> Vec<ConsoleError> {
        self.console
            .since(seq)
            .filter(|(_, m)| m.level == ConsoleLevel::Error)
            .map(|(_, m)| ConsoleError::from(m))
            .collect()
    }

    /// The ring entry of request `request_id` (its latest hop).
    pub fn request(&self, request_id: &str) -> Option<&NetworkEntry> {
        let seq = *self.request_seq.get(request_id)?;
        self.network.get(seq)
    }

    /// Apply one event. Answers a line for the Output window when the event is a console error.
    pub fn apply(&mut self, e: &CdpEvent) -> Option<String> {
        let p = &e.params;
        match e.method.as_str() {
            "Page.frameNavigated" => {
                let frame = &p["frame"];
                if frame.get("parentId").is_none() {
                    self.main_frame = frame["id"].as_str().map(str::to_owned);
                    self.main_loader = frame["loaderId"].as_str().map(str::to_owned);
                    self.url = frame["url"].as_str().unwrap_or_default().to_owned()
                        + frame["urlFragment"].as_str().unwrap_or_default();
                    self.page_generation += 1;
                    self.navigations += 1;
                    if let Some(loader) = self.main_loader.clone() {
                        let old: Vec<String> = self
                            .in_flight
                            .iter()
                            .filter(|id| self.flight_loader.get(*id).is_some_and(|l| *l != loader))
                            .cloned()
                            .collect();
                        for id in old {
                            self.in_flight.remove(&id);
                            self.flight_loader.remove(&id);
                        }
                    }
                }
            }
            "Page.navigatedWithinDocument" => {
                if self.is_main(p["frameId"].as_str()) {
                    self.url = p["url"].as_str().unwrap_or_default().to_owned();
                    self.navigations += 1;
                }
            }
            "DOM.documentUpdated" => self.page_generation += 1,
            "Page.lifecycleEvent" => {
                if self.is_main(p["frameId"].as_str())
                    && let Some(loader) = p["loaderId"].as_str()
                {
                    let name = p["name"].as_str().unwrap_or_default();
                    let l = self.lifecycle_mut(loader);
                    match name {
                        "DOMContentLoaded" => l.dom_content_loaded = true,
                        "load" => l.load = true,
                        _ => {}
                    }
                }
            }
            "Page.javascriptDialogOpening" => {
                let kind = p["type"].as_str().unwrap_or("alert").to_owned();
                // Chrome sends `defaultPrompt: ""` for every kind; only a prompt has default text (as the embedded
                // engine's own `tab/dialog` says it, whichever of the two reports the dialog first).
                let default_text = (kind == "prompt")
                    .then(|| p["defaultPrompt"].as_str().map(str::to_owned))
                    .flatten();
                self.dialog = Some(crate::engine::PendingDialog {
                    id: 0,
                    kind,
                    message: p["message"].as_str().unwrap_or_default().to_owned(),
                    default_text,
                });
            }
            "Page.javascriptDialogClosed" => self.dialog = None,
            "Page.frameStartedLoading" => {
                if self.is_main(p["frameId"].as_str()) {
                    self.loading = true;
                }
            }
            "Page.frameStoppedLoading" => {
                if self.is_main(p["frameId"].as_str()) {
                    self.loading = false;
                }
            }
            "Runtime.consoleAPICalled" => return self.push_console(console_api_called(p)),
            "Runtime.exceptionThrown" => return self.push_console(exception_thrown(p)),
            "Log.entryAdded" => return self.push_console(log_entry(&p["entry"])),
            "Network.requestWillBeSent" => self.request_will_be_sent(p),
            "Network.responseReceived" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_owned();
                let r = &p["response"];
                let status = r["status"].as_f64().map(|s| s as u16);
                if p["type"] == "Document" && p["loaderId"].as_str() == Some(id.as_str()) {
                    if self.document_status.len() == LOADERS {
                        self.document_status.pop_front();
                    }
                    if let Some(s) = status {
                        self.document_status.push_back((id.clone(), s));
                    }
                }
                if let Some(entry) = self.entry(&id) {
                    entry.row.status = status;
                    entry.row.mime_type = r["mimeType"].as_str().map(str::to_owned);
                    entry.headers = r["headers"]
                        .as_object()
                        .into_iter()
                        .flatten()
                        .take(HEADERS)
                        .map(|(k, v)| {
                            (
                                k.clone(),
                                v.as_str().map_or_else(|| v.to_string(), str::to_owned),
                            )
                        })
                        .collect();
                    if let Some(t) = p["type"].as_str() {
                        entry.row.resource_type = t.to_owned();
                    }
                }
                self.in_flight.remove(&id);
                self.flight_loader.remove(&id);
                self.last_network_activity = Instant::now();
            }
            "Network.dataReceived" => self.last_network_activity = Instant::now(),
            "Network.loadingFinished" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_owned();
                let ts = p["timestamp"].as_f64();
                if let Some(entry) = self.entry(&id) {
                    entry.row.encoded_bytes = p["encodedDataLength"].as_f64().map(|b| b as u64);
                    if let Some(ts) = ts {
                        entry.row.duration_ms = Some(((ts - entry.started) * 1e3).max(0.));
                    }
                }
                self.in_flight.remove(&id);
                self.flight_loader.remove(&id);
                self.last_network_activity = Instant::now();
            }
            "Network.loadingFailed" => {
                let id = p["requestId"].as_str().unwrap_or_default().to_owned();
                let ts = p["timestamp"].as_f64();
                if let Some(entry) = self.entry(&id) {
                    let mut why = p["errorText"].as_str().unwrap_or("failed").to_owned();
                    if p["canceled"].as_bool() == Some(true) {
                        why.push_str(" (canceled)");
                    }
                    if let Some(reason) = p["blockedReason"].as_str() {
                        why.push_str(&format!(" (blocked: {reason})"));
                    }
                    entry.row.failed = Some(why);
                    if let Some(ts) = ts {
                        entry.row.duration_ms = Some(((ts - entry.started) * 1e3).max(0.));
                    }
                }
                self.in_flight.remove(&id);
                self.flight_loader.remove(&id);
                self.last_network_activity = Instant::now();
            }
            "Inspector.detached" | "Inspector.targetCrashed" => self.gone = true,
            _ => {}
        }
        None
    }

    fn entry(&mut self, request_id: &str) -> Option<&mut NetworkEntry> {
        let seq = *self.request_seq.get(request_id)?;
        self.network.get_mut(seq)
    }

    fn request_will_be_sent(&mut self, p: &Value) {
        let id = p["requestId"].as_str().unwrap_or_default().to_owned();
        let ts = p["timestamp"].as_f64().unwrap_or_default();
        // A redirect ends the previous hop of the same request id.
        if let Some(redirect) = p.get("redirectResponse")
            && let Some(prev) = self.entry(&id)
        {
            prev.row.status = redirect["status"].as_f64().map(|s| s as u16);
            prev.row.mime_type = redirect["mimeType"].as_str().map(str::to_owned);
            prev.row.duration_ms = Some(((ts - prev.started) * 1e3).max(0.));
        }
        let request = &p["request"];
        let initiator = &p["initiator"];
        let initiator_url = initiator["url"].as_str().map(str::to_owned).or_else(|| {
            initiator["stack"]["callFrames"][0]["url"]
                .as_str()
                .filter(|u| !u.is_empty())
                .map(str::to_owned)
        });
        let row = NetworkRequestRow {
            seq: 0,
            request_id: id.clone(),
            method: request["method"].as_str().unwrap_or("GET").to_owned(),
            url: request["url"].as_str().unwrap_or_default().to_owned()
                + request["urlFragment"].as_str().unwrap_or_default(),
            status: None,
            resource_type: p["type"].as_str().unwrap_or("Other").to_owned(),
            mime_type: None,
            started_at: p["wallTime"].as_f64().map_or(0., |w| w * 1e3),
            duration_ms: None,
            encoded_bytes: None,
            failed: None,
            initiator: InitiatorRow {
                type_: initiator["type"].as_str().unwrap_or("other").to_owned(),
                url: initiator_url,
            },
        };
        let seq = self.network.push(NetworkEntry {
            row,
            started: ts,
            headers: BTreeMap::new(),
        });
        if let Some(e) = self.network.get_mut(seq) {
            e.row.seq = seq;
        }
        self.request_seq.insert(id.clone(), seq);
        if self.request_seq.len() > NETWORK_RING * 2 {
            let floor = self.network.last_seq().saturating_sub(NETWORK_RING as u64);
            self.request_seq.retain(|_, s| *s > floor);
        }
        if let Some(loader) = p["loaderId"].as_str() {
            self.flight_loader.insert(id.clone(), loader.to_owned());
        }
        self.in_flight.insert(id);
        self.last_network_activity = Instant::now();
    }

    fn push_console(&mut self, m: ConsoleMessage) -> Option<String> {
        let line = (m.level == ConsoleLevel::Error).then(|| {
            let at = match (&m.url, m.line) {
                (Some(u), Some(l)) if !u.is_empty() => format!("{u}:{l}"),
                (Some(u), None) if !u.is_empty() => u.clone(),
                _ => self.url.clone(),
            };
            format!("[{}] {at}: {}", self.id, m.text)
        });
        let seq = self.console.push(m);
        if let Some(e) = self.console.get_mut(seq) {
            e.seq = seq;
        }
        line
    }
}

/// A tab's state shared between its pump thread and the command thread.
#[derive(Debug)]
pub struct Shared {
    pub state: Mutex<TabState>,
    pub changed: Condvar,
}

impl Shared {
    pub fn new(id: &str) -> Self {
        Self {
            state: Mutex::new(TabState::new(id)),
            changed: Condvar::new(),
        }
    }

    pub fn lock(&self) -> std::sync::MutexGuard<'_, TabState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }
}

/// A `Runtime.RemoteObject` as console text: strings as they are, primitives as JSON, objects by description.
pub fn remote_object_text(o: &Value) -> String {
    if let Some(s) = o["value"].as_str() {
        return s.to_owned();
    }
    if let Some(u) = o["unserializableValue"].as_str() {
        return u.to_owned();
    }
    match o["type"].as_str() {
        Some("undefined") => "undefined".into(),
        _ if o
            .get("value")
            .is_some_and(|v| !v.is_object() && !v.is_array()) =>
        {
            o["value"].to_string()
        }
        _ => o["description"]
            .as_str()
            .or(o["className"].as_str())
            .unwrap_or("Object")
            .to_owned(),
    }
}

/// `Runtime.StackTrace` as `at f (url:line:column)` lines (1-based).
pub fn stack_text(st: &Value) -> Option<String> {
    let frames = st["callFrames"].as_array()?;
    if frames.is_empty() {
        return None;
    }
    Some(
        frames
            .iter()
            .map(|f| {
                let name = f["functionName"]
                    .as_str()
                    .filter(|n| !n.is_empty())
                    .unwrap_or("<anonymous>");
                format!(
                    "at {name} ({}:{}:{})",
                    f["url"].as_str().unwrap_or_default(),
                    f["lineNumber"].as_i64().unwrap_or(0) + 1,
                    f["columnNumber"].as_i64().unwrap_or(0) + 1
                )
            })
            .collect::<Vec<_>>()
            .join("\n"),
    )
}

fn top_frame(st: &Value) -> (Option<String>, Option<u32>, Option<u32>) {
    let f = &st["callFrames"][0];
    (
        f["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned),
        f["lineNumber"].as_i64().map(|l| (l + 1) as u32),
        f["columnNumber"].as_i64().map(|c| (c + 1) as u32),
    )
}

fn console_api_called(p: &Value) -> ConsoleMessage {
    let level = match p["type"].as_str().unwrap_or("log") {
        "error" | "assert" => ConsoleLevel::Error,
        "warning" => ConsoleLevel::Warning,
        "info" => ConsoleLevel::Info,
        "debug" => ConsoleLevel::Debug,
        _ => ConsoleLevel::Log,
    };
    let text = p["args"]
        .as_array()
        .map(|a| {
            a.iter()
                .map(remote_object_text)
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    let (url, line, column) = top_frame(&p["stackTrace"]);
    ConsoleMessage {
        seq: 0,
        level,
        source: "console".into(),
        text,
        url,
        line,
        column,
        stack: (level == ConsoleLevel::Error)
            .then(|| stack_text(&p["stackTrace"]))
            .flatten(),
        timestamp: p["timestamp"].as_f64().unwrap_or_default(),
    }
}

fn exception_thrown(p: &Value) -> ConsoleMessage {
    let d = &p["exceptionDetails"];
    let description = d["exception"]["description"].as_str().unwrap_or_default();
    let first = description.lines().next().unwrap_or_default();
    let text = match d["text"].as_str().unwrap_or("Uncaught") {
        t if first.is_empty() => t.to_owned(),
        t => format!("{t} {first}"),
    };
    let (mut url, mut line, mut column) = top_frame(&d["stackTrace"]);
    if url.is_none() {
        url = d["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned);
        line = d["lineNumber"].as_i64().map(|l| (l + 1) as u32);
        column = d["columnNumber"].as_i64().map(|c| (c + 1) as u32);
    }
    let stack = stack_text(&d["stackTrace"]).or_else(|| {
        let rest: Vec<&str> = description.lines().skip(1).map(str::trim).collect();
        (!rest.is_empty()).then(|| rest.join("\n"))
    });
    ConsoleMessage {
        seq: 0,
        level: ConsoleLevel::Error,
        source: "exception".into(),
        text,
        url,
        line,
        column,
        stack,
        timestamp: p["timestamp"].as_f64().unwrap_or_default(),
    }
}

fn log_entry(e: &Value) -> ConsoleMessage {
    let level = match e["level"].as_str().unwrap_or("info") {
        "error" => ConsoleLevel::Error,
        "warning" => ConsoleLevel::Warning,
        "verbose" => ConsoleLevel::Debug,
        _ => ConsoleLevel::Info,
    };
    let (st_url, st_line, st_col) = top_frame(&e["stackTrace"]);
    ConsoleMessage {
        seq: 0,
        level,
        source: e["source"].as_str().unwrap_or("other").to_owned(),
        text: e["text"].as_str().unwrap_or_default().to_owned(),
        url: e["url"]
            .as_str()
            .filter(|u| !u.is_empty())
            .map(str::to_owned)
            .or(st_url),
        line: e["lineNumber"].as_i64().map(|l| (l + 1) as u32).or(st_line),
        column: st_col,
        stack: (level == ConsoleLevel::Error)
            .then(|| stack_text(&e["stackTrace"]))
            .flatten(),
        timestamp: e["timestamp"].as_f64().unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn ev(method: &str, params: Value) -> CdpEvent {
        CdpEvent {
            method: method.into(),
            params,
            session_id: Some("S".into()),
        }
    }

    #[test]
    fn refs_belong_to_their_generation() {
        let mut t = TabState::new("t1");
        let g1 = t.page_generation;
        let a = t.refs.issue(101, g1);
        let b = t.refs.issue(102, g1);
        assert_eq!((a.as_str(), b.as_str()), ("e1", "e2"));
        assert_eq!(t.refs.issue(101, g1), "e1", "the same node keeps its ref");
        assert_eq!(t.refs.resolve("e2", g1), Ok(102));
        // The main frame navigates: generation 2.
        t.apply(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "F", "loaderId": "L2", "url": "http://x/2"}}),
        ));
        let g2 = t.page_generation;
        assert_eq!(g2, g1 + 1);
        let err = t.refs.resolve("e1", g2).unwrap_err();
        assert_eq!(
            err,
            RefError::Stale {
                current: 2,
                issued: 1
            }
        );
        assert_eq!(
            err.to_string(),
            "stale ref: the page changed (generation 2, ref from 1); call read_page again"
        );
        // New refs in generation 2 never reuse a number.
        assert_eq!(t.refs.issue(101, g2), "e3");
        assert_eq!(t.refs.resolve("e3", g2), Ok(101));
        assert!(matches!(
            t.refs.resolve("e9", g2),
            Err(RefError::Unknown(_))
        ));
        assert!(matches!(t.refs.resolve("x", g2), Err(RefError::Unknown(_))));
        // A replaced document (DOM.documentUpdated) changes the generation too; a child frame's navigation does not.
        t.apply(&ev("DOM.documentUpdated", json!({})));
        assert_eq!(t.page_generation, g2 + 1);
        t.apply(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "C", "parentId": "F", "loaderId": "L9", "url": "http://x/frame"}}),
        ));
        assert_eq!(t.page_generation, g2 + 1);
        assert_eq!(t.url, "http://x/2");
        assert!(matches!(
            t.refs.resolve("e3", t.page_generation),
            Err(RefError::Stale { issued: 2, .. })
        ));
    }

    #[test]
    fn only_a_prompt_has_default_text() {
        let mut t = TabState::new("t1");
        t.apply(&ev(
            "Page.javascriptDialogOpening",
            json!({"type": "confirm", "message": "Delete it?", "defaultPrompt": ""}),
        ));
        let d = t.dialog.clone().unwrap();
        assert_eq!((d.kind.as_str(), d.default_text), ("confirm", None));
        t.apply(&ev(
            "Page.javascriptDialogOpening",
            json!({"type": "prompt", "message": "Name?", "defaultPrompt": ""}),
        ));
        assert_eq!(t.dialog.clone().unwrap().default_text.as_deref(), Some(""));
    }

    #[test]
    fn console_events_fill_the_ring() {
        let mut t = TabState::new("t1");
        t.url = "http://127.0.0.1/errors.html".into();
        let line = t.apply(&ev(
            "Runtime.consoleAPICalled",
            json!({
                "type": "error",
                "args": [{"type": "string", "value": "boom"}, {"type": "number", "value": 42}],
                "executionContextId": 1,
                "timestamp": 1000.0,
                "stackTrace": {"callFrames": [{"functionName": "", "scriptId": "1", "url": "http://127.0.0.1/errors.html", "lineNumber": 9, "columnNumber": 14}]}
            }),
        ));
        assert_eq!(
            line.as_deref(),
            Some("[t1] http://127.0.0.1/errors.html:10: boom 42")
        );
        assert!(
            t.apply(&ev(
                "Runtime.consoleAPICalled",
                json!({"type": "log", "args": [{"type": "undefined"}], "timestamp": 1.0})
            ))
            .is_none()
        );
        t.apply(&ev(
            "Runtime.exceptionThrown",
            json!({"timestamp": 2.0, "exceptionDetails": {"exceptionId": 1, "text": "Uncaught", "lineNumber": 3, "columnNumber": 5, "url": "http://127.0.0.1/a.js",
                "exception": {"type": "object", "subtype": "error", "description": "TypeError: x is not a function\n    at f (http://127.0.0.1/a.js:4:6)"}}}),
        ));
        t.apply(&ev(
            "Log.entryAdded",
            json!({"entry": {"source": "network", "level": "error", "text": "Failed to load resource: the server responded with a status of 404 (Not Found)", "url": "http://127.0.0.1/missing.json", "timestamp": 3.0}}),
        ));
        let p = t.console.read(0, 10, |_| true);
        let msgs: Vec<_> = p.items.iter().map(|(_, m)| m.clone()).collect();
        assert_eq!(msgs.len(), 4);
        assert_eq!(msgs[0].seq, 1);
        assert_eq!((msgs[0].line, msgs[0].column), (Some(10), Some(15)));
        assert_eq!(
            msgs[0].stack.as_deref(),
            Some("at <anonymous> (http://127.0.0.1/errors.html:10:15)")
        );
        assert_eq!(msgs[1].level, ConsoleLevel::Log);
        assert_eq!(msgs[1].text, "undefined");
        assert_eq!(msgs[2].text, "Uncaught TypeError: x is not a function");
        assert_eq!(
            (msgs[2].url.as_deref(), msgs[2].line),
            (Some("http://127.0.0.1/a.js"), Some(4))
        );
        assert_eq!(
            msgs[2].stack.as_deref(),
            Some("at f (http://127.0.0.1/a.js:4:6)")
        );
        assert_eq!(msgs[3].source, "network");
        assert_eq!(t.console_errors_since(0), 3);
        assert_eq!(t.console_errors_since(2), 2);
        // An action's console_errors: the errors since it began, with their locations.
        let errors = t.console_error_list_since(2);
        assert_eq!(errors.len(), 2);
        assert_eq!(errors[0].text, "Uncaught TypeError: x is not a function");
        assert_eq!(errors[0].source, "exception");
        assert_eq!(
            (errors[0].url.as_deref(), errors[0].line),
            (Some("http://127.0.0.1/a.js"), Some(4))
        );
    }

    #[test]
    fn network_events_fill_the_ring_and_track_flight() {
        let mut t = TabState::new("t1");
        t.apply(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "F", "loaderId": "L1", "url": "http://127.0.0.1/"}}),
        ));
        t.apply(&ev(
            "Network.requestWillBeSent",
            json!({"requestId": "L1", "loaderId": "L1", "type": "Document", "timestamp": 10.0, "wallTime": 1759446000.0,
                "request": {"url": "http://127.0.0.1/", "method": "GET"}, "initiator": {"type": "other"}}),
        ));
        t.apply(&ev(
            "Network.responseReceived",
            json!({"requestId": "L1", "loaderId": "L1", "type": "Document", "timestamp": 10.01,
                "response": {"status": 200, "mimeType": "text/html", "url": "http://127.0.0.1/"}}),
        ));
        assert_eq!(t.status_of("L1"), Some(200));
        // Answered: no longer in flight, though its body may still be loading.
        assert!(t.in_flight.is_empty());
        t.apply(&ev(
            "Network.loadingFinished",
            json!({"requestId": "L1", "timestamp": 10.02, "encodedDataLength": 512}),
        ));
        assert!(t.in_flight.is_empty());
        t.apply(&ev(
            "Network.requestWillBeSent",
            json!({"requestId": "7.2", "loaderId": "L1", "type": "Fetch", "timestamp": 11.0, "wallTime": 1759446001.0,
                "request": {"url": "http://127.0.0.1/missing.json", "method": "GET"},
                "initiator": {"type": "script", "stack": {"callFrames": [{"url": "http://127.0.0.1/", "lineNumber": 1, "columnNumber": 1, "functionName": "", "scriptId": "3"}]}}}),
        ));
        t.apply(&ev(
            "Network.responseReceived",
            json!({"requestId": "7.2", "type": "Fetch", "timestamp": 11.003, "response": {"status": 404, "mimeType": "text/plain",
                "headers": {"Content-Type": "text/plain", "Content-Length": 9}}}),
        ));
        // network_body reads the request's entry and its response headers (brief 0024).
        let e = t.request("7.2").unwrap();
        assert_eq!(e.row.url, "http://127.0.0.1/missing.json");
        assert_eq!(e.headers["Content-Type"], "text/plain");
        assert_eq!(e.headers["Content-Length"], "9");
        assert!(t.request("nope").is_none());
        t.apply(&ev(
            "Network.requestWillBeSent",
            json!({"requestId": "7.3", "type": "Image", "timestamp": 12.0, "wallTime": 1759446002.0, "request": {"url": "http://127.0.0.1/x.png", "method": "GET"}, "initiator": {"type": "parser", "url": "http://127.0.0.1/"}}),
        ));
        t.apply(&ev(
            "Network.loadingFailed",
            json!({"requestId": "7.3", "timestamp": 12.5, "type": "Image", "errorText": "net::ERR_ABORTED", "canceled": true}),
        ));
        let rows: Vec<_> = t
            .network
            .read(0, 10, |_| true)
            .items
            .into_iter()
            .map(|(_, e)| e.row)
            .collect();
        assert_eq!(rows.len(), 3);
        assert_eq!(rows[0].status, Some(200));
        assert_eq!(rows[0].encoded_bytes, Some(512));
        assert!((rows[0].duration_ms.unwrap() - 20.).abs() < 0.01);
        assert_eq!(rows[0].started_at, 1759446000000.);
        assert_eq!(rows[1].status, Some(404));
        assert_eq!(rows[1].resource_type, "Fetch");
        assert_eq!(rows[1].initiator.url.as_deref(), Some("http://127.0.0.1/"));
        assert!(
            t.in_flight.is_empty(),
            "a fetch whose body is never read is not in flight once answered"
        );
        t.apply(&ev(
            "Network.requestWillBeSent",
            json!({"requestId": "7.4", "type": "XHR", "timestamp": 13.0, "request": {"url": "http://127.0.0.1/slow", "method": "POST"}, "initiator": {"type": "script"}}),
        ));
        assert!(t.in_flight.contains("7.4"));
        // A request of the page navigated away from is forgotten: Chrome may never finish it.
        t.apply(&ev(
            "Network.requestWillBeSent",
            json!({"requestId": "7.5", "loaderId": "L1", "type": "Other", "timestamp": 14.0, "request": {"url": "http://127.0.0.1/favicon.ico", "method": "GET"}, "initiator": {"type": "other"}}),
        ));
        t.apply(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "F", "loaderId": "L2", "url": "http://127.0.0.1/next"}}),
        ));
        assert!(!t.in_flight.contains("7.5"));
        assert_eq!(
            rows[2].failed.as_deref(),
            Some("net::ERR_ABORTED (canceled)")
        );
        assert_eq!(rows[2].seq, 3);
    }

    #[test]
    fn lifecycle_per_loader_of_the_main_frame() {
        let mut t = TabState::new("t1");
        t.apply(&ev(
            "Page.frameNavigated",
            json!({"frame": {"id": "F", "loaderId": "L1", "url": "about:blank"}}),
        ));
        t.apply(&ev(
            "Page.lifecycleEvent",
            json!({"frameId": "F", "loaderId": "L1", "name": "DOMContentLoaded", "timestamp": 1.0}),
        ));
        t.apply(&ev(
            "Page.lifecycleEvent",
            json!({"frameId": "C", "loaderId": "L1", "name": "load", "timestamp": 1.0}),
        ));
        assert_eq!(
            t.lifecycle_of("L1"),
            Lifecycle {
                dom_content_loaded: true,
                load: false
            }
        );
        t.apply(&ev(
            "Page.lifecycleEvent",
            json!({"frameId": "F", "loaderId": "L1", "name": "load", "timestamp": 1.0}),
        ));
        assert!(t.lifecycle_of("L1").load);
        assert_eq!(t.lifecycle_of("L2"), Lifecycle::default());
        t.apply(&ev("Page.frameStartedLoading", json!({"frameId": "F"})));
        assert!(t.loading);
        t.apply(&ev("Page.frameStoppedLoading", json!({"frameId": "F"})));
        assert!(!t.loading);
        t.apply(&ev(
            "Page.navigatedWithinDocument",
            json!({"frameId": "F", "url": "about:blank#x"}),
        ));
        assert_eq!((t.url.as_str(), t.navigations), ("about:blank#x", 2));
    }
}
