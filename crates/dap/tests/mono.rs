//! The client against the real `eludite-dbg-mono` under the located Mono, debugging the built
//! `Eludite.Debugger.Mono.TestApp` (brief 0022): the launch configuration of an SDK-style net472 project, launch, a
//! breakpoint, the stack, locals, evaluate, a step, continue and the end of the session. Skipped with a message unless
//! Mono (the setting's search order: `ELUDITE_MONO_PREFIX`, `PATH`, the usual prefixes), the adapter
//! (`ELUDITE_DBG_MONO`, else the repository's build output) and the TestApp are found; `dotnet build
//! dotnet/Eludite.slnx` builds both.

mod common;

use std::path::{Path, PathBuf};
use std::time::Instant;

use eludite_dap::discovery::{MonoAdapterSearch, MonoSearch};
use eludite_dap::launch::{self, FrameworkKind, Platform};
use eludite_dap::session::{self, StartKind, StartPlan};
use eludite_dap::types::{Event, ExceptionFilterOptions, FunctionBreakpoint, SourceBreakpoint};
use eludite_dap::{ClientEvent, DapClient, transport};
use serde_json::json;

use common::{Recorder, T};

/// The first of `bin/Debug` and `bin/Release` holding `file` under `project_dir`.
fn built(project_dir: &Path, file: &str) -> Option<PathBuf> {
    ["Debug", "Release"]
        .iter()
        .map(|c| project_dir.join("bin").join(c).join("net472").join(file))
        .find(|p| p.is_file())
}

/// What the tests need: Mono, the adapter, the TestApp's launch configuration and its `Program.cs`; `None` (with a
/// message) to skip.
struct Found {
    mono: eludite_dap::discovery::MonoInstall,
    adapter: PathBuf,
    config: launch::LaunchConfig,
    source: PathBuf,
    text: String,
    /// `eludite-dbg-mono under mono 6.8.0.105`, for recordings.
    version: String,
}

impl Found {
    fn line_of(&self, mark: &str) -> i64 {
        self.text
            .lines()
            .position(|l| l.ends_with(&format!("// MARK: {mark}")))
            .unwrap_or_else(|| panic!("MARK: {mark}")) as i64
            + 1
    }

    /// Start the adapter and launch the TestApp with `args`, breaking at `marks`, with exception `filters`.
    fn launch(&self, args: &[&str], marks: &[&str], filters: &[&str]) -> (DapClient, Recorder) {
        let breakpoints = marks
            .iter()
            .map(|m| SourceBreakpoint {
                line: self.line_of(m),
                ..Default::default()
            })
            .collect();
        self.launch_plan(args, breakpoints, Vec::new(), filters, Vec::new())
    }

    /// As [`Found::launch`] with source breakpoints as given, function breakpoints and exception filter options.
    fn launch_plan(
        &self,
        args: &[&str],
        breakpoints: Vec<SourceBreakpoint>,
        functions: Vec<FunctionBreakpoint>,
        filters: &[&str],
        options: Vec<ExceptionFilterOptions>,
    ) -> (DapClient, Recorder) {
        let rec = Recorder::default();
        let client = DapClient::start(
            common::recorded(
                transport::connect_with_env(
                    &self.mono.adapter_transport(&self.adapter),
                    &self.mono.env,
                )
                .unwrap(),
                "mono",
                &self.version,
                None,
            ),
            rec.sink(),
        );
        let mut arguments = self.config.mono_arguments(&self.mono.mono);
        arguments["args"] = json!(args);
        session::start(
            &client,
            &StartPlan {
                adapter_id: "mono".into(),
                kind: StartKind::Launch,
                arguments,
                breakpoints: vec![(self.source.to_string_lossy().into_owned(), breakpoints)],
                exception_filters: filters.iter().map(|f| (*f).to_owned()).collect(),
                exception_options: options,
                function_breakpoints: functions,
            },
            T,
        )
        .unwrap();
        (client, rec)
    }
}

