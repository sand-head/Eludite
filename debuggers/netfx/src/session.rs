//! The DAP session: the read loop, the engine loop, and the [`Debugger`] trait the ICorDebug engine implements.
//!
//! Threading (brief 0004): the connection's read loop only parses frames and forwards requests into a channel; it
//! never calls the debugger. One engine thread owns the [`Debugger`] and drains that channel, which also carries the
//! debugger's own events (on Windows, ICorDebug callbacks posted from mscordbi's callback thread). So requests and
//! callbacks are handled one at a time, in arrival order, on one thread, and no read ever waits on ICorDebug.

use std::io::{self, BufReader, Read, Write};
use std::net::{Shutdown, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::framing::{read_message, write_message};
use crate::protocol::{
    BreakpointResult, Command, Request, ScopeInfo, StackFrameInfo, ThreadInfo, VariableInfo,
    capabilities, parse_command, parse_request,
};

/// What the engine thread receives.
#[derive(Debug)]
pub enum Msg<E> {
    Request(Request),
    /// The client closed the connection (or it broke).
    Closed,
    /// An event from the debugger (on Windows, a marshalled ICorDebug callback).
    Debugger(E),
}

/// Events the debugger asks the session to send.
#[derive(Debug, Clone, PartialEq)]
pub enum DebugEvent {
    Stopped {
        reason: &'static str,
        thread_id: u32,
        hit_breakpoint_ids: Vec<i64>,
        /// When the debugger saw the stop (the callback's arrival), for the hit-to-`stopped` budget.
        observed_at: Option<Instant>,
    },
    BreakpointChanged(BreakpointResult),
    Exited {
        exit_code: i32,
    },
    Terminated,
    Output {
        text: String,
    },
}

/// The debugger behind the session. Every method runs on the engine thread.
pub trait Debugger {
    type Event: Send + 'static;
    fn attach(&mut self, process_id: u32) -> Result<(), String>;
    fn set_breakpoints(
        &mut self,
        source_path: &str,
        lines: &[u32],
    ) -> Result<Vec<BreakpointResult>, String>;
    fn configuration_done(&mut self) -> Result<(), String>;
    fn threads(&mut self) -> Result<Vec<ThreadInfo>, String>;
    fn stack_trace(&mut self, thread_id: u32) -> Result<Vec<StackFrameInfo>, String>;
    fn scopes(&mut self, frame_id: i64) -> Result<Vec<ScopeInfo>, String>;
    fn variables(&mut self, reference: i64) -> Result<Vec<VariableInfo>, String>;
    fn continue_all(&mut self) -> Result<(), String>;
    fn disconnect(&mut self, terminate_debuggee: bool) -> Result<(), String>;
    /// Handles one of its own events; returns the DAP events to send.
    fn on_event(&mut self, event: Self::Event) -> Vec<DebugEvent>;
}

/// Writes framed responses and events, numbering them.
pub struct Output {
    writer: Mutex<Box<dyn Write + Send>>,
    seq: Mutex<i64>,
}

impl Output {
    pub fn new(writer: Box<dyn Write + Send>) -> Self {
        Self {
            writer: Mutex::new(writer),
            seq: Mutex::new(0),
        }
    }

    fn send(&self, mut message: Value) {
        let mut writer = self.writer.lock().unwrap_or_else(|e| e.into_inner());
        let mut seq = self.seq.lock().unwrap_or_else(|e| e.into_inner());
        *seq += 1;
        message["seq"] = json!(*seq);
        let body = serde_json::to_vec(&message).expect("a JSON value serializes");
        if let Err(e) = write_message(&mut *writer, &body) {
            log(&format!("write failed: {e}"));
        }
    }

    pub fn respond(&self, request: &Request, result: Result<Value, String>) {
        let mut m = json!({
            "type": "response",
            "request_seq": request.seq,
            "command": request.command,
        });
        match result {
            Ok(body) => {
                m["success"] = json!(true);
                if !body.is_null() {
                    m["body"] = body;
                }
            }
            Err(message) => {
                m["success"] = json!(false);
                m["message"] = json!(message);
            }
        }
        self.send(m);
    }

    pub fn event(&self, event: &str, body: Value) {
        let mut m = json!({ "type": "event", "event": event });
        if !body.is_null() {
            m["body"] = body;
        }
        self.send(m);
    }
}

/// Writes one diagnostic line to stderr (never stdout).
pub fn log(message: &str) {
    eprintln!("eludite-dbg-netfx: {message}");
}

/// Dispatches requests to a debugger and writes the answers.
pub struct Session<D: Debugger> {
    debugger: D,
    out: Output,
}

impl<D: Debugger> Session<D> {
    pub fn new(debugger: D, out: Output) -> Self {
        Self { debugger, out }
    }

    /// Handles one request. Returns true when the session is over (`disconnect`).
    pub fn handle(&mut self, request: Request) -> bool {
        let command = match parse_command(&request) {
            Ok(c) => c,
            Err(e) => {
                self.out.respond(&request, Err(e));
                return false;
            }
        };
        match command {
            Command::Initialize => self.out.respond(&request, Ok(capabilities())),
            Command::Attach { process_id } => {
                let started = Instant::now();
                match self.debugger.attach(process_id) {
                    Ok(()) => {
                        self.out.respond(&request, Ok(Value::Null));
                        self.out.event("initialized", Value::Null);
                        log(&format!(
                            "attached to {process_id}; attach request to initialized event {:.1} ms",
                            ms(started)
                        ));
                    }
                    Err(e) => self.out.respond(&request, Err(e)),
                }
            }
            Command::SetBreakpoints { source_path, lines } => {
                let r = self
                    .debugger
                    .set_breakpoints(&source_path, &lines)
                    .map(|bps| {
                        json!({ "breakpoints": bps.iter().map(BreakpointResult::to_json).collect::<Vec<_>>() })
                    });
                self.out.respond(&request, r);
            }
            Command::ConfigurationDone => {
                let r = self.debugger.configuration_done().map(|()| Value::Null);
                self.out.respond(&request, r);
            }
            Command::Threads => {
                let r = self.debugger.threads().map(|ts| {
                    json!({ "threads": ts.iter().map(|t| json!({ "id": t.id, "name": t.name })).collect::<Vec<_>>() })
                });
                self.out.respond(&request, r);
            }
            Command::StackTrace {
                thread_id,
                start_frame,
                levels,
            } => {
                let r = self.debugger.stack_trace(thread_id).map(|frames| {
                    let total = frames.len();
                    let page: Vec<Value> = frames
                        .iter()
                        .skip(start_frame)
                        .take(levels.unwrap_or(usize::MAX))
                        .map(StackFrameInfo::to_json)
                        .collect();
                    json!({ "stackFrames": page, "totalFrames": total })
                });
                self.out.respond(&request, r);
            }
            Command::Scopes { frame_id } => {
                let r = self.debugger.scopes(frame_id).map(|ss| {
                    json!({ "scopes": ss.iter().map(|s| json!({
                        "name": s.name,
                        "presentationHint": "locals",
                        "variablesReference": s.reference,
                        "expensive": false,
                    })).collect::<Vec<_>>() })
                });
                self.out.respond(&request, r);
            }
            Command::Variables { reference } => {
                let r = self.debugger.variables(reference).map(|vs| {
                    json!({ "variables": vs.iter().map(VariableInfo::to_json).collect::<Vec<_>>() })
                });
                self.out.respond(&request, r);
            }
            Command::Continue { .. } => {
                let r = self
                    .debugger
                    .continue_all()
                    .map(|()| json!({ "allThreadsContinued": true }));
                self.out.respond(&request, r);
            }
            Command::Disconnect { terminate_debuggee } => {
                let r = self.debugger.disconnect(terminate_debuggee);
                if r.is_ok() {
                    self.out.event("terminated", Value::Null);
                }
                self.out.respond(&request, r.map(|()| Value::Null));
                return true;
            }
            Command::Unsupported(name) => self.out.respond(
                &request,
                Err(format!("{name} is not supported by eludite-dbg-netfx")),
            ),
        }
        false
    }

    /// Handles one debugger event. Returns true when the debuggee is gone.
    pub fn on_event(&mut self, event: D::Event) -> bool {
        let mut over = false;
        for e in self.debugger.on_event(event) {
            match e {
                DebugEvent::Stopped {
                    reason,
                    thread_id,
                    hit_breakpoint_ids,
                    observed_at,
                } => {
                    let mut body = json!({
                        "reason": reason,
                        "threadId": thread_id,
                        "allThreadsStopped": true,
                    });
                    if !hit_breakpoint_ids.is_empty() {
                        body["hitBreakpointIds"] = json!(hit_breakpoint_ids);
                    }
                    self.out.event("stopped", body);
                    if let Some(at) = observed_at {
                        log(&format!(
                            "stopped ({reason}) on thread {thread_id}; callback to stopped event {:.3} ms",
                            ms(at)
                        ));
                    }
                }
                DebugEvent::BreakpointChanged(bp) => self.out.event(
                    "breakpoint",
                    json!({ "reason": "changed", "breakpoint": bp.to_json() }),
                ),
                DebugEvent::Exited { exit_code } => {
                    self.out.event("exited", json!({ "exitCode": exit_code }))
                }
                DebugEvent::Terminated => {
                    self.out.event("terminated", Value::Null);
                    over = true;
                }
                DebugEvent::Output { text } => self
                    .out
                    .event("output", json!({ "category": "console", "output": text })),
            }
        }
        over
    }

    /// The client went away without `disconnect`: detach so the debuggee keeps running.
    pub fn closed(&mut self) {
        if let Err(e) = self.debugger.disconnect(false) {
            log(&format!("detach after the client closed failed: {e}"));
        }
    }
}

/// How often a blocked read wakes to check whether the session is over.
const READ_POLL: Duration = Duration::from_millis(100);

/// The socket's read half, ending (`Ok(0)`) once the session is over. On Windows, `shutdown` from another thread
/// does not wake a blocked `recv`, so the read loop polls with a timeout instead; a timeout is retried, never
/// surfaced, so a message split across timeouts still frames correctly.
struct SessionReader {
    stream: TcpStream,
    done: Arc<AtomicBool>,
}

impl Read for SessionReader {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        loop {
            if self.done.load(Ordering::Acquire) {
                return Ok(0);
            }
            match self.stream.read(buf) {
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) => {}
                r => return r,
            }
        }
    }
}

