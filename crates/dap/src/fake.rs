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
//! For brief 0026 it also answers `setFunctionBreakpoints` (a stop with reason `function breakpoint` where a run enters
//! a statement of the named method, `Namespace.Type.Method` matched against the statement's function without its
//! parameters), `setExceptionBreakpoints` with `filterOptions` (a filter's `condition` is the comma-separated exception
//! types it stops for), `setVariable` and `setExpression` (the new value is kept in the statement's locals; an `int`
//! takes integers only), and behind flags in [`FakeProgram::extra_capabilities`]: log points (`supportsLogPoints`: a
//! breakpoint with `logMessage` writes the interpolated message as a `console` output event and does not stop) and Set
//! Next Statement (`supportsGotoTargetsRequest`: `gotoTargets` lists the statements of the stopped method on a line, and
//! `goto` moves there with a `stopped` event of reason `goto`). Function breakpoints, filter options, `setVariable` and
//! `setExpression` are advertised by default and refused when turned off. [`hot_loop`] builds a loop's statements for
//! the tracepoint overhead measurements.
//!
//! For brief 0027 it also answers `attach` (the `process` event names the attached `processId` or `pid`, `startMethod`
//! `attach`; `disconnect` then detaches: the session ends with `terminated` and no `exited`, as it does after a launch with
//! `terminateDebuggee: false`) and, behind `supportsRestartRequest` in [`FakeProgram::extra_capabilities`], `restart`
//! (the program starts over in the same session: a new `process` event, and a run at once when
//! [`FakeProgram::run_at_start`] is set); without the flag `restart` is refused as any unknown request.
//!
//! For brief 0036 it refuses and rejects breakpoints as eludite-dbg-mono does: a line without a statement is answered
//! `verified: false` with `The breakpoint location is invalid: line N has no code.` once the program runs (at
//! `configurationDone` as a `breakpoint` `changed` event for one set before), and a condition comparing to a dotted name
//! (`coin == Coin.Quarter`: the fake's evaluator knows no type names, as Mono's did before brief 0036) is rejected with
//! `Unknown identifier: Coin`: in the `setBreakpoints` answer (`verified: false`) once the program runs, and for one set
//! before, at its first hit with a `breakpoint` `changed` event; a rejected breakpoint does not stop.
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