fn find() -> Option<Found> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let mono_dir = root.join("debuggers/mono");
    let mono = match (MonoSearch {
        configured: std::env::var_os("ELUDITE_MONO_PREFIX")
            .filter(|v| !v.is_empty())
            .map(PathBuf::from),
        ..MonoSearch::from_env()
    })
    .find_mono()
    {
        Ok(m) if !cfg!(windows) => m,
        Ok(_) => {
            eprintln!(
                "skipped: .NET Framework debugging on Windows is eludite-dbg-netfx (brief 0004)"
            );
            return None;
        }
        Err(e) => {
            eprintln!("skipped: {e}");
            return None;
        }
    };
    let adapter = match std::env::var_os("ELUDITE_DBG_MONO").filter(|v| !v.is_empty()) {
        Some(c) => MonoAdapterSearch {
            exe_dir: None,
            configured: Some(PathBuf::from(c)),
        }
        .find()
        .ok(),
        None => built(
            &mono_dir.join("Eludite.Debugger.Mono"),
            "eludite-dbg-mono.exe",
        ),
    };
    let Some(adapter) = adapter else {
        eprintln!(
            "skipped: eludite-dbg-mono.exe is not built (dotnet build dotnet/Eludite.slnx) and ELUDITE_DBG_MONO is unset"
        );
        return None;
    };
    let app_dir = mono_dir.join("Eludite.Debugger.Mono.TestApp");
    if built(&app_dir, "Eludite.Debugger.Mono.TestApp.exe").is_none() {
        eprintln!("skipped: the TestApp is not built (dotnet build dotnet/Eludite.slnx)");
        return None;
    }
    // The launch configuration as the shell computes it for an SDK-style net472 project.
    let project = app_dir.join("Eludite.Debugger.Mono.TestApp.csproj");
    let config = launch::launch_config(&project, None).unwrap();
    assert_eq!(config.kind, FrameworkKind::NetFramework);
    assert!(config.program.to_string_lossy().ends_with(".exe"));
    assert_eq!(
        launch::select_adapter(config.kind, Platform::Linux),
        Ok(launch::AdapterKind::Mono)
    );
    let source = std::fs::canonicalize(app_dir.join("Program.cs")).unwrap();
    let text = std::fs::read_to_string(&source).unwrap();
    let version = mono.version().expect("mono --version");
    eprintln!(
        "eludite-dbg-mono under mono {version} ({})",
        mono.mono.display()
    );
    Some(Found {
        mono,
        adapter,
        config,
        source,
        text,
        version: format!("eludite-dbg-mono under mono {version}"),
    })
}

#[test]
fn eludite_dbg_mono_debugs_the_test_app() {
    let Some(found) = find() else { return };
    let line_of = |mark: &str| found.line_of(mark);
    let (mono, adapter, config, source) =
        (&found.mono, &found.adapter, &found.config, &found.source);

    let rec = Recorder::default();
    let clock = Instant::now();
    let client = DapClient::start(
        common::recorded(
            transport::connect_with_env(&mono.adapter_transport(adapter), &mono.env).unwrap(),
            "mono",
            &found.version,
            None,
        ),
        rec.sink(),
    );
    let started = session::start(
        &client,
        &StartPlan {
            adapter_id: "mono".into(),
            kind: StartKind::Launch,
            arguments: config.mono_arguments(&mono.mono),
            breakpoints: vec![(
                source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line: line_of("add-sum"),
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
    assert!(started.capabilities.supports_conditional_breakpoints);
    assert!(started.capabilities.supports_hit_conditional_breakpoints);
    assert!(started.capabilities.supports_exception_info_request);
    assert_eq!(started.exception_filters, ["user-unhandled"]);
    let s = rec.stopped(1);
    eprintln!(
        "timing: start of the adapter to the first stopped through eludite-dap: {:.0} ms",
        clock.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(s.reason, "breakpoint");
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 20}), T)
        .unwrap();
    assert_eq!(
        st["stackFrames"][0]["name"],
        "Eludite.Debugger.Mono.TestApp.Calculator.Add(int a, int b)"
    );
    assert_eq!(st["stackFrames"][0]["line"], line_of("add-sum"));
    let frame = st["stackFrames"][0]["id"].clone();
    let scopes = client
        .request_wait("scopes", json!({"frameId": frame}), T)
        .unwrap();
    let vars = client
        .request_wait(
            "variables",
            json!({"variablesReference": scopes["scopes"][0]["variablesReference"]}),
            T,
        )
        .unwrap();
    let names: Vec<(&str, &str)> = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| (v["name"].as_str().unwrap(), v["value"].as_str().unwrap()))
        .collect();
    assert_eq!(&names[1..4], [("a", "2"), ("b", "3"), ("sum", "0")]);
    assert_eq!(names[0].0, "this");
    let e = client
        .request_wait(
            "evaluate",
            json!({"expression": "a + b * 2", "frameId": frame, "context": "watch"}),
            T,
        )
        .unwrap();
    assert_eq!(e["result"], "8");
    client
        .request_wait("next", json!({"threadId": tid}), T)
        .unwrap();
    assert_eq!(rec.stopped(2).reason, "step");
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 1}), T)
        .unwrap();
    assert_eq!(st["stackFrames"][0]["line"], line_of("add-twice"));
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    let exited = rec.wait_nth(1, "exited", |e| {
        matches!(e, ClientEvent::Event(Event::Exited(_)))
    });
    assert!(matches!(exited, ClientEvent::Event(Event::Exited(x)) if x.exit_code == 3));
    rec.wait_nth(
        1,
        "the program's output",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("result 10")),
    );
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
}

