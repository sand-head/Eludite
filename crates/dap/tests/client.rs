//! The DAP client against the scripted fake adapter, in-process and over TCP: the handshake, launch and attach,
//! breakpoints set and hit, stack and variables, evaluate, steps, continue, stop, and an adapter that crashes.

mod common;

use std::time::Duration;

use eludite_dap::fake::{self, FakeHandle, FakeThrow};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{
    EvaluateResponse, Event, ScopesResponse, SourceBreakpoint, StackTraceResponse, ThreadsResponse,
    VariablesResponse,
};
use eludite_dap::{AdapterTransport, ClientEvent, DapClient, DapError, transport};
use serde_json::{Value, json};

use common::{CALC, PROGRAM, Recorder, T, program};

fn plan(kind: StartKind) -> StartPlan {
    StartPlan {
        adapter_id: "coreclr".into(),
        kind,
        arguments: json!({"program": "/src/App/bin/Debug/net10.0/App.dll"}),
        breakpoints: vec![
            (
                CALC.into(),
                vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }],
            ),
            (
                PROGRAM.into(),
                vec![
                    SourceBreakpoint {
                        line: 7,
                        condition: Some("x == 1".into()),
                        ..Default::default()
                    },
                    SourceBreakpoint {
                        line: 99,
                        ..Default::default()
                    },
                ],
            ),
        ],
        exception_filters: vec!["user-unhandled".into(), "not-offered".into()],
        exception_options: Vec::new(),
        function_breakpoints: Vec::new(),
    }
}

fn wait<T: serde::de::DeserializeOwned>(c: &DapClient, command: &str, args: Value) -> T {
    serde_json::from_value(c.request_wait(command, args, T).unwrap()).unwrap()
}

fn top(c: &DapClient, thread: i64) -> StackTraceResponse {
    wait(
        c,
        "stackTrace",
        json!({"threadId": thread, "startFrame": 0, "levels": 50}),
    )
}

fn locals(c: &DapClient, frame: i64) -> VariablesResponse {
    let scopes: ScopesResponse = wait(c, "scopes", json!({"frameId": frame}));
    assert_eq!(scopes.scopes[0].name, "Locals");
    wait(
        c,
        "variables",
        json!({"variablesReference": scopes.scopes[0].variables_reference}),
    )
}

