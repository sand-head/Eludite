//! Brief 0026's requests: their shapes as DAP defines them (`logMessage` on source breakpoints,
//! `setFunctionBreakpoints`, `setExceptionBreakpoints` with `filterOptions`, `setVariable`, `setExpression`,
//! `gotoTargets` and `goto`), and the scripted fake adapter's answers through `DapClient`: function breakpoints, filter
//! options by exception type, values changed and read back, Set Next Statement and log points behind their flags, and
//! each refused where its capability is off.

mod common;

use eludite_dap::fake::{self, FakeHandle, FakeProgram, FakeStep, FakeThrow, FakeVar};
use eludite_dap::session::{self, StartKind, StartPlan, Started};
use eludite_dap::types::{
    Event, ExceptionFilterOptions, FunctionBreakpoint, GotoArguments, GotoTargetsArguments,
    GotoTargetsResponse, SetExpressionArguments, SetVariableArguments, SetVariableResponse, Source,
    SourceBreakpoint, StackTraceResponse, VariablesResponse,
};
use eludite_dap::{ClientEvent, DapClient, DapError};
use serde_json::{Value, json};

use common::{CALC, PROGRAM, Recorder, T, program};

#[test]
fn requests_have_dap_shapes() {
    let bp = SourceBreakpoint {
        line: 7,
        condition: Some("i > 2".into()),
        log_message: Some("i = {i}".into()),
        ..Default::default()
    };
    assert_eq!(
        session::set_breakpoints_arguments("/s/A.cs", &[bp]),
        json!({"source": {"name": "A.cs", "path": "/s/A.cs"},
               "breakpoints": [{"line": 7, "condition": "i > 2", "logMessage": "i = {i}"}]})
    );
    assert_eq!(
        session::set_function_breakpoints_arguments(&[FunctionBreakpoint {
            name: "App.Calc.Add".into(),
            condition: Some("a == 1".into()),
            hit_condition: Some("2".into()),
        }]),
        json!({"breakpoints": [{"name": "App.Calc.Add", "condition": "a == 1", "hitCondition": "2"}]})
    );
    assert_eq!(
        session::set_exception_breakpoints_arguments(
            &["user-unhandled".to_owned()],
            &[ExceptionFilterOptions {
                filter_id: "all".into(),
                condition: Some("System.InvalidOperationException, System.FormatException".into()),
            }]
        ),
        json!({"filters": ["user-unhandled"],
               "filterOptions": [{"filterId": "all",
                                  "condition": "System.InvalidOperationException, System.FormatException"}]})
    );
    // Without options the member is left out (adapters without supportsExceptionFilterOptions).
    assert_eq!(
        session::set_exception_breakpoints_arguments(&["all".to_owned()], &[]),
        json!({"filters": ["all"]})
    );
    assert_eq!(
        serde_json::to_value(SetVariableArguments {
            variables_reference: 3,
            name: "x".into(),
            value: "42".into()
        })
        .unwrap(),
        json!({"variablesReference": 3, "name": "x", "value": "42"})
    );
    assert_eq!(
        serde_json::to_value(SetExpressionArguments {
            expression: "order.Name".into(),
            value: "\"B\"".into(),
            frame_id: Some(1000)
        })
        .unwrap(),
        json!({"expression": "order.Name", "value": "\"B\"", "frameId": 1000})
    );
    assert_eq!(
        serde_json::to_value(GotoTargetsArguments {
            source: Source {
                name: None,
                path: Some("/s/A.cs".into())
            },
            line: 9
        })
        .unwrap(),
        json!({"source": {"path": "/s/A.cs"}, "line": 9})
    );
    assert_eq!(
        serde_json::to_value(GotoArguments {
            thread_id: 1,
            target_id: 5004
        })
        .unwrap(),
        json!({"threadId": 1, "targetId": 5004})
    );
    let answer: SetVariableResponse =
        serde_json::from_value(json!({"value": "42", "type": "int", "variablesReference": 0}))
            .unwrap();
    assert_eq!(answer.value, "42");
    let targets: GotoTargetsResponse = serde_json::from_value(
        json!({"targets": [{"id": 1, "label": "Main:9", "line": 9, "column": 1}]}),
    )
    .unwrap();
    assert_eq!(targets.targets[0].line, 9);
}

struct Session {
    client: DapClient,
    handle: FakeHandle,
    rec: Recorder,
    started: Started,
}