/// One session of the TestApp to the end, the same requests every time: launch breaking at `add-sum`, the stack, the
/// locals, a step, an `evaluate`, continue to the exit. Returns every event and answer the client saw, in order (not
/// the adapter's stderr or its exit code, which are not DAP).
fn add_sum_session(found: &Found, connection: eludite_dap::Connection) -> Vec<String> {
    let rec = Recorder::default();
    let client = DapClient::start(connection, rec.sink());
    let mut arguments = found.config.mono_arguments(&found.mono.mono);
    arguments["args"] = json!([]);
    session::start(
        &client,
        &StartPlan {
            adapter_id: "mono".into(),
            kind: StartKind::Launch,
            arguments,
            breakpoints: vec![(
                found.source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line: found.line_of("add-sum"),
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
    let mut seen = Vec::new();
    let tid = rec.stopped(1).thread_id.unwrap();
    let mut ask = |command: &str, args: serde_json::Value| -> serde_json::Value {
        let a = client.request_wait(command, args, T).unwrap();
        seen.push(format!("{command} -> {a}"));
        a
    };
    ask("threads", serde_json::Value::Null);
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
    ask(
        "evaluate",
        json!({"expression": "a + b * 2", "frameId": frame, "context": "watch"}),
    );
    ask("next", json!({"threadId": tid}));
    rec.stopped(2);
    ask(
        "stackTrace",
        json!({"threadId": tid, "startFrame": 0, "levels": 1}),
    );
    ask("continue", json!({"threadId": tid}));
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
    rec.closed();
    seen.extend(rec.events().into_iter().filter_map(|e| match e {
        ClientEvent::Event(ev) => Some(format!("{ev:?}")),
        _ => None,
    }));
    seen
}

/// Brief 0033's proof against the real adapter: the session is recorded (to `RECORD_DAP`'s folder when set, else a
/// temporary one) and replayed in the same run through the replaying adapter, and the client sees the same events
/// and answers from the recording as from Mono.
#[test]
fn eludite_dbg_mono_recording_replays_with_the_same_events() {
    let Some(found) = find() else { return };
    let tmp = tempfile::tempdir().unwrap();
    let dir = common::record_dir().unwrap_or_else(|| tmp.path().to_path_buf());
    let path = dir.join("mono/client/recorded-then-replayed.dap.json");
    let version = found.version.clone();
    let roots = vec![
        ("${ROOT}".to_owned(), common::repo_root()),
        ("${MONO}".to_owned(), found.mono.mono.clone()),
    ];
    let (connection, handle) = eludite_dap::record::record(
        transport::connect_with_env(
            &found.mono.adapter_transport(&found.adapter),
            &found.mono.env,
        )
        .unwrap(),
        eludite_dap::record::RecordOptions {
            path: path.clone(),
            adapter: "mono".into(),
            version,
            roots: roots.clone(),
        },
    );
    let live = add_sum_session(&found, connection);
    let recording = handle.write().unwrap();
    assert!(
        recording.messages.len() > 20,
        "{}",
        recording.messages.len()
    );
    let text = recording.to_text();
    assert!(text.contains("\"runtimeExecutable\":\"${MONO}\""), "{text}");
    assert!(!text.contains(&*common::repo_root().to_string_lossy()));
    // Replayed: the same events and answers, in the same order.
    let (connection, replayer) = eludite_dap::replay::serve(
        &recording,
        eludite_dap::replay::ReplayOptions {
            roots,
            pid: 4_000_001,
            real_time: false,
        },
    );
    let clock = Instant::now();
    let replayed = add_sum_session(&found, connection);
    eprintln!(
        "timing: replay of the recorded Mono session ({} messages, {} bytes): {:.1} ms",
        recording.messages.len(),
        text.len(),
        clock.elapsed().as_secs_f64() * 1e3
    );
    replayer.check().unwrap();
    assert!(replayer.finished(), "unsent: {:?}", replayer.unsent());
    // The process id is this run's in the replay; everything else is identical.
    let pid = |lines: &[String]| -> Vec<String> {
        lines
            .iter()
            .map(|l| {
                l.split("system_process_id: Some(")
                    .next()
                    .unwrap_or_default()
                    .to_owned()
            })
            .collect()
    };
    assert_eq!(pid(&replayed), pid(&live));
}

/// Brief 0025's requests against the real adapter: `pause` of the TestApp sleeping, `exceptionInfo` at the
/// first-chance `InvalidOperationException` (with its stack trace), `stackTrace` paging (`startFrame`, `levels`,
/// `totalFrames`) and `variables` paging (`start`, `count`) over the 201 locals of `Many` and Main's `int[1000]`.
#[test]
fn eludite_dbg_mono_pauses_and_pages() {
    let Some(found) = find() else { return };
    // Pause: the TestApp sleeps for a minute after printing `sleeping`.
    let (client, rec) = found.launch(&["sleep"], &[], &["user-unhandled"]);
    let caps = client.capabilities();
    assert!(caps.supports_delayed_stack_trace_loading);
    assert!(caps.supports_exception_info_request);
    assert!(
        !caps.supports_variable_paging,
        "not advertised (brief 0022 report, section 9)"
    );
    rec.wait_nth(
        1,
        "sleeping",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("sleeping")),
    );
    std::thread::sleep(std::time::Duration::from_millis(200));
    let clock = Instant::now();
    client
        .request_wait("pause", json!({"threadId": 0}), T)
        .unwrap();
    let s = rec.stopped(1);
    eprintln!(
        "timing: pause to stopped: {:.1} ms",
        clock.elapsed().as_secs_f64() * 1e3
    );
    assert_eq!(s.reason, "pause");
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid}), T)
        .unwrap();
    let names: Vec<&str> = st["stackFrames"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f["name"].as_str().unwrap())
        .collect();
    eprintln!("pause stack: {names:?}");
    assert!(
        names.iter().any(|n| n.contains("Program.Main")),
        "{names:?}"
    );
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();

    // exceptionInfo at the first-chance throw (filter `all`), then the stack in pages.
    let (client, rec) = found.launch(&[], &["many"], &["all"]);
    let s = rec.stopped(1);
    assert_eq!(s.reason, "exception");
    let tid = s.thread_id.unwrap();
    let info = client
        .request_wait("exceptionInfo", json!({"threadId": tid}), T)
        .unwrap();
    eprintln!("exceptionInfo: {info}");
    assert_eq!(info["exceptionId"], "System.InvalidOperationException");
    assert_eq!(info["description"], "boom");
    assert!(
        info["details"]["stackTrace"]
            .as_str()
            .unwrap()
            .contains("Fail"),
        "{info}"
    );
    let top = client
        .request_wait(
            "stackTrace",
            json!({"threadId": tid, "startFrame": 0, "levels": 1}),
            T,
        )
        .unwrap();
    assert_eq!(top["stackFrames"].as_array().unwrap().len(), 1);
    let total = top["totalFrames"].as_i64().unwrap();
    assert!(total >= 2, "{top}");
    assert_eq!(top["stackFrames"][0]["line"], found.line_of("throw"));
    let second = client
        .request_wait(
            "stackTrace",
            json!({"threadId": tid, "startFrame": 1, "levels": 1}),
            T,
        )
        .unwrap();
    assert_eq!(second["stackFrames"][0]["line"], found.line_of("call-fail"));
    eprintln!("stack paging: totalFrames {total}");
    // Main's variables, paged: `big` is an int[1000] given as ranges.
    let main_frame = second["stackFrames"][0]["id"].clone();
    let scopes = client
        .request_wait("scopes", json!({"frameId": main_frame}), T)
        .unwrap();
    let locals = scopes["scopes"][0]["variablesReference"].clone();
    let page = client
        .request_wait(
            "variables",
            json!({"variablesReference": locals, "start": 1, "count": 2}),
            T,
        )
        .unwrap();
    assert_eq!(page["variables"].as_array().unwrap().len(), 2, "{page}");
    // On to Many: 201 locals, paged by `start` and `count`.
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    let s = rec.stopped(2);
    assert_eq!(s.reason, "breakpoint");
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 1}), T)
        .unwrap();
    let scopes = client
        .request_wait("scopes", json!({"frameId": st["stackFrames"][0]["id"]}), T)
        .unwrap();
    let locals = scopes["scopes"][0]["variablesReference"].clone();
    let clock = Instant::now();
    let page = client
        .request_wait(
            "variables",
            json!({"variablesReference": locals, "start": 190, "count": 50}),
            T,
        )
        .unwrap();
    eprintln!(
        "timing: a page of variables: {:.1} ms",
        clock.elapsed().as_secs_f64() * 1e3
    );
    let names: Vec<&str> = page["variables"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.first(), Some(&"l190"), "{names:?}");
    assert_eq!(names.len(), 11, "l190 to l199 and `last`: {names:?}");
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
}

