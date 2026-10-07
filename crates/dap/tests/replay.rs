//! The recorder and the replaying adapter (brief 0033) against the scripted fake: a session recorded to a file reads
//! back scrubbed (paths under `${ROOT}`, the process id `${PID}`), replays to a client with the same client-visible
//! events (this run's paths and process id substituted back), answers a request the recording did not see with
//! `success: false` naming the nearest recorded one and the difference, and holds each answer and the events after it
//! until the requests recorded before them have arrived; a mark (a test's action outside DAP) holds what followed it,
//! and the connections of one server replay as a group in their recorded order (brief 0038).

mod common;

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use eludite_dap::record::{self, Dir, RecordOptions, Recording};
use eludite_dap::replay::{self, ReplayOptions};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{Event, SourceBreakpoint};
use eludite_dap::{ClientEvent, Connection, DapClient, DapError, fake};
use serde_json::{Value, json};

use common::{Recorder, T};

/// The fake's program with its files under `root`, run once at `configurationDone` and exiting with 0.
fn program(root: &Path) -> eludite_dap::fake::FakeProgram {
    let mut p = common::program();
    let main = root
        .join("App")
        .join("Program.cs")
        .to_string_lossy()
        .into_owned();
    let calc = root
        .join("App")
        .join("Calc.cs")
        .to_string_lossy()
        .into_owned();
    for s in &mut p.steps {
        s.path = if s.path.ends_with("Calc.cs") {
            calc.clone()
        } else {
            main.clone()
        };
    }
    p.process_id = 48_213;
    p.run_at_start = true;
    p.exit_at_end = Some(0);
    p
}

/// A session: launch with a breakpoint in `Calc.cs` line 10, the stop's stack and locals, a step, an `evaluate`,
/// continue to the end. Returns what the client saw, without the closing details (exit code, stderr).
fn drive(connection: Connection, root: &Path) -> (Vec<ClientEvent>, Vec<Value>) {
    let rec = Recorder::default();
    let client = DapClient::start(connection, rec.sink());
    let calc = root
        .join("App")
        .join("Calc.cs")
        .to_string_lossy()
        .into_owned();
    session::start(
        &client,
        &StartPlan {
            adapter_id: "coreclr".into(),
            kind: StartKind::Launch,
            arguments: json!({"program": root.join("App/bin/App.dll"), "cwd": root}),
            breakpoints: vec![(
                calc,
                vec![SourceBreakpoint {
                    line: 10,
                    ..Default::default()
                }],
            )],
            exception_filters: vec!["user-unhandled".into()],
            exception_options: Vec::new(),
            function_breakpoints: Vec::new(),
        },
        T,
    )
    .unwrap();
    let s = rec.stopped(1);
    let tid = s.thread_id.unwrap();
    let mut answers = Vec::new();
    let mut ask = |command: &str, args: Value| {
        let a = client.request_wait(command, args, T).unwrap();
        answers.push(a.clone());
        a
    };
    ask("threads", Value::Null);
    let st = ask(
        "stackTrace",
        json!({"threadId": tid, "startFrame": 0, "levels": 20}),
    );
    let frame = st["stackFrames"][0]["id"].clone();
    let scopes = ask("scopes", json!({"frameId": frame}));
    ask(
        "variables",
        json!({"variablesReference": scopes["scopes"][0]["variablesReference"]}),
    );
    ask("next", json!({"threadId": tid}));
    rec.stopped(2);
    ask(
        "evaluate",
        json!({"expression": "sum", "frameId": frame, "context": "watch"}),
    );
    ask("continue", json!({"threadId": tid}));
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    ask("disconnect", json!({}));
    rec.closed();
    let events = rec
        .events()
        .into_iter()
        .filter(|e| !matches!(e, ClientEvent::Stderr(_) | ClientEvent::Closed { .. }))
        .collect();
    (events, answers)
}