fn ms(since: Instant) -> f64 {
    since.elapsed().as_secs_f64() * 1000.0
}

/// Serves one DAP client on `stream`. `make` builds the debugger on the engine thread (ICorDebug objects never leave
/// it) and receives the sender its callbacks post to.
pub fn serve<D, F>(stream: TcpStream, make: F) -> io::Result<()>
where
    D: Debugger,
    F: FnOnce(Sender<Msg<D::Event>>) -> D + Send + 'static,
{
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_POLL))?;
    let writer = stream.try_clone()?;
    let closer = stream.try_clone()?;
    let done = Arc::new(AtomicBool::new(false));
    let engine_done = done.clone();
    let (tx, rx) = mpsc::channel::<Msg<D::Event>>();
    let engine_tx = tx.clone();
    let engine = std::thread::Builder::new()
        .name("dap-engine".into())
        .spawn(move || {
            let debugger = make(engine_tx);
            let mut session = Session::new(debugger, Output::new(Box::new(writer)));
            while let Ok(msg) = rx.recv() {
                match msg {
                    Msg::Request(r) => {
                        if session.handle(r) {
                            break;
                        }
                    }
                    Msg::Debugger(e) => {
                        // The debuggee exiting does not end the session: the client still disconnects.
                        session.on_event(e);
                    }
                    Msg::Closed => {
                        session.closed();
                        break;
                    }
                }
            }
            // End the read loop if the session ended first (disconnect).
            engine_done.store(true, Ordering::Release);
            let _ = closer.shutdown(Shutdown::Both);
        })?;

    let mut reader = BufReader::new(SessionReader { stream, done });
    loop {
        match read_message(&mut reader) {
            Ok(Some(body)) => match parse_request(&body) {
                Ok(r) => {
                    if tx.send(Msg::Request(r)).is_err() {
                        break;
                    }
                }
                Err(e) => log(&format!("ignored a message: {e}")),
            },
            Ok(None) => break,
            Err(e) => {
                if e.kind() != io::ErrorKind::UnexpectedEof {
                    log(&format!("read failed: {e}"));
                }
                break;
            }
        }
    }
    let _ = tx.send(Msg::Closed);
    drop(tx);
    let _ = engine.join();
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    #[derive(Clone, Default)]
    struct Sink(Arc<Mutex<Vec<u8>>>);
    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl Sink {
        fn messages(&self) -> Vec<Value> {
            let bytes = self.0.lock().unwrap().clone();
            let mut r = BufReader::new(&bytes[..]);
            let mut out = Vec::new();
            while let Some(b) = read_message(&mut r).unwrap() {
                out.push(serde_json::from_slice(&b).unwrap());
            }
            out
        }
    }

    #[derive(Default)]
    struct Fake {
        attached: Option<u32>,
        detached: bool,
    }
    impl Debugger for Fake {
        type Event = &'static str;
        fn attach(&mut self, pid: u32) -> Result<(), String> {
            if pid == 1 {
                return Err("no such process".into());
            }
            self.attached = Some(pid);
            Ok(())
        }
        fn set_breakpoints(
            &mut self,
            path: &str,
            lines: &[u32],
        ) -> Result<Vec<BreakpointResult>, String> {
            Ok(lines
                .iter()
                .enumerate()
                .map(|(i, &l)| BreakpointResult {
                    id: i as i64 + 1,
                    verified: false,
                    line: l,
                    source_path: path.into(),
                    message: Some("pending".into()),
                })
                .collect())
        }
        fn configuration_done(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn threads(&mut self) -> Result<Vec<ThreadInfo>, String> {
            Ok(vec![ThreadInfo {
                id: 7,
                name: "Main".into(),
            }])
        }
        fn stack_trace(&mut self, _: u32) -> Result<Vec<StackFrameInfo>, String> {
            Ok((1..=3)
                .map(|i| StackFrameInfo {
                    id: i,
                    name: format!("f{i}"),
                    source_path: None,
                    line: 0,
                    column: 0,
                })
                .collect())
        }
        fn scopes(&mut self, f: i64) -> Result<Vec<ScopeInfo>, String> {
            Ok(vec![ScopeInfo {
                name: "Locals".into(),
                reference: f + 1000,
            }])
        }
        fn variables(&mut self, _: i64) -> Result<Vec<VariableInfo>, String> {
            Ok(vec![VariableInfo {
                name: "counter".into(),
                value: "3".into(),
                type_name: Some("int".into()),
                reference: 0,
            }])
        }
        fn continue_all(&mut self) -> Result<(), String> {
            Ok(())
        }
        fn disconnect(&mut self, _: bool) -> Result<(), String> {
            self.detached = true;
            Ok(())
        }
        fn on_event(&mut self, e: &'static str) -> Vec<DebugEvent> {
            match e {
                "bp" => vec![DebugEvent::Stopped {
                    reason: "breakpoint",
                    thread_id: 7,
                    hit_breakpoint_ids: vec![1],
                    observed_at: None,
                }],
                "exit" => vec![DebugEvent::Exited { exit_code: 0 }, DebugEvent::Terminated],
                _ => vec![],
            }
        }
    }

    fn request(seq: i64, command: &str, arguments: Value) -> Request {
        Request {
            seq,
            command: command.into(),
            arguments,
        }
    }

    #[test]
    fn attach_answers_then_sends_initialized() {
        let sink = Sink::default();
        let mut s = Session::new(Fake::default(), Output::new(Box::new(sink.clone())));
        assert!(!s.handle(request(1, "initialize", json!({}))));
        assert!(!s.handle(request(2, "attach", json!({ "processId": 42 }))));
        let m = sink.messages();
        assert_eq!(m[0]["command"], "initialize");
        assert_eq!(m[0]["body"]["supportsConfigurationDoneRequest"], true);
        assert_eq!(m[1]["type"], "response");
        assert_eq!(m[1]["request_seq"], 2);
        assert_eq!(m[1]["success"], true);
        assert_eq!(m[2]["event"], "initialized");
        let seqs: Vec<i64> = m.iter().map(|v| v["seq"].as_i64().unwrap()).collect();
        assert_eq!(seqs, vec![1, 2, 3]);
        assert_eq!(s.debugger.attached, Some(42));
    }

    #[test]
    fn failed_attach_sends_no_initialized() {
        let sink = Sink::default();
        let mut s = Session::new(Fake::default(), Output::new(Box::new(sink.clone())));
        s.handle(request(1, "attach", json!({ "processId": 1 })));
        let m = sink.messages();
        assert_eq!(m.len(), 1);
        assert_eq!(m[0]["success"], false);
        assert_eq!(m[0]["message"], "no such process");
    }

    #[test]
    fn answers_the_inspection_requests() {
        let sink = Sink::default();
        let mut s = Session::new(Fake::default(), Output::new(Box::new(sink.clone())));
        s.handle(request(
            1,
            "setBreakpoints",
            json!({ "source": { "path": "/remote/Program.cs" }, "breakpoints": [{ "line": 5 }] }),
        ));
        s.handle(request(2, "threads", json!(null)));
        s.handle(request(
            3,
            "stackTrace",
            json!({ "threadId": 7, "startFrame": 1, "levels": 1 }),
        ));
        s.handle(request(4, "scopes", json!({ "frameId": 2 })));
        s.handle(request(
            5,
            "variables",
            json!({ "variablesReference": 1002 }),
        ));
        s.handle(request(6, "continue", json!({ "threadId": 7 })));
        s.handle(request(7, "evaluate", json!({ "expression": "x" })));
        s.handle(request(8, "stackTrace", json!({})));
        let m = sink.messages();
        assert_eq!(
            m[0]["body"]["breakpoints"][0]["source"]["path"],
            "/remote/Program.cs"
        );
        assert_eq!(m[0]["body"]["breakpoints"][0]["verified"], false);
        assert_eq!(m[1]["body"]["threads"][0]["id"], 7);
        assert_eq!(m[2]["body"]["totalFrames"], 3);
        assert_eq!(m[2]["body"]["stackFrames"].as_array().unwrap().len(), 1);
        assert_eq!(m[2]["body"]["stackFrames"][0]["name"], "f2");
        assert_eq!(m[3]["body"]["scopes"][0]["variablesReference"], 1002);
        assert_eq!(m[4]["body"]["variables"][0]["name"], "counter");
        assert_eq!(m[4]["body"]["variables"][0]["type"], "int");
        assert_eq!(m[5]["body"]["allThreadsContinued"], true);
        assert_eq!(m[6]["success"], false);
        assert_eq!(m[7]["success"], false);
    }

    #[test]
    fn debugger_events_become_dap_events_and_disconnect_ends() {
        let sink = Sink::default();
        let mut s = Session::new(Fake::default(), Output::new(Box::new(sink.clone())));
        assert!(!s.on_event("bp"));
        assert!(s.on_event("exit"));
        assert!(s.handle(request(9, "disconnect", json!({}))));
        assert!(s.debugger.detached);
        let m = sink.messages();
        assert_eq!(m[0]["event"], "stopped");
        assert_eq!(m[0]["body"]["reason"], "breakpoint");
        assert_eq!(m[0]["body"]["hitBreakpointIds"][0], 1);
        assert_eq!(m[1]["event"], "exited");
        assert_eq!(m[2]["event"], "terminated");
        assert_eq!(m[3]["event"], "terminated");
        assert_eq!(m[4]["command"], "disconnect");
    }

    #[test]
    fn serve_runs_over_tcp() {
        use std::net::TcpListener;
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            serve(stream, |_tx| Fake::default()).unwrap();
        });
        let mut client = TcpStream::connect(addr).unwrap();
        let mut reader = BufReader::new(client.try_clone().unwrap());
        let send = |c: &mut TcpStream, v: Value| {
            write_message(c, &serde_json::to_vec(&v).unwrap()).unwrap()
        };
        send(
            &mut client,
            json!({ "seq": 1, "type": "request", "command": "initialize", "arguments": {} }),
        );
        let r: Value =
            serde_json::from_slice(&read_message(&mut reader).unwrap().unwrap()).unwrap();
        assert_eq!(r["success"], true);
        send(
            &mut client,
            json!({ "seq": 2, "type": "request", "command": "disconnect", "arguments": {} }),
        );
        let e: Value =
            serde_json::from_slice(&read_message(&mut reader).unwrap().unwrap()).unwrap();
        assert_eq!(e["event"], "terminated");
        let r: Value =
            serde_json::from_slice(&read_message(&mut reader).unwrap().unwrap()).unwrap();
        assert_eq!(r["command"], "disconnect");
        assert!(read_message(&mut reader).unwrap().is_none());
        server.join().unwrap();
    }
}
