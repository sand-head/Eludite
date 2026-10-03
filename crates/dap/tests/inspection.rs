//! Brief 0025's requests against the scripted fake adapter through `DapClient`: `pause` of a statement that runs until
//! paused, `exceptionInfo` with details and inner exceptions, `stackTrace` paging with `startFrame` and `levels`,
//! `variables` paging with `start` and `count` over a 10,000-element array, a deep object graph, output per
//! category, another thread's stack, and the same requests when the fake does not page (the client then gets
//! everything, as from netcoredbg).

mod common;

use eludite_dap::fake::{self, FakeHandle, FakeProgram, FakeStep, FakeThrow, FakeVar};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{
    ExceptionInfoResponse, ScopesResponse, SourceBreakpoint, StackTraceResponse, VariablesResponse,
};
use eludite_dap::{ClientEvent, DapClient, DapError};
use serde_json::{Value, json};

use common::{PROGRAM, Recorder, T};

fn start(
    p: FakeProgram,
    breakpoints: &[i64],
    filters: &[&str],
) -> (DapClient, FakeHandle, Recorder) {
    let (conn, handle) = fake::connect(p);
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let plan = StartPlan {
        adapter_id: "coreclr".into(),
        kind: StartKind::Launch,
        arguments: json!({"program": "/src/App/bin/Debug/net10.0/App.dll"}),
        breakpoints: vec![(
            PROGRAM.into(),
            breakpoints
                .iter()
                .map(|l| SourceBreakpoint {
                    line: *l,
                    ..Default::default()
                })
                .collect(),
        )],
        function_breakpoints: Vec::new(),
        exception_filters: filters.iter().map(|f| (*f).to_owned()).collect(),
    };
    session::start(&client, &plan, T).unwrap();
    (client, handle, rec)
}

fn wait<T: serde::de::DeserializeOwned>(c: &DapClient, command: &str, args: Value) -> T {
    serde_json::from_value(c.request_wait(command, args, T).unwrap()).unwrap()
}

/// Thirty nested calls (`F0` to `F29`, one statement each on line 100 + depth); the deepest holds 120 locals, a
/// 10,000-element array, a 1,000-character string and a five-level object graph.
fn deep_program() -> FakeProgram {
    let mut steps: Vec<FakeStep> = (0..30)
        .map(|d| {
            FakeStep::new(
                PROGRAM,
                100 + d as i64,
                &format!("App.F{d}()"),
                d,
                Vec::new(),
            )
        })
        .collect();
    let mut locals: Vec<FakeVar> = (0..120)
        .map(|i| FakeVar::new(&format!("v{i:03}"), &i.to_string(), "int"))
        .collect();
    locals.push(FakeVar::array("big", 10_000));
    locals.push(FakeVar::new("text", &"x".repeat(1000), "string"));
    locals.push(FakeVar::deep("graph", 5, 3));
    steps[29].locals = locals;
    FakeProgram {
        steps,
        other_threads: vec![(2, ".NET TP Worker".into())],
        ..FakeProgram::default()
    }
}

#[test]
fn stack_and_variables_page_and_large_sets_expand() {
    let (client, handle, rec) = start(deep_program(), &[129], &[]);
    let caps = client.capabilities();
    assert!(caps.supports_delayed_stack_trace_loading && caps.supports_variable_paging);
    handle.trigger();
    let s = rec.stopped(1);
    let tid = s.thread_id.unwrap();
    // 30 managed frames and [Native Frames]: a page of 5 from frame 10.
    let page: StackTraceResponse = wait(
        &client,
        "stackTrace",
        json!({"threadId": tid, "startFrame": 10, "levels": 5}),
    );
    assert_eq!(page.total_frames, Some(31));
    let names: Vec<&str> = page.stack_frames.iter().map(|f| f.name.as_str()).collect();
    assert_eq!(
        names,
        [
            "App.F19()",
            "App.F18()",
            "App.F17()",
            "App.F16()",
            "App.F15()"
        ]
    );
    let all: StackTraceResponse = wait(&client, "stackTrace", json!({"threadId": tid}));
    assert_eq!(all.stack_frames.len(), 31);
    assert_eq!(
        all.stack_frames[30].path(),
        None,
        "[Native Frames] has no source"
    );
    // Another thread waits in external code.
    let other: StackTraceResponse = wait(&client, "stackTrace", json!({"threadId": 2}));
    assert_eq!(
        other.stack_frames[0].name,
        "System.Threading.Monitor.Wait()"
    );
    assert_eq!(other.stack_frames[0].path(), None);
    assert_eq!(
        other.stack_frames[0].presentation_hint.as_deref(),
        Some("subtle")
    );

    let scopes: ScopesResponse = wait(
        &client,
        "scopes",
        json!({"frameId": all.stack_frames[0].id}),
    );
    assert_eq!(scopes.scopes[0].named_variables, Some(123));
    let locals_ref = scopes.scopes[0].variables_reference;
    let first: VariablesResponse = wait(
        &client,
        "variables",
        json!({"variablesReference": locals_ref, "start": 0, "count": 50}),
    );
    assert_eq!(first.variables.len(), 50);
    assert_eq!(first.variables[49].name, "v049");
    let rest: VariablesResponse = wait(
        &client,
        "variables",
        json!({"variablesReference": locals_ref, "start": 120, "count": 50}),
    );
    let names: Vec<&str> = rest.variables.iter().map(|v| v.name.as_str()).collect();
    assert_eq!(names, ["big", "text", "graph"]);
    let big = &rest.variables[0];
    assert_eq!(big.indexed_variables, Some(10_000));
    assert_eq!(rest.variables[1].value.chars().count(), 1000);
    // The array pages by index.
    let tail: VariablesResponse = wait(
        &client,
        "variables",
        json!({"variablesReference": big.variables_reference, "start": 9990, "count": 50}),
    );
    assert_eq!(tail.variables.len(), 10);
    assert_eq!(tail.variables[0].name, "[9990]");
    assert_eq!(tail.variables[9].value, "9999");
    // The graph is five levels deep: graph.next.next.next.next is a leaf.
    let mut reference = rest.variables[2].variables_reference;
    let mut levels = 1;
    while reference > 0 {
        let members: VariablesResponse = wait(
            &client,
            "variables",
            json!({"variablesReference": reference}),
        );
        assert_eq!(members.variables.len(), 3);
        assert_eq!(members.variables[2].name, "next");
        reference = members.variables[2].variables_reference;
        levels += 1;
    }
    assert_eq!(levels, 5);
    client.request_wait("disconnect", json!({}), T).unwrap();
}