fn record_fake(dir: &Path, root: &Path) -> (Vec<ClientEvent>, Vec<Value>, Recording) {
    let path = dir.join("fake/launch-break-step.dap.json");
    let (conn, _) = fake::connect(program(root));
    let (conn, handle) = record::record(
        conn,
        RecordOptions {
            path: path.clone(),
            adapter: "fake".into(),
            version: "fake 1".into(),
            roots: vec![("${ROOT}".into(), root.to_path_buf())],
        },
    );
    let (events, answers) = drive(conn, root);
    // The client's threads end with the session: the file is written then.
    let deadline = Instant::now() + T;
    while handle.ended().is_none() || !path.is_file() {
        assert!(Instant::now() < deadline, "the recording was not written");
        std::thread::sleep(Duration::from_millis(5));
    }
    (events, answers, handle.write().unwrap())
}

#[test]
fn a_fake_session_round_trips_through_a_file_and_replays_with_the_same_events() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(root.join("App")).unwrap();
    let (live, live_answers, recording) = record_fake(dir.path(), &root);
    // Scrubbed: no path of this machine, the process id a placeholder.
    let text = recording.to_text();
    assert!(
        !text.contains(&*root.to_string_lossy()),
        "a machine path was left: {text}"
    );
    assert!(text.contains("${ROOT}/App/Calc.cs"), "{text}");
    assert!(text.contains("\"systemProcessId\":\"${PID}\""), "{text}");
    assert!(!text.contains("48213"));
    assert_eq!(recording.adapter, "fake");
    assert_eq!(recording.ended, Some(record::Ended::Adapter));
    assert!(text.lines().count() > recording.messages.len());
    let commands: Vec<String> = recording.requests().into_iter().map(|(c, _)| c).collect();
    assert_eq!(
        commands,
        [
            "initialize",
            "launch",
            "setBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone",
            "threads",
            "stackTrace",
            "scopes",
            "variables",
            "next",
            "evaluate",
            "continue",
            "disconnect"
        ]
    );
    // Monotonic timestamps.
    assert!(
        recording
            .messages
            .windows(2)
            .all(|w| w[0].t_ms <= w[1].t_ms)
    );

    // Replayed in the same place: the client sees exactly what it saw live.
    let (conn, handle) = replay::serve(
        &recording,
        ReplayOptions {
            roots: vec![("${ROOT}".into(), root.clone())],
            pid: 48_213,
            real_time: false,
            tokens: Vec::new(),
        },
    );
    let clock = Instant::now();
    let (replayed, answers) = drive(conn, &root);
    eprintln!(
        "timing: replay of the fake session ({} messages): {:.1} ms",
        recording.messages.len(),
        clock.elapsed().as_secs_f64() * 1e3
    );
    handle.check().unwrap();
    assert!(handle.finished(), "unsent: {:?}", handle.unsent());
    assert_eq!(replayed, live);
    assert_eq!(answers, live_answers);

    // Replayed elsewhere: this run's root and process id come back.
    let other = dir.path().join("elsewhere");
    std::fs::create_dir_all(other.join("App")).unwrap();
    let (conn, handle) = replay::serve(
        &recording,
        ReplayOptions {
            roots: vec![("${ROOT}".into(), other.clone())],
            pid: 7777,
            real_time: false,
            tokens: Vec::new(),
        },
    );
    let (moved, _) = drive(conn, &other);
    handle.check().unwrap();
    assert_eq!(moved.len(), live.len());
    let pid = moved.iter().find_map(|e| match e {
        ClientEvent::Event(Event::Process(p)) => p.system_process_id,
        _ => None,
    });
    assert_eq!(pid, Some(7777));
    let bound = moved.iter().find_map(|e| match e {
        ClientEvent::Event(Event::Breakpoint(b)) => b.breakpoint.source.as_ref()?.path.clone(),
        _ => None,
    });
    if let Some(path) = bound {
        assert_eq!(PathBuf::from(path), other.join("App/Calc.cs"));
    }
}

