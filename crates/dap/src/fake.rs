//! A scripted fake debug adapter (feature `fake`) for the client's tests and the shell's headless tests.
//!
//! It plays a [`FakeProgram`]: a list of statements ([`FakeStep`]s), each in a method at a call depth with its locals.
//! A run walks the statements in order, as a server handling one request would; it starts at `configurationDone`
//! when [`FakeProgram::run_at_start`] is set and whenever the test calls [`FakeHandle::trigger`] (the "ping"). It
//! breaks at enabled breakpoints (with conditions of the form `name == value` or `name != value`), at exceptions the
//! filters ask for, and after steps: step into stops at the next statement, step over at the next one at the same
//! depth or shallower, step out at the next shallower one. Between runs the program waits, still alive.
//!
//! It answers as netcoredbg 3.2.0-1092 does where that matters to a client: `initialized` right after the
//! `initialize` answer (before `launch`), breakpoints set before `configurationDone` are pending (`verified: false`)
//! until a `breakpoint` `changed` event, no `hitCondition` support (it is ignored), no `hitBreakpointIds` in
//! `stopped`, a sourceless `[Native Frames]` frame under the managed ones, and new frame ids at every stop.
//!
//! For brief 0025 it also answers `pause` (a stop with reason `pause` while the program "runs" a statement marked
//! [`FakeStep::runs_until_paused`]), `exceptionInfo` with details (stack trace, inner exceptions), `stackTrace` with
//! `startFrame` and `levels` (it advertises `supportsDelayedStackTraceLoading`) and `variables` with `start` and
//! `count` (it advertises `supportsVariablePaging`; [`FakeProgram::extra_capabilities`] can turn either off, and the
//! fake then ignores the paging arguments as such an adapter would), large variable sets ([`FakeVar::array`]) and deep
//! object graphs ([`FakeVar::deep`]), output per category ([`FakeStep::prints`]), and other threads' stacks (one
//! frame without source).
//!
//! [`connect`] serves it in-process over pipes, [`listen_tcp`] over a loopback TCP socket, and [`serve_stdio`] on
//! this process's stdin and stdout (a child process). Every request is recorded ([`FakeHandle::requests`]).

use std::collections::HashMap;
use std::io::{BufReader, Read, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant};

use serde_json::{Value, json};

use crate::types::SourceBreakpoint;
use crate::{Connection, ProtocolMessage, framing};

/// A variable, with members.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FakeVar {
    pub name: String,
    pub value: String,
    pub type_name: String,
    pub children: Vec<FakeVar>,
    /// The members are elements (`indexedVariables`), not fields (`namedVariables`).
    pub indexed: bool,
    /// The DAP `presentationHint.kind` of the row, if any.
    pub hint: Option<String>,
}

impl FakeVar {
    pub fn new(name: &str, value: &str, type_name: &str) -> Self {
        Self {
            name: name.into(),
            value: value.into(),
            type_name: type_name.into(),
            ..Self::default()
        }
    }

    pub fn with_children(mut self, children: Vec<FakeVar>) -> Self {
        self.children = children;
        self
    }

    pub fn with_hint(mut self, kind: &str) -> Self {
        self.hint = Some(kind.into());
        self
    }

    /// An `int[n]` whose element `[i]` is `i`.
    pub fn array(name: &str, n: usize) -> Self {
        let mut a = Self::new(name, &format!("{{int[{n}]}}"), "int[]").with_children(
            (0..n)
                .map(|i| Self::new(&format!("[{i}]"), &i.to_string(), "int"))
                .collect(),
        );
        a.indexed = true;
        a
    }

    /// An object graph `depth` levels deep (1 is a leaf), each object with `breadth` members: `breadth - 1` leaves
    /// (`f0`, `f1`, ...) and one more object, `next`.
    pub fn deep(name: &str, depth: usize, breadth: usize) -> Self {
        if depth <= 1 {
            return Self::new(name, "7", "int");
        }
        let mut children: Vec<FakeVar> = (0..breadth.saturating_sub(1))
            .map(|i| Self::new(&format!("f{i}"), &i.to_string(), "int"))
            .collect();
        children.push(Self::deep("next", depth - 1, breadth));
        Self::new(name, "{App.Node}", "App.Node").with_children(children)
    }
}