/// The whole session against a fake already connected to `client`.
fn session(client: &DapClient, handle: &FakeHandle, rec: &Recorder) {
    // Handshake: initialize, launch, initialized, breakpoints (pending until configurationDone), filters,
    // configurationDone, the launch answer.
    let started = session::start(client, &plan(StartKind::Launch), T).unwrap();
    assert!(started.capabilities.supports_conditional_breakpoints);
    assert!(!started.capabilities.supports_hit_conditional_breakpoints);
    assert_eq!(started.exception_filters, ["user-unhandled"]);
    assert_eq!(started.breakpoints.len(), 2);
    assert!(
        started
            .breakpoints
            .iter()
            .all(|(_, b)| b.iter().all(|b| !b.verified))
    );
    assert_eq!(
        handle.commands(),
        [
            "initialize",
            "launch",
            "setBreakpoints",
            "setBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone"
        ]
    );
    // The two breakpoints on real lines bind once the program runs; line 99 stays pending.
    for n in 1..=2 {
        rec.wait_nth(
            n,
            "breakpoint changed",
            |e| matches!(e, ClientEvent::Event(Event::Breakpoint(b)) if b.breakpoint.verified),
        );
    }
    rec.wait_nth(
        1,
        "process",
        |e| matches!(e, ClientEvent::Event(Event::Process(p)) if p.system_process_id == Some(4242)),
    );
    rec.wait_nth(
        1,
        "output",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output == "listening\n"),
    );
    let threads: ThreadsResponse = wait(client, "threads", Value::Null);
    assert_eq!(threads.threads.len(), 2);

    // A run hits the breakpoint in Calc.Add.
    handle.trigger();
    let s = rec.stopped(1);
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    let st = top(client, tid);
    let lines: Vec<(String, i64)> = st
        .stack_frames
        .iter()
        .map(|f| (f.name.clone(), f.line))
        .collect();
    assert_eq!(
        lines,
        [
            ("App.Calc.Add(int, int)".to_owned(), 10),
            ("App.Program.Main()".to_owned(), 6),
            ("[Native Frames]".to_owned(), 0)
        ]
    );
    assert_eq!(st.stack_frames[0].path(), Some(CALC));
    assert_eq!(st.stack_frames[2].path(), None);
    let v = locals(client, st.stack_frames[0].id);
    assert_eq!(
        v.variables
            .iter()
            .map(|v| (v.name.as_str(), v.value.as_str()))
            .collect::<Vec<_>>(),
        [("a", "1"), ("b", "2")]
    );
    // The caller's frame has its own locals.
    let v = locals(client, st.stack_frames[1].id);
    assert_eq!(v.variables[1].name, "x");

    // Evaluate in the top frame; an unknown name is the adapter's error.
    let e: EvaluateResponse = wait(
        client,
        "evaluate",
        json!({"expression": "a", "frameId": st.stack_frames[0].id, "context": "watch"}),
    );
    assert_eq!(
        (e.result.as_str(), e.type_name.as_deref()),
        ("1", Some("int"))
    );
    let err = client
        .request_wait(
            "evaluate",
            json!({"expression": "nope", "frameId": st.stack_frames[0].id}),
            T,
        )
        .unwrap_err();
    assert!(
        matches!(&err, DapError::Failed { message, .. } if message.contains("does not exist")),
        "{err}"
    );

    // Step over within Calc.Add, then out to Main, where the conditional breakpoint on line 7 holds.
    client
        .request_wait("next", json!({"threadId": tid}), T)
        .unwrap();
    let s = rec.stopped(2);
    assert_eq!(s.reason, "step");
    assert_eq!(top(client, tid).stack_frames[0].line, 11);
    rec.wait_nth(1, "continued", |e| {
        matches!(e, ClientEvent::Event(Event::Continued(_)))
    });
    client
        .request_wait("stepOut", json!({"threadId": tid}), T)
        .unwrap();
    rec.stopped(3);
    let st = top(client, tid);
    assert_eq!((st.stack_frames[0].line, st.stack_frames.len()), (7, 2));
    // Members expand lazily through variables.
    let v = locals(client, st.stack_frames[0].id);
    let order = v.variables.iter().find(|v| v.name == "order").unwrap();
    assert!(order.variables_reference > 0);
    let members: VariablesResponse = wait(
        client,
        "variables",
        json!({"variablesReference": order.variables_reference}),
    );
    assert_eq!(members.variables[1].value, "\"A\"");
    let e: EvaluateResponse = wait(
        client,
        "evaluate",
        json!({"expression": "order.Name", "frameId": st.stack_frames[0].id, "context": "hover"}),
    );
    assert_eq!(e.result, "\"A\"");

    // Step into goes to the next statement.
    client
        .request_wait("stepIn", json!({"threadId": tid}), T)
        .unwrap();
    rec.stopped(4);
    assert_eq!(top(client, tid).stack_frames[0].line, 8);

    // Continue: the handled exception on line 9 does not break with user-unhandled only; the run ends and the
    // program waits. A step now is refused by the adapter.
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    std::thread::sleep(Duration::from_millis(50));
    assert_eq!(
        rec.events()
            .iter()
            .filter(|e| matches!(e, ClientEvent::Event(Event::Stopped(_))))
            .count(),
        4
    );
    assert!(
        client
            .request_wait("next", json!({"threadId": tid}), T)
            .is_err()
    );

    // Stop: disconnect ends the session normally.
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
    match rec.closed() {
        ClientEvent::Closed { terminated, .. } => assert!(terminated),
        _ => unreachable!(),
    }
    assert!(client.is_closed());
    assert_eq!(
        client.request("threads", Value::Null),
        Err(DapError::Closed)
    );
}

#[test]
fn full_session_in_process() {
    let (conn, handle) = fake::connect(program());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    session(&client, &handle, &rec);
}