#[test]
fn an_unknown_request_fails_with_the_nearest_recorded_one_and_the_difference() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(root.join("App")).unwrap();
    let (_, _, recording) = record_fake(dir.path(), &root);
    let (conn, handle) = replay::serve(
        &recording,
        ReplayOptions {
            roots: vec![("${ROOT}".into(), root.clone())],
            pid: 1,
            real_time: false,
            tokens: Vec::new(),
        },
    );
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let init =
        serde_json::to_value(eludite_dap::types::InitializeArguments::eludite("coreclr")).unwrap();
    client.request_wait("initialize", init, T).unwrap();
    // `launch` with another program: refused, naming the recorded launch and the difference.
    let err = client
        .request_wait(
            "launch",
            json!({"program": root.join("App/bin/Other.dll"), "cwd": root}),
            T,
        )
        .unwrap_err();
    let DapError::Failed { message, .. } = err else {
        panic!("{err:?}")
    };
    assert!(
        message.contains("the client sent `launch`")
            && message.contains("nearest recorded: message #")
            && message.contains(
                ".program: expected \"${ROOT}/App/bin/App.dll\", got \"${ROOT}/App/bin/Other.dll\""
            ),
        "{message}"
    );
    // A command the recording never had says what it expects next.
    let err = client
        .request_wait("goto", json!({"threadId": 1, "targetId": 1}), T)
        .unwrap_err();
    assert!(
        err.to_string()
            .contains("the recording has no `goto` at all")
            && err
                .to_string()
                .contains("the next request it expects is `launch`"),
        "{err}"
    );
    assert_eq!(handle.failures().len(), 2);
    assert!(handle.check().is_err());
    // Nothing recorded after `launch` is played while it is missing.
    let waiting = handle.waiting_for().unwrap();
    assert!(
        waiting.contains("waits for the client's `launch`"),
        "{waiting}"
    );
    client.kill();
}

#[test]
fn answers_and_their_events_wait_for_the_requests_recorded_before_them() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("work");
    std::fs::create_dir_all(root.join("App")).unwrap();
    let (_, _, recording) = record_fake(dir.path(), &root);
    // In the recording the run's first stop comes after configurationDone's answer.
    let order: Vec<String> = recording
        .messages
        .iter()
        .filter(|m| m.dir == Dir::Adapter)
        .map(|m| record::label(&m.message))
        .collect();
    let stop = order.iter().position(|l| l == "event stopped").unwrap();
    let done = order
        .iter()
        .position(|l| l == "response configurationDone")
        .unwrap();
    assert!(done < stop, "{order:?}");
    let (conn, handle) = replay::serve(
        &recording,
        ReplayOptions {
            roots: vec![("${ROOT}".into(), root.clone())],
            pid: 1,
            real_time: false,
            tokens: Vec::new(),
        },
    );
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    let init =
        serde_json::to_value(eludite_dap::types::InitializeArguments::eludite("coreclr")).unwrap();
    client.request_wait("initialize", init, T).unwrap();
    let launched = client
        .request_channel(
            "launch",
            json!({"program": root.join("App/bin/App.dll"), "cwd": root}),
        )
        .unwrap();
    assert!(client.wait_initialized(T), "initialized came with its turn");
    // The stop is held back until the configuration recorded before it arrives.
    std::thread::sleep(Duration::from_millis(100));
    assert!(
        !rec.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Stopped(_)))),
        "a stop was early"
    );
    let waiting = handle.waiting_for().unwrap();
    assert!(waiting.contains("`setBreakpoints`"), "{waiting}");
    let calc = root
        .join("App")
        .join("Calc.cs")
        .to_string_lossy()
        .into_owned();
    session::set_breakpoints(
        &client,
        &calc,
        &[SourceBreakpoint {
            line: 10,
            ..Default::default()
        }],
        T,
    )
    .unwrap();
    client
        .request_wait(
            "setExceptionBreakpoints",
            session::set_exception_breakpoints_arguments(&["user-unhandled".into()], &[]),
            T,
        )
        .unwrap();
    client
        .request_wait("configurationDone", Value::Null, T)
        .unwrap();
    launched.recv_timeout(T).unwrap().unwrap();
    rec.stopped(1);
    // The events played so far are the recording's, in its order.
    let seen: Vec<String> = rec
        .events()
        .iter()
        .filter_map(|e| match e {
            ClientEvent::Event(ev) => Some(event_name(ev)),
            _ => None,
        })
        .collect();
    let recorded: Vec<String> = recording
        .messages
        .iter()
        .filter(|m| m.dir == Dir::Adapter && m.message["type"] == "event")
        .map(|m| m.message["event"].as_str().unwrap().to_owned())
        .take(seen.len())
        .collect();
    assert_eq!(seen, recorded);
    assert!(handle.check().is_ok());
    client.kill();
}