/// An exception thrown at a statement.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FakeThrow {
    pub exception: String,
    pub message: String,
    /// Caught by user code (a first-chance stop only with the `all` filter).
    pub handled: bool,
    /// `exceptionInfo`'s `details.stackTrace`.
    pub stack_trace: Option<String>,
    /// `exceptionInfo`'s `details.innerException`.
    pub inner: Option<Box<FakeThrow>>,
}

impl FakeThrow {
    pub fn new(exception: &str, message: &str, handled: bool) -> Self {
        Self {
            exception: exception.into(),
            message: message.into(),
            handled,
            ..Self::default()
        }
    }

    fn details(&self) -> Value {
        let short = self.exception.rsplit('.').next().unwrap_or_default();
        let mut d =
            json!({"message": self.message, "typeName": short, "fullTypeName": self.exception});
        if let Some(t) = &self.stack_trace {
            d["stackTrace"] = json!(t);
        }
        if let Some(i) = &self.inner {
            d["innerException"] = json!([i.details()]);
        }
        d
    }
}

/// One statement.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeStep {
    pub path: String,
    /// 1-based.
    pub line: i64,
    pub function: String,
    /// Call depth: 0 is the outermost method of a run.
    pub depth: usize,
    pub locals: Vec<FakeVar>,
    pub throws: Option<FakeThrow>,
    /// `output` events (category, text) sent when the statement runs.
    pub prints: Vec<(String, String)>,
    /// The statement runs until `pause` arrives (a hang or a long loop); then it stops there with reason `pause`, and
    /// resuming goes on after it.
    pub runs_until_paused: bool,
}

impl FakeStep {
    pub fn new(path: &str, line: i64, function: &str, depth: usize, locals: Vec<FakeVar>) -> Self {
        Self {
            path: path.into(),
            line,
            function: function.into(),
            depth,
            locals,
            throws: None,
            prints: Vec::new(),
            runs_until_paused: false,
        }
    }

    /// The statement writes `text` to stdout when it runs.
    pub fn printing(mut self, text: &str) -> Self {
        self.prints.push(("stdout".into(), text.into()));
        self
    }
}

/// The program the fake debugs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FakeProgram {
    pub steps: Vec<FakeStep>,
    /// The thread runs execute on, (id, name).
    pub thread: (i64, String),
    pub other_threads: Vec<(i64, String)>,
    /// Run once at `configurationDone` (a console program); otherwise wait for [`FakeHandle::trigger`] (a server).
    pub run_at_start: bool,
    /// Printed (as `output` events, category `stdout`) at start.
    pub output_at_start: Vec<String>,
    pub process_id: i64,
    /// Exit (code 3) when this command arrives: the adapter crashes.
    pub crash_on: Option<String>,
    /// Members merged into the `initialize` answer (for example `{"supportsVariablePaging": false}`).
    pub extra_capabilities: Value,
}

impl Default for FakeProgram {
    fn default() -> Self {
        Self {
            steps: Vec::new(),
            thread: (1, "Main Thread".into()),
            other_threads: Vec::new(),
            run_at_start: false,
            output_at_start: Vec::new(),
            process_id: 4242,
            crash_on: None,
            extra_capabilities: Value::Null,
        }
    }
}

enum Control {
    Trigger,
    Crash,
    Stall(Duration),
}

enum Incoming {
    Message(ProtocolMessage),
    Control(Control),
    Eof,
}

/// The test's handle on a running fake.
#[derive(Clone)]
pub struct FakeHandle {
    tx: mpsc::Sender<Incoming>,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    done: Arc<AtomicBool>,
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

impl FakeHandle {
    /// Run the program once from its first statement (a request reaching a server).
    pub fn trigger(&self) {
        let _ = self.tx.send(Incoming::Control(Control::Trigger));
    }

    /// The adapter dies: its streams close without `terminated`.
    pub fn crash(&self) {
        let _ = self.tx.send(Incoming::Control(Control::Crash));
    }

    /// Answer nothing for `d` (the adapter hangs).
    pub fn stall(&self, d: Duration) {
        let _ = self.tx.send(Incoming::Control(Control::Stall(d)));
    }