fn start(
    p: FakeProgram,
    breakpoints: Vec<SourceBreakpoint>,
    functions: Vec<FunctionBreakpoint>,
    filters: &[&str],
    options: Vec<ExceptionFilterOptions>,
) -> Session {
    let (conn, handle) = fake::connect(p);
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: json!({"program": "/src/App/bin/Debug/net10.0/App.dll"}),
            breakpoints: vec![(PROGRAM.into(), breakpoints)],
            exception_filters: filters.iter().map(|f| (*f).to_owned()).collect(),
            exception_options: options,
            function_breakpoints: functions,
        },
        T,
    )
    .unwrap();
    Session {
        client,
        handle,
        rec,
        started,
    }
}

fn wait<T: serde::de::DeserializeOwned>(c: &DapClient, command: &str, args: Value) -> T {
    serde_json::from_value(c.request_wait(command, args, T).unwrap()).unwrap()
}

fn line(l: i64) -> SourceBreakpoint {
    SourceBreakpoint {
        line: l,
        ..Default::default()
    }
}

fn func(name: &str) -> FunctionBreakpoint {
    FunctionBreakpoint {
        name: name.into(),
        ..Default::default()
    }
}

fn top(s: &Session, tid: i64) -> (i64, i64, String) {
    let st: StackTraceResponse = wait(&s.client, "stackTrace", json!({"threadId": tid}));
    let f = &st.stack_frames[0];
    (f.id, f.line, f.name.clone())
}

#[test]
fn function_breakpoints_bind_by_name_and_stop_on_entry() {
    let s = start(
        program(),
        vec![],
        vec![func("App.Calc.Add"), func("Calc.Missing")],
        &[],
        vec![],
    );
    assert!(s.started.capabilities.supports_function_breakpoints);
    assert_eq!(s.started.function_breakpoints.len(), 2);
    assert!(s.started.function_breakpoints[0].verified);
    assert!(!s.started.function_breakpoints[1].verified);
    assert!(
        s.started.function_breakpoints[1]
            .message
            .as_deref()
            .unwrap()
            .contains("Calc.Missing")
    );
    s.handle.trigger();
    let stopped = s.rec.stopped(1);
    assert_eq!(stopped.reason, "function breakpoint");
    assert_eq!(
        stopped.hit_breakpoint_ids,
        [s.started.function_breakpoints[0].id.unwrap()]
    );
    let tid = stopped.thread_id.unwrap();
    // The first statement of Add, not the second.
    assert_eq!(top(&s, tid).1, 10);
    s.client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    // A condition on a function breakpoint.
    let r: Value = wait(
        &s.client,
        "setFunctionBreakpoints",
        json!({"breakpoints": [{"name": "Calc.Add", "condition": "a == 5"}]}),
    );
    assert_eq!(r["breakpoints"][0]["verified"], true);
    s.handle.trigger();
    std::thread::sleep(std::time::Duration::from_millis(100));
    let stops = s
        .rec
        .events()
        .iter()
        .filter(|e| matches!(e, ClientEvent::Event(Event::Stopped(_))))
        .count();
    assert_eq!(stops, 1, "a = 1: the condition does not hold");

    // Turned off: refused, and the handshake leaves them out.
    let mut p = program();
    p.extra_capabilities = json!({"supportsFunctionBreakpoints": false});
    let s = start(p, vec![], vec![func("App.Calc.Add")], &[], vec![]);
    assert!(s.started.function_breakpoints.is_empty());
    assert!(
        !s.handle
            .commands()
            .contains(&"setFunctionBreakpoints".to_owned())
    );
    let e = s
        .client
        .request_wait(
            "setFunctionBreakpoints",
            json!({"breakpoints": [{"name": "App.Calc.Add"}]}),
            T,
        )
        .unwrap_err();
    assert!(matches!(e, DapError::Failed { .. }), "{e}");
}

/// Statement 0 throws a handled `FormatException`, statement 1 a handled `InvalidOperationException`.
fn throwing() -> FakeProgram {
    let mut a = FakeStep::new(PROGRAM, 5, "App.Program.Main()", 0, vec![]);
    a.throws = Some(FakeThrow::new("System.FormatException", "bad", true));
    let mut b = FakeStep::new(PROGRAM, 6, "App.Program.Main()", 0, vec![]);
    b.throws = Some(FakeThrow::new(
        "System.InvalidOperationException",
        "boom",
        true,
    ));
    FakeProgram {
        steps: vec![a, b],
        ..FakeProgram::default()
    }
}