fn event_name(e: &Event) -> String {
    let v = format!("{e:?}");
    let name = v.split(['(', ' ']).next().unwrap_or_default().to_owned();
    let mut c = name.chars();
    match c.next() {
        Some(f) => f.to_lowercase().chain(c).collect(),
        None => name,
    }
}

#[test]
fn real_time_keeps_the_recorded_gaps() {
    let recording = Recording {
        adapter: "fake".into(),
        version: "1".into(),
        recorded_at: record::utc_now(),
        platform: record::platform(),
        description: String::new(),
        ended: Some(record::Ended::Adapter),
        messages: vec![
            record::RecordedMessage {
                t_ms: 0,
                dir: Dir::Client,
                message: json!({"seq": 1, "type": "request", "command": "threads"}),
                order: None,
            },
            record::RecordedMessage {
                t_ms: 1,
                dir: Dir::Adapter,
                message: json!({"seq": 1, "type": "response", "request_seq": 1, "command": "threads", "success": true, "body": {"threads": []}}),
                order: None,
            },
            record::RecordedMessage {
                t_ms: 301,
                dir: Dir::Adapter,
                message: json!({"seq": 2, "type": "event", "event": "output", "body": {"category": "stdout", "output": "late\n"}}),
                order: None,
            },
        ],
    };
    for (real_time, at_least, under) in [(false, 0, 200), (true, 250, 2000)] {
        let (conn, handle) = replay::serve(
            &recording,
            ReplayOptions {
                real_time,
                ..Default::default()
            },
        );
        let rec = Recorder::default();
        let client = DapClient::start(conn, rec.sink());
        let clock = Instant::now();
        client.request_wait("threads", Value::Null, T).unwrap();
        rec.wait_nth(1, "output", |e| {
            matches!(e, ClientEvent::Event(Event::Output(_)))
        });
        let took = clock.elapsed();
        assert!(
            took >= Duration::from_millis(at_least),
            "real_time {real_time}: {took:?}"
        );
        common::assert_budget(
            &format!("the replay, real_time {real_time}"),
            took,
            Duration::from_millis(under),
        );
        rec.closed();
        assert!(handle.finished());
    }
}

