//! The DAP client against the fake adapter as a real child process over stdio: a session, and an adapter that
//! crashes mid-session (exit code 3) while requests are pending.
//!
//! This binary is both the fake adapter (with `--serve-fake-dap`) and the test runner: it re-executes itself as the
//! adapter. It has no libtest harness because the harness would print to stdout, the protocol stream.

mod common;

use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::SourceBreakpoint;
use eludite_dap::{AdapterTransport, ClientEvent, DapClient, DapError, transport};
use serde_json::{Value, json};

use common::{CALC, Recorder, T, program};

const SERVE: &str = "--serve-fake-dap";

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some(SERVE) {
        let mut p = program();
        p.run_at_start = true;
        p.crash_on = args.get(2).cloned();
        eludite_dap::fake::serve_stdio(p);
        return;
    }
    stdio_session();
    println!("stdio: session ok");
    stdio_crash();
    println!("stdio: crash ok");
}

fn spawn(crash_on: Option<&str>) -> (DapClient, Recorder) {
    let exe = std::env::current_exe().unwrap();
    let mut args = vec![SERVE.to_owned()];
    args.extend(crash_on.map(str::to_owned));
    let t = AdapterTransport::Stdio {
        command: exe.to_string_lossy().into_owned(),
        args,
    };
    let conn = transport::connect(&t).unwrap();
    assert!(conn.child.is_some());
    let rec = Recorder::default();
    (DapClient::start(conn, rec.sink()), rec)
}

fn plan() -> StartPlan {
    StartPlan {
        adapter_id: "coreclr".into(),
        kind: StartKind::Launch,
        arguments: json!({}),
        breakpoints: vec![(
            CALC.into(),
            vec![SourceBreakpoint {
                line: 11,
                ..Default::default()
            }],
        )],
        function_breakpoints: Vec::new(),
        exception_filters: vec![],
    }
}

fn stdio_session() {
    let (client, rec) = spawn(None);
    assert!(client.adapter_pid().is_some());
    session::start(&client, &plan(), T).unwrap();
    // A console program: it runs at configurationDone and breaks in Calc.Add.
    let s = rec.stopped(1);
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid}), T)
        .unwrap();
    assert_eq!(st["stackFrames"][0]["line"], 11);
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
    match rec.closed() {
        ClientEvent::Closed {
            terminated,
            exit_code,
            ..
        } => {
            assert!(terminated);
            assert_eq!(exit_code, Some(0));
        }
        _ => unreachable!(),
    }
}

fn stdio_crash() {
    let (client, rec) = spawn(Some("next"));
    session::start(&client, &plan(), T).unwrap();
    let s = rec.stopped(1);
    let r = client.request_wait("next", json!({"threadId": s.thread_id}), T);
    assert_eq!(r, Err(DapError::Closed));
    match rec.closed() {
        ClientEvent::Closed {
            terminated,
            exit_code,
            ..
        } => {
            assert!(!terminated);
            assert_eq!(exit_code, Some(3));
        }
        _ => unreachable!(),
    }
    assert_eq!(
        client.request("threads", Value::Null),
        Err(DapError::Closed)
    );
}