fn wait_terminated(rec: &Recorder) {
    rec.wait_nth(1, "terminated", |e| {
        matches!(e, ClientEvent::Event(Event::Terminated))
    });
}

fn console_lines(rec: &Recorder, containing: &str) -> Vec<String> {
    rec.events()
        .into_iter()
        .filter_map(|e| match e {
            ClientEvent::Event(Event::Output(o))
                if o.category.as_deref() == Some("console") && o.output.contains(containing) =>
            {
                Some(o.output)
            }
            _ => None,
        })
        .collect()
}

/// Brief 0026's requests against the real adapter: exception filter options by type (the TestApp throws an
/// `InvalidOperationException`; a `FormatException` condition lets it pass), `setVariable` on a parameter whose new
/// value the program then computes with (timed), a function breakpoint on `Calculator.Twice`, and the adapter's own log
/// points on the loop body (100 lines, no stop).
#[test]
fn eludite_dbg_mono_runs_under_control() {
    let Some(found) = find() else { return };
    let caps = {
        let (client, _) = found.launch(&["sleep"], &[], &[]);
        let c = client.capabilities();
        let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
        c
    };
    assert!(caps.supports_exception_filter_options && caps.supports_function_breakpoints);
    assert!(caps.supports_set_variable && caps.supports_log_points);
    assert!(!caps.supports_set_expression && !caps.supports_goto_targets_request);

    // Filter options: `all` for InvalidOperationException stops at the throw...
    let option = |types: &str| ExceptionFilterOptions {
        filter_id: "all".into(),
        condition: Some(types.into()),
    };
    let (client, rec) = found.launch_plan(
        &[],
        Vec::new(),
        Vec::new(),
        &[],
        vec![option(
            "System.FormatException, System.InvalidOperationException",
        )],
    );
    let s = rec.stopped(1);
    assert_eq!(s.reason, "exception");
    let st = client
        .request_wait(
            "stackTrace",
            json!({"threadId": s.thread_id.unwrap(), "levels": 1}),
            T,
        )
        .unwrap();
    assert_eq!(st["stackFrames"][0]["line"], found.line_of("throw"));
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();
    // ...and for FormatException only, the program runs to its end.
    let (client, rec) = found.launch_plan(
        &[],
        Vec::new(),
        Vec::new(),
        &[],
        vec![option("System.FormatException")],
    );
    wait_terminated(&rec);
    assert!(
        !rec.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Stopped(_))))
    );
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);

    // setVariable: `a` becomes 10 before `sum = a + b`, so Add returns (10 + 3) * 2.
    let bp = |mark: &str| SourceBreakpoint {
        line: found.line_of(mark),
        ..Default::default()
    };
    let (client, rec) = found.launch_plan(&[], vec![bp("add-sum")], Vec::new(), &[], Vec::new());
    let s = rec.stopped(1);
    let tid = s.thread_id.unwrap();
    let st = client
        .request_wait("stackTrace", json!({"threadId": tid, "levels": 1}), T)
        .unwrap();
    let scopes = client
        .request_wait("scopes", json!({"frameId": st["stackFrames"][0]["id"]}), T)
        .unwrap();
    let locals = scopes["scopes"][0]["variablesReference"].clone();
    let mut times = Vec::new();
    for v in (0..20).rev() {
        let clock = Instant::now();
        let r = client
            .request_wait(
                "setVariable",
                json!({"variablesReference": locals, "name": "a", "value": (v + 10).to_string()}),
                T,
            )
            .unwrap();
        times.push(clock.elapsed());
        assert_eq!(r["value"], (v + 10).to_string());
    }
    times.sort();
    eprintln!(
        "timing: setVariable round trip against eludite-dbg-mono p95 {:.2} ms (max {:.2} ms, 20 calls)",
        times[18].as_secs_f64() * 1e3,
        times[19].as_secs_f64() * 1e3
    );
    let bad = client
        .request_wait(
            "setVariable",
            json!({"variablesReference": locals, "name": "a", "value": "\"text\""}),
            T,
        )
        .unwrap_err();
    eprintln!("setVariable of a string to an int: {bad}");
    let vars = client
        .request_wait("variables", json!({"variablesReference": locals}), T)
        .unwrap();
    let a = vars["variables"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "a")
        .unwrap()
        .clone();
    assert_eq!(a["value"], "10");
    client
        .request_wait("continue", json!({"threadId": tid}), T)
        .unwrap();
    rec.wait_nth(
        1,
        "result 26",
        |e| matches!(e, ClientEvent::Event(Event::Output(o)) if o.output.contains("result 26")),
    );
    wait_terminated(&rec);
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);

    // A function breakpoint by name.
    let (client, rec) = found.launch_plan(
        &[],
        Vec::new(),
        vec![FunctionBreakpoint {
            name: "Eludite.Debugger.Mono.TestApp.Calculator.Twice".into(),
            ..Default::default()
        }],
        &[],
        Vec::new(),
    );
    let s = rec.stopped(1);
    assert_eq!(s.reason, "function breakpoint");
    let st = client
        .request_wait(
            "stackTrace",
            json!({"threadId": s.thread_id.unwrap(), "levels": 1}),
            T,
        )
        .unwrap();
    eprintln!(
        "function breakpoint stop: {} line {}",
        st["stackFrames"][0]["name"], st["stackFrames"][0]["line"]
    );
    assert!(
        st["stackFrames"][0]["name"]
            .as_str()
            .unwrap()
            .contains("Calculator.Twice"),
        "{st}"
    );
    client
        .request_wait("disconnect", json!({"terminateDebuggee": true}), T)
        .unwrap();

    // The same tracepoint emulated as the shell does on an adapter without log points: a stop, the stack, an evaluation
    // and a resume per hit, 100 hits.
    let clock = Instant::now();
    let (client, rec) = found.launch_plan(&[], vec![bp("loop-body")], Vec::new(), &[], Vec::new());
    let hits = common::emulate_tracepoint(&client, &rec, 100, "i");
    let total = clock.elapsed();
    assert_eq!(hits[0].0, "0");
    assert_eq!(hits[99].0, "99");
    let (mean, p95) = common::mean_p95(&hits.iter().map(|(_, t)| *t).collect::<Vec<_>>());
    eprintln!(
        "timing: an emulated tracepoint against eludite-dbg-mono through eludite-dap, stop to resume: mean {mean:.2} ms, \
         p95 {p95:.2} ms over 100 hits; launch to the last hit {:.0} ms",
        total.as_secs_f64() * 1e3
    );
    wait_terminated(&rec);
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);

    // The adapter's own log points: 100 lines from the loop body, no stop.
    let clock = Instant::now();
    let (client, rec) = found.launch_plan(
        &[],
        vec![SourceBreakpoint {
            line: found.line_of("loop-body"),
            log_message: Some("i={i} total={total}".into()),
            ..Default::default()
        }],
        Vec::new(),
        &[],
        Vec::new(),
    );
    wait_terminated(&rec);
    let lines = console_lines(&rec, "total=");
    eprintln!(
        "timing: the TestApp with an adapter log point on its loop body, launch to end: {:.0} ms for {} lines",
        clock.elapsed().as_secs_f64() * 1e3,
        lines.len()
    );
    assert_eq!(lines.len(), 100, "{lines:?}");
    assert_eq!(lines[0].trim_end(), "i=0 total=0");
    assert_eq!(lines[99].trim_end(), "i=99 total=4851");
    assert!(
        !rec.events()
            .iter()
            .any(|e| matches!(e, ClientEvent::Event(Event::Stopped(_))))
    );
    let _ = client.request_wait("disconnect", json!({"terminateDebuggee": true}), T);
}