/// Brief 0038: a mark (a test's action outside DAP, here a click in a page) holds the adapter's later messages until
/// the test passes it, and a token placeholder (the page's origin) comes back as the replay's own value.
#[test]
fn a_mark_holds_what_followed_it_until_the_test_passes_it() {
    let at = |t_ms, dir, message| record::RecordedMessage {
        t_ms,
        dir,
        message,
        order: None,
    };
    let recording = Recording {
        adapter: "js-debug".into(),
        version: "test".into(),
        recorded_at: String::new(),
        platform: record::platform(),
        description: String::new(),
        ended: None,
        messages: vec![
            at(
                0,
                Dir::Client,
                json!({"seq": 1, "type": "request", "command": "initialize", "arguments": {"adapterID": "pwa-chrome"}}),
            ),
            at(
                1,
                Dir::Adapter,
                json!({"seq": 1, "type": "response", "request_seq": 1, "command": "initialize", "success": true, "body": {}}),
            ),
            at(
                2,
                Dir::Client,
                json!({"type": record::MARK, "label": "click"}),
            ),
            at(
                3,
                Dir::Adapter,
                json!({"seq": 2, "type": "event", "event": "output", "body": {"category": "stdout", "output": "clicked at http://${ORIGIN}/app.js\n"}}),
            ),
        ],
    };
    let (conn, handle) = replay::serve(
        &recording,
        ReplayOptions {
            tokens: vec![("${ORIGIN}".into(), "127.0.0.1:5180".into())],
            ..ReplayOptions::default()
        },
    );
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    client
        .request_wait("initialize", json!({"adapterID": "pwa-chrome"}), T)
        .unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let output = |rec: &Recorder| {
        rec.events().iter().find_map(|e| match e {
            ClientEvent::Event(Event::Output(o)) => Some(o.output.clone()),
            _ => None,
        })
    };
    assert_eq!(output(&rec), None, "the output came before the click");
    let waiting = handle.waiting_for().unwrap();
    assert!(waiting.contains("the test's mark `click`"), "{waiting}");
    assert_eq!(handle.unsent(), ["mark click"]);
    assert!(handle.mark("click"));
    assert!(!handle.mark("click"), "a mark is passed once");
    let deadline = Instant::now() + T;
    while output(&rec).is_none() {
        assert!(Instant::now() < deadline, "the output never came");
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(
        output(&rec).unwrap(),
        "clicked at http://127.0.0.1:5180/app.js\n"
    );
    assert!(handle.finished());
    client.kill();
}

/// Brief 0038: two connections of one server recorded with one counter replay as a group: an adapter message of one
/// waits for the client's request the other connection sent before it (vscode-js-debug's parent hears its child
/// went away only after the child's `disconnect`).
#[test]
fn a_group_keeps_the_order_across_connections() {
    let at = |order, dir, message| record::RecordedMessage {
        t_ms: 0,
        dir,
        message,
        order: Some(order),
    };
    let recording = |messages| Recording {
        adapter: "js-debug".into(),
        version: "test".into(),
        recorded_at: String::new(),
        platform: record::platform(),
        description: String::new(),
        ended: None,
        messages,
    };
    let parent = recording(vec![
        at(
            0,
            Dir::Client,
            json!({"seq": 1, "type": "request", "command": "threads"}),
        ),
        at(
            1,
            Dir::Adapter,
            json!({"seq": 1, "type": "response", "request_seq": 1, "command": "threads", "success": true, "body": {"threads": []}}),
        ),
        at(
            4,
            Dir::Adapter,
            json!({"seq": 2, "type": "event", "event": "terminated", "body": {}}),
        ),
    ]);
    let child = recording(vec![
        at(
            2,
            Dir::Client,
            json!({"seq": 1, "type": "request", "command": "disconnect", "arguments": {"terminateDebuggee": false}}),
        ),
        at(
            3,
            Dir::Adapter,
            json!({"seq": 1, "type": "response", "request_seq": 1, "command": "disconnect", "success": true}),
        ),
    ]);
    let group = replay::ReplayGroup::new(&[parent.clone(), child.clone()]);
    let (conn, parent_handle) =
        replay::serve_in_group(&parent, ReplayOptions::default(), group.clone());
    let rec = Recorder::default();
    let client = DapClient::start(conn, rec.sink());
    client.request_wait("threads", Value::Null, T).unwrap();
    std::thread::sleep(Duration::from_millis(100));
    let terminated = |rec: &Recorder| {
        rec.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Terminated)))
    };
    assert!(!terminated(&rec), "the parent ended before its child left");
    let waiting = parent_handle.waiting_for().unwrap();
    assert!(
        waiting.contains("another connection's message"),
        "{waiting}"
    );
    let (conn, _) = replay::serve_in_group(&child, ReplayOptions::default(), group);
    let child_client = DapClient::start(conn, Recorder::default().sink());
    child_client
        .request_wait("disconnect", json!({"terminateDebuggee": false}), T)
        .unwrap();
    let deadline = Instant::now() + T;
    while !terminated(&rec) {
        assert!(Instant::now() < deadline, "the parent never heard");
        std::thread::sleep(Duration::from_millis(5));
    }
    client.kill();
    child_client.kill();
}