#[test]
fn filter_options_stop_for_the_types_named() {
    let s = start(
        throwing(),
        vec![],
        vec![],
        &["user-unhandled"],
        vec![ExceptionFilterOptions {
            filter_id: "all".into(),
            condition: Some("System.InvalidOperationException".into()),
        }],
    );
    let sent = s.handle.last("setExceptionBreakpoints").unwrap();
    assert_eq!(
        sent,
        json!({"filters": ["user-unhandled"],
               "filterOptions": [{"filterId": "all", "condition": "System.InvalidOperationException"}]})
    );
    s.handle.trigger();
    let stopped = s.rec.stopped(1);
    assert_eq!(stopped.reason, "exception");
    assert_eq!(
        stopped.text.as_deref(),
        Some("boom"),
        "the FormatException passed"
    );
    let tid = stopped.thread_id.unwrap();
    s.client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    // Both types, as the shell sends them (comma-separated).
    s.client
        .request_wait(
            "setExceptionBreakpoints",
            session::set_exception_breakpoints_arguments(
                &[],
                &[ExceptionFilterOptions {
                    filter_id: "all".into(),
                    condition: Some(
                        "System.FormatException, System.InvalidOperationException".into(),
                    ),
                }],
            ),
            T,
        )
        .unwrap();
    s.handle.trigger();
    assert_eq!(s.rec.stopped(2).text.as_deref(), Some("bad"));

    // Without the capability the options are not sent at start, and refused later.
    let mut p = throwing();
    p.extra_capabilities = json!({"supportsExceptionFilterOptions": false});
    let s = start(
        p,
        vec![],
        vec![],
        &["user-unhandled"],
        vec![ExceptionFilterOptions {
            filter_id: "all".into(),
            condition: Some("System.InvalidOperationException".into()),
        }],
    );
    assert_eq!(
        s.handle.last("setExceptionBreakpoints").unwrap(),
        json!({"filters": ["user-unhandled"]})
    );
    assert!(
        s.client
            .request_wait(
                "setExceptionBreakpoints",
                json!({"filters": [], "filterOptions": [{"filterId": "all"}]}),
                T
            )
            .is_err()
    );
}

#[test]
fn values_change_through_set_variable_and_set_expression() {
    let s = start(program(), vec![line(7)], vec![], &[], vec![]);
    s.handle.trigger();
    let tid = s.rec.stopped(1).thread_id.unwrap();
    let (frame, l, _) = top(&s, tid);
    assert_eq!(l, 7);
    let scopes: Value = wait(&s.client, "scopes", json!({"frameId": frame}));
    let locals = scopes["scopes"][0]["variablesReference"].as_i64().unwrap();
    let a: SetVariableResponse = wait(
        &s.client,
        "setVariable",
        json!({"variablesReference": locals, "name": "y", "value": "42"}),
    );
    assert_eq!(
        (a.value.as_str(), a.type_name.as_deref()),
        ("42", Some("int"))
    );
    let e = s
        .client
        .request_wait(
            "setVariable",
            json!({"variablesReference": locals, "name": "y", "value": "\"text\""}),
            T,
        )
        .unwrap_err();
    assert!(e.to_string().contains("CS0029"), "{e}");
    // A member, by the reference of its object.
    let vars: VariablesResponse = wait(
        &s.client,
        "variables",
        json!({"variablesReference": locals}),
    );
    let order = vars.variables.iter().find(|v| v.name == "order").unwrap();
    assert_eq!(
        vars.variables.iter().find(|v| v.name == "y").unwrap().value,
        "42"
    );
    wait::<Value>(
        &s.client,
        "setVariable",
        json!({"variablesReference": order.variables_reference, "name": "Name", "value": "\"B\""}),
    );
    // setExpression in the frame.
    wait::<Value>(
        &s.client,
        "setExpression",
        json!({"expression": "x", "value": "9", "frameId": frame}),
    );
    // Read back: a fresh read of the frame.
    let scopes: Value = wait(&s.client, "scopes", json!({"frameId": frame}));
    let vars: VariablesResponse = wait(
        &s.client,
        "variables",
        json!({"variablesReference": scopes["scopes"][0]["variablesReference"]}),
    );
    let value = |n: &str| {
        vars.variables
            .iter()
            .find(|v| v.name == n)
            .unwrap()
            .value
            .clone()
    };
    assert_eq!((value("x"), value("y")), ("9".to_owned(), "42".to_owned()));
    let order = vars.variables.iter().find(|v| v.name == "order").unwrap();
    let members: VariablesResponse = wait(
        &s.client,
        "variables",
        json!({"variablesReference": order.variables_reference}),
    );
    assert_eq!(members.variables[1].value, "\"B\"");
    let e: Value = wait(
        &s.client,
        "evaluate",
        json!({"expression": "order.Name", "frameId": frame}),
    );
    assert_eq!(e["result"], "\"B\"");

    // setVariable off: refused; setExpression still works (the shell's fallback).
    let mut p = program();
    p.extra_capabilities = json!({"supportsSetVariable": false});
    let s = start(p, vec![line(7)], vec![], &[], vec![]);
    assert!(!s.started.capabilities.supports_set_variable);
    assert!(s.started.capabilities.supports_set_expression);
    s.handle.trigger();
    let tid = s.rec.stopped(1).thread_id.unwrap();
    let (frame, _, _) = top(&s, tid);
    let scopes: Value = wait(&s.client, "scopes", json!({"frameId": frame}));
    assert!(
        s.client
            .request_wait(
                "setVariable",
                json!({"variablesReference": scopes["scopes"][0]["variablesReference"], "name": "y", "value": "1"}),
                T
            )
            .is_err()
    );
    wait::<Value>(
        &s.client,
        "setExpression",
        json!({"expression": "y", "value": "5", "frameId": frame}),
    );
}