/// A free loopback port (bound and released: the debugger agent binds it next).
fn free_port() -> u16 {
    std::net::TcpListener::bind(("127.0.0.1", 0))
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Brief 0027: attach to the TestApp started by `mono` with a debugger agent that listens (`server=y`, `suspend=y`, so
/// it waits), through eludite-dap's attach plan (the agent's address read from the command line as the process listing
/// sees it), run to a breakpoint set during the handshake, then detach: the program runs on to its end by itself.
#[test]
fn eludite_dbg_mono_attaches_to_a_waiting_test_app_and_detaches() {
    use eludite_dap::attach::{AttachAdapter, attach_plan};
    use eludite_dap::processes;
    let Some(found) = find() else { return };
    let port = free_port();
    let agent =
        format!("--debugger-agent=transport=dt_socket,server=y,suspend=y,address=127.0.0.1:{port}");
    let mut app = std::process::Command::new(&found.mono.mono)
        .args(["--debug", &agent])
        .arg(&found.config.program)
        .envs(found.mono.env.iter().map(|(k, v)| (k, v)))
        .stdout(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    // The process listing sees it as a Mono program with an agent to attach to.
    let deadline = Instant::now() + T;
    let listed = loop {
        let all = processes::list().unwrap();
        if let Some(p) = all.iter().find(|p| p.pid == app.id()) {
            break p.clone();
        }
        assert!(Instant::now() < deadline, "the TestApp is not listed");
    };
    assert_eq!(listed.runtime, processes::Runtime::Mono);
    let at = processes::mono_agent(&listed.argv);
    assert_eq!(at, Some(("127.0.0.1".to_owned(), port)));
    let plan = attach_plan(
        AttachAdapter::for_runtime(listed.runtime).unwrap(),
        app.id(),
        at,
        Platform::Linux,
    )
    .unwrap();
    let rec = Recorder::default();
    let clock = Instant::now();
    let client = DapClient::start(
        common::recorded(
            transport::connect_with_env(
                &found.mono.adapter_transport(&found.adapter),
                &found.mono.env,
            )
            .unwrap(),
            "mono",
            &found.version,
            None,
        ),
        rec.sink(),
    );
    session::start(
        &client,
        &StartPlan {
            adapter_id: plan.adapter_id.into(),
            kind: StartKind::Attach,
            arguments: plan.arguments,
            breakpoints: vec![(
                found.source.to_string_lossy().into_owned(),
                vec![SourceBreakpoint {
                    line: found.line_of("add-sum"),
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
    let took = clock.elapsed();
    eprintln!(
        "timing: attach to a waiting TestApp to the first stopped: {:.0} ms",
        took.as_secs_f64() * 1e3
    );
    assert_eq!(s.reason, "breakpoint");
    assert!(took.as_secs_f64() < 3.0, "budget: under 3 s ({took:?})");
    // Detach: the program goes on without the debugger and exits with its code. eludite-dbg-mono stays up after a
    // detach (and spins), so the client ends it, as the shell does when the detach is answered.
    client
        .request_wait("disconnect", json!({"terminateDebuggee": false}), T)
        .unwrap();
    client.kill();
    let deadline = Instant::now() + T;
    let status = loop {
        if let Some(s) = app.try_wait().unwrap() {
            break s;
        }
        assert!(Instant::now() < deadline, "the TestApp did not run on");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };
    assert_eq!(status.code(), Some(3));
    let mut out = String::new();
    std::io::Read::read_to_string(app.stdout.as_mut().unwrap(), &mut out).unwrap();
    assert!(out.contains("result 10"), "{out}");
}