/// `iterations` statements of a loop body on `line` of `path` in `function` at `depth`, each with the locals `i` (the
/// iteration, from 0) and `sum` (0 + 1 + ... + i), for the tracepoint overhead measurements.
pub fn hot_loop(
    path: &str,
    line: i64,
    function: &str,
    depth: usize,
    iterations: usize,
) -> Vec<FakeStep> {
    (0..iterations)
        .map(|i| {
            FakeStep::new(
                path,
                line,
                function,
                depth,
                vec![
                    FakeVar::new("i", &i.to_string(), "int"),
                    FakeVar::new("sum", &(i * (i + 1) / 2).to_string(), "int"),
                ],
            )
        })
        .collect()
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
    /// The program exits with this code when a run ends (a console program), instead of waiting for the next one.
    pub exit_at_end: Option<i64>,
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
            exit_at_end: None,
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
    /// Breakpoints whose condition failed to evaluate (brief 0036): they no longer stop.
    rejected: Vec<i64>,
    /// Function breakpoints: (id, name, condition).
    functions: Vec<(i64, String, Option<String>)>,
    next_id: i64,
    /// Exception filters, each with the types its condition names (`None`: every exception).
    filters: Vec<(String, Option<Vec<String>>)>,
    stops: i64,
    /// Frame id -> statement index of that frame.
    frames: HashMap<i64, usize>,
    /// Variables reference -> its members.
    refs: HashMap<i64, Vec<FakeVar>>,
    /// Variables reference -> the statement whose locals hold its members and the names leading to them from there
    /// (empty for the locals themselves): where `setVariable` writes.
    origins: HashMap<i64, (usize, Vec<String>)>,
    next_ref: i64,
    exception: Option<FakeThrow>,
    /// The stop was an exception: resuming does not throw it again.
    stopped_on_exception: bool,
    /// The statement running until `pause` arrives.
    running_at: Option<usize>,
    /// What `initialize` answered.
    caps: Value,
    /// The program exited (`exited` was sent).
    exited: bool,
    /// The session began with `attach` (brief 0027): the process id it named, if any.
    attached: Option<Option<i64>>,
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
            rejected: Vec::new(),
            functions: Vec::new(),
            next_id: 1,
            filters: Vec::new(),
            stops: 0,
            frames: HashMap::new(),
            refs: HashMap::new(),
            origins: HashMap::new(),
            next_ref: 1,
            exception: None,
            stopped_on_exception: false,
            running_at: None,
            caps: Value::Null,
            exited: false,
            attached: None,
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
                    "supportsFunctionBreakpoints": true,
                    "supportsExceptionFilterOptions": true,
                    "supportsSetVariable": true,
                    "supportsSetExpression": true,
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
            "launch" => self.respond(seq, command, Ok(json!({}))),
            // Attach (brief 0027): like a launch, but the process is the one named, and disconnecting leaves it running.
            "attach" => {
                self.attached = Some(args["processId"].as_i64().or(args["pid"].as_i64()));
                self.respond(seq, command, Ok(json!({})));
            }
            // Restart (brief 0027), behind `supportsRestartRequest`: the program starts over in the same session.
            "restart" if self.supports("supportsRestartRequest") => {
                self.pc = None;
                self.running_at = None;
                self.exception = None;
                self.stopped_on_exception = false;
                self.frames.clear();
                self.refs.clear();
                self.origins.clear();
                self.exited = false;
                self.respond(seq, command, Ok(json!({})));
                self.event(
                    "process",
                    json!({"name": "dotnet", "systemProcessId": self.program.process_id,
                           "isLocalProcess": true, "startMethod": "launch"}),
                );
                if self.program.run_at_start {
                    self.run_from(0, Mode::Continue);
                }
            }
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
                    // Once the program runs, a condition is checked when it is set.
                    let rejected = self
                        .configured
                        .then(|| {
                            let locals = self.locals_at(&path, bp.line);
                            condition_error(bp.condition.as_deref(), &locals)
                        })
                        .flatten();
                    answer.push(if self.configured {
                        let mut a = json!({"id": id, "line": bp.line, "verified": verified && rejected.is_none(),
                               "source": {"path": path}});
                        if !verified {
                            a["message"] = json!(no_code(bp.line));
                        } else if let Some(m) = &rejected {
                            a["message"] = json!(m);
                        }
                        a
                    } else {
                        json!({"id": id, "line": bp.line, "verified": false,
                               "message": "The breakpoint is pending and will be resolved when debugging starts."})
                    });
                    if rejected.is_some() {
                        self.rejected.push(id);
                    }
                    set.push((id, bp));
                }
                self.breakpoints.insert(path, set);
                self.respond(seq, command, Ok(json!({ "breakpoints": answer })));
            }
            "setExceptionBreakpoints" => {
                let plain: Vec<String> =
                    serde_json::from_value(args["filters"].clone()).unwrap_or_default();
                let mut filters: Vec<(String, Option<Vec<String>>)> =
                    plain.into_iter().map(|f| (f, None)).collect();
                let options = args["filterOptions"]
                    .as_array()
                    .cloned()
                    .unwrap_or_default();
                if !options.is_empty() && !self.supports("supportsExceptionFilterOptions") {
                    self.respond(
                        seq,
                        command,
                        Err("Failed command 'setExceptionBreakpoints' : filterOptions are not supported".into()),
                    );
                    return true;
                }
                for o in options {
                    let types = o["condition"].as_str().map(|c| {
                        c.split(',')
                            .map(|t| t.trim().to_owned())
                            .filter(|t| !t.is_empty())
                            .collect::<Vec<_>>()
                    });
                    filters.push((
                        o["filterId"].as_str().unwrap_or_default().to_owned(),
                        types.filter(|t| !t.is_empty()),
                    ));
                }
                self.filters = filters;
                self.respond(seq, command, Ok(json!({})));
            }
            "setFunctionBreakpoints" => {
                if !self.supports("supportsFunctionBreakpoints") {
                    self.respond(
                        seq,
                        command,
                        Err("Failed command 'setFunctionBreakpoints' : not supported".into()),
                    );
                    return true;
                }
                let mut answer = Vec::new();
                self.functions.clear();
                for b in args["breakpoints"].as_array().cloned().unwrap_or_default() {
                    let id = self.next_id;
                    self.next_id += 1;
                    let name = b["name"].as_str().unwrap_or_default().to_owned();
                    let verified = self
                        .program
                        .steps
                        .iter()
                        .any(|s| function_matches(&s.function, &name));
                    let mut a = json!({"id": id, "verified": verified});
                    if !verified {
                        a["message"] = json!(format!("No method {name} was found."));
                    }
                    answer.push(a);
                    self.functions
                        .push((id, name, b["condition"].as_str().map(str::to_owned)));
                }
                self.respond(seq, command, Ok(json!({ "breakpoints": answer })));
            }
            "setVariable" | "setExpression" => {
                let r = self.set_value(command, args);
                self.respond(seq, command, r);
            }
            "gotoTargets" => {
                let r = self.goto_targets(args);
                self.respond(seq, command, r);
            }
            "goto" => {
                let target = args["targetId"].as_i64().unwrap_or(-1);
                let k = usize::try_from(target - GOTO_BASE).ok();
                match (self.pc, k) {
                    (Some(_), Some(k))
                        if self.supports("supportsGotoTargetsRequest")
                            && k < self.program.steps.len() =>
                    {
                        let tid = self.thread_id();
                        self.respond(seq, command, Ok(json!({})));
                        self.event(
                            "continued",
                            json!({"threadId": tid, "allThreadsContinued": true}),
                        );
                        self.stop(k, "goto", None);
                    }
                    _ => self.respond(
                        seq,
                        command,
                        Err(format!("Failed command 'goto' : no target {target}")),
                    ),
                }
            }
            "configurationDone" => {
                self.configured = true;
                let (pid, method) = match self.attached {
                    Some(pid) => (pid.unwrap_or(self.program.process_id), "attach"),
                    None => (self.program.process_id, "launch"),
                };
                self.event(
                    "process",
                    json!({"name": "dotnet", "systemProcessId": pid,
                           "isLocalProcess": true, "startMethod": method}),
                );
                self.respond(seq, command, Ok(json!({})));
                let set: Vec<(String, i64, i64)> = self
                    .breakpoints
                    .iter()
                    .flat_map(|(p, v)| v.iter().map(move |(id, b)| (p.clone(), *id, b.line)))
                    .collect();
                for (path, id, line) in set {
                    let mut bp =
                        json!({"id": id, "line": line, "verified": true, "source": {"path": path}});
                    if !self.has_line(&path, line) {
                        bp["verified"] = json!(false);
                        bp["message"] = json!(no_code(line));
                    }
                    self.event("breakpoint", json!({"reason": "changed", "breakpoint": bp}));
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
                        let r = self.alloc(locals, Some((step, Vec::new())));
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
                let origin = self.origins.get(&r).cloned();
                let rows: Vec<Value> = vars
                    .into_iter()
                    .map(|v| self.variable_json(v, origin.as_ref()))
                    .collect();
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
                self.exit(0);
                self.respond(seq, command, Ok(json!({})));
            }
            "disconnect" => {
                // Detaching (an attached session's default, or `terminateDebuggee: false`) leaves the program running:
                // the session ends without `exited`.
                let terminate = args["terminateDebuggee"]
                    .as_bool()
                    .unwrap_or(self.attached.is_none());
                if terminate {
                    self.exit(0);
                } else if !self.exited {
                    self.exited = true;
                    self.event("terminated", json!({}));
                }
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

    /// The locals of the first statement on a line (none when it has no statement).
    fn locals_at(&self, path: &str, line: i64) -> Vec<FakeVar> {
        self.program
            .steps
            .iter()
            .find(|s| s.path == path && s.line == line)
            .map(|s| s.locals.clone())
            .unwrap_or_default()
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

    fn alloc(&mut self, vars: Vec<FakeVar>, origin: Option<(usize, Vec<String>)>) -> i64 {
        let r = self.next_ref;
        self.next_ref += 1;
        self.refs.insert(r, vars);
        if let Some(o) = origin {
            self.origins.insert(r, o);
        }
        r
    }

    /// A variable's row; `parent` is where the variable itself lives (its members live under its name).
    fn variable_json(&mut self, v: FakeVar, parent: Option<&(usize, Vec<String>)>) -> Value {
        let n = v.children.len();
        let reference = if v.children.is_empty() {
            0
        } else {
            let origin = parent.map(|(step, path)| {
                let mut p = path.clone();
                p.push(v.name.clone());
                (*step, p)
            });
            self.alloc(v.children, origin)
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
                let wants = |filter: &str| {
                    self.filters.iter().any(|(f, types)| {
                        f == filter && types.as_ref().is_none_or(|ts| ts.contains(&t.exception))
                    })
                };
                let first_chance = wants("all");
                let unhandled = !t.handled && wants("user-unhandled");
                if first_chance || unhandled {
                    self.exception = Some(t.clone());
                    self.stop(k, "exception", Some(t.message.clone()));
                    self.stopped_on_exception = true;
                    return;
                }
            }
            let entering = k == 0 || {
                let prev = &self.program.steps[k - 1];
                prev.depth < step.depth
                    || (prev.depth == step.depth && prev.function != step.function)
            };
            if entering
                && let Some((id, _, _)) = self.functions.iter().find(|(_, name, condition)| {
                    function_matches(&step.function, name)
                        && condition_holds(condition.as_deref(), &step.locals)
                })
            {
                let id = *id;
                self.stop_with(k, "function breakpoint", None, vec![id]);
                return;
            }
            // A condition the fake cannot evaluate fails at its first hit (brief 0036).
            let failing: Vec<(i64, i64, String)> = self
                .breakpoints
                .get(&step.path)
                .map(|bps| {
                    bps.iter()
                        .filter(|(id, b)| b.line == step.line && !self.rejected.contains(id))
                        .filter_map(|(id, b)| {
                            condition_error(b.condition.as_deref(), &step.locals)
                                .map(|m| (*id, b.line, m))
                        })
                        .collect()
                })
                .unwrap_or_default();
            for (id, line, message) in failing {
                self.rejected.push(id);
                self.event(
                    "breakpoint",
                    json!({"reason": "changed", "breakpoint": {"id": id, "line": line, "verified": false,
                           "message": message, "source": {"path": step.path}}}),
                );
            }
            let rejected = &self.rejected;
            let hit = self.breakpoints.get(&step.path).and_then(|bps| {
                bps.iter()
                    .find(|(id, b)| {
                        b.line == step.line
                            && !rejected.contains(id)
                            && condition_holds(b.condition.as_deref(), &step.locals)
                    })
                    .map(|(_, b)| b.log_message.clone())
            });
            match hit {
                Some(Some(message)) if self.supports("supportsLogPoints") => {
                    let text = interpolate(&message, &step.locals);
                    self.event(
                        "output",
                        json!({"category": "console", "output": format!("{text}\n")}),
                    );
                }
                Some(_) => {
                    self.stop(k, "breakpoint", None);
                    return;
                }
                None => {}
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
        // The run is over; the program waits for the next one, or exits.
        self.pc = None;
        if let Some(code) = self.program.exit_at_end {
            self.exit(code);
        }
    }

    /// The program exits with `code` (once) and the session ends.
    fn exit(&mut self, code: i64) {
        if !self.exited {
            self.exited = true;
            self.event("exited", json!({ "exitCode": code }));
            self.event("terminated", json!({}));
        }
    }

    fn stop(&mut self, k: usize, reason: &str, text: Option<String>) {
        self.stop_with(k, reason, text, Vec::new());
    }

    fn stop_with(&mut self, k: usize, reason: &str, text: Option<String>, hit_ids: Vec<i64>) {
        self.pc = Some(k);
        self.stops += 1;
        self.frames.clear();
        self.refs.clear();
        self.origins.clear();
        let mut body =
            json!({"reason": reason, "threadId": self.thread_id(), "allThreadsStopped": true});
        if let Some(t) = text {
            body["text"] = json!(t);
        }
        if !hit_ids.is_empty() {
            body["hitBreakpointIds"] = json!(hit_ids);
        }
        self.event("stopped", body);
    }

    /// `setVariable` (member `name` of `variablesReference`) or `setExpression` (`expression` in `frameId`): the new
    /// value is kept in the statement's locals.
    fn set_value(&mut self, command: &str, args: &Value) -> Result<Value, String> {
        let capability = if command == "setVariable" {
            "supportsSetVariable"
        } else {
            "supportsSetExpression"
        };
        if !self.supports(capability) {
            return Err(format!("Failed command '{command}' : not supported"));
        }
        let value = args["value"].as_str().unwrap_or_default().trim().to_owned();
        let (step, mut path) = if command == "setVariable" {
            let r = args["variablesReference"].as_i64().unwrap_or(0);
            let (step, mut path) = self
                .origins
                .get(&r)
                .cloned()
                .ok_or_else(|| format!("Failed command '{command}' : unknown reference {r}"))?;
            path.push(args["name"].as_str().unwrap_or_default().to_owned());
            (step, path)
        } else {
            let step = args["frameId"]
                .as_i64()
                .and_then(|f| self.frames.get(&f).copied())
                .or(self.pc)
                .ok_or_else(|| format!("Failed command '{command}' : 0x80131301"))?;
            let path = args["expression"]
                .as_str()
                .unwrap_or_default()
                .split('.')
                .map(|p| p.trim().to_owned())
                .collect::<Vec<_>>();
            (step, path)
        };
        let name = path.pop().unwrap_or_default();
        let mut scope = &mut self.program.steps[step].locals;
        for part in &path {
            scope = &mut scope
                .iter_mut()
                .find(|v| &v.name == part)
                .ok_or_else(|| format!("error: '{part}' is not a member"))?
                .children;
        }
        let var = scope.iter_mut().find(|v| v.name == name).ok_or_else(|| {
            format!("error: The name '{name}' does not exist in the current context")
        })?;
        if var.type_name == "int" && value.parse::<i64>().is_err() {
            return Err(format!(
                "error CS0029: Cannot implicitly convert type '{value}' to 'int'"
            ));
        }
        var.value = value.clone();
        let type_name = var.type_name.clone();
        // What the client read at this stop shows the new value too.
        for vars in self.refs.values_mut() {
            for v in vars.iter_mut().filter(|v| v.name == name) {
                v.value = value.clone();
            }
        }
        Ok(json!({"value": value, "type": type_name, "variablesReference": 0}))
    }

    /// `gotoTargets`: the statements of the stopped method on `line` of `source`.
    fn goto_targets(&mut self, args: &Value) -> Result<Value, String> {
        if !self.supports("supportsGotoTargetsRequest") {
            return Err("Failed command 'gotoTargets' : 0x80004001".into());
        }
        let pc = self
            .pc
            .ok_or_else(|| "Failed command 'gotoTargets' : 0x80131301".to_owned())?;
        let path = args["source"]["path"].as_str().unwrap_or_default();
        let line = args["line"].as_i64().unwrap_or(0);
        let here = &self.program.steps[pc];
        let targets: Vec<Value> = self
            .program
            .steps
            .iter()
            .enumerate()
            .filter(|(_, s)| {
                s.path == path && s.line == line && s.function == here.function && s.depth == here.depth
            })
            .map(|(k, s)| json!({"id": GOTO_BASE + k as i64, "label": format!("{}:{}", s.function, s.line), "line": s.line}))
            .take(1)
            .collect();
        Ok(json!({ "targets": targets }))
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
            let path = expr.split('.').map(str::to_owned).collect();
            self.alloc(v.children.clone(), Some((step, path)))
        };
        Ok(json!({"result": v.value, "type": v.type_name, "variablesReference": reference}))
    }
}

/// `goto` target ids: this plus the statement's index.
const GOTO_BASE: i64 = 5_000;

/// Whether a statement of `function` (`App.Calc.Add(int, int)`) is in the method `name` (`App.Calc.Add`, or a suffix
/// of it at a dot: `Calc.Add`).
fn function_matches(function: &str, name: &str) -> bool {
    let base = function.split('(').next().unwrap_or_default().trim();
    let name = name.split('(').next().unwrap_or_default().trim();
    !name.is_empty() && (base == name || base.ends_with(&format!(".{name}")))
}

/// A log message with its `{expression}` segments evaluated against `locals` (an unknown name is written as
/// `{name: error}`); `{{` and `}}` are literal braces.
fn interpolate(message: &str, locals: &[FakeVar]) -> String {
    let mut out = String::new();
    let mut chars = message.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '{' if chars.peek() == Some(&'{') => {
                chars.next();
                out.push('{');
            }
            '}' if chars.peek() == Some(&'}') => {
                chars.next();
                out.push('}');
            }
            '{' => {
                let expr: String = chars.by_ref().take_while(|c| *c != '}').collect();
                let mut scope = locals;
                let mut value = None;
                for part in expr.trim().split('.') {
                    match scope.iter().find(|v| v.name == part) {
                        Some(v) => {
                            scope = &v.children;
                            value = Some(v.value.clone());
                        }
                        None => {
                            value = None;
                            break;
                        }
                    }
                }
                match value {
                    Some(v) => out.push_str(&v),
                    None => out.push_str(&format!("{{{expr}: error}}")),
                }
            }
            c => out.push(c),
        }
    }
    out
}

/// `name == value`, `name != value` (value compared as shown, quotes optional), `true`, `false`; anything else holds.
/// What the fake answers for a breakpoint on a line without a statement (brief 0036).
fn no_code(line: i64) -> String {
    format!("The breakpoint location is invalid: line {line} has no code.")
}

/// Why the fake cannot evaluate `condition` (brief 0036): a comparison to a dotted name whose first part is no local
/// (`coin == Coin.Quarter`), as Mono's evaluator once failed on an unqualified type name.
fn condition_error(condition: Option<&str>, locals: &[FakeVar]) -> Option<String> {
    let c = condition?.trim();
    let op = c.find("==").or_else(|| c.find("!="))?;
    let value = c[op + 2..].trim();
    let (first, rest) = value.split_once('.')?;
    let identifier = |t: &str| {
        t.chars().next().is_some_and(|ch| ch.is_ascii_uppercase())
            && t.chars().all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
    };
    (identifier(first) && !rest.is_empty() && !locals.iter().any(|v| v.name == first))
        .then(|| format!("Unknown identifier: {first}"))
}

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