    /// Every request received, in order: (command, arguments).
    pub fn requests(&self) -> Vec<(String, Value)> {
        lock(&self.requests).clone()
    }

    pub fn commands(&self) -> Vec<String> {
        lock(&self.requests)
            .iter()
            .map(|(c, _)| c.clone())
            .collect()
    }

    /// The arguments of the last `command` received.
    pub fn last(&self, command: &str) -> Option<Value> {
        lock(&self.requests)
            .iter()
            .rev()
            .find(|(c, _)| c == command)
            .map(|(_, a)| a.clone())
    }

    /// Wait until `command` has been received `count` times.
    pub fn wait_for(&self, command: &str, count: usize, timeout: Duration) -> bool {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if lock(&self.requests)
                .iter()
                .filter(|(c, _)| c == command)
                .count()
                >= count
            {
                return true;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        false
    }

    /// The fake has stopped serving (disconnect, crash or the client hung up).
    pub fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

/// Serve `program` in-process: the returned connection is the client's end.
pub fn connect(program: FakeProgram) -> (Connection, FakeHandle) {
    let (client_reader, adapter_writer) = std::io::pipe().expect("pipe");
    let (adapter_reader, client_writer) = std::io::pipe().expect("pipe");
    let handle = serve(adapter_reader, adapter_writer, program, None);
    (
        Connection::from_streams(client_reader, client_writer, "fake adapter (in-process)"),
        handle,
    )
}

/// Serve `program` to the first client connecting to the returned loopback port.
pub fn listen_tcp(program: FakeProgram) -> std::io::Result<(u16, FakeHandle)> {
    let listener = TcpListener::bind(("127.0.0.1", 0))?;
    let port = listener.local_addr()?.port();
    let (handle, rx) = channels();
    let h = handle.clone();
    std::thread::Builder::new()
        .name("fake-dap-accept".into())
        .spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let reader = stream.try_clone().expect("clone socket");
                let closer = stream.try_clone().expect("clone socket");
                // Hang up when the session ends, as an adapter process exiting would.
                let hang_up: Box<dyn FnOnce(bool) + Send> = Box::new(move |_| {
                    let _ = closer.shutdown(std::net::Shutdown::Both);
                });
                serve_with(reader, stream, program, Some(hang_up), &h, rx);
            }
        })?;
    Ok((port, handle))
}

/// Serve `program` on this process's stdin and stdout until the client disconnects, then return. A crash exits the
/// process with code 3.
pub fn serve_stdio(program: FakeProgram) {
    let handle = serve(
        std::io::stdin(),
        std::io::stdout(),
        program,
        Some(Box::new(|crashed| {
            if crashed {
                std::process::exit(3)
            }
        })),
    );
    while !handle.is_done() {
        std::thread::sleep(Duration::from_millis(5));
    }
}

fn channels() -> (FakeHandle, mpsc::Receiver<Incoming>) {
    let (tx, rx) = mpsc::channel();
    (
        FakeHandle {
            tx,
            requests: Arc::default(),
            done: Arc::default(),
        },
        rx,
    )
}

/// Serve `program` on any pair of streams. `on_end` runs when the fake stops serving, with whether it crashed (by
/// default the streams are just dropped).
pub fn serve(
    reader: impl Read + Send + 'static,
    writer: impl Write + Send + 'static,
    program: FakeProgram,
    on_end: Option<Box<dyn FnOnce(bool) + Send>>,
) -> FakeHandle {
    let (handle, rx) = channels();
    serve_with(reader, writer, program, on_end, &handle, rx);
    handle
}