#[test]
fn without_paging_capabilities_the_fake_answers_everything() {
    let mut p = deep_program();
    p.extra_capabilities =
        json!({"supportsDelayedStackTraceLoading": false, "supportsVariablePaging": false});
    let (client, handle, rec) = start(p, &[129], &[]);
    let caps = client.capabilities();
    assert!(!caps.supports_delayed_stack_trace_loading && !caps.supports_variable_paging);
    handle.trigger();
    let tid = rec.stopped(1).thread_id.unwrap();
    let st: StackTraceResponse = wait(
        &client,
        "stackTrace",
        json!({"threadId": tid, "startFrame": 10, "levels": 5}),
    );
    assert_eq!(st.stack_frames.len(), 31, "startFrame and levels ignored");
    let scopes: ScopesResponse = wait(&client, "scopes", json!({"frameId": st.stack_frames[0].id}));
    let vars: VariablesResponse = wait(
        &client,
        "variables",
        json!({"variablesReference": scopes.scopes[0].variables_reference, "start": 0, "count": 50}),
    );
    assert_eq!(vars.variables.len(), 123, "start and count ignored");
    client.request_wait("disconnect", json!({}), T).unwrap();
}

#[test]
fn pause_stops_a_running_statement_and_exception_info_has_details() {
    let mut p = common::program();
    // Line 7 loops until paused; the statement before it prints, and line 8 prints to stderr when it runs.
    p.steps[3].prints = vec![("stdout".into(), "working\n".into())];
    p.steps[4].runs_until_paused = true;
    p.steps[5].prints = vec![
        ("stderr".into(), "warn\n".into()),
        ("console".into(), "adapter note\n".into()),
    ];
    let mut inner = FakeThrow::new("System.FormatException", "bad digits", true);
    inner.stack_trace = Some("   at App.Parse()".into());
    let throw = p.steps[6].throws.as_mut().unwrap();
    throw.stack_trace = Some("   at App.Program.Main() in /src/App/Program.cs:line 9".into());
    throw.inner = Some(Box::new(inner));
    let (client, handle, rec) = start(p, &[], &["all"]);
    // Nothing runs yet: pause is refused.
    match client.request_wait("pause", json!({"threadId": 1}), T) {
        Err(DapError::Failed { message, .. }) => {
            assert!(message.contains("not running"), "{message}")
        }
        other => panic!("{other:?}"),
    }
    handle.trigger();
    rec.wait_nth(
        1,
        "the statement before the loop",
        |e| matches!(e, ClientEvent::Event(eludite_dap::types::Event::Output(o)) if o.output == "working\n"),
    );
    client
        .request_wait("pause", json!({"threadId": 1}), T)
        .unwrap();
    let s = rec.stopped(1);
    assert_eq!(s.reason, "pause");
    let st: StackTraceResponse = wait(&client, "stackTrace", json!({"threadId": 1}));
    assert_eq!(st.stack_frames[0].line, 7);
    // Resuming goes on after the loop: line 8 prints, then line 9 throws (first chance with `all`).
    client
        .request_wait("continue", json!({"threadId": 1}), T)
        .unwrap();
    let s = rec.stopped(2);
    assert_eq!(s.reason, "exception");
    for (category, text) in [("stderr", "warn\n"), ("console", "adapter note\n")] {
        rec.wait_nth(1, text, |e| {
            matches!(e, ClientEvent::Event(eludite_dap::types::Event::Output(o))
                if o.category.as_deref() == Some(category) && o.output == text)
        });
    }
    let info: ExceptionInfoResponse = wait(&client, "exceptionInfo", json!({"threadId": 1}));
    assert_eq!(info.exception_id, "System.InvalidOperationException");
    assert_eq!(info.break_mode, "always");
    let d = info.details.unwrap();
    assert_eq!(d.type_name.as_deref(), Some("InvalidOperationException"));
    assert_eq!(
        d.full_type_name.as_deref(),
        Some("System.InvalidOperationException")
    );
    assert!(d.stack_trace.unwrap().contains("Program.cs:line 9"));
    assert_eq!(d.inner_exception[0].message.as_deref(), Some("bad digits"));
    assert_eq!(
        d.inner_exception[0].stack_trace.as_deref(),
        Some("   at App.Parse()")
    );
    client.request_wait("disconnect", json!({}), T).unwrap();
}