#[test]
fn goto_moves_the_execution_point_behind_its_flag() {
    // Without the flag (netcoredbg, eludite-dbg-mono): gotoTargets fails.
    let s = start(program(), vec![line(7)], vec![], &[], vec![]);
    assert!(!s.started.capabilities.supports_goto_targets_request);
    s.handle.trigger();
    s.rec.stopped(1);
    assert!(
        s.client
            .request_wait(
                "gotoTargets",
                json!({"source": {"path": PROGRAM}, "line": 5}),
                T
            )
            .is_err()
    );

    let mut p = program();
    p.extra_capabilities = json!({"supportsGotoTargetsRequest": true});
    let s = start(p, vec![line(7)], vec![], &[], vec![]);
    assert!(s.started.capabilities.supports_goto_targets_request);
    s.handle.trigger();
    let tid = s.rec.stopped(1).thread_id.unwrap();
    // Back to line 6 of Main (Calc's lines are another method: no target).
    let none: GotoTargetsResponse = wait(
        &s.client,
        "gotoTargets",
        json!({"source": {"path": CALC}, "line": 10}),
    );
    assert!(none.targets.is_empty());
    let t: GotoTargetsResponse = wait(
        &s.client,
        "gotoTargets",
        json!({"source": {"path": PROGRAM}, "line": 6}),
    );
    assert_eq!(t.targets.len(), 1);
    s.client
        .request_wait(
            "goto",
            serde_json::to_value(GotoArguments {
                thread_id: tid,
                target_id: t.targets[0].id,
            })
            .unwrap(),
            T,
        )
        .unwrap();
    let stopped = s.rec.stopped(2);
    assert_eq!(stopped.reason, "goto");
    assert_eq!(top(&s, tid).1, 6);
    // Running on from there passes line 6's callee again and stops at the breakpoint on 7.
    s.client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    assert_eq!(s.rec.stopped(3).reason, "breakpoint");
    assert_eq!(top(&s, tid).1, 7);
}

#[test]
fn log_points_print_without_stopping_behind_their_flag() {
    let traced = || {
        let mut p = FakeProgram {
            steps: fake::hot_loop(PROGRAM, 12, "App.Program.Main()", 0, 5),
            exit_at_end: Some(0),
            ..FakeProgram::default()
        };
        p.steps.push(FakeStep::new(
            PROGRAM,
            14,
            "App.Program.Main()",
            0,
            vec![FakeVar::new("i", "5", "int")],
        ));
        p
    };
    let bp = SourceBreakpoint {
        line: 12,
        log_message: Some("i={i} sum={sum} {{literal}} {nope}".into()),
        ..Default::default()
    };
    // With supportsLogPoints: lines, no stop.
    let mut p = traced();
    p.extra_capabilities = json!({"supportsLogPoints": true});
    let s = start(p, vec![bp.clone()], vec![], &[], vec![]);
    assert!(s.started.capabilities.supports_log_points);
    s.handle.trigger();
    s.rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    let lines: Vec<String> = s
        .rec
        .events()
        .into_iter()
        .filter_map(|e| match e {
            ClientEvent::Event(Event::Output(o)) if o.category.as_deref() == Some("console") => {
                Some(o.output)
            }
            _ => None,
        })
        .collect();
    assert_eq!(lines.len(), 5);
    assert_eq!(lines[0], "i=0 sum=0 {literal} {nope: error}\n");
    assert_eq!(lines[4], "i=4 sum=10 {literal} {nope: error}\n");
    assert!(
        !s.rec
            .events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Stopped(_))))
    );
    // Without it (netcoredbg): the log message is ignored and the breakpoint breaks, five times.
    let s = start(traced(), vec![bp], vec![], &[], vec![]);
    s.handle.trigger();
    for n in 1..=5 {
        let st = s.rec.stopped(n);
        assert_eq!(st.reason, "breakpoint");
        s.client
            .request_wait("continue", json!({"threadId": st.thread_id.unwrap()}), T)
            .unwrap();
    }
    s.rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
}