fn serve_with(
    reader: impl Read + Send + 'static,
    writer: impl Write + Send + 'static,
    program: FakeProgram,
    on_end: Option<Box<dyn FnOnce(bool) + Send>>,
    handle: &FakeHandle,
    rx: mpsc::Receiver<Incoming>,
) {
    let reader_tx = handle.tx.clone();
    std::thread::Builder::new()
        .name("fake-dap-reader".into())
        .spawn(move || {
            let mut r = BufReader::new(reader);
            while let Ok(Some(body)) = framing::read_message(&mut r) {
                if let Ok(m) = serde_json::from_slice::<ProtocolMessage>(&body)
                    && reader_tx.send(Incoming::Message(m)).is_err()
                {
                    return;
                }
            }
            let _ = reader_tx.send(Incoming::Eof);
        })
        .expect("spawn fake-dap-reader");
    let mut machine = Machine::new(program, Box::new(writer), handle.requests.clone());
    let done_flag = handle.done.clone();
    std::thread::Builder::new()
        .name("fake-dap".into())
        .spawn(move || {
            let crashed = machine.run(rx);
            drop(machine);
            done_flag.store(true, Ordering::Release);
            if let Some(f) = on_end {
                f(crashed);
            }
        })
        .expect("spawn fake-dap");
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Mode {
    Continue,
    Into,
    Over(usize),
    Out(usize),
}

struct Machine {
    program: FakeProgram,
    out: Box<dyn Write + Send>,
    seq: i64,
    requests: Arc<Mutex<Vec<(String, Value)>>>,
    configured: bool,
    /// The statement the program is stopped at.
    pc: Option<usize>,
    /// Breakpoints by path: (id, breakpoint).
    breakpoints: HashMap<String, Vec<(i64, SourceBreakpoint)>>,
    next_id: i64,
    filters: Vec<String>,
    stops: i64,
    /// Frame id -> statement index of that frame.
    frames: HashMap<i64, usize>,
    /// Variables reference -> its members.
    refs: HashMap<i64, Vec<FakeVar>>,
    next_ref: i64,
    exception: Option<FakeThrow>,
    /// The stop was an exception: resuming does not throw it again.
    stopped_on_exception: bool,
    /// The statement running until `pause` arrives.
    running_at: Option<usize>,
    /// What `initialize` answered.
    caps: Value,
}

impl Machine {
    fn new(
        program: FakeProgram,
        out: Box<dyn Write + Send>,
        requests: Arc<Mutex<Vec<(String, Value)>>>,
    ) -> Self {
        Self {
            program,
            out,
            seq: 1,
            requests,
            configured: false,
            pc: None,
            breakpoints: HashMap::new(),
            next_id: 1,
            filters: Vec::new(),
            stops: 0,
            frames: HashMap::new(),
            refs: HashMap::new(),
            next_ref: 1,
            exception: None,
            stopped_on_exception: false,
            running_at: None,
            caps: Value::Null,
        }
    }

    fn write(&mut self, msg: &ProtocolMessage) {
        let bytes = serde_json::to_vec(msg).expect("serialize");
        let _ = framing::write_message(&mut self.out, &bytes);
        self.seq += 1;
    }

    fn event(&mut self, event: &str, body: Value) {
        let msg = ProtocolMessage::Event {
            seq: self.seq,
            event: event.into(),
            body: Some(body),
        };
        self.write(&msg);
    }

    fn respond(&mut self, request_seq: i64, command: &str, result: Result<Value, String>) {
        let (success, message, body) = match result {
            Ok(b) => (true, None, Some(b)),
            Err(m) => (false, Some(m), None),
        };
        let msg = ProtocolMessage::Response {
            seq: self.seq,
            request_seq,
            success,
            command: command.into(),
            message,
            body,
        };
        self.write(&msg);
    }

    /// Serve until disconnect, EOF or a crash (returns true for a crash).
    fn run(&mut self, rx: mpsc::Receiver<Incoming>) -> bool {
        while let Ok(incoming) = rx.recv() {
            if let Incoming::Control(Control::Stall(d)) = incoming {
                // Hang for `d`, but keep listening: a crash during the stall ends the session
                // before any deferred request is answered (deterministic for the crash tests),
                // and requests that arrive meanwhile are answered once the stall ends.
                let deadline = Instant::now() + d;
                let mut deferred = Vec::new();
                loop {
                    let now = Instant::now();
                    if now >= deadline {
                        break;
                    }
                    match rx.recv_timeout(deadline - now) {
                        Ok(Incoming::Control(Control::Crash)) => return true,
                        Ok(Incoming::Eof) => return false,
                        Ok(other) => deferred.push(other),
                        Err(mpsc::RecvTimeoutError::Timeout) => break,
                        Err(mpsc::RecvTimeoutError::Disconnected) => return false,
                    }
                }
                for m in deferred {
                    if let Some(done) = self.handle(m) {
                        return done;
                    }
                }
                continue;
            }
            if let Some(done) = self.handle(incoming) {
                return done;
            }
        }
        false
    }

    /// Handle one incoming item; `Some(true)` is a crash, `Some(false)` ends the session.
    fn handle(&mut self, incoming: Incoming) -> Option<bool> {
        match incoming {
            Incoming::Eof => Some(false),
            Incoming::Control(Control::Crash) => Some(true),
            Incoming::Control(Control::Stall(d)) => {
                std::thread::sleep(d);
                None
            }
            Incoming::Control(Control::Trigger) => {
                if self.configured && self.pc.is_none() && self.running_at.is_none() {
                    self.run_from(0, Mode::Continue);
                }
                None
            }
            Incoming::Message(ProtocolMessage::Request {
                seq,
                command,
                arguments,
            }) => {
                let args = arguments.unwrap_or(Value::Null);
                lock(&self.requests).push((command.clone(), args.clone()));
                if self.program.crash_on.as_deref() == Some(command.as_str()) {
                    return Some(true);
                }
                if !self.request(seq, &command, &args) {
                    return Some(false);
                }
                None
            }
            Incoming::Message(_) => None,
        }
    }

    fn thread_id(&self) -> i64 {
        self.program.thread.0
    }

    /// Handle one request; false ends the session.
    fn request(&mut self, seq: i64, command: &str, args: &Value) -> bool {
        match command {
            "initialize" => {
                let mut caps = json!({
                    "supportsConfigurationDoneRequest": true,
                    "supportsConditionalBreakpoints": true,
                    "supportsExceptionInfoRequest": true,
                    "supportsTerminateRequest": true,
                    "supportTerminateDebuggee": true,
                    "supportsDelayedStackTraceLoading": true,
                    "supportsVariablePaging": true,
                    "exceptionBreakpointFilters": [
                        {"filter": "user-unhandled", "label": "user-unhandled"},
                        {"filter": "all", "label": "all"}
                    ]
                });
                if let Value::Object(extra) = &self.program.extra_capabilities {
                    for (k, v) in extra {
                        caps[k] = v.clone();
                    }
                }
                self.caps = caps.clone();
                self.event("capabilities", json!({ "capabilities": caps.clone() }));
                self.respond(seq, command, Ok(caps));
                self.event("initialized", json!({}));
            }
            "launch" | "attach" => self.respond(seq, command, Ok(json!({}))),
            "setBreakpoints" => {
                let path = args["source"]["path"]
                    .as_str()
                    .unwrap_or_default()
                    .to_owned();
                let asked: Vec<SourceBreakpoint> =
                    serde_json::from_value(args["breakpoints"].clone()).unwrap_or_default();
                let mut set = Vec::new();
                let mut answer = Vec::new();
                for bp in asked {
                    let id = self.next_id;
                    self.next_id += 1;
                    let verified = self.configured && self.has_line(&path, bp.line);
                    answer.push(if self.configured {
                        json!({"id": id, "line": bp.line, "verified": verified,
                               "source": {"path": path}})
                    } else {
                        json!({"id": id, "line": bp.line, "verified": false,
                               "message": "The breakpoint is pending and will be resolved when debugging starts."})
                    });
                    set.push((id, bp));
                }
                self.breakpoints.insert(path, set);
                self.respond(seq, command, Ok(json!({ "breakpoints": answer })));
            }
            "setExceptionBreakpoints" => {
                self.filters = serde_json::from_value(args["filters"].clone()).unwrap_or_default();
                self.respond(seq, command, Ok(json!({})));
            }
            "configurationDone" => {
                self.configured = true;
                self.event(
                    "process",
                    json!({"name": "dotnet", "systemProcessId": self.program.process_id,
                           "isLocalProcess": true, "startMethod": "launch"}),
                );
                self.respond(seq, command, Ok(json!({})));
                let bound: Vec<(String, i64, i64)> = self
                    .breakpoints
                    .iter()
                    .flat_map(|(p, v)| v.iter().map(move |(id, b)| (p.clone(), *id, b.line)))
                    .filter(|(p, _, line)| self.has_line(p, *line))
                    .collect();
                for (path, id, line) in bound {
                    self.event(
                        "breakpoint",
                        json!({"reason": "changed", "breakpoint": {"id": id, "line": line, "verified": true,
                               "source": {"path": path}}}),
                    );
                }
                let (tid, _) = self.program.thread.clone();
                self.event("thread", json!({"reason": "started", "threadId": tid}));
                for (id, _) in self.program.other_threads.clone() {
                    self.event("thread", json!({"reason": "started", "threadId": id}));
                }
                for line in self.program.output_at_start.clone() {
                    self.event("output", json!({"category": "stdout", "output": line}));
                }
                if self.program.run_at_start {
                    self.run_from(0, Mode::Continue);
                }
            }
            "threads" => {
                let mut threads = vec![self.program.thread.clone()];
                threads.extend(self.program.other_threads.clone());
                let threads: Vec<Value> = threads
                    .into_iter()
                    .map(|(id, name)| json!({"id": id, "name": name}))
                    .collect();
                self.respond(seq, command, Ok(json!({ "threads": threads })));
            }
            "stackTrace" => {
                let r = self.stack_trace(args);
                self.respond(seq, command, r);
            }
            "scopes" => {
                let frame = args["frameId"].as_i64().unwrap_or(-1);
                let r = match self.frames.get(&frame).copied() {
                    Some(step) => {
                        let locals = self.program.steps[step].locals.clone();
                        let n = locals.len();
                        let r = self.alloc(locals);
                        Ok(
                            json!({"scopes": [{"name": "Locals", "variablesReference": r,
                                              "namedVariables": n, "expensive": false}]}),
                        )
                    }
                    None => Err(format!("Failed command 'scopes' : unknown frame {frame}")),
                };
                self.respond(seq, command, r);
            }
            "variables" => {
                let r = args["variablesReference"].as_i64().unwrap_or(0);
                let mut vars = self.refs.get(&r).cloned().unwrap_or_default();
                if self.supports("supportsVariablePaging") {
                    let start = (args["start"].as_u64().unwrap_or(0) as usize).min(vars.len());
                    let count = args["count"].as_u64().unwrap_or(0) as usize;
                    vars.drain(..start);
                    if count > 0 {
                        vars.truncate(count);
                    }
                }
                let rows: Vec<Value> = vars.into_iter().map(|v| self.variable_json(v)).collect();
                self.respond(seq, command, Ok(json!({ "variables": rows })));
            }
            "evaluate" => {
                let r = self.evaluate(args);
                self.respond(seq, command, r);
            }
            "exceptionInfo" => {
                let r = match (&self.exception, self.stopped_on_exception) {
                    (Some(e), true) => {
                        Ok(json!({"exceptionId": e.exception, "description": e.message,
                               "breakMode": if e.handled { "always" } else { "unhandled" },
                               "details": e.details()}))
                    }
                    _ => Err("Failed command 'exceptionInfo' : 0x80004005".into()),
                };
                self.respond(seq, command, r);
            }
            "pause" => match (self.running_at, self.pc) {
                (Some(k), _) => {
                    self.respond(seq, command, Ok(json!({})));
                    self.running_at = None;
                    self.stop(k, "pause", None);
                }
                // Already stopped: nothing to do.
                (None, Some(_)) => self.respond(seq, command, Ok(json!({}))),
                (None, None) => self.respond(
                    seq,
                    command,
                    Err("Failed command 'pause' : the program is not running".into()),
                ),
            },
            "continue" | "next" | "stepIn" | "stepOut" => {
                let Some(pc) = self.pc else {
                    self.respond(
                        seq,
                        command,
                        Err(format!("Failed command '{command}' : 0x80131301")),
                    );
                    return true;
                };
                let tid = self.thread_id();
                self.event(
                    "continued",
                    json!({"threadId": tid, "allThreadsContinued": true}),
                );
                self.respond(seq, command, Ok(json!({"allThreadsContinued": true})));
                // The statement stopped at runs now.
                self.print(pc);
                let depth = self.program.steps[pc].depth;
                let mode = match command {
                    "continue" => Mode::Continue,
                    "next" => Mode::Over(depth),
                    "stepIn" => Mode::Into,
                    _ => Mode::Out(depth),
                };
                self.pc = None;
                self.run_from(pc + 1, mode);
            }
            "terminate" => {
                self.event("exited", json!({"exitCode": 0}));
                self.event("terminated", json!({}));
                self.respond(seq, command, Ok(json!({})));
            }
            "disconnect" => {
                self.event("exited", json!({"exitCode": 0}));
                self.event("terminated", json!({}));
                self.respond(seq, command, Ok(json!({})));
                return false;
            }
            other => {
                self.respond(
                    seq,
                    other,
                    Err(format!(
                        "Failed command '{other}' : not supported by the fake"
                    )),
                );
            }
        }
        true
    }

    fn has_line(&self, path: &str, line: i64) -> bool {
        self.program
            .steps
            .iter()
            .any(|s| s.path == path && s.line == line)
    }

    fn supports(&self, capability: &str) -> bool {
        self.caps[capability].as_bool().unwrap_or(false)
    }

    /// Statement `k` runs: its output.
    fn print(&mut self, k: usize) {
        for (category, text) in self.program.steps[k].prints.clone() {
            self.event("output", json!({"category": category, "output": text}));
        }
    }

    fn alloc(&mut self, vars: Vec<FakeVar>) -> i64 {
        let r = self.next_ref;
        self.next_ref += 1;
        self.refs.insert(r, vars);
        r
    }

    fn variable_json(&mut self, v: FakeVar) -> Value {
        let n = v.children.len();
        let reference = if v.children.is_empty() {
            0
        } else {
            self.alloc(v.children)
        };
        let mut row = json!({"name": v.name, "value": v.value, "type": v.type_name,
                             "variablesReference": reference, "evaluateName": v.name});
        if n > 0 {
            row[if v.indexed {
                "indexedVariables"
            } else {
                "namedVariables"
            }] = json!(n);
        }
        if let Some(kind) = v.hint {
            row["presentationHint"] = json!({ "kind": kind });
        }
        row
    }

    /// Walk from statement `start` until something stops the program, or the run ends.
    fn run_from(&mut self, start: usize, mode: Mode) {
        // Resuming after an exception goes on past the statement that threw it.
        self.stopped_on_exception = false;
        for k in start..self.program.steps.len() {
            let step = self.program.steps[k].clone();
            if let Some(t) = &step.throws {
                let first_chance = self.filters.iter().any(|f| f == "all");
                let unhandled = !t.handled && self.filters.iter().any(|f| f == "user-unhandled");
                if first_chance || unhandled {
                    self.exception = Some(t.clone());
                    self.stop(k, "exception", Some(t.message.clone()));
                    self.stopped_on_exception = true;
                    return;
                }
            }
            let hit = self.breakpoints.get(&step.path).is_some_and(|bps| {
                bps.iter().any(|(_, b)| {
                    b.line == step.line && condition_holds(b.condition.as_deref(), &step.locals)
                })
            });
            if hit {
                self.stop(k, "breakpoint", None);
                return;
            }
            let step_done = match mode {
                Mode::Continue => false,
                Mode::Into => true,
                Mode::Over(d) => step.depth <= d,
                Mode::Out(d) => step.depth < d,
            };
            if step_done {
                self.stop(k, "step", None);
                return;
            }
            if step.runs_until_paused {
                self.pc = None;
                self.running_at = Some(k);
                return;
            }
            self.print(k);
        }
        // The run is over; the program waits for the next one.
        self.pc = None;
    }

    fn stop(&mut self, k: usize, reason: &str, text: Option<String>) {
        self.pc = Some(k);
        self.stops += 1;
        self.frames.clear();
        self.refs.clear();
        let mut body =
            json!({"reason": reason, "threadId": self.thread_id(), "allThreadsStopped": true});
        if let Some(t) = text {
            body["text"] = json!(t);
        }
        self.event("stopped", body);
    }

    fn stack_trace(&mut self, args: &Value) -> Result<Value, String> {
        let Some(pc) = self.pc else {
            return Err("Failed command 'stackTrace' : 0x80131301".into());
        };
        let thread = args["threadId"].as_i64().unwrap_or(self.thread_id());
        if thread != self.thread_id() {
            // Another thread waits in external code.
            let frames = vec![
                json!({"id": self.stops * 1000 + 900, "name": "System.Threading.Monitor.Wait()", "line": 0,
                       "column": 0, "presentationHint": "subtle"}),
                json!({"id": self.stops * 1000 + 901, "name": "[Native Frames]", "line": 0, "column": 0}),
            ];
            return Ok(self.page_frames(frames, args));
        }
        let steps = &self.program.steps;
        let mut chain = Vec::new();
        let top = steps[pc].depth;
        for d in (0..=top).rev() {
            if let Some(i) = (0..=pc).rev().find(|&i| steps[i].depth == d) {
                chain.push(i);
            }
        }
        let mut frames = Vec::new();
        for (n, i) in chain.into_iter().enumerate() {
            let id = self.stops * 1000 + n as i64;
            self.frames.insert(id, i);
            let s = &self.program.steps[i];
            let name = std::path::Path::new(&s.path)
                .file_name()
                .map(|f| f.to_string_lossy().into_owned());
            frames.push(
                json!({"id": id, "name": s.function, "line": s.line, "column": 9,
                               "endLine": s.line, "endColumn": 40,
                               "source": {"name": name, "path": s.path}}),
            );
        }
        frames.push(
            json!({"id": self.stops * 1000 + 999, "name": "[Native Frames]", "line": 0, "column": 0,
                           "endLine": 0, "endColumn": 0, "moduleId": ""}),
        );
        Ok(self.page_frames(frames, args))
    }

    /// `startFrame` and `levels` when the fake advertises delayed stack loading; the whole stack otherwise.
    fn page_frames(&self, mut frames: Vec<Value>, args: &Value) -> Value {
        let total = frames.len();
        if self.supports("supportsDelayedStackTraceLoading") {
            let start = (args["startFrame"].as_u64().unwrap_or(0) as usize).min(total);
            let levels = args["levels"].as_u64().unwrap_or(0) as usize;
            frames.drain(..start);
            if levels > 0 {
                frames.truncate(levels);
            }
        }
        json!({"stackFrames": frames, "totalFrames": total})
    }

    fn evaluate(&mut self, args: &Value) -> Result<Value, String> {
        let expr = args["expression"]
            .as_str()
            .unwrap_or_default()
            .trim()
            .to_owned();
        let step = args["frameId"]
            .as_i64()
            .and_then(|f| self.frames.get(&f).copied())
            .or(self.pc)
            .ok_or_else(|| "Failed command 'evaluate' : 0x80131301".to_owned())?;
        let mut scope = self.program.steps[step].locals.clone();
        let mut found: Option<FakeVar> = None;
        for part in expr.split('.') {
            let v = scope
                .iter()
                .find(|v| v.name == part)
                .cloned()
                .ok_or_else(|| {
                    format!("error: The name '{expr}' does not exist in the current context")
                })?;
            scope = v.children.clone();
            found = Some(v);
        }
        let v = found.ok_or_else(|| "error: empty expression".to_owned())?;
        let reference = if v.children.is_empty() {
            0
        } else {
            self.alloc(v.children.clone())
        };
        Ok(json!({"result": v.value, "type": v.type_name, "variablesReference": reference}))
    }
}

/// `name == value`, `name != value` (value compared as shown, quotes optional), `true`, `false`; anything else holds.
fn condition_holds(condition: Option<&str>, locals: &[FakeVar]) -> bool {
    let Some(c) = condition.map(str::trim).filter(|c| !c.is_empty()) else {
        return true;
    };
    match c {
        "true" => return true,
        "false" => return false,
        _ => {}
    }
    let (op, eq) = if let Some(i) = c.find("==") {
        (i, true)
    } else if let Some(i) = c.find("!=") {
        (i, false)
    } else {
        return true;
    };
    let name = c[..op].trim();
    let value = c[op + 2..].trim().trim_matches('"');
    let Some(v) = locals.iter().find(|v| v.name == name) else {
        return true;
    };
    (v.value.trim_matches('"') == value) == eq
}