#[test]
fn tcp_transport_round_trip() {
    let (port, handle) = fake::listen_tcp(program()).unwrap();
    let transport = AdapterTransport::Tcp {
        host: "127.0.0.1".into(),
        port,
    };
    let conn = transport::connect(&transport).unwrap();
    assert_eq!(conn.description, format!("tcp 127.0.0.1:{port}"));
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    session(&client, &handle, &rec);
    assert!(handle.wait_for("disconnect", 1, T));
}

#[test]
fn responses_reach_the_sink_without_waiting() {
    let (conn, handle) = fake::connect(program());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    session::start(&client, &plan(StartKind::Attach), T).unwrap();
    assert!(handle.commands().contains(&"attach".to_owned()));
    // The adapter hangs: `request` still returns at once.
    handle.stall(Duration::from_millis(300));
    let t = std::time::Instant::now();
    let seq = client.request("threads", Value::Null).unwrap();
    common::assert_budget("request", t.elapsed(), Duration::from_millis(50));
    let r = rec.wait_nth(
        1,
        "threads response",
        |e| matches!(e, ClientEvent::Response { request_seq, .. } if *request_seq == seq),
    );
    match r {
        ClientEvent::Response {
            command, result, ..
        } => {
            assert_eq!(command, "threads");
            assert_eq!(result.unwrap()["threads"][0]["name"], "Main Thread");
        }
        _ => unreachable!(),
    }
}

#[test]
fn adapter_crash_fails_pending_requests_and_reports_closed() {
    let (conn, handle) = fake::connect(program());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    session::start(&client, &plan(StartKind::Launch), T).unwrap();
    handle.trigger();
    rec.stopped(1);
    // A long stall: the crash must land while the requests are still unanswered.
    handle.stall(Duration::from_secs(10));
    let pending = client.request_channel("threads", Value::Null).unwrap();
    let sunk = client
        .request("stackTrace", json!({"threadId": 1}))
        .unwrap();
    handle.crash();
    assert_eq!(pending.recv_timeout(T).unwrap(), Err(DapError::Closed));
    rec.wait_nth(1, "failed stackTrace", |e| {
        matches!(e, ClientEvent::Response { request_seq, result: Err(_), .. } if *request_seq == sunk)
    });
    match rec.closed() {
        ClientEvent::Closed { terminated, .. } => assert!(!terminated),
        _ => unreachable!(),
    }
    assert!(client.is_closed());
}

#[test]
fn first_chance_exceptions_and_ignored_hit_conditions() {
    let mut p = program();
    // An unhandled exception too, on its own statement.
    let mut unhandled = p.steps[5].clone();
    unhandled.line = 12;
    unhandled.throws = Some(FakeThrow {
        exception: "System.NullReferenceException".into(),
        message: "null".into(),
        handled: false,
        ..Default::default()
    });
    p.steps.push(unhandled);
    let (conn, handle) = fake::connect(p);
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let mut plan = plan(StartKind::Launch);
    plan.breakpoints = vec![(
        CALC.into(),
        vec![SourceBreakpoint {
            line: 10,
            hit_condition: Some("2".into()),
            ..Default::default()
        }],
    )];
    plan.exception_filters = vec!["all".into()];
    session::start(&client, &plan, T).unwrap();
    handle.trigger();
    // netcoredbg ignores hitCondition: the first hit breaks. Clients emulate hit counts.
    let s = rec.stopped(1);
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    assert!(
        client
            .request_wait("exceptionInfo", json!({"threadId": tid}), T)
            .is_err()
    );
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    let s = rec.stopped(2);
    assert_eq!(
        (s.reason.as_str(), s.text.as_deref()),
        ("exception", Some("boom"))
    );
    let info = client
        .request_wait("exceptionInfo", json!({"threadId": tid}), T)
        .unwrap();
    assert_eq!(info["exceptionId"], "System.InvalidOperationException");
    // Resuming does not throw the same exception again; the next one breaks too.
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    let s = rec.stopped(3);
    assert_eq!(s.text.as_deref(), Some("null"));
    assert_eq!(top(&client, tid).stack_frames[0].line, 12);
}
